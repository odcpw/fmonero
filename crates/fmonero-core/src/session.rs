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
    #[derive(Clone, Debug)]
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
        ctx: &Context,
        w: &WalletClient,
        name: &str,
        password: String,
        cancel: &CancelToken,
    ) -> Result<Opened, Error> {
        // The units are written DISABLED and started on demand (§4.2), and `ensure_running`
        // probes first — so this is a no-op against `fmonero-mock` with no unit installed.
        let unit_started = crate::units::ensure_running(
            ctx,
            &ctx.settings.wallet.unit,
            &|| w.get_version().is_ok(),
            ctx.timeouts.unit_probe,
            cancel,
        )?;
        let opened = open_already_running(ctx, w, name, password);
        // `password` is moved in and zeroed before return (§5b). It is owned, so this is the
        // only copy; `open_already_running` takes it by value and drops it.
        opened.map(|net| Opened {
            unit_started,
            name: name.to_string(),
            net,
        })
    }

    /// `open_wallet`, then the three steps that must follow it.
    ///
    /// `wallet create` reaches the same three through [`open`] — `on_create_wallet` ends
    /// `m_wallet = wal.release()` (`wallet_rpc_server.cpp:3639`), so the new wallet is left
    /// open, and re-opening it with its own password is what lets one code path install the
    /// daemon credential and record the name for both commands (§4.1).
    fn open_already_running(
        ctx: &Context,
        w: &WalletClient,
        name: &str,
        mut password: String,
    ) -> Result<NetType, Error> {
        let result = w.open_wallet(name, &password);
        // Overwrite the buffer before it is freed (§2b.4's rule, applied to the CLI path too).
        // SAFETY-adjacent: `String` may reallocate on push, so write over the bytes in place.
        unsafe {
            for b in password.as_mut_vec().iter_mut() {
                *b = 0;
            }
        }
        drop(password);
        result?;
        install_daemon_and_record(ctx, w, name)
    }

    /// `set_daemon` carrying the node credential, then `auto_refresh`, then
    /// `state::set_last_open`. **In that order**: `on_set_daemon` answers `-13 NOT_OPEN` with
    /// no wallet loaded (`wallet_rpc_server.cpp:4692`), so it can only come after the open.
    fn install_daemon_and_record(
        ctx: &Context,
        w: &WalletClient,
        name: &str,
    ) -> Result<NetType, Error> {
        let login = crate::rpc::Login::from_monerod_conf(&ctx.paths.monerod_conf())?;
        let address = crate::units::host_port(&ctx.settings.node.url);
        if let Err(e) = w.set_daemon(&address, true, login.as_ref()) {
            // §4.2 exempts exactly ONE failure: `-38 NO_DAEMON_CONNECTION`, raised when
            // wallet2 cannot reach the address (`wallet_rpc_server.cpp:4757-4761`) — the
            // wallet is open, it simply has no daemon yet. Exit 4 is a whole class, not that
            // one code: a 401 (`auth_refused`), `-7 DENIED`, `-48 DISABLED` and `-23
            // NO_WALLET_DIR` all land there, and swallowing them reports a successful
            // `wallet open` for a wallet left with NO daemon credential — the step §4.2 calls
            // the one "whose omission means the wallet can never sync". `-38` and a transport
            // failure are the two that carry `reason: unreachable` (§1b.4, jsonrpc.rs).
            if e.reason() != Some("unreachable") {
                return Err(e);
            }
            let _ = crate::state::set_last_error(ctx, &e, None);
        }
        // Explicit rather than inherited, and deliberately NOT raised: a larger period means
        // longer blocking bursts, not fewer stalls (§2d).
        w.auto_refresh(true, AUTO_REFRESH_PERIOD_SEC)?;
        // The wallet's OWN nettype, from its own primary address — the §2a.6 send-path
        // authority, which cannot drift from the daemon the way `settings.node.network` can.
        let net = crate::address::parse(&w.get_address(ctx.settings.wallet.account)?.primary)?.net;
        crate::state::set_last_open(ctx, name, net)?;
        Ok(net)
    }

    /// `DEFAULT_AUTO_REFRESH_PERIOD` (`wallet_rpc_server.cpp:63`), sent explicitly (§2d).
    pub const AUTO_REFRESH_PERIOD_SEC: u32 = 20;

    /// `close_wallet { autosave_current: true }` then `state::clear_last_open`.
    pub fn close(ctx: &Context, w: &WalletClient) -> Result<(), Error> {
        w.close_wallet()?;
        crate::state::clear_last_open(ctx)
    }
}

/// `node up` / `node down` / `node restart` (§4.1, §2i).
pub mod node_session {
    use super::*;

