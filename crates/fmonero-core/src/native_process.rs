//! Private bounded Monero CLI supervision. No shell, inherited locale or raw output.
use crate::{secrets::PrivateBuffer, CancelToken, Error};
use std::{
    ffi::CString,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
const OUTPUT_LIMIT: usize = 1024 * 1024;
/// A seekable, sealed in-memory file; never a path on disk. The child reads fd3.
pub(crate) fn sealed_input(bytes: &[u8]) -> Result<File, Error> {
    let name = CString::new("fmonero-protected").unwrap();
    // SAFETY: valid NUL-terminated static label, no data in label.
    let fd =
        unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(Error::backend(
            "The protected in-memory input could not be created.",
        ));
    }
    // SAFETY: new fd is exclusively transferred to File.
    let mut file = unsafe { File::from_raw_fd(fd) };
    // Even an explicitly reopened /proc fd retains owner-only file permissions.
    if unsafe { libc::fchmod(fd, 0o600) } != 0 {
        return Err(Error::backend(
            "Protected native input permissions could not be restricted.",
        ));
    }
    file.write_all(bytes)
        .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
        .map_err(|_| Error::backend("Protected native input could not be prepared."))?;
    // SAFETY: valid fd; seals prevent any later content modification.
    let rc = unsafe {
        libc::fcntl(
            fd,
            libc::F_ADD_SEALS,
            libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL,
        )
    };
    if rc < 0 {
        return Err(Error::backend(
            "Protected native input could not be sealed.",
        ));
    }
    Ok(file)
}
pub(crate) struct NativeProcess {
    child: Child,
    input: Option<NativeInput>,
    stdout: ChildStdout,
    stderr: ChildStderr,
    pub(crate) output: PrivateBuffer,
    errors: PrivateBuffer,
    deadline: Instant,
    exited: Option<ExitStatus>,
}
enum NativeInput {
    Pipe(ChildStdin),
    Tty(File),
}
impl NativeInput {
    fn as_raw_fd(&self) -> i32 {
        match self {
            Self::Pipe(pipe) => pipe.as_raw_fd(),
            Self::Tty(master) => master.as_raw_fd(),
        }
    }
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Pipe(pipe) => pipe.write(bytes),
            Self::Tty(master) => master.write(bytes),
        }
    }
}

