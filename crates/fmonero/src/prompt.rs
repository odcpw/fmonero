//! The TTY password prompt (DESIGN.md §2b.1).
//!
//! Three sources, consulted in this order, first hit wins:
//!
//! 1. `--password-file PATH` — read, **one** trailing newline stripped, and refused with
//!    exit 3 if the file has any group or other permission bit;
//! 2. `$FMONERO_WALLET_PASSWORD`;
//! 3. a TTY prompt with echo off (`termios` through `libc`; the original terminal settings
//!    are restored on every path out, `SIGINT` included).
//!
//! With no TTY and neither of the first two, this exits **9 `wallet_locked`** with
//! `hint: FMONERO_WALLET_PASSWORD=… fmonero wallet open <name>   # or --password-file`, so an
//! agent is never left waiting on a prompt that cannot appear. The password never reaches
//! argv — not fmonero's, and not `monero-wallet-rpc`'s (§2b.1).

use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::Path;

use fmonero_core::{Error, SecretString};

/// Advanced workflows accept secrets only from a masked terminal or a bounded
/// private pipe. Each piped secret occupies one line, in the documented order.
pub fn secret(label: &str, confirm: bool, stdin: bool) -> Result<SecretString, Error> {
    if stdin {
        return wallet_password_stdin();
    }
    let mut tty = Tty::open().ok_or_else(|| {
        Error::wallet_locked("This action needs a masked terminal prompt or --secret-stdin.")
    })?;
    let value = SecretString::new(tty.ask(label)?)?;
    if confirm {
        let again = SecretString::new(tty.ask("Again: ")?)?;
        if value.expose_secret() != again.expose_secret() {
            return Err(Error::usage(
                "The two values do not match. Nothing was changed.",
            ));
        }
    }
    Ok(value)
}

/// The environment variable §5b documents as the way an agent opens a wallet with no TTY.
pub const PASSWORD_ENV: &str = "FMONERO_WALLET_PASSWORD";
const PROTECTED_INPUT_BYTES: usize = fmonero_core::secrets::MAX_SECRET_BYTES + 3;

fn protected_buffer() -> zeroize::Zeroizing<Box<[u8]>> {
    // A fixed slice cannot grow and free unwiped prefixes while reading a secret.
    zeroize::Zeroizing::new(vec![0; PROTECTED_INPUT_BYTES].into_boxed_slice())
}

/// The widget supplies one line through a private pipe. Never echo it, consult the environment,
/// or wait indefinitely for an abandoned writer. The same one-newline rule applies as for files.
pub fn wallet_password_stdin() -> Result<SecretString, Error> {
    let fd = std::io::stdin().as_raw_fd();
    // SAFETY: fd is the process's standard input descriptor.
    if unsafe { libc::isatty(fd) } == 1 {
        return Err(Error::usage(
            "Use the normal password prompt in a terminal.",
        ));
    }
    SecretString::new(password_line(fd, std::time::Duration::from_secs(10))?)
}

fn password_line(fd: i32, timeout: std::time::Duration) -> Result<String, Error> {
    let started = std::time::Instant::now();
    let mut bytes = protected_buffer();
    let mut used = 0;
    let result = (|| {
        loop {
            if crate::SIGNALLED.load(std::sync::atomic::Ordering::SeqCst) != 0 {
                return Err(Error::Interrupted);
            }
            let left = timeout.saturating_sub(started.elapsed());
            let mut poll = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll points to one initialized descriptor and lives for this call.
            let ready = unsafe { libc::poll(&mut poll, 1, left.as_millis().min(250) as i32) };
            if left.is_zero() {
                return Err(Error::timeout(
                    "The protected input was not received in time. Try again.",
                ));
            }
            if ready == 0 {
                continue;
            }
            if ready < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(Error::failure("Could not read the protected input."));
            }
            if used == bytes.len() {
                return Err(Error::usage("The protected input is too long."));
            }
            // SAFETY: used indexes one writable byte in a fixed live allocation.
            let n = unsafe { libc::read(fd, bytes.as_mut_ptr().add(used).cast(), 1) };
            if n < 0 {
                return Err(Error::failure("Could not read the protected input."));
            }
            if n == 0 {
                return Err(Error::usage(
                    "The protected input ended before a complete line arrived.",
                ));
            }
            used += 1;
            if bytes[used - 1] == b'\n' {
                let text = std::str::from_utf8(&bytes[..used])
                    .map_err(|_| Error::usage("The protected input is not valid text."))?;
                let value = strip_one_newline(text);
                if value.len() > fmonero_core::secrets::MAX_SECRET_BYTES {
                    return Err(Error::usage("The protected input is too long."));
                }
                return Ok(value.to_string());
            }
        }
    })();
    result
}

