//! The §5 Status document — the contract the plugin, the TUI and agents all parse.
//!
//! The **types** are Phase 1a's and are frozen here (§5b); the call set that fills them in
//! is Phase 1b's (§8). At the 1a gate [`status`] returns the **empty document**: every
//! daemon field `null`, `reachable: false`, no warnings — which is the shape §5's rules
//! demand of a machine with nothing running, and which is what the 1a gate asserts.
//!
//! Shape rules, restated because every later phase depends on them: every key is always
//! present; unknown is `null` — never an absent key, never `{}`, never a zero standing in
//! for unknown; timestamps are RFC 3339 with a **local** offset; durations are integers
//! named `*_sec`; every monetary value appears twice, `*_pico` (integer) and `*_xmr`
//! (pre-rendered string).

use serde::{Deserialize, Serialize};

use crate::amount::Piconero;
use crate::context::Context;
use crate::error::Error;
use crate::state::{PendingKind, PendingSend};
use crate::units;

/// The whole §5 document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub ok: bool,
    pub schema: u32,
    pub generated_at: String,
    pub node: NodeStatus,
    pub wallet: WalletStatus,
    pub units: UnitsStatus,
    pub pending: Option<PendingStatus>,
    pub warnings: Vec<Warning>,
    pub last_error: Option<LastError>,
    pub staleness: Staleness,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeStatus {
    /// §2i detection — **configuration, not occupancy**: who holds the data dir is
    /// `node up`'s precondition, not a term here.
    pub layout: units::Layout,
    /// **The address in force** (v5.2): `settings.node.url` in local mode,
    /// `settings.node.remote` — verbatim, scheme included — in remote mode, both through
    /// `units::daemon_target`. It means "where the daemon is", which is what every consumer
    /// already reads it as; a loopback URL nothing contacts would be the §2j failure in the one
    /// field people copy.
    pub url: String,
    /// **v5.2.** `true` in local mode (the unit renders `--trusted-daemon` and `set_daemon`
    /// sends `trusted: true`); `settings.node.remote_trusted` in remote mode. It is here rather
    /// than only in settings because it changes what wallet2 puts *in a request* —
    /// `select_available_outputs_from_histogram` sends the wallet's own unspent amounts when
    /// trusted (`wallet2.cpp:12281-12282`, §1b.9 item 4) — so it is a privacy fact about the
    /// running system, not a preference.
    pub trusted: bool,
    /// **v5.2: `Option`.** `null` in remote mode: fmonero constructs no node client there and
    /// no wallet-RPC method reports the daemon's state (§1b.9 item 5), so `false` would be a
    /// claim and `true` a guess (§5).
    pub reachable: Option<bool>,
    pub restricted: Option<bool>,
    /// What the wire did, never what `monerod.conf` holds (§2j).
    pub authenticated: Option<bool>,
    pub nettype: Option<String>,
    pub height: Option<u64>,
    pub target_height: Option<u64>,
    pub sync_percent: Option<f64>,
    pub synchronized: Option<bool>,
    pub busy_syncing: Option<bool>,
    pub offline: Option<bool>,
    /// `null` when the node is unreachable **or not synchronized**:
    /// `get_last_block_header` is the one call fmonero makes that `CHECK_CORE_READY()`
    /// gates, and it answers `status: "BUSY"` with an all-zero header (§1b.3).
    pub top_block_age_sec: Option<i64>,
    pub outgoing_connections_count: Option<u64>,
    pub incoming_connections_count: Option<u64>,
    pub white_peerlist_size: Option<u64>,
    pub grey_peerlist_size: Option<u64>,
    /// Rendered even on a restricted node, where it is a **lower bound** (§1b.3).
    pub tx_pool_size: Option<u64>,
    pub alt_blocks_count: Option<u64>,
    pub database_size: Option<u64>,
    /// `true` when the node is restricted and rounded `database_size` up to 5 GiB (§1b.3).
    pub database_size_rounded: bool,
    pub free_space: Option<u64>,
    pub hard_fork_version: Option<u8>,
    pub version: Option<String>,
    /// Always `false` under fmonero's own unit, which sets `check-updates=disabled` (§4.2).
    pub update_available: Option<bool>,
    pub uptime_sec: Option<i64>,
    pub net: NetStatus,
    pub fee: FeeStatus,
    /// **v5.5, §2n.4.** The **cached** result of the last `get_info` probe of the configured
    /// remote node, read from `state.json` and **never measured here** — `status` spawns nothing
    /// (§2e). It is the only field in this document whose source is a measurement taken by an
    /// earlier invocation, so its `age_sec` is not decoration: a four-hour-old
    /// `synchronized: true` says nothing about now, and §6.5's row shows the age for that
    /// reason. `null` in local mode, and `null` in remote mode until something probed.
    pub remote_probe: Option<RemoteProbeStatus>,
}

/// [`crate::nodes::Probe`] as §5 renders it: the stored form plus the age, which the stored
/// form cannot carry because it is computed at render time. The same relationship
/// [`PendingStatus`] already has to the `pending.json` record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteProbeStatus {
    pub reachable: bool,
    pub height: Option<u64>,
    pub synchronized: Option<bool>,
    pub restricted: Option<bool>,
    pub nettype: Option<String>,
    pub version: Option<String>,
    pub latency_ms: Option<u64>,
    pub reason: Option<String>,
    pub probed_at: i64,
    pub age_sec: i64,
}

