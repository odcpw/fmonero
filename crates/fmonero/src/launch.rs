//! Desktop destinations. A specific destination opens a fresh window so it cannot
//! accidentally focus a TUI that has since navigated to another page.
use clap::ValueEnum;
use fmonero_core::{proc, CancelToken, Error};
use std::ffi::OsString;
use std::time::Duration;

// A major/minor upgrade opens its own fresh instance instead of focusing an old
// process that still has the previous feature and payment-guard implementation.
const APP_ID: &str = concat!(
    "--app-id=org.odcpw.fmonero.v",
    env!("CARGO_PKG_VERSION_MAJOR"),
    "_",
    env!("CARGO_PKG_VERSION_MINOR")
);

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Intent {
    #[default]
    Wallet,
    Node,
    Send,
    Logs,
    Receive,
    Requests,
    NewAddress,
    Restore,
    Tools,
}

impl Intent {
    pub fn flag(self) -> Option<&'static str> {
        match self {
            Self::Wallet => None,
            Self::Node => Some("--node"),
            Self::Send => Some("--send"),
            Self::Logs => Some("--log"),
            Self::Receive => Some("--receive"),
            Self::Requests => Some("--requests"),
            Self::NewAddress => Some("--new-address"),
            Self::Restore => Some("--restore"),
            Self::Tools => Some("--tools"),
        }
    }

    fn command(self, binary: OsString) -> (&'static str, Vec<OsString>) {
        if let Some(flag) = self.flag() {
            (
                "omarchy-launch-tui",
                vec![APP_ID.into(), binary, "tui".into(), flag.into()],
            )
        } else {
            // This Omarchy helper joins/evaluates its arguments. Pass only fixed
            // literals; arbitrary paths remain on the argv-only helper above.
            (
                "omarchy-launch-or-focus-tui",
                vec![APP_ID.into(), "fmonero".into(), "tui".into()],
            )
        }
    }
}

pub fn run(intent: Intent, cancel: &CancelToken) -> Result<(), Error> {
    let binary = std::env::current_exe()
        .map_err(|_| Error::failure("Could not find the installed fmonero program."))?;
    let (helper, args) = intent.command(binary.into_os_string());
    let mut argv = vec![helper];
    for arg in &args {
        argv.push(
            arg.to_str()
                .ok_or_else(|| Error::usage("The installed program path is not valid text."))?,
        );
    }
    let result = proc::run_with_deadline(&argv, Duration::from_secs(10), Some(cancel))?;
    if result.success() {
        Ok(())
    } else if result.timed_out {
        Err(Error::timeout(
            "The desktop did not open the terminal in time.",
        ))
    } else {
        Err(
            Error::failure("The desktop could not open the fmonero terminal.")
                .with_hint("Run fmonero tui in a terminal."),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_are_argv_and_always_open_the_requested_page() {
        let binary = OsString::from("/tmp/space in path/fmonero");
        for intent in [
            Intent::Node,
            Intent::Send,
            Intent::Logs,
            Intent::Receive,
            Intent::Requests,
            Intent::NewAddress,
            Intent::Restore,
            Intent::Tools,
        ] {
            let (helper, args) = intent.command(binary.clone());
            assert_eq!(helper, "omarchy-launch-tui");
            assert_eq!(args[1], binary);
            assert_eq!(args.last().unwrap(), intent.flag().unwrap());
        }
        let (helper, args) = Intent::Wallet.command(binary);
        assert_eq!(helper, "omarchy-launch-or-focus-tui");
        assert_eq!(args.len(), 3);
    }
}
