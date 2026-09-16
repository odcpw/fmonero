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
    pub url: String,
    pub reachable: bool,
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
    pub unlocked_pico: Option<Piconero>,
    pub unlocked_xmr: Option<String>,
    pub blocks_to_unlock: Option<u64>,
    pub time_to_unlock_sec: Option<u64>,
    /// `get_address.addresses.len()` — every generated subaddress. **Not**
    /// `get_balance.per_subaddress.len()`, which holds only funded ones (§5).
    pub subaddress_count: Option<u32>,
    pub accounts: Option<u32>,
    pub rpc_version: Option<String>,
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
    pub hint: String,
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
    "node_down",
    "node_restricted",
    "node_syncing",
    "node_offline",
    "node_no_peers",
    "node_low_disk",
    "node_update_available",
    "foreign_node_running",
    "wallet_down",
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
    pub fn unreachable(url: String, layout: units::Layout) -> NodeStatus {
        NodeStatus {
            layout,
            url,
            reachable: false,
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
            unlocked_pico: None,
            unlocked_xmr: None,
            blocks_to_unlock: None,
            time_to_unlock_sec: None,
            subaddress_count: None,
            accounts: None,
            rpc_version: None,
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
pub fn status(ctx: &Context) -> Result<Status, Error> {
    let now = crate::now_unix();
    let layout = units::detect_layout(ctx).unwrap_or(units::Layout::None);
    let unit_node = units::state(ctx, &ctx.settings.node.unit)
        .unwrap_or_else(|_| units::UnitState::missing(&ctx.settings.node.unit));
    let unit_wallet = units::state(ctx, &ctx.settings.wallet.unit)
        .unwrap_or_else(|_| units::UnitState::missing(&ctx.settings.wallet.unit));

    // ---- the node's hot path -------------------------------------------------
    let mut node = NodeStatus::unreachable(ctx.settings.node.url.clone(), layout);
    // `for_status`, never `from_ctx`: `status` arms no breaker and is blocked by none (§2d).
    let nclient = crate::rpc::NodeClient::for_status(ctx).ok();
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
    let wclient = crate::rpc::WalletClient::for_status(ctx).ok();
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
        wallet.blocks_behind = Some(nh as i64 - wh as i64);
    }

    // ---- the cold path, at most once per 60 s (§5) --------------------------
    let cold = cold_pass(
        ctx,
        nclient.as_ref().filter(|_| node.reachable),
        wclient.as_ref().filter(|_| wallet.reachable),
        wallet.open,
        now,
    );
    if let Some(c) = cold.as_ref() {
        if node.reachable {
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
    if node.reachable && node.restricted != Some(true) {
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
    node.reachable = true;
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
fn warnings(
    ctx: &Context,
    s: &Status,
    pending_unreadable: bool,
    state: &crate::state::StateDoc,
) -> Vec<Warning> {
    let mut out = Vec::new();
    let mut add = |id: &str, severity: Severity, text: String, hint: &str| {
        out.push(Warning {
            id: id.to_string(),
            severity,
            text,
            hint: hint.to_string(),
        });
    };
    if crate::proc::which("monerod").is_none() {
        add(
            "monero_not_installed",
            Severity::Error,
            "monerod is not installed".into(),
            "omarchy pkg add monero",
        );
    }
    if s.node.layout != units::Layout::None && !s.node.reachable {
        add(
            "node_down",
            Severity::Warn,
            "the node is not answering".into(),
            "fmonero node up",
        );
    }
    if s.node.restricted == Some(true) {
        add(
            "node_restricted",
            Severity::Warn,
            "this monerod runs with restricted RPC: sync_info, connections, the peer list, bans, \
             net stats and the rate limits have no source, and tx_pool_size is a LOWER BOUND \
             because non-relayed and local transactions are left out"
                .into(),
            "fmonero doctor --only node",
        );
    }
    if s.node.synchronized == Some(false) {
        let pct = s.node.sync_percent.unwrap_or(0.0);
        add(
            "node_syncing",
            Severity::Info,
            format!("the node is at {pct:.1}% and still synchronizing"),
            "fmonero node up --wait --wait-timeout 86400",
        );
    }
    if s.node.offline == Some(true) {
        add(
            "node_offline",
            Severity::Warn,
            "the node was started with --offline".into(),
            "fmonero setup --units --yes",
        );
    }
    if s.node.outgoing_connections_count == Some(0)
        && s.node.offline == Some(false)
        && s.node.restricted != Some(true)
    {
        add(
            "node_no_peers",
            Severity::Warn,
            "the node has no outgoing connections".into(),
            "fmonero node limit --out-peers 16",
        );
    }
    if let Some(free) = s.node.free_space {
        if free < DISK_FLOOR_BYTES {
            add(
                "node_low_disk",
                Severity::Warn,
                format!(
                    "the node reports {free} bytes free, under the 120 GB floor for a pruned \
                     chain — a figure that is an ESTIMATE, not measured on this machine"
                ),
                "fmonero doctor --only storage",
            );
        }
    }
    if s.node.update_available == Some(true) {
        add(
            "node_update_available",
            Severity::Info,
            "the node reports that an update is available".into(),
            "omarchy pkg upgrade",
        );
    }
    if s.node.layout == units::Layout::SystemUnit {
        add(
            "foreign_node_running",
            Severity::Info,
            "this monerod is Arch's system unit: fmonero uses it read-only and will not start, \
             stop or reconfigure it"
                .into(),
            "sudo systemctl status monerod",
        );
    }
    if !s.wallet.unit_active && !s.wallet.reachable {
        add(
            "wallet_down",
            Severity::Warn,
            "monero-wallet-rpc is not running".into(),
            "fmonero wallet open <name>",
        );
    }
    if s.wallet.reachable && !s.wallet.open {
        add(
            "wallet_closed",
            Severity::Info,
            "monero-wallet-rpc is running with no wallet open".into(),
            "fmonero wallet open <name>",
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
                "the wallet is {} blocks behind the node",
                s.wallet.blocks_behind.unwrap_or(0)
            ),
            "fmonero wallet refresh",
        );
    }
    if s.wallet.blocks_to_unlock.is_some_and(|b| b > 0) {
        add(
            "wallet_locked_funds",
            Severity::Info,
            format!(
                "some funds unlock in {} blocks",
                s.wallet.blocks_to_unlock.unwrap_or(0)
            ),
            "fmonero wallet balance",
        );
    }
    let in_flight = s
        .pending
        .as_ref()
        .is_some_and(|p| p.txs.iter().any(|t| !t.relayed));
    if in_flight || pending_unreadable {
        let txid = s
            .pending
            .as_ref()
            .and_then(|p| p.txs.iter().find(|t| !t.relayed))
            .map(|t| t.txid.clone())
            .unwrap_or_else(|| "the recorded transaction".to_string());
        add(
            "send_may_be_in_flight",
            Severity::Error,
            format!("a send was built and may have been broadcast: {txid}"),
            "fmonero wallet history --limit 5   # then: fmonero wallet clear-pending --yes",
        );
    }
    for (who, id) in [
        (crate::state::Daemon::Node, "node_stalled"),
        (crate::state::Daemon::Wallet, "wallet_stalled"),
    ] {
        if let Some(stall) = state.stalls.get(who.key()) {
            let left = stall.backoff_until - crate::now_unix();
            if left > 0 {
                add(
                    id,
                    Severity::Error,
                    format!(
                        "a {} call was killed at its deadline; {left}s of backoff left",
                        who.key()
                    ),
                    "fmonero doctor   # or re-run the command with --force",
                );
            }
        }
    }
    if s.node.reachable && s.node.authenticated == Some(false) {
        add(
            "auth_missing",
            Severity::Warn,
            "the node accepted fmonero with no credential at all".into(),
            "fmonero setup --units --yes",
        );
    }
    if let Some((_, net)) = state.last_open.as_ref() {
        if *net != ctx.settings.node.network.net_type() {
            add(
                "nettype_mismatch",
                Severity::Warn,
                format!(
                    "the open wallet is {} while node.network is {}: the send path follows the \
                     WALLET, never the setting",
                    net.as_str(),
                    ctx.settings.node.network
                ),
                "fmonero doctor --only wallet",
            );
        }
    }
    if let Some(h) = crate::history::header(ctx).ok().flatten() {
        if let Some(wh) = s.wallet.height {
            if wh.saturating_sub(h.high_water) > HISTORY_STALE_BLOCKS {
                add(
                    "history_cache_stale",
                    Severity::Info,
                    format!(
                        "the history snapshot stops {} blocks behind the wallet",
                        wh - h.high_water
                    ),
                    "fmonero wallet history --limit 1   # rebuilds the snapshot",
                );
            }
        }
    }
    out
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
        assert_eq!(WARNING_IDS.len(), 19);
        assert!(
            !WARNING_IDS.contains(&"plugin_not_linked"),
            "that one is the doctor's (§5)"
        );
    }
}