/// Only stdin is a terminal. stdout/stderr stay bounded private pipes. Disable
/// echo before the slave is exposed to any child or protected input is written.
fn private_terminal() -> Result<(File, File), Error> {
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: output pointers are live; optional name/termios/window pointers are null.
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    } != 0
    {
        return Err(Error::backend(
            "A private wallet terminal could not be created.",
        ));
    }
    // SAFETY: successful openpty gives us exclusive ownership of both descriptors.
    let master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: valid tty descriptor and writable termios storage.
    if unsafe { libc::tcgetattr(slave.as_raw_fd(), attributes.as_mut_ptr()) } != 0 {
        return Err(Error::backend(
            "Private wallet terminal settings could not be read.",
        ));
    }
    // SAFETY: tcgetattr initialized attributes on success.
    let mut attributes = unsafe { attributes.assume_init() };
    // Raw input avoids the canonical line-size limit; Monero may adjust its own
    // tty mode while reading, but its saved mode already has all echo disabled.
    unsafe { libc::cfmakeraw(&mut attributes) };
    attributes.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ECHOE | libc::ECHOK);
    if unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &attributes) } != 0 {
        return Err(Error::backend(
            "Private wallet terminal echo could not be disabled.",
        ));
    }
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(Error::backend(
                "Private wallet terminal inheritance could not be restricted.",
            ));
        }
    }
    Ok((master, slave))
}
impl NativeProcess {
    pub(crate) fn start(
        args: &[String],
        directory: &Path,
        secret_file: Option<&File>,
        timeout: Duration,
    ) -> Result<Self, Error> {
        let files: Vec<&File> = secret_file.into_iter().collect();
        Self::start_with_inputs(args, directory, &files, timeout)
    }
    pub(crate) fn start_with_inputs(
        args: &[String],
        directory: &Path,
        inputs: &[&File],
        timeout: Duration,
    ) -> Result<Self, Error> {
        Self::spawn(
            Command::new("monero-wallet-cli"),
            args,
            directory,
            inputs,
            timeout,
            false,
        )
    }
    pub(crate) fn start_review(
        args: &[String],
        directory: &Path,
        inputs: &[&File],
        timeout: Duration,
    ) -> Result<Self, Error> {
        Self::spawn(
            Command::new("monero-wallet-cli"),
            args,
            directory,
            inputs,
            timeout,
            true,
        )
    }
    fn spawn(
        mut command: Command,
        args: &[String],
        directory: &Path,
        inputs: &[&File],
        timeout: Duration,
        dialogue: bool,
    ) -> Result<Self, Error> {
        if inputs.len() > 2 {
            return Err(Error::failure("Too many protected native inputs."));
        }
        let mut inherited = Vec::new();
        for file in inputs {
            // Reserve high descriptors so dup2 to 3/4 cannot overwrite another
            // input source or a Command pipe before it is installed.
            let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
            if fd < 0 {
                return Err(Error::backend(
                    "Protected native input could not be inherited.",
                ));
            }
            inherited.push(unsafe { File::from_raw_fd(fd) });
        }
        let (terminal_input, stdin) = if dialogue {
            let (master, slave) = private_terminal()?;
            (Some(NativeInput::Tty(master)), Stdio::from(slave))
        } else {
            (None, Stdio::piped())
        };
        command
            .args(args)
            .current_dir(directory)
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env_remove("FMONERO_WALLET_PASSWORD")
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let descriptors: Vec<i32> = inherited.iter().map(AsRawFd::as_raw_fd).collect();
        // SAFETY: only async-signal-safe fd operations between fork and exec.
        // Parent-owned duplicates stay alive through spawn; destinations are disjoint.
        unsafe {
            command.pre_exec(move || {
                for (i, fd) in descriptors.iter().enumerate() {
                    let target = 3 + i as i32;
                    if libc::dup2(*fd, target) < 0 || libc::fcntl(target, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .map_err(|_| Error::backend("Monero's wallet command-line engine could not start."))?;
        let input = terminal_input.or_else(|| child.stdin.take().map(NativeInput::Pipe));
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        for fd in [
            stdout.as_raw_fd(),
            stderr.as_raw_fd(),
            input.as_ref().unwrap().as_raw_fd(),
        ] {
            // SAFETY: valid live pipe descriptors; nonblocking avoids pipe deadlocks.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::backend(
                    "The protected process pipes could not be configured.",
                ));
            }
        }
        Ok(Self {
            child,
            input,
            stdout,
            stderr,
            output: PrivateBuffer::new(OUTPUT_LIMIT),
            errors: PrivateBuffer::new(OUTPUT_LIMIT),
            deadline: Instant::now() + timeout,
            exited: None,
        })
    }
    pub(crate) fn pump(&mut self, cancel: &CancelToken) -> Result<(), Error> {
        if cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        if Instant::now() >= self.deadline {
            return Err(Error::upstream(
                "Monero's wallet operation exceeded its time limit. Its outcome may be unknown.",
            )
            .with_reason("killed_at_deadline"));
        }
        drain(&mut self.stdout, &mut self.output)?;
        drain(&mut self.stderr, &mut self.errors)?;
        self.exited = self
            .child
            .try_wait()
            .map_err(|_| Error::backend("The wallet process could not be observed."))?;
        if self.exited.is_some() {
            // The final write can race the first drain. Capture it before a
            // submit caller observes exit and reconciles proven hashes.
            drain(&mut self.stdout, &mut self.output)?;
            drain(&mut self.stderr, &mut self.errors)?;
        }
        Ok(())
    }
    pub(crate) fn exited(&self) -> bool {
        self.exited.is_some()
    }
    pub(crate) fn success(&self) -> bool {
        self.exited.is_some_and(|status| status.success())
    }
    pub(crate) fn send(&mut self, bytes: &[u8], cancel: &CancelToken) -> Result<(), Error> {
        let mut offset = 0;
        while offset < bytes.len() {
            self.pump(cancel)?;
            let input = self
                .input
                .as_mut()
                .ok_or_else(|| Error::backend("The wallet input is closed."))?;
            match input.write(&bytes[offset..]) {
                Ok(0) => return Err(Error::backend("The wallet stopped accepting input.")),
                Ok(n) => offset += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return Err(Error::backend("The wallet stopped accepting input.")),
            }
        }
        Ok(())
    }
    pub(crate) fn wait(&mut self, cancel: &CancelToken) -> Result<(), Error> {
        // Closing a PTY master hangs up the slave and may discard an approval
        // before Monero reads it. Keep it alive until this one command exits.
        if matches!(&self.input, Some(NativeInput::Pipe(_))) {
            self.input.take();
        }
        loop {
            self.pump(cancel)?;
            if self.exited() {
                drain(&mut self.stdout, &mut self.output)?;
                drain(&mut self.stderr, &mut self.errors)?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// A password is supplied only at the exact visible unlock prompt. In
    /// ask-password=never mode this does nothing, even for a password like yes.
    pub(crate) fn unlock_if_prompted(
        &mut self,
        password: &crate::SecretString,
        sent: &mut bool,
        cancel: &CancelToken,
    ) -> Result<(), Error> {
        if !*sent && self.output.as_slice().ends_with(b"Wallet password: ") {
            let mut line = PrivateBuffer::new(crate::secrets::MAX_SECRET_BYTES + 1);
            line.write_all(password.expose_secret().as_bytes())
                .and_then(|()| line.write_all(b"\n"))
                .map_err(|_| Error::usage("Protected input exceeded its bound."))?;
            self.send(line.as_slice(), cancel)?;
            *sent = true;
        }
        Ok(())
    }
    pub(crate) fn public_addresses(&self) -> Vec<String> {
        std::str::from_utf8(self.output.as_slice())
            .unwrap_or("")
            .split_whitespace()
            .filter_map(|s| crate::address::parse(s).ok().map(|a| a.text))
            .collect()
    }
}
fn drain(pipe: &mut impl Read, target: &mut PrivateBuffer) -> Result<(), Error> {
    let mut bytes = Zeroizing::new([0u8; 8192]);
    loop {
        match pipe.read(&mut *bytes) {
            Ok(0) => return Ok(()),
            Ok(n) => target
                .write_all(&bytes[..n])
                .map_err(|_| Error::backend("The wallet process exceeded its output bound."))?,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                return Err(Error::backend(
                    "The wallet process output could not be read.",
                ))
            }
        }
    }
}
impl Drop for NativeProcess {
    fn drop(&mut self) {
        if self.exited.is_some() {
            return;
        }
        self.input.take();
        let pid = self.child.id() as i32;
        // SAFETY: child was launched in its own process group and has not been reaped.
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        let until = Instant::now() + Duration::from_millis(500);
        while Instant::now() < until {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // SAFETY: same unreaped child/process-group ownership.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}
pub(crate) fn base_args(network: crate::NetType) -> Vec<String> {
    let mut args = vec![
        "--log-file".into(),
        "/dev/null".into(),
        "--log-level".into(),
        "0".into(),
        "--max-concurrency".into(),
        "2".into(),
    ];
    match network {
        crate::NetType::Mainnet => {}
        crate::NetType::Stagenet => args.push("--stagenet".into()),
        crate::NetType::Testnet => args.push("--testnet".into()),
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_base_arguments_match_the_monero_cli_flag_contract() {
        for (network, flag) in [
            (crate::NetType::Mainnet, None),
            (crate::NetType::Stagenet, Some("--stagenet")),
            (crate::NetType::Testnet, Some("--testnet")),
        ] {
            let mut expected = vec![
                "--log-file",
                "/dev/null",
                "--log-level",
                "0",
                "--max-concurrency",
                "2",
            ];
            expected.extend(flag);
            let args = base_args(network);
            assert_eq!(args, expected);
            assert!(!args.iter().any(|arg| arg == "--no-initial-sync"));
            // Offline is selected by the workflow, never imposed on submission.
            assert!(!args.iter().any(|arg| arg == "--offline"));
        }
    }
    // A subprocess fixture exercises the real PTY/fd/process supervisor. It is
    // deliberately not a Monero simulator or evidence of native interoperability.
    #[test]
    fn native_dialogue_child() {
        let Ok(mode) = std::env::var("FMONERO_TEST_NATIVE_DIALOGUE") else {
            return;
        };
        let tty = unsafe { libc::isatty(0) } == 1;
        assert_eq!(tty, mode != "pipe");
        if tty {
            let mut attrs = std::mem::MaybeUninit::<libc::termios>::uninit();
            assert_eq!(unsafe { libc::tcgetattr(0, attrs.as_mut_ptr()) }, 0);
            assert_eq!(
                unsafe { attrs.assume_init() }.c_lflag & (libc::ECHO | libc::ECHONL),
                0
            );
        }
        // Synthetic yes-like input stays on the same sealed fd as production.
        let mut initial = unsafe { File::from_raw_fd(3) };
        let mut secret = Zeroizing::new(String::new());
        initial.read_to_string(&mut secret).unwrap();
        assert!(secret.as_str() == "yes");
        if mode == "pipe" {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input).unwrap();
            assert!(input.is_empty());
            println!("PIPE_FINISHED");
            return;
        }
        if mode == "enabled" {
            print!("Wallet password: ");
            std::io::stdout().flush().unwrap();
            let mut entered = Zeroizing::new(String::new());
            std::io::stdin().read_line(&mut entered).unwrap();
            assert!(entered.trim_end_matches('\n') == secret.as_str());
        }
        print!("REVIEW_READY Is this okay? ");
        std::io::stdout().flush().unwrap();
        let mut approval = String::new();
        std::io::stdin().read_line(&mut approval).unwrap();
        match approval.trim() {
            "y" | "yes" => println!("APPROVED"),
            "n" => println!("DECLINED"),
            _ => panic!("unexpected review response"),
        }
    }
    fn dialogue(mode: &str, tty: bool) -> NativeProcess {
        let input = sealed_input(b"yes").unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "native_process::tests::native_dialogue_child",
                "--nocapture",
            ])
            .env("FMONERO_TEST_NATIVE_DIALOGUE", mode);
        NativeProcess::spawn(
            command,
            &[],
            &std::env::temp_dir(),
            &[&input],
            Duration::from_secs(10),
            tty,
        )
        .unwrap()
    }
    #[test]
    fn private_tty_unlocks_only_when_prompted_and_yes_password_never_approves() {
        for mode in ["enabled", "disabled"] {
            let mut child = dialogue(mode, true);
            let cancel = CancelToken::new();
            let password = crate::SecretString::new("yes".into()).unwrap();
            let mut sent = false;
            loop {
                child.pump(&cancel).unwrap();
                child
                    .unlock_if_prompted(&password, &mut sent, &cancel)
                    .unwrap();
                if child
                    .output
                    .as_slice()
                    .ends_with(b"REVIEW_READY Is this okay? ")
                {
                    break;
                }
                assert!(!child.exited());
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(sent, mode == "enabled");
            let until = Instant::now() + Duration::from_millis(100);
            while Instant::now() < until {
                child.pump(&cancel).unwrap();
                assert!(
                    !child.exited(),
                    "the review must remain held without approval"
                );
                assert!(!child.output.as_slice().windows(8).any(|s| s == b"APPROVED"));
                std::thread::sleep(Duration::from_millis(5));
            }
            if let Some(NativeInput::Tty(master)) = child.input.as_mut() {
                let mut echoed = [0; 16];
                assert_eq!(
                    master.read(&mut echoed).unwrap_err().kind(),
                    io::ErrorKind::WouldBlock
                );
            } else {
                panic!("expected private tty");
            }
            child.send(b"n\n", &cancel).unwrap();
            child.wait(&cancel).unwrap();
            assert!(child.success());
            assert!(child.output.as_slice().windows(8).any(|s| s == b"DECLINED"));
        }
    }
    #[test]
    fn private_tty_wait_delivers_explicit_approval_before_hangup() {
        let mut child = dialogue("disabled", true);
        let cancel = CancelToken::new();
        loop {
            child.pump(&cancel).unwrap();
            if child
                .output
                .as_slice()
                .ends_with(b"REVIEW_READY Is this okay? ")
            {
                break;
            }
            assert!(!child.exited());
            std::thread::sleep(Duration::from_millis(5));
        }
        child.send(b"y\n", &cancel).unwrap();
        child.wait(&cancel).unwrap();
        assert!(child.success());
        assert!(child.output.as_slice().windows(8).any(|s| s == b"APPROVED"));
    }
    #[test]
    fn noninteractive_native_input_still_closes_on_wait() {
        let mut child = dialogue("pipe", false);
        child.wait(&CancelToken::new()).unwrap();
        assert!(child.success());
        assert!(String::from_utf8_lossy(child.output.as_slice()).contains("PIPE_FINISHED"));
    }
    #[test]
    fn cancelled_private_dialogue_is_reaped_without_approval() {
        let mut child = dialogue("disabled", true);
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(child.send(b"y\n", &cancel).is_err());
        let pid = child.child.id() as libc::pid_t;
        drop(child);
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
    #[test]
    fn protected_input_is_private_seekable_and_immutable() {
        use std::os::unix::fs::PermissionsExt;
        let value = Zeroizing::new(crate::rpc::random_hex(32).unwrap());
        let mut file = sealed_input(value.as_bytes()).unwrap();
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        let mut readback = Zeroizing::new(String::new());
        file.read_to_string(&mut readback).unwrap();
        assert_eq!(readback.as_str(), value.as_str());
        file.seek(SeekFrom::Start(0)).unwrap();
        assert!(file.write_all(b"change").is_err());
        assert!(file.set_len(0).is_err());
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }
}
