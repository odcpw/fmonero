//! On-demand application lifetime, independent of a wallet being open.
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use crate::{state, units, CancelToken, Context, Error};
use serde::Serialize;

/// A shared kernel lease exists only while a TUI is alive. Crashes and reboot release it.
pub struct Session {
    _lease: File,
    generation: u64,
}
impl Session {
    pub fn enter(ctx: &Context) -> Result<Self, Error> {
        crate::paths::ensure_private_dir(&ctx.paths.state_dir)?;
        let path = ctx.paths.state_dir.join("app-session.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|e| Error::io("open", "the application session", &path, &e))?;
        let began = Instant::now();
        loop {
            // SAFETY: file owns this valid descriptor for the lifetime of the lease.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
                break;
            }
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::WouldBlock {
                return Err(Error::io("lock", "the application session", &path, &err));
            }
            if began.elapsed() >= Duration::from_millis(200) {
                return Err(Error::timeout(
                    "The application session is busy. Open fmonero again.",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(Self {
            _lease: file,
            generation: generation(ctx)?,
        })
    }
    pub fn stop_requested(&self, ctx: &Context) -> Result<bool, Error> {
        Ok(self.generation != generation(ctx)?)
    }
}

fn generation(ctx: &Context) -> Result<u64, Error> {
    let path = ctx.paths.state_dir.join("app-stop-generation.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| Error::failure("fmonero could not read its application session.")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(Error::io("read", "the application session", &path, &e)),
    }
}

fn frontend_running(ctx: &Context) -> Result<bool, Error> {
    let path = ctx.paths.state_dir.join("app-session.lock");
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(Error::io("open", "the application session", &path, &e)),
    };
    // SAFETY: file owns a valid descriptor; nonblocking probe never waits for a TUI.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(false); // Closing file immediately releases the probe lock.
    }
    let err = std::io::Error::last_os_error();
    if err.kind() == std::io::ErrorKind::WouldBlock {
        return Ok(true);
    }
    Err(Error::io("lock", "the application session", &path, &err))
}

#[derive(Serialize)]
pub struct Status {
    pub running: bool,
    pub frontend_running: bool,
    pub wallet_running: bool,
    pub node_running: bool,
}
/// Local lifetime checks only: no RPC, remote probe, start, or wallet read.
pub fn status(ctx: &Context) -> Result<Status, Error> {
    let frontend_running = frontend_running(ctx)?;
    let wallet_running = units::state(ctx, &ctx.settings.wallet.unit)?.active;
    let node_running = units::state(ctx, &ctx.settings.node.unit)?.active;
    Ok(Status {
        running: frontend_running || wallet_running || node_running,
        frontend_running,
        wallet_running,
        node_running,
    })
}

/// Save/stop the owned services, then ask every existing TUI to exit cooperatively.
pub fn stop(ctx: &Context, cancel: &CancelToken) -> Result<(), Error> {
    let _lock = crate::send::take_send_lock(ctx)?;
    crate::send::refuse_if_send_in_flight(ctx)?;
    let next = generation(ctx)?
        .checked_add(1)
        .ok_or_else(|| Error::failure("The application session counter is full."))?;
    for unit in [&ctx.settings.wallet.unit, &ctx.settings.node.unit] {
        let info = units::state(ctx, unit)?;
        if info.exists || info.active {
            units::stop(ctx, unit, cancel)?;
        }
    }
    state::clear_last_open(ctx)?;
    crate::write_private(
        &ctx.paths.state_dir.join("app-stop-generation.json"),
        &serde_json::to_vec(&next)
            .map_err(|_| Error::failure("Could not close the application session."))?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context(name: &str, rows: &str) -> Context {
        let path = std::env::temp_dir().join(format!(
            "fmonero-app-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        let mut ctx = Context::for_test(&path).unwrap();
        let fixture = path.join("systemctl.txt");
        std::fs::write(&fixture, rows).unwrap();
        ctx.systemctl = crate::Systemctl::Fixture(fixture);
        ctx
    }
    #[test]
    fn leases_release_on_exit_and_shutdown_does_not_close_a_later_session() {
        let ctx = context("leases", "");
        assert!(!status(&ctx).unwrap().running);
        let first = Session::enter(&ctx).unwrap();
        let second = Session::enter(&ctx).unwrap();
        assert!(status(&ctx).unwrap().frontend_running);
        stop(&ctx, &CancelToken::new()).unwrap();
        assert!(first.stop_requested(&ctx).unwrap());
        assert!(second.stop_requested(&ctx).unwrap());
        let later = Session::enter(&ctx).unwrap();
        assert!(!later.stop_requested(&ctx).unwrap());
        drop(first);
        drop(second);
        assert!(status(&ctx).unwrap().running);
        drop(later);
        assert!(!status(&ctx).unwrap().running);
    }
    #[test]
    fn stopping_saves_the_session_and_stops_both_owned_services() {
        let ctx = context("stop", "fmonero-wallet-rpc.service active disabled yes 42\nfmonero-monerod.service active disabled yes 43\nmonerod.service active enabled yes 44\n");
        state::set_last_open(&ctx, "example", crate::NetType::Mainnet).unwrap();
        assert!(status(&ctx).unwrap().running);
        stop(&ctx, &CancelToken::new()).unwrap();
        assert!(!status(&ctx).unwrap().running);
        assert!(state::last_open(&ctx).unwrap().is_none());
        // A separately-managed system node was never a shutdown target.
        if let crate::Systemctl::Fixture(path) = &ctx.systemctl {
            assert!(std::fs::read_to_string(path)
                .unwrap()
                .contains("monerod.service active enabled yes 44"));
        }
    }
    #[test]
    fn installation_disables_existing_enabled_units_without_stopping_them() {
        let ctx = context("disable", "fmonero-wallet-rpc.service active enabled yes 42\nfmonero-monerod.service inactive enabled yes 0\n");
        for unit in [&ctx.settings.wallet.unit, &ctx.settings.node.unit] {
            units::disable_autostart(&ctx, unit, &CancelToken::new()).unwrap();
            assert!(!units::state(&ctx, unit).unwrap().enabled);
        }
        assert!(
            units::state(&ctx, &ctx.settings.wallet.unit)
                .unwrap()
                .active
        );
    }
}
