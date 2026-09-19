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

    /// A context whose **wallet probe deadline is `timeouts.refresh`** — the one the
    /// `wallet create` and `wallet open` sequences run under (v5.3, §4.2, §2m.3).
    ///
    /// `on_create_wallet` leaves the new wallet open and `monero-wallet-rpc`'s idle handler
    /// immediately starts refreshing it from the restore height the daemon just handed it
    /// (`wallet_rpc_server.cpp:3596`, `:3606-3609`), and in remote mode those are round trips
    /// to a stranger's node over the internet. **Every call of the sequence that follows is
    /// then waiting behind that refresh** — not only `set_daemon`, which was the first one
    /// observed to be killed: a live rehearsal against a real public node also killed
    /// `ensure_running`'s own `get_version` probe (reported as `unit_missing`, a daemon that is
    /// running and merely busy) and `auto_refresh`. Under the 5 s `timeouts.wallet_sec` probe
    /// deadline one `wallet create` in three failed **after the wallet file existed**, which is
    /// the worst shape a failure can have here.
    ///
    /// `timeouts.refresh` (120 s) rather than `timeouts.open` (300 s) because the thing being
    /// waited on **is a refresh**: it is this design's one budget for "the wallet is fetching
    /// blocks from a daemon", while `open_sec` is the budget for decrypting a wallet file — a
    /// different quantity, five minutes long, during which a command would look like a hang.
    ///
    /// A daemon that is **down** is unaffected: a refused connection is an error, not a
    /// deadline, so `wallet create` against a dead `monero-wallet-rpc` still fails in
    /// milliseconds. `--timeout N` lowers both deadlines before this runs, so `max` can never
    /// raise one above the ceiling the user set (§4).
    #[must_use]
    pub fn with_wallet_probe_at_refresh(mut self) -> Context {
        self.timeouts.wallet = self.timeouts.wallet.max(self.timeouts.refresh);
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
    ///
    /// **And the settings belong to the sandbox too, never to the machine** (v5.4, §5b.3).
    /// Until v5.4 this started from [`Context::load`], which reads the real
    /// `~/.config/fmonero/config.toml`, and only then moved the paths — so every test context
    /// carried whatever `node.mode` the owner had set, and the suite was green only while that
    /// was the default `local`. The day the owner's installation switched to remote mode (§2m)
    /// 34 tests that assume the default failed on a clean tree, deterministically, and with
    /// `cargo test` stopping at the first failing binary the §2a.11 `send.lock` suite never
    /// ran at all. `Settings::load` against the sandbox's config dir, which holds no
    /// `config.toml` yet, is the defaults exactly as the binary computes them on a fresh
    /// machine, with `wallet.dir` derived from the sandbox's own `wallet_dir` (§4.2). A test
    /// that a correct `node remote --yes` breaks asserts the host, not fmonero.
    pub fn for_test(dir: &std::path::Path) -> Result<Context, Error> {
        let mut paths = Paths::discover()?;
        paths.state_dir = dir.join("state");
        paths.config_dir = dir.join("config");
        paths.cache_dir = dir.join("cache");
        paths.run_dir = paths.state_dir.join("run");
        paths.systemd_user_dir = dir.join("systemd/user");
        paths.local_bin = dir.join("bin");
        paths.wallet_dir = dir.join("wallets");
        // W21: and the chain folder and the log, which `Settings::load` derives from this.
        paths.bitmonero_dir = dir.join("bitmonero");
        let settings = Settings::load(&paths)?;
        let timeouts = Timeouts::from_settings(&settings.timeouts);
        let mut ctx = Context {
            settings,
            paths,
            timeouts,
            json: false,
            verbose: 0,
            force: false,
            systemctl: Systemctl::discover(),
        };
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

    /// v5.3, §4.2: the create/open sequences run under `timeouts.refresh`, and `--timeout N`
    /// still wins. `max` rather than a plain assignment is what makes the second half true —
    /// `clamp_to` has already lowered both by the time this runs, so the raise can never climb
    /// back above the ceiling the user set (§4).
    #[test]
    fn the_create_and_open_probe_deadline_is_the_refresh_one_and_timeout_still_lowers_it() {
        let ctx = Context::load()
            .expect("context")
            .with_wallet_probe_at_refresh();
        assert_eq!(ctx.timeouts.wallet, ctx.timeouts.refresh);
        assert!(
            ctx.timeouts.wallet > std::time::Duration::from_secs(5),
            "the 5 s probe deadline is what killed a create after the wallet existed"
        );

        let clamped = Context::load()
            .expect("context")
            .with_flags(true, 0, Some(2), false)
            .with_wallet_probe_at_refresh();
        assert_eq!(clamped.timeouts.wallet, std::time::Duration::from_secs(2));
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
        // W21: nor the owner's chain folder, and nor his node's log — the two defaults that
        // `FMONERO_STATE_DIR` used to leave pointing at `~/.bitmonero`.
        assert!(
            ctx.settings.node.data_dir.starts_with(&dir),
            "nor ~/.bitmonero: {}",
            ctx.settings.node.data_dir.display()
        );
        assert!(
            ctx.settings.node.log_file.starts_with(&dir),
            "nor the owner's node log: {}",
            ctx.settings.node.log_file.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
