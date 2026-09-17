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
    ///
    /// **Every path a test can observe belongs to `dir`, including the three that are not
    /// fmonero's own** — `systemd_user_dir`, `local_bin` and `settings.wallet.dir`. They
    /// were left pointing at the real machine until v5.1, and the Phase 4 verifier's one
    /// real `scripts/install.sh --yes` then made five unit tests fail on their own premise:
    /// `binary_on_path` saw `~/.local/bin/fmonero`, `detect_layout` saw an installed unit,
    /// and `units::install`'s dry run saw a `wallet.dir` that already existed. A test that a
    /// correct installer breaks is a test that gets deleted, so the sandbox is the fix
    /// rather than the assertions.
    pub fn for_test(dir: &std::path::Path) -> Result<Context, Error> {
        let mut ctx = Context::load()?;
        ctx.paths.state_dir = dir.join("state");
        ctx.paths.config_dir = dir.join("config");
        ctx.paths.cache_dir = dir.join("cache");
        ctx.paths.run_dir = ctx.paths.state_dir.join("run");
        ctx.paths.systemd_user_dir = dir.join("systemd/user");
        ctx.paths.local_bin = dir.join("bin");
        ctx.paths.wallet_dir = dir.join("wallets");
        // `settings.wallet.dir` carries the network segment, exactly as §4's default does —
        // and it deliberately does NOT exist, because `units::install`'s dry run lists one
        // `CreateDir` action per missing ancestor (§4.2) and that is what the test asserts.
        ctx.settings.wallet.dir = ctx
            .paths
            .wallet_dir
            .join(ctx.settings.node.network.as_str());
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

    /// The regression the Phase 4 verifier's install exposed: a test context that could see
    /// the real machine. Every one of these five paths made an assertion fail once
    /// `scripts/install.sh --yes` had run (DECISIONS.md, Phase 4 verification, finding 3).
    #[test]
    fn a_test_context_puts_every_directory_under_one_root() {
        let dir = std::env::temp_dir().join(format!("fmonero-ctx-{}", std::process::id()));
        let ctx = Context::for_test(&dir).expect("test context");
        assert!(ctx.paths.state_dir.starts_with(&dir));
        assert!(ctx.paths.config_file().starts_with(&dir));
        assert!(ctx.paths.state_dir.is_dir());
        assert!(
            ctx.paths.systemd_user_dir.starts_with(&dir),
            "a test may never see the machine's units: {}",
            ctx.paths.systemd_user_dir.display()
        );
        assert!(
            ctx.paths.local_bin.starts_with(&dir),
            "nor ~/.local/bin: {}",
            ctx.paths.local_bin.display()
        );
        assert!(
            ctx.settings.wallet.dir.starts_with(&dir),
            "nor the owner's wallets: {}",
            ctx.settings.wallet.dir.display()
        );
        assert!(
            !ctx.settings.wallet.dir.exists(),
            "and wallet.dir must be ABSENT, or the CreateDir action of §4.2 is unobservable"
        );
        assert!(ctx.settings.wallet.dir.ends_with("mainnet"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
