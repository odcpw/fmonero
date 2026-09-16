//! The one value every core call takes (DESIGN.md §5b).
//!
//! Plain data: `Clone + Send + Sync + 'static`, no open handles, no interior state. The
//! TUI owns its own clone; the binary builds one per invocation. Ported from
//! `fproton/crates/fproton-core/src/context.rs`, plus the two fields fmonero needs:
//! `force` (the §4 global flag, which ignores an armed §2d breaker) and `systemctl` (the
//! §2i test seam).

use std::path::PathBuf;

use serde::Serialize;

use crate::error::Error;
use crate::paths::Paths;
use crate::settings::{Settings, Timeouts};

/// How `units::{state, main_pid, start, stop}` and `detect_layout` reach systemd.
///
/// The §2i test seam, frozen at the 1a gate because no agent on this machine can create
/// `/etc/monerod.conf` or a system unit (§1.1). `Fixture` names a file of
/// `<unit> <is-active> <is-enabled> <exists> <main-pid>` lines (`-` for an absent pid) and
/// is honoured **only** when `FMONERO_STATE_DIR` is also set — the same guard
/// `units::install` uses to refuse writing a real unit.
#[derive(Clone, Debug, Serialize)]
pub enum Systemctl {
    Real,
    Fixture(PathBuf),
}

impl Systemctl {
    /// `FMONERO_SYSTEMCTL_FIXTURE`, honoured only under a test state dir (§2i).
    pub fn discover() -> Systemctl {
        match std::env::var_os("FMONERO_SYSTEMCTL_FIXTURE") {
            Some(p) if !p.is_empty() && crate::paths::under_test_state_dir() => {
                Systemctl::Fixture(PathBuf::from(p))
            }
            _ => Systemctl::Real,
        }
    }
}

/// Settings, paths and deadlines, plus how the caller wants to be talked to.
#[derive(Clone, Debug, Serialize)]
pub struct Context {
    pub settings: Settings,
    pub paths: Paths,
    pub timeouts: Timeouts,
    /// The caller asked for machine output. Core never prints; the TUI and the doctor use
    /// it to decide their own rendering.
    pub json: bool,
    /// `-v` repetitions.
    pub verbose: u8,
    /// `--force`: ignore an armed §2d breaker for this invocation. That is **all** it
    /// does — it never bypasses a `pending_send`, the §2i layout gate, a loopback check
    /// or any other money check (§4).
    pub force: bool,
    pub systemctl: Systemctl,
}

impl Context {
    /// Discover paths, read `config.toml`, resolve the deadlines from it.
    pub fn load() -> Result<Context, Error> {
        let paths = Paths::discover()?;
        let settings = Settings::load(&paths)?;
        let timeouts = Timeouts::from_settings(&settings.timeouts);
        Ok(Context {
            settings,
            paths,
            timeouts,
            json: false,
            verbose: 0,
            force: false,
            systemctl: Systemctl::discover(),
        })
    }

    /// Apply the global flags. `--timeout` **lowers** every deadline and never raises one
    /// (§4).
    #[must_use]
    pub fn with_flags(
        mut self,
        json: bool,
        verbose: u8,
        timeout: Option<u64>,
        force: bool,
    ) -> Context {
        self.json = json;
        self.verbose = verbose;
        self.force = force;
        if let Some(secs) = timeout {
            self.timeouts.clamp_to(secs);
        }
        self
    }

    /// A context for tests: every fmonero directory under `dir`, default settings.
    pub fn for_test(dir: &std::path::Path) -> Result<Context, Error> {
        let mut ctx = Context::load()?;
        ctx.paths.state_dir = dir.join("state");
        ctx.paths.config_dir = dir.join("config");
        ctx.paths.cache_dir = dir.join("cache");
        ctx.paths.run_dir = ctx.paths.state_dir.join("run");
        ctx.paths.ensure_dirs()?;
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_is_send_and_sync_and_clonable() {
        fn assert_context<T: Send + Sync + Clone + 'static>() {}
        assert_context::<Context>();
    }

    #[test]
    fn the_timeout_flag_clamps_every_deadline_downwards() {
        let ctx = Context::load()
            .expect("context")
            .with_flags(true, 1, Some(3), true);
        assert_eq!(ctx.timeouts.open, std::time::Duration::from_secs(3));
        assert_eq!(ctx.timeouts.node, std::time::Duration::from_secs(3));
        assert!(ctx.json);
        assert!(ctx.force);
        assert_eq!(ctx.verbose, 1);
    }

    #[test]
    fn a_test_context_puts_every_directory_under_one_root() {
        let dir = std::env::temp_dir().join(format!("fmonero-ctx-{}", std::process::id()));
        let ctx = Context::for_test(&dir).expect("test context");
        assert!(ctx.paths.state_dir.starts_with(&dir));
        assert!(ctx.paths.config_file().starts_with(&dir));
        assert!(ctx.paths.state_dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