/// Read the wallet password (§2b.1).
///
/// `confirm` prompts **twice and compares** — `wallet create` on a TTY, where a mistyped
/// password creates a wallet that can never be opened and whose seed can never be read
/// (§2b.2). A mismatch is exit 2 and nothing is created. It applies only to the prompt: a
/// `--password-file` or an environment variable has nothing to mistype (§4.1).
pub fn wallet_password(password_file: Option<&Path>, confirm: bool) -> Result<SecretString, Error> {
    if let Some(path) = password_file {
        return SecretString::new(from_file(path)?);
    }
    if let Some(value) = std::env::var_os(PASSWORD_ENV) {
        // One trailing newline, as for a file: an agent that writes the variable from a
        // here-doc should not end up with a password nobody can retype.
        use std::os::unix::ffi::OsStringExt;
        let bytes = zeroize::Zeroizing::new(value.into_vec());
        if bytes.len() > PROTECTED_INPUT_BYTES {
            return Err(Error::usage("The password input is too long."));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Error::usage("The password input is not valid text."))?;
        return SecretString::new(strip_one_newline(text).to_owned());
    }
    let mut tty =
        match Tty::open() {
            Some(t) => t,
            None => return Err(Error::wallet_locked(
                "fmonero needs the wallet password and has no terminal to ask on. Give it with \
                 --password-file, or in FMONERO_WALLET_PASSWORD.",
            )
            .with_reason("bad_password")
            .with_hint(
                "FMONERO_WALLET_PASSWORD=… fmonero wallet open <name>   # or --password-file",
            )),
        };
    let first = SecretString::new(tty.ask("Wallet password: ")?)?;
    if !confirm {
        return Ok(first);
    }
    let again = SecretString::new(tty.ask("Wallet password (again): ")?)?;
    if first.expose_secret() != again.expose_secret() {
        return Err(
            Error::usage("The two passwords are not the same. Nothing was created.")
                .with_reason("bad_args")
                .with_hint("fmonero wallet create <name> --yes"),
        );
    }
    Ok(first)
}

/// §2b.1: refused with exit 3 if the file has **any** group or other permission bit — a
/// password file every local user can read is not a password file, and the threat model
/// includes other local users (§2c).
fn from_file(path: &Path) -> Result<String, Error> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    // S6: the operating system's own words go to `detail`, not into the sentence.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| {
            Error::not_found(fmonero_core::io_words(
                "read",
                "the password file",
                path,
                &e,
            ))
            .with_detail(e.to_string())
            .with_hint("fmonero wallet open <name> --password-file <path>")
        })?;
    let meta = file
        .metadata()
        .map_err(|e| Error::io("inspect", "the password file", path, &e))?;
    if !meta.is_file() {
        return Err(Error::safety("The password input must be a regular file."));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        // M14's shape: a file mode in octal is two numbers a reader cannot act on. What it
        // means is that other people using this machine can read the password.
        return Err(Error::safety(format!(
            "Other users of this machine can read {}. A password file must be readable by you \
             alone.",
            path.display()
        ))
        .with_detail(format!("mode {mode:04o}"))
        .with_hint(format!("chmod 600 {}", path.display())));
    }
    let mut bytes = protected_buffer();
    let mut reader = file;
    let mut used = 0;
    loop {
        if used == bytes.len() {
            return Err(Error::usage("The password input is too long."));
        }
        let read = reader
            .read(&mut bytes[used..])
            .map_err(|e| Error::io("read", "the password file", path, &e))?;
        if read == 0 {
            break;
        }
        used += read;
    }
    let text = std::str::from_utf8(&bytes[..used])
        .map_err(|_| Error::usage("The password file is not valid text."))?;
    let value = strip_one_newline(text);
    if value.len() > fmonero_core::secrets::MAX_SECRET_BYTES {
        return Err(Error::usage("The password input is too long."));
    }
    Ok(value.to_string())
}

/// **One** trailing newline, not every trailing whitespace character: a password may end in a
/// space, and `trim_end` would silently change it into a different password.
fn strip_one_newline(s: &str) -> &str {
    s.strip_suffix('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s))
        .unwrap_or(s)
}

/// The controlling terminal, with echo turned off for the life of the guard.
///
/// `/dev/tty` rather than stdin, so `fmonero wallet open main < /dev/null` on a terminal
/// still prompts, and so the prompt itself never lands in the data stream (§4: stdout is
/// data).
struct Tty {
    file: std::fs::File,
    saved: libc::termios,
}

impl Tty {
    fn open() -> Option<Tty> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;
        let fd = file.as_raw_fd();
        // SAFETY: `fd` is an open descriptor; `saved` is fully written by tcgetattr, and the
        // call is checked before it is read.
        let saved = unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) != 0 {
                return None;
            }
            t
        };
        let mut quiet = saved;
        quiet.c_lflag &= !libc::ECHO;
        // Keep ECHONL so the newline the user types is still shown: without it the cursor
        // does not move and the prompt looks hung.
        quiet.c_lflag |= libc::ECHONL;
        // SAFETY: `fd` is open and `quiet` is a valid termios derived from `saved`.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) } != 0 {
            return None;
        }
        Some(Tty { file, saved })
    }

    fn ask(&mut self, prompt: &str) -> Result<String, Error> {
        // The prompt goes to the terminal, never to stdout.
        let mut out = self.file.try_clone().map_err(|e| {
            Error::failure("fmonero could not write to this terminal.").with_detail(e.to_string())
        })?;
        out.write_all(prompt.as_bytes())
            .and_then(|()| out.flush())
            .map_err(|e| {
                Error::failure("fmonero could not write to this terminal.")
                    .with_detail(e.to_string())
            })?;
        password_line(self.file.as_raw_fd(), std::time::Duration::from_secs(600))
    }
}

