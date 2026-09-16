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
    Ok(Status {
        ok: true,
        schema: crate::SCHEMA_VERSION,
        generated_at: crate::now_rfc3339(),
        node: NodeStatus::unreachable(ctx.settings.node.url.clone(), units::Layout::None),
        wallet: WalletStatus::unreachable(
            ctx.settings.wallet.url.clone(),
            ctx.settings.wallet.account,
        ),
        units: UnitsStatus {
            node: units::UnitState::missing(&ctx.settings.node.unit),
            wallet: units::UnitState::missing(&ctx.settings.wallet.unit),
        },
        pending: None,
        warnings: Vec::new(),
        last_error: None,
        staleness: Staleness::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PendingTx;

    #[test]
    fn the_empty_document_is_schema_one_and_all_null() {
        let dir = std::env::temp_dir().join(format!("fmonero-status-{}", std::process::id()));
        let ctx = Context::for_test(&dir).expect("context");
        let s = status(&ctx).expect("status never fails on a machine with no daemons");
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["schema"], 1);
        assert_eq!(v["node"]["reachable"], false);
        assert_eq!(v["node"]["height"], serde_json::Value::Null);
        assert_eq!(v["node"]["layout"], "none");
        assert_eq!(v["wallet"]["open"], false);
        assert_eq!(v["pending"], serde_json::Value::Null);
        assert!(v["warnings"].as_array().unwrap().is_empty());
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
