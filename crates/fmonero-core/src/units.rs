//! The two systemd **user** units, and the §2i layout detection.
//!
//! Types frozen at the Phase 1a gate; bodies are Phase 1b's (§8). The rules the bodies must
//! honour, restated here because getting one of them wrong corrupts an LMDB (§1.2):
//!
//! * [`data_dir_in_use`] probes with `fcntl(F_GETLK)` and **`l_type: F_WRLCK`**, never
//!   `flock`: LMDB locks the environment with POSIX record locks
//!   (`mdb.c:5083-5099`), a running `monerod` holds a **shared** lock on that byte
//!   (`mdb_env_share_locks`, `mdb.c:5022-5057`), and only a write-lock *query* conflicts
//!   with a held read lock. An `flock` probe — or an `F_GETLK{F_RDLCK}` one — reports a live
//!   data dir as free, and `node up` then starts a second writer.
//! * It is used **only** by `node up` (as a start-time precondition) and by the doctor's
//!   `node_data_dir_single_writer`, **never by [`detect_layout`]** (§2i): it cannot tell our
//!   own monerod from a foreign one, so folding it into layout detection would disable every
//!   node mutator the moment our own node starts.
//! * [`install`] refuses to run at all when `FMONERO_STATE_DIR` is set, so a test can never
//!   half-write a real unit (§4.2).
//! * [`ensure_running`] **probes first** and starts the unit only when the probe fails —
//!   "start, then probe" would spawn `systemctl start` against a unit that does not exist on
//!   every AMBER wallet row of §4b.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::context::Context;
use crate::error::Error;
use crate::proc::CancelToken;
use crate::rpc::Login;

/// Which node fmonero is looking at (§2i). **Configuration, not occupancy.**
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    UserUnit,
    SystemUnit,
    Remote,
    None,
}

impl Layout {
    /// The wire spelling, which §5's `node.layout` promises.
    pub fn as_str(self) -> &'static str {
        match self {
            Layout::UserUnit => "user_unit",
            Layout::SystemUnit => "system_unit",
            Layout::Remote => "remote",
            Layout::None => "none",
        }
    }
}

/// What `systemctl --user is-active` / `is-enabled` and a file check say about one unit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnitState {
    pub name: String,
    pub exists: bool,
    pub active: bool,
    pub enabled: bool,
}

impl UnitState {
    /// The state of a unit that is not installed — what §5 reports on a machine where
    /// `setup --units` has never run.
    pub fn missing(name: &str) -> UnitState {
        UnitState {
            name: name.to_string(),
            exists: false,
            active: false,
            enabled: false,
        }
    }
}

/// What [`install`] did, or would do, to one file. This **is** §5c's `setup.actions`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileAction {
    pub what: String,
    pub path: std::path::PathBuf,
    pub status: FileStatus,
    /// Present only for a dry run: the unified diff of the file's current bytes against the
    /// rendered bytes (§4.2), with the `rpc-login=` value replaced by `<redacted>`. `None`
    /// when `Unchanged`.
    pub diff: Option<String>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Written,
    Unchanged,
    Skipped,
    Planned,
}

impl FileStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            FileStatus::Written => "written",
            FileStatus::Unchanged => "unchanged",
            FileStatus::Skipped => "skipped",
            FileStatus::Planned => "planned",
        }
    }
}

/// §2i: user unit / system unit / remote / none. Reads unit files and `systemctl`, and
/// **never** [`data_dir_in_use`].
pub fn detect_layout(_ctx: &Context) -> Result<Layout, Error> {
    Err(Error::unimplemented("units::detect_layout"))
}

/// `systemctl --user is-active` / `is-enabled`, or the fixture file under
/// `Systemctl::Fixture` (§2i).
pub fn state(_ctx: &Context, _unit: &str) -> Result<UnitState, Error> {
    Err(Error::unimplemented("units::state"))
}

/// The node unit (§4.2). Renders for `ctx.settings.node.network`; `%h`/`%S` appear here
/// because systemd expands them in a `.service` file.
pub fn render_node_unit(_ctx: &Context) -> String {
    String::new()
}

/// `~/.config/fmonero/monerod.conf` (§4.2, §2c). **Every path in it is ABSOLUTE**: boost's
/// `program_options` does no specifier expansion, so a `%h` here would make `monerod`
/// create a directory literally named `%h` and resync the chain from zero — a failure no
/// mock can catch, which is why its guard is the permanent unit test
/// `render_node_conf_has_no_systemd_specifiers` (Phase 1b).
pub fn render_node_conf(_ctx: &Context, _login: &Login) -> String {
    String::new()
}

