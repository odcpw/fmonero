//! Every path fmonero reads or writes, in one place (DESIGN.md §3, §5b).
//!
//! Ported from `fproton/crates/fproton-core/src/paths.rs`. `FMONERO_STATE_DIR`,
//! `FMONERO_CONFIG_DIR` and `FMONERO_CACHE_DIR` override the XDG locations so every test
//! and every §4b row can run in its own directory; `FMONERO_PROC_ROOT` overrides `/proc`
//! for the §2i detection fixtures and `FMONERO_SYSTEMD_USER_DIR` overrides
//! `~/.config/systemd/user`, and both are honoured **only** when `FMONERO_STATE_DIR` is
//! also set.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Error;

/// Resolved locations. Plain data: cloneable, no open handles.
#[derive(Clone, Debug, Serialize)]
pub struct Paths {
    /// `$XDG_STATE_HOME/fmonero` — `state.json`, `pending.json`, `send.lock`, doctor runs.
    pub state_dir: PathBuf,
    /// `$XDG_CONFIG_HOME/fmonero` — `config.toml` and `monerod.conf` (§2c).
    pub config_dir: PathBuf,
    /// `$XDG_CACHE_HOME/fmonero` — the postcard history snapshot.
    pub cache_dir: PathBuf,
    /// The **default root** of the per-network wallet directories,
    /// `$XDG_DATA_HOME/fmonero/wallets`. The authority for every command is
    /// `settings.wallet.dir` (`…/wallets/<network>`, §4), which is also what the wallet
    /// unit's `--wallet-dir` carries; this field only names where that default lives.
    pub wallet_dir: PathBuf,
    /// `state_dir/run` — `monero-wallet-rpc`'s working directory, where it writes its own
    /// login file (§2c). Follows `FMONERO_STATE_DIR`, so a test's `run_dir` and a real
    /// unit's `WorkingDirectory` deliberately diverge (§4.2).
    pub run_dir: PathBuf,
    /// `~/.bitmonero` — Monero's own default data dir (§1b.1).
    pub bitmonero_dir: PathBuf,
    /// `~/.config/systemd/user` — where `setup --units` writes the two units (§4.2) — or
    /// `FMONERO_SYSTEMD_USER_DIR` when `FMONERO_STATE_DIR` is also set (§2i's test seam,
    /// widened in v5.1). Without the override no test can make `units::detect_layout` or
    /// `units::preflight` see a machine that has **no** units, because `Paths::discover`
    /// reads the real `XDG_CONFIG_HOME` and `units::install` refuses to write a unit under
    /// a test state dir at all (§4.2) — which is how a set-up machine made ten assertions
    /// of the suite fail on their own premise.
    pub systemd_user_dir: PathBuf,
    /// `~/.config/omarchy` — the plugin directory and the menu extension live below this.
    pub omarchy_config_dir: PathBuf,
    /// `~/.local/bin` — where the shell's PATH expects the binary (§1.5).
    pub local_bin: PathBuf,
    /// `~` — used to build the paths above and to render absolute paths into the units.
    pub home: PathBuf,
    /// `/proc`, or `FMONERO_PROC_ROOT` when `FMONERO_STATE_DIR` is also set (§2i).
    pub proc_root: PathBuf,
}

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// True when the process is running against a test state directory. The §2i overrides
/// (`FMONERO_PROC_ROOT`, `FMONERO_SYSTEMCTL_FIXTURE`, `FMONERO_SYSTEMD_USER_DIR`) are
/// honoured only then, which is the same guard `units::install` uses to refuse writing a
/// real unit.
pub fn under_test_state_dir() -> bool {
    env_dir("FMONERO_STATE_DIR").is_some()
}

/// True when the systemd user directory is itself redirected — `FMONERO_SYSTEMD_USER_DIR`, under
/// the [`under_test_state_dir`] guard (§2i, v5.1).
///
/// It is what lets `units::install` write the two unit files **inside the sandbox** instead of
/// refusing outright: the rule §4.2 states is "a test never installs a REAL unit", and a unit
/// written into a redirected directory is not one. The `systemctl` calls that follow are
/// suppressed separately, by `Systemctl::Fixture` (§2i) — a sandboxed *file* is harmless, a
/// `systemctl --user disable` against the owner's manager is not.
pub fn systemd_user_dir_is_sandboxed() -> bool {
    under_test_state_dir() && env_dir("FMONERO_SYSTEMD_USER_DIR").is_some()
}

