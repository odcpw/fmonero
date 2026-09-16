//! The ONE implementation of the `wallet open`/`close` and `node up`/`down`/`restart`
//! sequences (DESIGN.md §5b, §4.1, §4.2, §2i).
//!
//! Signature-only at the Phase 1a gate (§8). Both modules exist because each sequence has
//! **two** independently written callers — the CLI subcommand and the TUI's modal or key —
//! and §2a.3 already settles what happens to an invariant that lives in two places: it
//! holds in one. The two steps most easily lost are named in the doc comments below,
//! because losing either is silent.

use std::time::Duration;

use crate::address::NetType;
use crate::context::Context;
use crate::error::Error;
use crate::proc::CancelToken;
use crate::rpc::WalletClient;

/// `wallet open` / `wallet close` (§4.1, §4.2).
pub mod wallet_session {
    use super::*;

    /// What an open produced.
    pub struct Opened {
        pub unit_started: bool,
        pub name: String,
        /// The wallet's **own** nettype, read from its primary address — the §2a.6
        /// send-path authority, which cannot drift from the daemon the way
        /// `settings.node.network` can.
        pub net: NetType,
    }

    /// `units::ensure_running`, then `open_wallet`, then **exactly one**
    /// `set_daemon { address, trusted, username, password }` carrying the node credential
    /// from `Login::from_monerod_conf` (both fields omitted when there is none — the §2i
    /// `system_unit` layout), then `auto_refresh { enable: true, period: 20 }` (§2d), then
    /// `state::set_last_open(name, net)`.
    ///
    /// **The order is not free.** `on_set_daemon` answers `-13 NOT_OPEN` with no wallet
    /// loaded (`wallet_rpc_server.cpp:4692`), so issuing it first would fail every time,
    /// never install the credential, and present as "the node is unreachable" — while
    /// without it every wallet→daemon call against fmonero's own authenticated node gets a
    /// 401 and the wallet can never sync (§4.2). A `-38 NO_DAEMON_CONNECTION` from
    /// `set_daemon` does **not** fail the open: the wallet is open, it simply has no daemon
    /// yet. `password` is moved in and zeroed before return.
    pub fn open(
        _ctx: &Context,
        _w: &WalletClient,
        _name: &str,
        _password: String,
        _cancel: &CancelToken,
    ) -> Result<Opened, Error> {
        Err(Error::unimplemented("wallet_session::open"))
    }

    /// `close_wallet { autosave_current: true }` then `state::clear_last_open`.
    pub fn close(_ctx: &Context, _w: &WalletClient) -> Result<(), Error> {
        Err(Error::unimplemented("wallet_session::close"))
    }
}

/// `node up` / `node down` / `node restart` (§4.1, §2i).
pub mod node_session {
    use super::*;

    /// What a start produced.
    pub struct Started {
        /// `node up` against our own live node: exit 0, no second start (§2i).
        pub already_running: bool,
        pub waited_sec: u64,
        pub reachable: bool,
        pub synchronized: Option<bool>,
    }

    /// `units::detect_layout` (`UserUnit`, else `Error::Safety` with §2i's
    /// `sudo systemctl …` hint) → `units::data_dir_in_use`: no holder ⇒ start; holder equals
    /// `units::main_pid(unit)` ⇒ `already_running: true` with **no second start**; any other
    /// pid ⇒ `Error::Safety{reason:"data_dir_in_use"}` naming it → `units::start` → poll
    /// `get_info` to `timeouts.unit_probe` (`Error::Timeout`, exit 8) → when `wait` is
    /// `Some`, poll `get_info.synchronized` to that duration (`Error::Timeout`, exit 8).
    ///
    /// The single-writer precondition is the only guard against two `monerod` on one LMDB
    /// (§1.2), and it lives here rather than in `units::start` precisely so the TUI's `u`
    /// key cannot skip it.
    pub fn up(
        _ctx: &Context,
        _wait: Option<Duration>,
        _cancel: &CancelToken,
    ) -> Result<Started, Error> {
        Err(Error::unimplemented("node_session::up"))
    }

    /// `NodeClient::mutators(ctx)?.stop_daemon()` if reachable — so LMDB closes cleanly —
    /// then `units::stop`. Layout `UserUnit` only; the mutators type **is** the gate (§2i).
    pub fn down(_ctx: &Context, _cancel: &CancelToken) -> Result<(), Error> {
        Err(Error::unimplemented("node_session::down"))
    }

    /// `down` then `up`.
    pub fn restart(_ctx: &Context, _cancel: &CancelToken) -> Result<Started, Error> {
        Err(Error::unimplemented("node_session::restart"))
    }
}