/// The wallet unit (§4.2). `--wallet-dir` is rendered from `settings.wallet.dir` **and
/// therefore always carries the network segment**, never from a template constant, and
/// never as `%h/...`: the doctor's `wallet_dir_matches_unit` compares it against that
/// setting literally.
pub fn render_wallet_unit(_ctx: &Context) -> String {
    String::new()
}

/// Carries an existing valid `rpc-login=fmonero:<32 hex>` through unchanged; draws 16 fresh
/// bytes from `/dev/urandom` only when the line is absent, unreadable or malformed. This is
/// what makes `setup --units` idempotent and what stops a rewrite from locking out a
/// **running** node (§4.2). `rotate` forces a new one.
pub fn existing_or_new_node_login(_ctx: &Context, _rotate: bool) -> Result<Login, Error> {
    Err(Error::unimplemented("units::existing_or_new_node_login"))
}

/// Renders the three files, compares byte-for-byte against what is on disk, and (unless
/// `dry_run`) writes the changed ones (the conf at 0600), daemon-reloads and **disables**
/// both units — they are on-demand (§4.2). Returns one [`FileAction`] per file.
pub fn install(_ctx: &Context, _dry_run: bool) -> Result<Vec<FileAction>, Error> {
    Err(Error::unimplemented("units::install"))
}

pub fn start(_ctx: &Context, _unit: &str, _cancel: &CancelToken) -> Result<(), Error> {
    Err(Error::unimplemented("units::start"))
}

pub fn stop(_ctx: &Context, _unit: &str, _cancel: &CancelToken) -> Result<(), Error> {
    Err(Error::unimplemented("units::stop"))
}

/// Probe first; start the unit only when the probe fails; then poll until it succeeds or the
/// deadline. Returns whether a start was needed. fbittorrent's `backend::ensure_running`
/// shape (`fbittorrent/src/backend.rs:17-29`).
pub fn ensure_running(
    _ctx: &Context,
    _unit: &str,
    _probe: &dyn Fn() -> bool,
    _deadline: Duration,
    _cancel: &CancelToken,
) -> Result<bool, Error> {
    Err(Error::unimplemented("units::ensure_running"))
}

/// Two probes (§2i): `<paths.proc_root>/*/cmdline` for a `monerod` already serving
/// `data_dir`, and `fcntl(F_GETLK)` with **`l_type: F_WRLCK`** on
/// `<data_dir>/lmdb/lock.mdb`. `Some(pid)` names the holder.
pub fn data_dir_in_use(_ctx: &Context, _data_dir: &Path) -> Result<Option<i32>, Error> {
    Err(Error::unimplemented("units::data_dir_in_use"))
}

/// `systemctl --user show -p MainPID <unit>`, so `node up` can tell "our node is already
/// running" (exit 0) from "someone else's monerod holds this data dir" (exit 3, §2i).
pub fn main_pid(_ctx: &Context, _unit: &str) -> Result<Option<i32>, Error> {
    Err(Error::unimplemented("units::main_pid"))
}

/// True only for [`Layout::UserUnit`]. Read by `NodeClient::mutators` (§2i) and by the
/// doctor; it is **not** the thing callers check by hand — the mutators live on a type they
/// cannot construct without it, and there is no `--force` override.
pub fn may_mutate_node(_ctx: &Context) -> Result<bool, Error> {
    Err(Error::unimplemented("units::may_mutate_node"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5b rule 3: `Layout::UserUnit` serializes as `"user_unit"`. A plain derive would emit
    /// `"UserUnit"`, which is a JSON document no section of the design describes — and the
    /// plugin's parser, the doctor's output and the e2e assertions all read this key.
    #[test]
    fn the_layout_spellings_are_snake_case() {
        for (layout, wire) in [
            (Layout::UserUnit, "user_unit"),
            (Layout::SystemUnit, "system_unit"),
            (Layout::Remote, "remote"),
            (Layout::None, "none"),
        ] {
            assert_eq!(serde_json::to_value(layout).unwrap(), wire);
            assert_eq!(layout.as_str(), wire);
            let back: Layout = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(back, layout);
        }
    }

    #[test]
    fn the_file_status_spellings_are_lowercase() {
        for (status, wire) in [
            (FileStatus::Written, "written"),
            (FileStatus::Unchanged, "unchanged"),
            (FileStatus::Skipped, "skipped"),
            (FileStatus::Planned, "planned"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), wire);
            assert_eq!(status.as_str(), wire);
            let back: FileStatus = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn a_missing_unit_reports_itself_as_missing() {
        let u = UnitState::missing("fmonero-monerod.service");
        assert_eq!(u.name, "fmonero-monerod.service");
        assert!(!u.exists && !u.active && !u.enabled);
        let v = serde_json::to_value(&u).unwrap();
        assert_eq!(v["exists"], false);
    }
}
