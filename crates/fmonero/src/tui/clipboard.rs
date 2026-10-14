//! Public clipboard content travels on stdin, never in a process argument.
use std::{
    io::{ErrorKind, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn copy(text: &str) -> Result<(), &'static str> {
    if text.len() > 64 * 1024 {
        return Err("The clipboard value exceeds the 64 KiB limit. Export it to a private artifact instead.");
    }
    let mut child = Command::new("wl-copy")
        .env_remove("FMONERO_WALLET_PASSWORD")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|_| "Could not start wl-copy. Check that it is installed and the desktop clipboard is available.")?;
    let mut stdin = child.stdin.take();
    let configured = stdin.as_ref().is_some_and(|pipe| {
        // SAFETY: the owned ChildStdin keeps this descriptor valid for both fcntl calls.
        // O_NONBLOCK changes only this parent's pipe endpoint, so writes respect our deadline.
        unsafe {
            let fd = pipe.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFL);
            flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0
        }
    });
    let mut offset = 0;
    let started = Instant::now();
    let result = loop {
        if !configured || started.elapsed() >= Duration::from_secs(2) {
            break Err("Clipboard delivery was not confirmed before its deadline.");
        }
        if let Some(pipe) = &mut stdin {
            match pipe.write(&text.as_bytes()[offset..]) {
                Ok(0) if offset < text.len() => break Err("The desktop closed clipboard input."),
                Ok(n) => offset += n,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => break Err("The desktop did not accept the clipboard value."),
            }
            if offset == text.len() {
                drop(stdin.take());
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() && offset == text.len() {
                    Ok(())
                } else {
                    Err("The desktop did not accept the clipboard value.")
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break Err("Could not confirm clipboard delivery."),
        }
    };
    drop(stdin);
    // SAFETY: child created its own process group; the positive child PID is also its PGID.
    // Kill helpers too, so descendants cannot keep this request alive beyond its deadline.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
    result
}
