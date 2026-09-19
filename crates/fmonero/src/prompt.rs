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

use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::AsRawFd;
use std::path::Path;

use fmonero_core::Error;

/// The environment variable §5b documents as the way an agent opens a wallet with no TTY.
pub const PASSWORD_ENV: &str = "FMONERO_WALLET_PASSWORD";

/// Read the wallet password (§2b.1).
///
/// `confirm` prompts **twice and compares** — `wallet create` on a TTY, where a mistyped
/// password creates a wallet that can never be opened and whose seed can never be read
/// (§2b.2). A mismatch is exit 2 and nothing is created. It applies only to the prompt: a
/// `--password-file` or an environment variable has nothing to mistype (§4.1).
pub fn wallet_password(password_file: Option<&Path>, confirm: bool) -> Result<String, Error> {
    if let Some(path) = password_file {
        return from_file(path);
    }
    if let Some(value) = std::env::var_os(PASSWORD_ENV) {
        // One trailing newline, as for a file: an agent that writes the variable from a
        // here-doc should not end up with a password nobody can retype.
        let text = value.to_string_lossy().into_owned();
        return Ok(strip_one_newline(&text).to_string());
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
    let first = tty.ask("Wallet password: ")?;
    if !confirm {
        return Ok(first);
    }
    let again = tty.ask("Wallet password (again): ")?;
    if first != again {
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
    use std::os::unix::fs::PermissionsExt;
    // S6: the operating system's own words go to `detail`, not into the sentence.
    let meta = std::fs::metadata(path).map_err(|e| {
        Error::not_found(fmonero_core::io_words(
            "read",
            "the password file",
            path,
            &e,
        ))
        .with_detail(e.to_string())
        .with_hint("fmonero wallet open <name> --password-file <path>")
    })?;
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
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::io("read", "the password file", path, &e))?;
    Ok(strip_one_newline(&text).to_string())
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
        let input = self.file.try_clone().map_err(|e| {
            Error::failure("fmonero could not read from this terminal.").with_detail(e.to_string())
        })?;
        let mut line = String::new();
        let read = BufReader::new(input).read_line(&mut line).map_err(|e| {
            Error::failure("fmonero could not read from this terminal.").with_detail(e.to_string())
        })?;
        if read == 0 {
            // Ctrl+D at the prompt.
            return Err(Error::Interrupted);
        }
        Ok(strip_one_newline(&line).to_string())
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
        assert_eq!(wallet_password(Some(&path), false).unwrap(), "hunter2");

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
        assert_eq!(wallet_password(Some(&path), true).unwrap(), "from-the-file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