    /// What a start produced.
    #[derive(Clone, Debug)]
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
        ctx: &Context,
        wait: Option<Duration>,
        cancel: &CancelToken,
    ) -> Result<Started, Error> {
        let layout = crate::units::detect_layout(ctx)?;
        require_user_unit(layout)?;
        let unit = ctx.settings.node.unit.clone();

        // The §2i single-writer precondition — the only guard against two `monerod` on one
        // LMDB (§1.2). It is a precondition of `up`, never a term in `detect_layout`: it
        // cannot tell our own monerod from a foreign one.
        let mut already_running = false;
        if let Some(pid) = crate::units::data_dir_in_use(ctx, &ctx.settings.node.data_dir)? {
            match crate::units::main_pid(ctx, &unit)? {
                Some(ours) if ours == pid => already_running = true,
                _ => {
                    return Err(Error::safety(format!(
                        "monerod pid {pid} already has {} open",
                        ctx.settings.node.data_dir.display()
                    ))
                    .with_reason("data_dir_in_use")
                    .with_hint(
                        "fmonero status --json   # a monerod is already serving this data dir",
                    ))
                }
            }
        }
        let started_at = std::time::Instant::now();
        if !already_running {
            crate::units::start(ctx, &unit, cancel)?;
        }
        // Return as soon as one `get_info` answers, waiting at most `timeouts.unit_probe`.
        let n = crate::rpc::NodeClient::from_ctx(ctx)?;
        let info;
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Interrupted);
            }
            if let Ok(i) = n.get_info() {
                info = i;
                break;
            }
            if started_at.elapsed() >= ctx.timeouts.unit_probe {
                return Err(Error::timeout(format!(
                    "{unit} did not answer get_info within {}s",
                    ctx.timeouts.unit_probe.as_secs()
                ))
                .with_hint("fmonero status --json"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let mut info = info;
        if let Some(bound) = wait {
            // `--wait` polls until `synchronized`, bounded by `--wait-timeout`. There is no
            // unbounded default: a command that can block for days is one an agent cannot
            // call (§2d).
            let waiting_since = std::time::Instant::now();
            while !info.synchronized {
                if cancel.is_cancelled() {
                    return Err(Error::Interrupted);
                }
                // `--wait-timeout 0` is the explicit opt-in to "poll until interrupted", and
                // `main.rs` has already refused it without a TTY (§4.1).
                if !bound.is_zero() && waiting_since.elapsed() >= bound {
                    let pct = sync_percent(&info);
                    return Err(Error::timeout(format!(
                        "still syncing at {pct}% after {}s",
                        waiting_since.elapsed().as_secs()
                    ))
                    .with_hint(
                        "fmonero node up --wait --wait-timeout 86400   # or: fmonero status --json",
                    ));
                }
                std::thread::sleep(Duration::from_secs(1));
                info = n.get_info()?;
            }
        }
        Ok(Started {
            already_running,
            waited_sec: started_at.elapsed().as_secs(),
            reachable: true,
            synchronized: Some(info.synchronized),
        })
    }

    /// `NodeClient::mutators(ctx)?.stop_daemon()` if reachable — so LMDB closes cleanly —
    /// then `units::stop`. Layout `UserUnit` only; the mutators type **is** the gate (§2i).
    pub fn down(ctx: &Context, cancel: &CancelToken) -> Result<(), Error> {
        // `down` and `restart` require only the layout; they never consult `data_dir_in_use`,
        // because the holder of our own data dir is our own node (§2i).
        require_user_unit(crate::units::detect_layout(ctx)?)?;
        let n = crate::rpc::NodeClient::from_ctx(ctx)?;
        // The RPC first, so LMDB closes cleanly; a node that is already down simply does not
        // answer, which is not a failure of `node down`.
        if let Ok(m) = n.mutators(ctx) {
            let _ = m.stop_daemon();
        }
        crate::units::stop(ctx, &ctx.settings.node.unit, cancel)
    }

    /// `down` then `up`.
    pub fn restart(ctx: &Context, cancel: &CancelToken) -> Result<Started, Error> {
        down(ctx, cancel)?;
        up(ctx, None, cancel)
    }

    fn require_user_unit(layout: crate::units::Layout) -> Result<(), Error> {
        match layout {
            crate::units::Layout::UserUnit => Ok(()),
            crate::units::Layout::SystemUnit => Err(Error::safety(
                "this monerod is Arch's system unit, which fmonero uses read-only and never fights",
            )
            .with_reason("foreign_node")
            .with_hint("sudo systemctl start monerod   # fmonero does not use sudo")),
            crate::units::Layout::Remote => Err(Error::usage(
                "node.url is not loopback, so there is no local unit for fmonero to start",
            )
            .with_reason("non_loopback_url")
            .with_hint(
                "fmonero wallet set-daemon <addr>   # a remote node is reached by monero-wallet-rpc, not by fmonero",
            )),
            crate::units::Layout::None => Err(Error::backend(
                "no fmonero node unit is installed",
            )
            .with_reason("unit_missing")
            .with_hint("fmonero setup --units")),
        }
    }

    /// The §5 formula, which has to special-case `target_height == 0` — it is **0 when
    /// synced**, not equal to `height` (§1b.3).
    fn sync_percent(info: &crate::rpc::node::GetInfo) -> u64 {
        if info.synchronized || info.target_height == 0 {
            return 100;
        }
        info.height * 100 / info.target_height.max(1)
    }
}