impl RemoteProbeStatus {
    /// `now` is passed rather than read so the whole document shares one clock (§5).
    pub fn from_probe(p: &crate::nodes::Probe, now: i64) -> RemoteProbeStatus {
        RemoteProbeStatus {
            reachable: p.reachable,
            height: p.height,
            synchronized: p.synchronized,
            restricted: p.restricted,
            nettype: p.nettype.clone(),
            version: p.version.clone(),
            latency_ms: p.latency_ms,
            reason: p.reason.map(str::to_string),
            probed_at: p.probed_at,
            age_sec: now.saturating_sub(p.probed_at).max(0),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NetStatus {
    pub total_bytes_in: Option<u64>,
    pub total_bytes_out: Option<u64>,
    pub rx_bps: Option<u64>,
    pub tx_bps: Option<u64>,
    pub limit_up_kbps: Option<i64>,
    pub limit_down_kbps: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FeeStatus {
    /// `get_fee_estimate.fee`, which since HF 15 is `fees[0]` — the **cheapest** tier, not
    /// a default (§1b.3).
    pub per_byte_pico: Option<Piconero>,
    pub fees_pico: Option<Vec<Piconero>>,
    pub quantization_mask: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletStatus {
    pub unit_active: bool,
    pub reachable: bool,
    pub authenticated: Option<bool>,
    pub url: String,
    pub open: bool,
    pub name: Option<String>,
    pub account_index: u32,
    pub primary_address: Option<String>,
    pub height: Option<u64>,
    /// `node.height - wallet.height`, clamped at 0 — **positive means the wallet is
    /// behind**, which is the sign every consumer assumes (§5, §2d, §7.1).
    pub blocks_behind: Option<i64>,
    pub balance_pico: Option<Piconero>,
    pub balance_xmr: Option<String>,
    /// **v5.6: the same amount at four decimals, truncated** —
    /// [`Piconero::to_xmr_short`](crate::amount::Piconero::to_xmr_short).
    ///
    /// It exists for the bar widget's five-word header (§6.4), which rendered
    /// `Wallet open · 1.234567891234 XMR` — 32 characters, against an illustration that showed
    /// `1.23` (DECISIONS V16). §2a.5 forbids QML from doing arithmetic on a monetary value and
    /// shortening a decimal string is arithmetic, so the short form is rendered here, beside the
    /// exact one, and the widget picks the field rather than the substring. Every other surface
    /// keeps `balance_xmr`.
    pub balance_xmr_short: Option<String>,
    pub unlocked_pico: Option<Piconero>,
    pub unlocked_xmr: Option<String>,
    pub blocks_to_unlock: Option<u64>,
    pub time_to_unlock_sec: Option<u64>,
    /// `get_address.addresses.len()` — every generated subaddress. **Not**
    /// `get_balance.per_subaddress.len()`, which holds only funded ones (§5).
    pub subaddress_count: Option<u32>,
    pub accounts: Option<u32>,
    pub rpc_version: Option<String>,
    /// **v5.2.** RFC 3339 for the last `refresh` **fmonero itself issued** — written by
    /// `wallet_session::refresh`, read back from `state.json`. The qualifier is the whole
    /// point: `monero-wallet-rpc`'s own 20 s auto-refresh (§2d) is invisible to fmonero, so a
    /// field named "last refresh" without it would be a claim about the daemon's behaviour we
    /// cannot make (§2j). It matters most in remote mode, where `blocks_behind` is `null` and
    /// this is the only freshness signal left.
    pub last_refresh_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnitsStatus {
    pub node: units::UnitState,
    pub wallet: units::UnitState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Warning {
    pub id: String,
    pub severity: Severity,
    pub text: String,
    /// The CLI's runnable command, one per warning (§9). **Unchanged by v5.6**: agents and a
    /// person reading `fmonero status` in a terminal keep exactly what they had.
    pub hint: String,
    /// **v5.6, §2o.1: the in-app sentence** the TUI and the bar widget render where they used
    /// to render `hint`. `None` where nothing can be done from inside the app, and both
    /// surfaces then show the text alone.
    pub action: Option<String>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warn,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Error => "error",
        }
    }
}

/// Every warning id `status` can raise, in §5's order. Listed by `capabilities`, and
/// **exhaustive**: `plugin_not_linked` belongs to the doctor and never appears here.
pub const WARNING_IDS: &[&str] = &[
    "monero_not_installed",
    // v5.2, §2m: the mode itself (info) and a trusted non-private remote node (warn).
    "remote_node",
    "remote_node_trusted",
    "node_down",
    "node_restricted",
    "node_syncing",
    "node_offline",
    "node_no_peers",
    "node_low_disk",
    "node_update_available",
    "foreign_node_running",
    "wallet_down",
    // v5.5, §5: remote mode with no wallet open — the same fact as `wallet_down`, out of the
    // alarm channel, because the units are written disabled and start on demand (§4.2), so a
    // warning here fires on every correct fresh install.
    "wallet_idle",
    "wallet_closed",
    "wallet_behind",
    "wallet_locked_funds",
    "send_may_be_in_flight",
    "node_stalled",
    "wallet_stalled",
    "auth_missing",
    "nettype_mismatch",
    "history_cache_stale",
];

/// The `blocks_behind` threshold, in **one** constant because four places use it: §2d's
/// stall classification, §5's `wallet_behind` warning, and §7.1's `wallet_daemon_link` and
/// `wallet_daemon_auth`. 100 blocks ≈ 3.3 hours at Monero's 120 s target block time, and
/// comfortably exceeds the 10-block spendable age (§2d).
pub const WALLET_BEHIND_BLOCKS: i64 = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LastError {
    pub at: String,
    pub code: i32,
    pub code_name: String,
    pub text: String,
    pub hint: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Staleness {
    pub node_probe_ms: u64,
    pub wallet_probe_ms: u64,
    pub node_extras_age_sec: Option<u64>,
    pub history_cache_age_sec: Option<u64>,
}

/// The §5 `pending` document — what the plugin's `Model.pendingLabel`, the TUI's banner and
/// `wallet clear-pending`'s payload all parse.
///
/// Derived from [`PendingSend`] by [`status`]; keeping the two apart is what lets the
/// on-disk record stay minimal (§2a.4) while the document carries the derived counts and
/// the pre-rendered `*_xmr` strings the plugin may not compute itself (§2a.5, §6.5).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingStatus {
    pub kind: PendingKind,
    pub dest: String,
    pub account_index: u32,
    pub built_at: i64,
    pub elapsed_sec: i64,
    pub relayed_count: usize,
    pub total_count: usize,
    pub total_amount_pico: Piconero,
    pub total_amount_xmr: String,
    pub total_fee_pico: Piconero,
    pub total_fee_xmr: String,
    pub txs: Vec<PendingTxStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingTxStatus {
    pub txid: String,
    pub amount_pico: Piconero,
    pub amount_xmr: String,
    pub fee_pico: Piconero,
    pub fee_xmr: String,
    pub relayed: bool,
}

impl PendingStatus {
    /// The totals are summed in Rust with [`Piconero::checked_add`] and pre-rendered here,
    /// because a sweep's record holds several transactions and the plugin may not do
    /// arithmetic on money (§2a.5, §6.5).
    pub fn from_record(p: &PendingSend, now: i64) -> PendingStatus {
        let mut total_amount = Piconero(0);
        let mut total_fee = Piconero(0);
        let mut txs = Vec::with_capacity(p.txs.len());
        for t in &p.txs {
            total_amount = total_amount.checked_add(t.amount).unwrap_or(total_amount);
            total_fee = total_fee.checked_add(t.fee).unwrap_or(total_fee);
            txs.push(PendingTxStatus {
                txid: t.txid.clone(),
                amount_pico: t.amount,
                amount_xmr: t.amount.to_xmr_string(),
                fee_pico: t.fee,
                fee_xmr: t.fee.to_xmr_string(),
                relayed: t.relayed,
            });
        }
        PendingStatus {
            kind: p.kind,
            dest: p.dest.clone(),
            account_index: p.account_index,
            built_at: p.built_at,
            elapsed_sec: now.saturating_sub(p.built_at),
            relayed_count: p.txs.iter().filter(|t| t.relayed).count(),
            total_count: p.txs.len(),
            total_amount_pico: total_amount,
            total_amount_xmr: total_amount.to_xmr_string(),
            total_fee_pico: total_fee,
            total_fee_xmr: total_fee.to_xmr_string(),
            txs,
        }
    }
}

impl NodeStatus {
    /// The document a machine with no node produces: `reachable: false` and every daemon
    /// field `null` — never a zero standing in for unknown (§5).
    pub fn unreachable(url: String, layout: units::Layout, trusted: bool) -> NodeStatus {
        NodeStatus {
            layout,
            url,
            trusted,
            reachable: Some(false),
            restricted: None,
            authenticated: None,
            nettype: None,
            height: None,
            target_height: None,
            sync_percent: None,
            synchronized: None,
            busy_syncing: None,
            offline: None,
            top_block_age_sec: None,
            outgoing_connections_count: None,
            incoming_connections_count: None,
            white_peerlist_size: None,
            grey_peerlist_size: None,
            tx_pool_size: None,
            alt_blocks_count: None,
            database_size: None,
            database_size_rounded: false,
            free_space: None,
            hard_fork_version: None,
            version: None,
            update_available: None,
            uptime_sec: None,
            net: NetStatus::default(),
            fee: FeeStatus::default(),
            remote_probe: None,
        }
    }
}

impl WalletStatus {
    /// As [`NodeStatus::unreachable`], for the wallet daemon.
    pub fn unreachable(url: String, account_index: u32) -> WalletStatus {
        WalletStatus {
            unit_active: false,
            reachable: false,
            authenticated: None,
            url,
            open: false,
            name: None,
            account_index,
            primary_address: None,
            height: None,
            blocks_behind: None,
            balance_pico: None,
            balance_xmr: None,
            balance_xmr_short: None,
            unlocked_pico: None,
            unlocked_xmr: None,
            blocks_to_unlock: None,
            time_to_unlock_sec: None,
            subaddress_count: None,
            accounts: None,
            rpc_version: None,
            last_refresh_at: None,
        }
    }
}

/// The §5 document: five hot calls plus a 60 s-cached cold set, starts nothing (§2e), and
/// returns `Ok` even when both daemons are down.
///
/// **Phase 1b writes the call set** (§8). At the 1a gate this is the empty document —
/// honest on this machine, where no Monero binary exists (§1.1): nothing is reachable,
/// nothing is open, and every unknown is `null`. It issues no RPC, spawns no process and
/// writes no state, so it cannot claim anything it has not seen (§2j).
/// §5's `--check-node` (v5.6): **the identical document**, key for key, plus one bounded
/// side effect — a detached check of the configured public node, started only when the cached
/// one is stale, and never waited on.
///
/// It exists because §2e left a public node's height and sync flag with no refresher at all:
/// the bar widget polls `status`, `status` spawns nothing, and the TUI is the only thing that
/// ever checked, so `checked 4h ago` was the normal state of the bar. This call does not make
/// *itself* fresh — it makes the **next** one fresh, and returns at once with what is cached.
///
/// A check that cannot be started is not an error: this is a read-only document that answers
/// while everything else is down (§2e), and a bar widget whose poll failed because `curl` is
/// missing would be worse than one showing a height with its age.
pub fn status_checking_node(ctx: &Context) -> Result<Status, Error> {
    if let Some(address) = crate::nodes::configured_address(ctx) {
        // Harvest first. An answer that landed since the last poll is what makes this one
        // fresh, and testing staleness before harvesting would start a second check for an
        // answer already on disk.
        let _ = crate::nodes::harvest_check(ctx, &address);
        let _ = crate::nodes::spawn_check(ctx, &address);
    }
    status(ctx)
}

pub fn status(ctx: &Context) -> Result<Status, Error> {
    let now = crate::now_unix();
    let layout = units::detect_layout(ctx).unwrap_or(units::Layout::None);
    let unit_node = units::state(ctx, &ctx.settings.node.unit)
        .unwrap_or_else(|_| units::UnitState::missing(&ctx.settings.node.unit));
    let unit_wallet = units::state(ctx, &ctx.settings.wallet.unit)
        .unwrap_or_else(|_| units::UnitState::missing(&ctx.settings.wallet.unit));

    // ---- the node's hot path -------------------------------------------------
    // The address in force and the trust flag come from `units::daemon_target`, so `status`,
    // the wallet unit and `set_daemon` cannot name different nodes (§2m.3, §5).
    let target = units::daemon_target(ctx);
    let remote = ctx.settings.node.mode.is_remote();
    let mut node = NodeStatus::unreachable(target.address.clone(), layout, target.trusted);
    // **In remote mode every node call — hot and cold — is skipped and no `NodeClient` is
    // constructed at all** (§2m.3, §2c). `reachable` stays `null` rather than `false`: no
    // wallet-RPC method reports the daemon's state (§1b.9 item 5), so `false` would be a claim.
    // Both `systemctl --user is-active` spawns above stay, because `units.node` is a real field
    // a user switching back needs and one §2l budget that holds in both modes is worth more
    // than a mode-dependent pair. Remote mode therefore only ever makes FEWER calls.
    if remote {
        node.reachable = None;
        // The cached probe, **read only** — `status` never measures and never writes it (§2e,
        // §2n.4). `state::remote_probe` returns `None` unless the stored address still matches
        // the one in force, so switching nodes cannot leave the old node's height rendered as
        // the new node's (§2j on the one field a user checks to see whether their node works).
        node.remote_probe = crate::state::remote_probe(ctx, &target.address)
            .ok()
            .flatten()
            .map(|p| RemoteProbeStatus::from_probe(&p, now));
    }
    // `for_status`, never `from_ctx`: `status` arms no breaker and is blocked by none (§2d).
    //
    // The `?` is deliberate, and it is the one thing that can make `status` fail: both
    // constructors reject a non-loopback URL through `rpc::require_loopback` (§2c), and §2a's
    // `--force` list requires `status --force` with `node.url = http://10.0.0.5:18081` to exit
    // **2**, not to report a document about a node it may not speak to. A daemon that is merely
    // DOWN is still `reachable: false` and exit 0 (§2e) — that is a different condition.
    let nclient = if remote {
        None
    } else {
        Some(crate::rpc::NodeClient::for_status(ctx)?)
    };
    let node_started = std::time::Instant::now();
    let info = nclient.as_ref().and_then(|n| n.get_info().ok());
    let node_probe_ms = node_started.elapsed().as_millis() as u64;
    if let (Some(n), Some(i)) = (nclient.as_ref(), info.as_ref()) {
        fill_node(&mut node, i, n);
        // `get_last_block_header` is the one call `CHECK_CORE_READY()` gates: it answers
        // `status: "BUSY"` with an all-zero header for the whole initial sync, which
        // `rpc/jsonrpc.rs` turns into an error before deserializing — so **any** failure here
        // is `null`, never a block 56 years old (§1b.3, §5).
        node.top_block_age_sec = n
            .get_last_block_header()
            .ok()
            .filter(|h| h.timestamp > 0)
            .map(|h| now - h.timestamp as i64);
    }

    // ---- the wallet's hot path ----------------------------------------------
    let mut wallet =
        WalletStatus::unreachable(ctx.settings.wallet.url.clone(), ctx.settings.wallet.account);
    wallet.unit_active = unit_wallet.active;
    let wclient = Some(crate::rpc::WalletClient::for_status(ctx)?);
    let wallet_started = std::time::Instant::now();
    let height = wclient.as_ref().map(|w| w.get_height());
    let wallet_probe_ms = wallet_started.elapsed().as_millis() as u64;
    let account = ctx.settings.wallet.account;
    let mut closed_behind_our_back = false;
    if let (Some(w), Some(result)) = (wclient.as_ref(), height) {
        match result {
            Ok(h) => {
                // An answer means reachable AND open — `get_height` is both probes (§5).
                wallet.reachable = true;
                wallet.open = true;
                wallet.authenticated = w.authenticated();
                wallet.height = Some(h);
                wallet.name = crate::state::last_open(ctx).ok().flatten().map(|(n, _)| n);
                if let Ok(b) = w.get_balance(account) {
                    wallet.balance_pico = Some(b.balance);
                    wallet.balance_xmr = Some(b.balance.to_xmr_string());
                    wallet.balance_xmr_short = Some(b.balance.to_xmr_short());
                    wallet.unlocked_pico = Some(b.unlocked);
                    wallet.unlocked_xmr = Some(b.unlocked.to_xmr_string());
                    wallet.blocks_to_unlock = Some(b.blocks_to_unlock);
                    wallet.time_to_unlock_sec = Some(b.time_to_unlock_sec);
                }
                if let Ok(a) = w.get_address(account) {
                    wallet.primary_address = Some(a.primary);
                    // Every GENERATED subaddress, not only the funded ones (§5).
                    wallet.subaddress_count = Some(a.addresses.len() as u32);
                }
            }
            // `-13 NOT_OPEN`: reachable, and nothing open. Exactly two guaranteed `-13`s are
            // what §5 declines to spend on a closed wallet, so nothing else is called.
            Err(e) if e.exit_code() == 9 => {
                wallet.reachable = true;
                wallet.authenticated = w.authenticated();
                closed_behind_our_back = crate::state::last_open(ctx).ok().flatten().is_some();
            }
            // A transport error: unreachable. `status` exits 0 and warns (§2e).
            Err(_) => {}
        }
    }
    if let (Some(nh), Some(wh)) = (node.height, wallet.height) {
        // POSITIVE means the wallet is behind, which is the sign every consumer assumes (§5).
        // **In remote mode this stays `None`**, because `node.height` is: no wallet-RPC method
        // reports the daemon's height (§1b.9 item 5). That is why §2d exempts remote-mode
        // deadline kills from the breaker and why `last_refresh_at` exists (§2m.3).
        wallet.blocks_behind = Some(nh as i64 - wh as i64);
    }

    // ---- the cold path, at most once per 60 s (§5) --------------------------
    let cold = cold_pass(
        ctx,
        nclient.as_ref().filter(|_| node.reachable == Some(true)),
        wclient.as_ref().filter(|_| wallet.reachable),
        wallet.open,
        now,
    );
    if let Some(c) = cold.as_ref() {
        if node.reachable == Some(true) {
            node.hard_fork_version = c.hard_fork_version;
            node.fee = c.fee.clone();
            if node.restricted != Some(true) {
                node.net.total_bytes_in = c.total_bytes_in;
                node.net.total_bytes_out = c.total_bytes_out;
            }
            node.net.limit_up_kbps = c.limit_up_kbps;
            node.net.limit_down_kbps = c.limit_down_kbps;
        }
        if wallet.open {
            wallet.accounts = c.accounts;
            wallet.rpc_version = c.wallet_rpc_version.clone();
        }
    }
    // Two samples of `get_net_stats` make a rate; one does not (§5).
    if node.reachable == Some(true) && node.restricted != Some(true) {
        if let Some(n) = nclient.as_ref() {
            if let Ok(s) = n.get_net_stats() {
                if let Ok(Some((rx, tx))) = crate::state::net_sample(ctx, s) {
                    node.net.rx_bps = Some(rx);
                    node.net.tx_bps = Some(tx);
                }
            }
        }
    }

    // ---- the money record, read-only (§2a.4) --------------------------------
    // `status` is the only other READER of pending.json and never a writer. An unparsable
    // record is a `safety` error from `state::pending_send`; here it must not break a
    // read-only document, so it degrades to "there is something" through the warning below.
    let record = crate::state::pending_send(ctx);
    let pending = record
        .as_ref()
        .ok()
        .and_then(|r| r.as_ref())
        .map(|r| PendingStatus::from_record(r, now));

    let stale_doc = crate::state::load(ctx).unwrap_or_default();
    // The last refresh **fmonero** issued (§5, v5.2) — written only by
    // `wallet_session::refresh`, never by `status`, which mutates no wallet (§2e). It is read
    // from the document `status` already loads for `last_error`: one more `state::load` here
    // cost measurable milliseconds against §2l's 60 ms budget, which is the whole point of
    // that budget being a number rather than an aspiration.
    wallet.last_refresh_at = stale_doc.last_refresh.and_then(crate::rfc3339_from_unix);
    let staleness = Staleness {
        node_probe_ms,
        wallet_probe_ms,
        // The age of the cold set that is about to be stored, which is 0 on a pass that just
        // ran and the previous age in between — `null` before the first pass ever (§5).
        node_extras_age_sec: cold.as_ref().map(|c| (now - c.built_at).max(0) as u64),
        history_cache_age_sec: crate::history::header(ctx)
            .ok()
            .flatten()
            .filter(|h| h.built_at > 0)
            .map(|h| (now - h.built_at).max(0) as u64),
    };

    let mut status = Status {
        ok: true,
        schema: crate::SCHEMA_VERSION,
        generated_at: crate::now_rfc3339(),
        node,
        wallet,
        units: UnitsStatus {
            node: unit_node,
            wallet: unit_wallet,
        },
        pending,
        warnings: Vec::new(),
        last_error: stale_doc.last_error.clone(),
        staleness,
    };
    status.warnings = warnings(ctx, &status, record.is_err(), &stale_doc);

    // `status` writes state — the cold cache, the net sample pair and the last observed
    // `blocks_behind` — which is not a contradiction of "it starts nothing" (§2e), and a
    // wallet closed behind fmonero's back must not leave a stale name on the bar widget (§5).
    let behind = status.wallet.blocks_behind;
    let cold_to_store = cold;
    let _ = crate::state::with_state(ctx, |s| {
        s.wallet_behind = behind;
        if let Some(c) = cold_to_store {
            s.cold = Some(c);
        }
        if closed_behind_our_back {
            s.last_open = None;
        }
    });
    Ok(status)
}

/// Copy `get_info` into §5's node object, turning every field a **restricted** node poisons
/// into `null` (§1b.3, §5).
fn fill_node(node: &mut NodeStatus, i: &crate::rpc::node::GetInfo, n: &crate::rpc::NodeClient) {
    let restricted = i.restricted;
    node.reachable = Some(true);
    node.restricted = Some(restricted);
    node.authenticated = n.authenticated();
    node.nettype = Some(i.nettype.clone());
    node.height = Some(i.height);
    node.target_height = Some(i.target_height);
    node.synchronized = Some(i.synchronized);
    node.busy_syncing = Some(i.busy_syncing);
    node.offline = Some(i.offline);
    // `target_height` is **0 when synced**, not equal to `height` (§1b.3).
    node.sync_percent = Some(if i.synchronized || i.target_height == 0 {
        100.0
    } else {
        (i.height as f64) / (i.target_height.max(i.height) as f64) * 100.0
    });
    // Truncated, not poisoned: a lower bound under restriction, and the warning says so.
    node.tx_pool_size = Some(i.tx_pool_size);
    if restricted {
        // start_time 0, free_space 2^64-1, version "", update_available false,
        // alt_blocks_count 0 and all four counts 0 (`core_rpc_server.cpp:517-569`). Rendering
        // them would be a 1970 uptime, 16 exabytes free and a permanent false "no peers".
        node.database_size = Some(i.database_size);
        node.database_size_rounded = true;
    } else {
        node.outgoing_connections_count = Some(i.outgoing_connections_count);
        node.incoming_connections_count = Some(i.incoming_connections_count);
        node.white_peerlist_size = Some(i.white_peerlist_size);
        node.grey_peerlist_size = Some(i.grey_peerlist_size);
        node.alt_blocks_count = Some(i.alt_blocks_count);
        node.database_size = Some(i.database_size);
        node.free_space = Some(i.free_space);
        node.version = Some(i.version.clone());
        node.update_available = Some(i.update_available);
        node.uptime_sec = Some(crate::now_unix() - i.start_time as i64);
    }
}

/// How often the cold set is refreshed (§5).
const COLD_TTL_SEC: i64 = 60;

/// The §5 cold path: `hard_fork_info{version:0}`, `get_fee_estimate`, `get_net_stats`,
/// `get_limit`, wallet `get_accounts` and wallet `get_version` — at most once per 60 s, served
/// from `state.json` in between. `None` means "keep what state already has".
fn cold_pass(
    ctx: &Context,
    n: Option<&crate::rpc::NodeClient>,
    w: Option<&crate::rpc::WalletClient>,
    wallet_open: bool,
    now: i64,
) -> Option<crate::state::ColdCache> {
    let existing = crate::state::load(ctx).ok().and_then(|s| s.cold);
    if let Some(c) = existing.as_ref() {
        if now - c.built_at < COLD_TTL_SEC {
            return existing;
        }
    }
    // With nothing answering there is no cold pass to make, and a `built_at` of now over a
    // set of `null`s would report `node_extras_age_sec: 0` for data that does not exist —
    // the zero-standing-in-for-unknown §5 forbids.
    if n.is_none() && w.is_none() {
        return existing;
    }
    let mut cold = existing.unwrap_or_default();
    cold.built_at = now;
    if let Some(n) = n {
        // Only `version` and `state` are read from a `{version: 0}` call: the sibling voting
        // fields describe a DIFFERENT fork version (§1b.3).
        if let Ok(hf) = n.hard_fork_info(0) {
            cold.hard_fork_version = Some(hf.version);
        }
        // Always 10 grace blocks, the value wallet2 itself uses (§1b.3).
        if let Ok(f) = n.get_fee_estimate(10) {
            cold.fee = FeeStatus {
                per_byte_pico: Some(f.fee),
                fees_pico: Some(f.fees.clone()),
                quantization_mask: Some(f.quantization_mask),
            };
        }
        if let Ok(s) = n.get_net_stats() {
            cold.total_bytes_in = Some(s.total_bytes_in);
            cold.total_bytes_out = Some(s.total_bytes_out);
        }
        // `get_limit` is open even on a restricted node (§1b.2).
        if let Ok(l) = n.get_limit() {
            cold.limit_up_kbps = Some(l.limit_up);
            cold.limit_down_kbps = Some(l.limit_down);
        }
    }
    if let Some(w) = w {
        if wallet_open {
            if let Ok(a) = w.get_accounts() {
                cold.accounts = Some(a.len() as u32);
            }
        }
        if let Ok((major, minor, _)) = w.get_version() {
            cold.wallet_rpc_version = Some(format!("{major}.{minor}"));
        }
    }
    Some(cold)
}

/// The §5 warning table, in its order. Every id here is in [`WARNING_IDS`].
///
/// **Every `text` and every `action` here is plain language (§2o), and every `id` is
/// unchanged** (`--json` is the agent contract, §5). `hint` stays the CLI's runnable command
/// (§9); `action` is the in-app sentence the TUI and the widget render in its place (§2o.1),
/// and it is `None` where there is nothing the user can do from inside the app — a key
/// invented to fill the field would be worse than a sentence that stops.
fn warnings(
    ctx: &Context,
    s: &Status,
    pending_unreadable: bool,
    state: &crate::state::StateDoc,
) -> Vec<Warning> {
    let mut out = Vec::new();
    let mut add = |id: &str, severity: Severity, text: String, hint: &str, action: Option<&str>| {
        out.push(Warning {
            id: id.to_string(),
            severity,
            text,
            hint: hint.to_string(),
            action: action.map(str::to_string),
        });
    };
    // **Which binary is missing depends on the mode** (§2m.3): remote mode needs no `monerod`
    // at all, and raising an error about a binary the configuration does not use is the alarm a
    // user learns to ignore. The text no longer names which of the two programs it looked for:
    // both come out of one package, the answer is the same either way, and the name was an
    // implementation detail standing where the user's next step belongs (§2o rule 6).
    let needed = if ctx.settings.node.mode.is_remote() {
        "monero-wallet-rpc"
    } else {
        "monerod"
    };
    if crate::proc::which(needed).is_none() {
        add(
            "monero_not_installed",
            Severity::Error,
            "Monero is not installed on this machine.".into(),
            "omarchy pkg add monero",
            Some("In a terminal: omarchy pkg add monero"),
        );
    }
    // v5.2, §2m. `remote_node` is INFO: it is a configuration, not a fault. v5.6 drops the
    // clause about the node's own sync state not being visible from here — it is true, it is
    // an architecture fact, and §7a.3's pane now answers the question it was apologising for.
    if ctx.settings.node.mode.is_remote() {
        add(
            "remote_node",
            Severity::Info,
            format!(
                "You are using the public node {}. It sees your IP address and when you ask it \
                 anything.",
                s.node.url
            ),
            "fmonero node local --yes",
            Some("Press f on the Node screen to choose another."),
        );
        // A trusted daemon receives the wallet's own unspent amounts INSIDE a request
        // (`wallet2.cpp:12281-12282`, §1b.9 item 4). A private literal is the case the flag
        // exists for; a DNS name is never treated as private, because classifying one means
        // resolving it (§2m.6, §7.1).
        if s.node.trusted
            && !crate::settings::is_private_host(crate::settings::remote_host(&s.node.url))
        {
            add(
                "remote_node_trusted",
                Severity::Warn,
                format!(
                    "{} is set as trusted, so this wallet tells it which amounts it holds.",
                    s.node.url
                ),
                "fmonero node remote <addr> --yes   # re-set it without --trusted",
                Some("Choose the node again on the Node screen to clear that."),
            );
        }
    }
    // **`null` is not `false`.** In remote mode `node.reachable` is `null` and this warning —
    // like `node_restricted`, `node_syncing`, `node_offline`, `node_no_peers`, `node_low_disk`,
    // `node_update_available`, `auth_missing` and `node_stalled` — is never raised, because
    // every one of them tests a field that has no source there (§2m.3). The others fall out for
    // free: each compares against `Some(x)` and the field is `None`.
    if s.node.layout != units::Layout::None && s.node.reachable == Some(false) {
        // **W9: two words, one predicate, and the advice follows it.** A service that is not
        // running is not running; one that is running and silent is not answering — and telling a
        // reader to press the key that starts a service which is already started is advice that
        // cannot work. [`node_off_words`] is the predicate, shared with the TUI's badge, its hero
        // and its panes.
        let running = node_off_words(s) == "not answering";
        add(
            "node_down",
            Severity::Warn,
            format!("Your node is {}.", node_off_words(s)),
            "fmonero node up",
            Some(if running {
                "It may still be starting up. Press r to check again."
            } else {
                "Press u on the Node screen to start it."
            }),
        );
    }
    if s.node.restricted == Some(true) {
        add(
            "node_restricted",
            Severity::Warn,
            // §2o.2 says `payment`, and X8 found `transaction` on three surfaces at once.
            "This node only answers basic questions. Peers, traffic and bans are not shown, and \
             the count of payments waiting is a minimum."
                .into(),
            "fmonero doctor --only node",
            None,
        );
    }
    if s.node.synchronized == Some(false) {
        // **One rounding for the whole program** (X11). `{:.0}` rounds, so a node at 33.5% read
        // `Your node is 34% synced.` in the same frame as the bar's `Your node syncing 33%`.
        // `crate::percent` rounds down, and every surface calls it.
        let pct = crate::percent(s.node.sync_percent.unwrap_or(0.0));
        add(
            "node_syncing",
            Severity::Info,
            format!("Your node is {pct} synced."),
            "fmonero node up --wait --wait-timeout 86400",
            Some("It keeps going on its own."),
        );
    }
    if s.node.offline == Some(true) {
        add(
            "node_offline",
            Severity::Warn,
            "Your node is set to stay offline.".into(),
            "fmonero setup --units --yes",
            None,
        );
    }
    if s.node.outgoing_connections_count == Some(0)
        && s.node.offline == Some(false)
        && s.node.restricted != Some(true)
    {
        add(
            "node_no_peers",
            Severity::Warn,
            "Your node has no peers yet.".into(),
            "fmonero node limit --out-peers 16",
            None,
        );
    }
    if let Some(free) = s.node.free_space {
        if free < DISK_FLOOR_BYTES {
            add(
                "node_low_disk",
                Severity::Warn,
                // Round 4's own read of the captures: `A smaller copy of the blockchain needs
                // about 120 GB` reads as though the SMALLER copy is the expensive one. 120 GB is
                // the floor this check compares against, and what it is a floor for is a full copy.
                format!(
                    "Only {} free on the disk your node uses. A full copy of the blockchain needs \
                     about 120 GB, and it keeps growing.",
                    gigabytes(free)
                ),
                "fmonero doctor --only storage",
                None,
            );
        }
    }
    if s.node.update_available == Some(true) {
        add(
            "node_update_available",
            Severity::Info,
            "Your node says a newer version is out.".into(),
            "omarchy pkg upgrade",
            Some("In a terminal: omarchy pkg upgrade"),
        );
    }
    if s.node.layout == units::Layout::SystemUnit {
        add(
            "foreign_node_running",
            Severity::Info,
            crate::FOREIGN_NODE_INFO.into(),
            "sudo systemctl status monerod",
            None,
        );
    }
    if !s.wallet.unit_active && !s.wallet.reachable {
        // **Narrowed in v5.5.** In remote mode with no wallet recorded open this is
        // `wallet_idle`, severity **info**: the wallet service is *designed* to be down until a
        // wallet is opened, and a warning that fires on every correct fresh install is the
        // warning a user learns to ignore — which is precisely what the owner met (the bar icon
        // was urgent about a service that was behaving correctly). In local mode, and in remote
        // mode with a wallet recorded open, the warning is unchanged: an inactive unit under an
        // open wallet is a real fault, the wallet went away behind us.
        let idle = s.node.layout == units::Layout::Remote && s.wallet.name.is_none();
        if idle {
            add(
                "wallet_idle",
                Severity::Info,
                "No wallet is open. The wallet service starts when you open or create one.".into(),
                "fmonero wallet open <name>   # or: fmonero wallet create <name> --yes",
                Some("Press o to open a wallet, or c to create one."),
            );
        } else {
            add(
                "wallet_down",
                Severity::Warn,
                "The wallet service is not running.".into(),
                "fmonero wallet open <name>",
                Some("Press o on the Wallet screen to open a wallet."),
            );
        }
    }
    if s.wallet.reachable && !s.wallet.open {
        add(
            "wallet_closed",
            Severity::Info,
            "No wallet is open.".into(),
            "fmonero wallet open <name>",
            Some("Press o to open one."),
        );
    }
    if s.wallet
        .blocks_behind
        .is_some_and(|b| b > WALLET_BEHIND_BLOCKS)
    {
        add(
            "wallet_behind",
            Severity::Warn,
            format!(
                "This wallet has {} blocks to go.",
                crate::grouped(s.wallet.blocks_behind.unwrap_or(0).max(0) as u64)
            ),
            "fmonero wallet refresh",
            // Decision 7, and the whole of it: the scanning happens inside the wallet service,
            // so closing the TUI does not stop it. **One sentence, quoted from one place**
            // (W10): this warning, both TUI screens and the bar widget all said it differently.
            Some(crate::SYNC_KEEPS_GOING),
        );
    }
    if s.wallet.blocks_to_unlock.is_some_and(|b| b > 0) {
        let blocks = s.wallet.blocks_to_unlock.unwrap_or(0);
        // Minutes are the answer, blocks are the mechanism (§2o rule 9's sibling). The
        // amount comes with it, because "some funds" is not a figure anyone can act on.
        let mins = s
            .wallet
            .time_to_unlock_sec
            .map(|sec| sec.div_ceil(60))
            .unwrap_or(blocks * 2);
        let locked = match (s.wallet.balance_pico, s.wallet.unlocked_pico) {
            (Some(b), Some(u)) => Piconero(b.0.saturating_sub(u.0)).to_xmr_string(),
            _ => "some".to_string(),
        };
        add(
            "wallet_locked_funds",
            Severity::Info,
            format!("{locked} XMR unlocks in about {mins} min."),
            "fmonero wallet balance",
            None,
        );
    }
    let in_flight = s
        .pending
        .as_ref()
        .is_some_and(|p| p.txs.iter().any(|t| !t.relayed));
    if in_flight || pending_unreadable {
        // **The id is shortened here and whole in `--json`** (W8): 64 hexadecimal characters
        // rendered as prose is the shape this warning had on a bar 380 pixels wide, and it is
        // the most serious warning the program has. `pending.txs[].txid` carries the whole of
        // it for the agent that needs it.
        // **The colon only when an id follows it** (X33). With an unreadable `pending.json` the
        // sentence read `… may already have gone out: the recorded payment.` — a colon introducing
        // something that is not an id, in the one state the reader most needs explained. Two
        // sentences, and the second says what fmonero does not know.
        let text = match s
            .pending
            .as_ref()
            .and_then(|p| p.txs.iter().find(|t| !t.relayed))
            .map(|t| crate::short_id(&t.txid))
        {
            Some(id) => format!("A payment was prepared and may already have gone out: {id}."),
            None => "A payment was prepared and may already have gone out. fmonero cannot read \
                     which one it was."
                .to_string(),
        };
        add(
            "send_may_be_in_flight",
            Severity::Error,
            text,
            "fmonero wallet history --limit 5   # then: fmonero wallet clear-pending --yes",
            Some("Press 2 to look at your payments."),
        );
    }
    for (who, id) in [
        (crate::state::Daemon::Node, "node_stalled"),
        (crate::state::Daemon::Wallet, "wallet_stalled"),
    ] {
        if let Some(stall) = state.stalls.get(who.key()) {
            let left = stall.backoff_until - crate::now_unix();
            if left > 0 {
                let subject = match who {
                    crate::state::Daemon::Node => "Your node",
                    crate::state::Daemon::Wallet => "The wallet service",
                };
                add(
                    id,
                    Severity::Error,
                    format!(
                        "{subject} took too long to answer. fmonero will try again in {left}s."
                    ),
                    "fmonero doctor   # or re-run the command with --force",
                    None,
                );
            }
        }
    }
    if s.node.reachable == Some(true) && s.node.authenticated == Some(false) {
        add(
            "auth_missing",
            Severity::Warn,
            "Your node accepted fmonero without asking for a password.".into(),
            "fmonero setup --units --yes",
            None,
        );
    }
    if let Some((_, net)) = state.last_open.as_ref() {
        if *net != ctx.settings.node.network.net_type() {
            add(
                "nettype_mismatch",
                Severity::Warn,
                format!(
                    "This wallet is a {} wallet and your settings say {}. Sending follows the \
                     wallet.",
                    net.as_str(),
                    ctx.settings.node.network
                ),
                "fmonero doctor --only wallet",
                None,
            );
        }
    }
    if let Some(h) = crate::history::header(ctx).ok().flatten() {
        if let Some(wh) = s.wallet.height {
            if wh.saturating_sub(h.high_water) > HISTORY_STALE_BLOCKS {
                // **A high water of 0 is an EMPTY snapshot, not one 3.7 million blocks
                // stale**, and the arithmetic cannot tell the two apart: `high_water` is the
                // greatest height any recorded transfer sits at, so a wallet with no
                // transfers at all has 0 there and "3 764 638 blocks behind" is true
                // subtraction and a false sentence (§2j). The warning still fires — an empty
                // snapshot against a wallet at the tip is exactly as worth rebuilding — it
                // just says what it found.
                let detail = if h.high_water == 0 {
                    "The saved copy of your payments is empty.".to_string()
                } else {
                    format!(
                        "The saved copy of your payments is {} blocks behind.",
                        wh - h.high_water
                    )
                };
                add(
                    "history_cache_stale",
                    Severity::Info,
                    detail,
                    "fmonero wallet history --limit 1   # writes the saved copy of your payments again",
                    Some("Press R to rebuild it."),
                );
            }
        }
    }
    out
}

/// Free space as a person reads it, for the one warning that quotes it. Whole GB, because
/// the figure it is compared against is itself an estimate (§1.3) and a decimal place on an
/// estimate is false precision.
fn gigabytes(bytes: u64) -> String {
    format!("{} GB", bytes / 1_000_000_000)
}

/// The two words for a node that is not answering, from **one** predicate (W9).
///
/// `not running` and `not answering` are different facts and a reader cannot reconcile them in one
/// frame: v5.6.1 fixed the badge and the hero and left three panes, this warning and its advice
/// saying the other thing. The question is whether the service is up — if it is, the node is there
/// and silent (still opening its database, most often), and if it is not, it is simply off. A node
/// another program on this machine owns is treated as up, because fmonero does not start or stop
/// it (§2i) and "not running" would read as an instruction to.
pub fn node_off_words(s: &Status) -> &'static str {
    if s.units.node.active || s.node.layout == units::Layout::SystemUnit {
        "not answering"
    } else {
        "not running"
    }
}

/// §1.3's floor for a pruned chain plus headroom, as bytes. **An estimate, not measured on
/// this machine**, and every message built from it says so (§1.3, §2j).
pub const DISK_FLOOR_BYTES: u64 = 120 * 1000 * 1000 * 1000;
/// How far behind the history snapshot may fall before `history_cache_stale` fires (§5).
pub const HISTORY_STALE_BLOCKS: u64 = 1_000;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PendingTx;

    /// §4b's GREEN row: both daemons absent is **exit 0 with warnings**, every daemon field
    /// `null`, and nothing started (§2e). It is also the §5 shape test: every documented key
    /// is present even when its value is `null`.
    #[test]
    fn with_both_daemons_absent_the_document_is_all_null_and_still_ok() {
        let dir = std::env::temp_dir().join(format!("fmonero-status-{}", std::process::id()));
        let ctx = Context::for_test(&dir).expect("context");
        let s = status(&ctx).expect("status never fails on a machine with no daemons");
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["schema"], 1);
        assert_eq!(v["node"]["reachable"], false);
        assert_eq!(v["node"]["layout"], "none");
        assert_eq!(v["wallet"]["open"], false);
        assert_eq!(v["wallet"]["reachable"], false);
        assert_eq!(v["pending"], serde_json::Value::Null);
        // Never a zero standing in for unknown (§5's shape rules).
        for key in [
            "height",
            "target_height",
            "sync_percent",
            "restricted",
            "authenticated",
            "top_block_age_sec",
            "uptime_sec",
            "free_space",
            "version",
            "hard_fork_version",
        ] {
            assert_eq!(
                v["node"][key],
                serde_json::Value::Null,
                "node.{key} must be null, not 0"
            );
        }
        for key in ["balance_pico", "balance_xmr", "name", "height", "accounts"] {
            assert_eq!(v["wallet"][key], serde_json::Value::Null, "wallet.{key}");
        }
        // Every documented key is present, even when its value is null (§5's shape rules).
        for key in [
            "node",
            "wallet",
            "units",
            "pending",
            "warnings",
            "last_error",
            "staleness",
        ] {
            assert!(v.get(key).is_some(), "missing key {key}");
        }
        assert!(v["generated_at"].as_str().unwrap().contains('T'));
        // On THIS machine no Monero binary exists (§1.1), so the honest document carries
        // warnings — and every id it raises is one §5 lists.
        let ids: Vec<&str> = v["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["id"].as_str().unwrap())
            .collect();
        for id in &ids {
            assert!(WARNING_IDS.contains(id), "{id} is not a §5 warning id");
        }
        assert!(
            ids.contains(&"wallet_down"),
            "the wallet daemon is not running: {ids:?}"
        );
        for w in v["warnings"].as_array().unwrap() {
            assert!(
                !w["hint"].as_str().unwrap_or_default().is_empty(),
                "every warning carries a runnable hint (§4)"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sweep_record_is_summed_in_rust_and_pre_rendered() {
        let p = PendingSend {
            kind: PendingKind::Sweep,
            account_index: 0,
            dest: "8".repeat(95),
            built_at: 1_700_000_000,
            txs: vec![
                PendingTx {
                    txid: "a".repeat(64),
                    amount: Piconero(1_500_000_000_000),
                    fee: Piconero(30_000_000),
                    relayed: true,
                    relayed_at: Some(1_700_000_001),
                },
                PendingTx {
                    txid: "b".repeat(64),
                    amount: Piconero(500_000_000_000),
                    fee: Piconero(20_000_000),
                    relayed: false,
                    relayed_at: None,
                },
            ],
        };
        let d = PendingStatus::from_record(&p, 1_700_000_042);
        assert_eq!(d.total_count, 2);
        assert_eq!(d.relayed_count, 1);
        assert_eq!(d.elapsed_sec, 42);
        assert_eq!(d.total_amount_pico, Piconero(2_000_000_000_000));
        assert_eq!(d.total_amount_xmr, "2.0000");
        assert_eq!(d.total_fee_pico, Piconero(50_000_000));
        assert_eq!(d.total_fee_xmr, "0.00005");
        assert_eq!(d.txs[0].amount_xmr, "1.5000");
        // An array of objects, never parallel arrays (§5).
        let v = serde_json::to_value(&d).unwrap();
        assert!(v["txs"].is_array());
        assert_eq!(v["kind"], "sweep");
    }

    #[test]
    fn the_severity_spellings_are_the_ones_the_plugin_parses() {
        for (s, wire) in [
            (Severity::Info, "info"),
            (Severity::Warn, "warn"),
            (Severity::Error, "error"),
        ] {
            assert_eq!(serde_json::to_value(s).unwrap(), wire);
            assert_eq!(s.as_str(), wire);
            let back: Severity = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(back, s);
        }
        // Nineteen before v5.2; §2m adds `remote_node` (info) and `remote_node_trusted` (warn).
        assert_eq!(WARNING_IDS.len(), 22);
        assert!(
            !WARNING_IDS.contains(&"plugin_not_linked"),
            "that one is the doctor's (§5)"
        );
    }

    // ---- §5's `--check-node`: four bounds, a test each (v5.6) --------------------

    /// A sandbox in remote mode against an address nothing answers on, which is what makes
    /// the spawn test cheap: `127.0.0.1:1` refuses instantly and leaves this machine.
    fn remote_ctx(tag: &str) -> Context {
        let dir = std::env::temp_dir().join(format!("fmonero-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ctx = Context::for_test(&dir).expect("context");
        ctx.settings.node.mode = crate::settings::NodeMode::Remote;
        ctx.settings.node.remote = "127.0.0.1:1".into();
        ctx.settings.node.remote_probe_sec = 60;
        ctx
    }

    /// A cached check that failed, which is all these tests need of one: what is being
    /// bounded is when a new check starts, never what an old one said.
    fn stale_probe() -> crate::nodes::Probe {
        crate::nodes::Probe {
            reachable: false,
            height: None,
            synchronized: None,
            restricted: None,
            nettype: None,
            version: None,
            latency_ms: Some(3),
            reason: Some("refused"),
            probed_at: crate::now_unix(),
        }
    }

    fn answered(height: u64) -> Vec<u8> {
        format!(
            r#"{{"result":{{"status":"OK","height":{height},"synchronized":true,"restricted":false,"nettype":"mainnet","version":"0.18.5.1"}}}}"#
        )
        .into_bytes()
    }

    /// Bound 1: **with a fresh cache the flag starts nothing.** Nothing is spawned and no
    /// attempt is recorded, so the 3 s poll of an open panel costs what plain `status` costs.
    #[test]
    fn the_check_flag_starts_nothing_when_the_last_check_is_fresh() {
        let ctx = remote_ctx("checkfresh");
        let address = crate::nodes::configured_address(&ctx).expect("remote mode");
        let mut p = stale_probe();
        p.probed_at = crate::now_unix();
        crate::state::note_remote_probe(&ctx, &address, &p).unwrap();
        assert!(!crate::nodes::spawn_check(&ctx, &address).unwrap());
        assert!(!crate::nodes::check_file(&ctx).exists());
        assert!(crate::state::remote_check(&ctx, &address)
            .unwrap()
            .is_none());
    }

    /// Bound 2 and bound 3: **a stale cache starts exactly one check, and three polls inside
    /// its deadline start one.** Without the recorded attempt a 3 s poll against a 60 s
    /// staleness window would fire twenty `curl`s per refresh cycle.
    #[test]
    fn a_stale_check_starts_one_and_three_polls_still_start_one() {
        let ctx = remote_ctx("checkstale");
        let address = crate::nodes::configured_address(&ctx).expect("remote mode");
        let mut p = stale_probe();
        p.probed_at = crate::now_unix() - 3_600;
        crate::state::note_remote_probe(&ctx, &address, &p).unwrap();
        // A machine with no `curl` cannot start a check at all (§2n.4 refuses before the
        // network), and there is then nothing to bound. It is detected by trying, rather than by
        // a `which` here, because §2c's own grep says only `nodes.rs` may name that program.
        let first = match crate::nodes::spawn_check(&ctx, &address) {
            Ok(started) => started,
            Err(_) => return,
        };
        assert!(first, "one");
        let started = crate::state::remote_check(&ctx, &address)
            .unwrap()
            .expect("the attempt is recorded beside the cached result");
        assert!(!crate::nodes::spawn_check(&ctx, &address).unwrap(), "two");
        assert!(!crate::nodes::spawn_check(&ctx, &address).unwrap(), "three");
        assert_eq!(
            crate::state::remote_check(&ctx, &address).unwrap(),
            Some(started),
            "the second and third polls did not re-record the attempt"
        );
    }

    /// Bound 4a: **with your own node the flag is a no-op**, where the live `get_info` of the
    /// ordinary hot path is the better answer and the cache is `null` by design.
    #[test]
    fn with_your_own_node_the_check_flag_does_nothing() {
        let dir = std::env::temp_dir().join(format!("fmonero-checklocal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = Context::for_test(&dir).expect("context");
        assert!(crate::nodes::configured_address(&ctx).is_none());
        let s = status_checking_node(&ctx).expect("a read-only document");
        assert!(s.node.remote_probe.is_none());
        assert!(!crate::nodes::check_file(&ctx).exists());
    }

    /// Bound 4b: **plain `status` is unchanged and still spawns nothing**, which is what every
    /// §2l measurement and every agent contract rests on — even with a cache stale enough that
    /// the flag would have started a check.
    #[test]
    fn plain_status_starts_nothing_even_with_a_stale_check() {
        let ctx = remote_ctx("checkplain");
        let address = crate::nodes::configured_address(&ctx).expect("remote mode");
        let mut p = stale_probe();
        p.probed_at = crate::now_unix() - 3_600;
        crate::state::note_remote_probe(&ctx, &address, &p).unwrap();
        let _ = status(&ctx).expect("status never fails on a machine with no daemons");
        assert!(!crate::nodes::check_file(&ctx).exists());
        assert!(crate::state::remote_check(&ctx, &address)
            .unwrap()
            .is_none());
    }

    /// The harvest, which is the half that makes the **next** poll fresh: an answer becomes
    /// the cached check with the time it took, an empty file past the child's own deadline
    /// becomes a recorded failure rather than `Not checked yet.` forever, and an answer about
    /// another address is discarded unread.
    #[test]
    fn a_harvested_answer_becomes_the_cached_check_and_a_silence_becomes_a_failure() {
        let ctx = remote_ctx("checkharvest");
        let address = crate::nodes::configured_address(&ctx).expect("remote mode");
        let file = crate::nodes::check_file(&ctx);

        std::fs::write(&file, answered(3_765_346)).unwrap();
        crate::state::with_state(&ctx, |s| {
            s.remote_check = Some((address.clone(), crate::now_unix_millis() - 240))
        })
        .unwrap();
        let p = crate::nodes::harvest_check(&ctx, &address)
            .unwrap()
            .expect("an answer");
        assert!(p.reachable);
        assert_eq!(p.height, Some(3_765_346));
        assert_eq!(p.synchronized, Some(true));
        assert!(p.latency_ms.is_some(), "the check's own duration");
        assert!(!file.exists(), "the answer file is cleaned up");
        assert!(crate::state::remote_check(&ctx, &address)
            .unwrap()
            .is_none());
        assert_eq!(
            crate::state::remote_probe(&ctx, &address)
                .unwrap()
                .and_then(|c| c.height),
            Some(3_765_346),
            "it is now THIS address's cached check"
        );

        // A silence past the deadline is a failure, and it is recorded as one.
        std::fs::write(&file, b"").unwrap();
        crate::state::with_state(&ctx, |s| {
            s.remote_check = Some((address.clone(), crate::now_unix_millis() - 60_000))
        })
        .unwrap();
        let p = crate::nodes::harvest_check(&ctx, &address)
            .unwrap()
            .expect("a failure is an answer too");
        assert!(!p.reachable);
        assert_eq!(p.reason, Some("refused"));

        // An answer about a node we are no longer using is never read.
        std::fs::write(&file, answered(1)).unwrap();
        crate::state::with_state(&ctx, |s| {
            s.remote_check = Some(("other.example:18081".into(), crate::now_unix_millis()))
        })
        .unwrap();
        assert!(crate::nodes::harvest_check(&ctx, &address)
            .unwrap()
            .is_none());
        assert!(!file.exists());
        assert_eq!(
            crate::state::remote_probe(&ctx, &address)
                .unwrap()
                .map(|c| c.reachable),
            Some(false),
            "the earlier failure is still the cached check, not the stranger's height"
        );
    }
}