impl Paths {
    /// Resolve every path from the environment. Creates nothing.
    pub fn discover() -> Result<Paths, Error> {
        let home =
            dirs::home_dir().ok_or_else(|| Error::failure("no home directory (HOME is unset)"))?;
        let xdg_config = dirs::config_dir().unwrap_or_else(|| home.join(".config"));
        let state_dir = env_dir("FMONERO_STATE_DIR").unwrap_or_else(|| {
            dirs::state_dir()
                .unwrap_or_else(|| home.join(".local/state"))
                .join("fmonero")
        });
        let config_dir =
            env_dir("FMONERO_CONFIG_DIR").unwrap_or_else(|| xdg_config.join("fmonero"));
        let cache_dir = env_dir("FMONERO_CACHE_DIR").unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| home.join(".cache"))
                .join("fmonero")
        });
        let wallet_dir = dirs::data_dir()
            .unwrap_or_else(|| home.join(".local/share"))
            .join("fmonero/wallets");
        let proc_root = match env_dir("FMONERO_PROC_ROOT") {
            Some(p) if under_test_state_dir() => p,
            _ => PathBuf::from("/proc"),
        };
        // The same guard, for the same reason: a seam that worked without a test state dir
        // would let an ordinary invocation be pointed at a directory of unit files.
        let systemd_user_dir = match env_dir("FMONERO_SYSTEMD_USER_DIR") {
            Some(p) if under_test_state_dir() => p,
            _ => xdg_config.join("systemd/user"),
        };
        Ok(Paths {
            run_dir: state_dir.join("run"),
            state_dir,
            config_dir,
            cache_dir,
            wallet_dir,
            bitmonero_dir: home.join(".bitmonero"),
            systemd_user_dir,
            omarchy_config_dir: xdg_config.join("omarchy"),
            local_bin: home.join(".local/bin"),
            home,
            proc_root,
        })
    }

    /// Create every fmonero-owned directory at mode **0700**, and be the only place they
    /// are created (§2b.7). Idempotent; never `chmod`s an existing directory.
    pub fn ensure_dirs(&self) -> Result<(), Error> {
        use std::os::unix::fs::DirBuilderExt;
        for d in [
            &self.state_dir,
            &self.config_dir,
            &self.cache_dir,
            &self.run_dir,
        ] {
            if d.is_dir() {
                continue;
            }
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(d)
                .map_err(|e| Error::failure(format!("cannot create {}: {e}", d.display())))?;
        }
        Ok(())
    }

    // ---- fmonero's own files -------------------------------------------------

    /// `config.toml` (§4's settings table).
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
    /// `state.json` — the 60 s cold cache, the net sample pair, the stall breakers,
    /// `last_open` and `last_error` (§5).
    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }
    /// `state_dir/pending.json` — the §2a.4 money record, in its OWN file so that
    /// `status`'s whole-document rewrite (3 s from the plugin, 1 s from the TUI) can never
    /// lose it.
    pub fn pending_file(&self) -> PathBuf {
        self.state_dir.join("pending.json")
    }
    /// `state_dir/.lock`, 0600 — the `flock` every read-modify-write of `state.json`
    /// holds for its whole duration (§5).
    pub fn state_lock(&self) -> PathBuf {
        self.state_dir.join(".lock")
    }
    /// `state_dir/send.lock`, 0600 — held from build to relay (§2a.11).
    pub fn send_lock(&self) -> PathBuf {
        self.state_dir.join("send.lock")
    }
    /// `config_dir/monerod.conf`, 0600 — it carries `rpc-login` (§2c).
    pub fn monerod_conf(&self) -> PathBuf {
        self.config_dir.join("monerod.conf")
    }
    /// `cache_dir/history.bin` — the postcard transfer snapshot (§5b `history`).
    pub fn history_cache(&self) -> PathBuf {
        self.cache_dir.join("history.bin")
    }
    /// `monero-wallet-rpc`'s own credential file, named after its **bind port**
    /// (`wallet_rpc_server.cpp:351-380`, §2c). fmonero only ever reads it.
    pub fn wallet_login_file(&self, port: u16) -> PathBuf {
        self.run_dir.join(format!("monero-wallet-rpc.{port}.login"))
    }
    /// Verbatim copies taken before any doctor or setup file edit (§7).
    pub fn backups_dir(&self) -> PathBuf {
        self.state_dir.join("backups")
    }
    /// One directory per `doctor --fix` (or `--save`) run (§7).
    pub fn doctor_runs_dir(&self) -> PathBuf {
        self.state_dir.join("doctor/runs")
    }
    /// Symlink to the newest doctor run (§7).
    pub fn doctor_latest(&self) -> PathBuf {
        self.state_dir.join("doctor/latest")
    }

    // ---- files owned by the desktop ------------------------------------------

    /// The user's menu extension, merged textually by `setup --menu` (§4.2).
    pub fn omarchy_menu(&self) -> PathBuf {
        self.omarchy_config_dir
            .join("extensions/omarchy-menu.jsonc")
    }
    /// Where `setup --plugin` links `plugin/oliver.monero` (§4.2).
    pub fn plugin_link(&self) -> PathBuf {
        self.omarchy_config_dir.join("plugins/oliver.monero")
    }
    /// Omarchy's own shell configuration — the file `omarchy plugin enable` and
    /// `omarchy bar put` write, and therefore the one place their effect can be read back
    /// without a running shell (§1.5: "Enabling writes `{\"id\":\"oliver.monero\"}` into
    /// `~/.config/omarchy/shell.json`"). `setup --plugin`'s pre-check reads it so a second
    /// run reports those two steps `unchanged` instead of spawning them again (§4.2).
    pub fn omarchy_shell_json(&self) -> PathBuf {
        self.omarchy_config_dir.join("shell.json")
    }
    /// `<data-dir>/lmdb/lock.mdb` — the file the §2i `fcntl(F_GETLK)` probe opens.
    pub fn lmdb_lock(&self, data_dir: &Path) -> PathBuf {
        data_dir.join("lmdb/lock.mdb")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, because the environment is process-wide: two tests that set
    /// `FMONERO_STATE_DIR` would race each other inside one test binary.
    #[test]
    fn the_env_overrides_move_every_file_and_proc_root_is_guarded() {
        let dir = std::env::temp_dir().join("fmonero-paths-test");
        let fake_proc = std::env::temp_dir().join("fmonero-fake-proc");
        let fake_units = std::env::temp_dir().join("fmonero-fake-units");
        let real_units = Paths::discover().expect("paths").systemd_user_dir;

        // (1) FMONERO_PROC_ROOT and FMONERO_SYSTEMD_USER_DIR alone are ignored: both §2i
        // overrides need a test state dir.
        std::env::remove_var("FMONERO_STATE_DIR");
        std::env::set_var("FMONERO_PROC_ROOT", &fake_proc);
        std::env::set_var("FMONERO_SYSTEMD_USER_DIR", &fake_units);
        let p = Paths::discover().expect("paths");
        assert_eq!(p.proc_root, PathBuf::from("/proc"), "§2i: guarded override");
        assert_eq!(
            p.systemd_user_dir, real_units,
            "§2i: the systemd seam is guarded by the test state dir too"
        );

        // (2) With both set, every fmonero file moves and the override is honoured.
        std::env::set_var("FMONERO_STATE_DIR", &dir);
        let p = Paths::discover().expect("paths");
        std::env::remove_var("FMONERO_STATE_DIR");
        std::env::remove_var("FMONERO_PROC_ROOT");
        std::env::remove_var("FMONERO_SYSTEMD_USER_DIR");
        assert_eq!(p.systemd_user_dir, fake_units);
        assert_eq!(p.state_dir, dir);
        assert_eq!(p.state_file(), dir.join("state.json"));
        assert_eq!(p.pending_file(), dir.join("pending.json"));
        assert_eq!(p.send_lock(), dir.join("send.lock"));
        assert_eq!(
            p.wallet_login_file(18083),
            dir.join("run/monero-wallet-rpc.18083.login")
        );
        assert_eq!(p.proc_root, fake_proc);
    }

    #[test]
    fn the_login_file_is_named_after_the_bind_port() {
        let p = Paths::discover().expect("paths");
        assert!(p
            .wallet_login_file(38083)
            .ends_with("monero-wallet-rpc.38083.login"));
    }
}