impl Drop for Tty {
    /// Restore the terminal on **every** path out — a normal return, an error, a panic and a
    /// SIGINT that unwinds — because a terminal left with echo off is a terminal the user has
    /// to `stty sane` by hand.
    fn drop(&mut self) {
        // SAFETY: the descriptor is still open and `self.saved` is the settings we read.
        unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.saved) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piped_passwords_preserve_spaces_and_have_a_deadline() {
        use std::os::unix::net::UnixStream;
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer.write_all(b"synthetic value \n").unwrap();
        assert_eq!(
            password_line(reader.as_raw_fd(), std::time::Duration::from_secs(1)).unwrap(),
            "synthetic value "
        );
        assert_eq!(
            password_line(reader.as_raw_fd(), std::time::Duration::from_millis(1))
                .unwrap_err()
                .exit_code(),
            8
        );
        writer.write_all(b"\n").unwrap();
        assert_eq!(
            password_line(reader.as_raw_fd(), std::time::Duration::from_secs(1)).unwrap(),
            ""
        );
        drop(writer);
        assert!(password_line(reader.as_raw_fd(), std::time::Duration::from_secs(1)).is_err());
    }

    #[test]
    fn one_trailing_newline_is_stripped_and_nothing_else_is() {
        assert_eq!(strip_one_newline("hunter2\n"), "hunter2");
        assert_eq!(strip_one_newline("hunter2\r\n"), "hunter2");
        assert_eq!(strip_one_newline("hunter2"), "hunter2");
        // A password may end in a space, and a second newline is part of it.
        assert_eq!(strip_one_newline("hunter2 \n"), "hunter2 ");
        assert_eq!(strip_one_newline("hunter2\n\n"), "hunter2\n");
        assert_eq!(strip_one_newline(""), "");
    }

    #[test]
    fn a_password_file_must_have_no_group_or_other_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("fmonero-pw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pw");
        std::fs::write(&path, b"hunter2\n").unwrap();

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            wallet_password(Some(&path), false).unwrap().expose_secret(),
            "hunter2"
        );

        for mode in [0o640, 0o604, 0o666, 0o700 | 0o004] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let e = wallet_password(Some(&path), false).unwrap_err();
            assert_eq!(e.exit_code(), 3, "mode {mode:04o} must be refused");
            assert!(e.hint().starts_with("chmod 600 "), "{}", e.hint());
        }
        // A missing file is exit 6, not a prompt.
        let e = wallet_password(Some(&dir.join("nope")), false).unwrap_err();
        assert_eq!(e.exit_code(), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--password-file` wins over the environment, and `confirm` does not apply to either:
    /// there is nothing to mistype (§4.1).
    #[test]
    fn the_file_wins_over_the_environment_and_confirm_does_not_prompt() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("fmonero-pw2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pw");
        std::fs::write(&path, b"from-the-file").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            wallet_password(Some(&path), true).unwrap().expose_secret(),
            "from-the-file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn private_password_files_are_bounded_and_refuse_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("fmonero-pw-bounds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pw");
        std::fs::write(
            &path,
            vec![b'x'; fmonero_core::secrets::MAX_SECRET_BYTES + 1],
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            wallet_password(Some(&path), false).unwrap_err().exit_code(),
            2
        );
        std::fs::write(&path, b"synthetic").unwrap();
        let link = dir.join("linked");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(wallet_password(Some(&link), false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn protected_input_accepts_its_exact_boundary_and_rejects_bad_text() {
        use std::os::unix::net::UnixStream;
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let mut input = vec![b'x'; fmonero_core::secrets::MAX_SECRET_BYTES];
        input.extend_from_slice(b"\r\n");
        writer.write_all(&input).unwrap();
        assert_eq!(
            password_line(reader.as_raw_fd(), std::time::Duration::from_secs(1))
                .unwrap()
                .len(),
            fmonero_core::secrets::MAX_SECRET_BYTES
        );
        writer.write_all(&[0xff, b'\n']).unwrap();
        assert_eq!(
            password_line(reader.as_raw_fd(), std::time::Duration::from_secs(1))
                .unwrap_err()
                .exit_code(),
            2
        );
        let mut too_large = vec![b'x'; fmonero_core::secrets::MAX_SECRET_BYTES + 1];
        too_large.push(b'\n');
        writer.write_all(&too_large).unwrap();
        assert_eq!(
            password_line(reader.as_raw_fd(), std::time::Duration::from_secs(1))
                .unwrap_err()
                .exit_code(),
            2
        );
    }
}
