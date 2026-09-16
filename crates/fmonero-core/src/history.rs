//! The transfer history: one postcard snapshot plus one incremental top-up
//! (DESIGN.md §1b.4, §5b, §4.1).
//!
//! **Signature-only at the Phase 1a gate** (§8): the types are frozen here and Phase 1b
//! writes the bodies. The two derived fields `confirmations` and `locked` are deliberately
//! **not** fields of [`Transfer`]: wallet-rpc recomputes them per response from the
//! **wallet's** current height (`wallet_rpc_server.cpp:162`, `:505`), so a cached copy is a
//! wrong answer about money one block later.

use serde::{Deserialize, Serialize};

use crate::amount::Piconero;
use crate::context::Context;
use crate::error::Error;
use crate::rpc::WalletClient;

/// `CRYPTONOTE_DEFAULT_TX_SPENDABLE_AGE` (`cryptonote_config.h:49`) — the age half of
/// `is_transfer_unlocked` (`wallet2.cpp:7313-7314`).
pub const SPENDABLE_AGE: u64 = 10;
/// `CRYPTONOTE_MAX_BLOCK_NUMBER` (`cryptonote_config.h:40`): below it an `unlock_time` is
/// an absolute **block height**, at or above it a **unix timestamp** (`wallet2.cpp:7319-7344`).
pub const MAX_BLOCK_NUMBER: u64 = 500_000_000;
/// The V2 timestamp leeway, 120 s (`wallet2.cpp:7336-7337`, `cryptonote_config.h:88-90`).
/// The V1 60 s applies only below the V2 height (mainnet 1 009 827), which no row fmonero
/// shows reaches (§1b.4).
pub const LOCK_LEEWAY_SEC: i64 = 120;
/// How far back a top-up re-reads, so a reorg inside the window is corrected instead of
/// being cached forever (≈ one day at Monero's 120 s target block time).
pub const REFETCH_WINDOW_BLOCKS: u64 = 720;

/// `transfer_entry.subaddr_index` is the JSON **object** `{major, minor}`
/// (`subaddress_index.h:38-55`), never a scalar. `major` **is** the account index, so
/// [`Transfer`] carries no separate `account_index`: a `u32` here would fail to
/// deserialize every history row (§1b.4).
#[derive(Copy, Clone, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SubaddrIndex {
    pub major: u32,
    pub minor: u32,
}

/// One entry of `transfer_entry.destinations[]` (`:1456`). Empty for `in` rows; it is the
/// one fact a user checks after a send, so §7a's detail pane renders it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Destination {
    pub address: String,
    pub amount: Piconero,
}

/// `transfer_entry.type` — **six** values, not five.
///
/// `fill_transfer_entry` types an incoming row `pd.m_coinbase ? "block" : "in"`
/// (`wallet_rpc_server.cpp:501`) and pushes it into the **`in`** list, so `"block"` is an
/// ordinary member of `get_transfers`' `in` array for any wallet holding a mined or p2pool
/// output. A five-variant enum fails to deserialize the *whole* response, and with it
/// `wallet history` and the §2a.4 resolve step.
///
/// `Block` needs no special unlock rule: upstream's `is_transfer_unlocked` uses the same
/// 10-block age for every row, and a coinbase's 60-block window arrives inside
/// `unlock_time` as an absolute height, which the height branch of [`locked`] handles.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferKind {
    #[default]
    In,
    Out,
    Pending,
    Failed,
    Pool,
    Block,
}

impl TransferKind {
    /// The wire spelling, which is also §5c's `type` key.
    pub fn as_str(self) -> &'static str {
        match self {
            TransferKind::In => "in",
            TransferKind::Out => "out",
            TransferKind::Pending => "pending",
            TransferKind::Failed => "failed",
            TransferKind::Pool => "pool",
            TransferKind::Block => "block",
        }
    }
    /// Every variant, in §5c's order — the type filter of §7a cycles through these.
    pub const ALL: &'static [TransferKind] = &[
        TransferKind::In,
        TransferKind::Out,
        TransferKind::Pending,
        TransferKind::Failed,
        TransferKind::Pool,
        TransferKind::Block,
    ];
}

impl std::fmt::Display for TransferKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One history row (`transfer_entry`, `:1446-1487`).
///
/// `confirmations` and `locked` are NOT fields: they are derived at render time from
/// `height`, `unlock_time` and the **wallet's** height through [`confirmations`] and
/// [`locked`]. Storing them would show stale numbers about money (§1b.4).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Transfer {
    pub txid: String,
    pub kind: TransferKind,
    pub height: u64,
    pub timestamp: i64,
    pub amount: Piconero,
    pub fee: Piconero,
    pub note: String,
    pub address: String,
    pub subaddr_index: SubaddrIndex,
    pub destinations: Vec<Destination>,
    pub unlock_time: u64,
    pub payment_id: String,
    pub double_spend_seen: bool,
}

/// What `wallet history` (and the TUI's Wallet screen) asks for.
#[derive(Clone, Debug, Default)]
pub struct HistoryQuery {
    pub account_index: u32,
    pub limit: usize,
    pub kinds: Vec<TransferKind>,
    pub since_height: Option<u64>,
    /// `--cached`: serve the snapshot alone, issue no RPC (§2l (d)).
    pub cached: bool,
    /// The **WALLET's** current height, which is the clock wallet-rpc itself recomputes
    /// `confirmations` and `locked` against (§1b.4) — never the node's. A wallet behind the
    /// node is the normal state for hours after `wallet open` (§2d), and against the node's
    /// height fmonero would report a payment as more settled than the wallet can see.
    /// **Written by `query` from its own `get_height`, never by a caller** — it is ignored
    /// on input, which is what makes handing it the node's height unrepresentable. Stays
    /// `None` with `--cached`, in which case `confirmations` and `locked` render `null`.
    pub wallet_height: Option<u64>,
    /// The clock the timestamp branch of [`locked`] uses: **local time**, which is
    /// upstream's own fallback when no daemon answers (`wallet2.cpp:7331-7333`), because
    /// `wallet history` contacts no node (§4.1). Written by `query` alongside
    /// `wallet_height` and ignored on input.
    pub adjusted_now: Option<i64>,
    /// **Written by `query`, never by a caller** (like the two fields above): `true` when
    /// the caller asked for pool rows and the daemon round trip inside `get_transfers`
    /// failed, so the rows are complete except for the pool (§4.1, §5c's `pool_unavailable`).
    ///
    /// DESIGN.md §5b does not list this field and §5c gives the key no other producer —
    /// `query` returns a plain `Vec<Transfer>`, and "no pool rows" is indistinguishable
    /// from "an empty pool". It is written back the same way `wallet_height` and
    /// `adjusted_now` are; the deviation is recorded in DECISIONS.md.
    pub pool_unavailable: bool,
}

/// The header of `cache_dir/history.bin` (§5b).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CacheHeader {
    pub magic_ok: bool,
    /// Bumped on **any** change to a serialized struct: postcard is not self-describing.
    pub format_version: u32,
    /// The first 16 bytes of `sha256(primary_address)` — the address is public (§2b).
    pub wallet_fp: [u8; 16],
    /// The highest **confirmed** height in the snapshot, never the tip (§5b).
    pub high_water: u64,
    pub built_at: i64,
}

/// `b"FMHX"` — the snapshot's magic (§5b).
pub const CACHE_MAGIC: &[u8; 4] = b"FMHX";
/// The current snapshot format version (§5b): bump on any change to a serialized struct.
pub const CACHE_FORMAT_VERSION: u32 = 1;

/// Load the snapshot for one account. A header mismatch is **STALE, not corrupt**:
/// ignored and refetched, never an error (§5b).
pub fn load(_ctx: &Context, _account: u32) -> Result<Vec<Transfer>, Error> {
    Err(Error::unimplemented("history::load"))
}

/// The incremental top-up. **Its only caller is [`query`]** (§5b rule 1): the top-up, the
/// `get_height` and the row merge are one operation, and splitting them across callers is
/// how the node's height ends up in `confirmations`.
pub fn refresh(_ctx: &Context, _w: &WalletClient, _account: u32) -> Result<usize, Error> {
    Err(Error::unimplemented("history::refresh"))
}

/// The one place the top-up happens. `w` is `Some` unless `q.cached`.
///
/// With `Some(w)`: one wallet `get_height`, one `get_transfers` with
/// `filter_by_height: true` (and `pool` only when `q.kinds` asks for it), confirmed
/// `in`/`out`/`block` rows merged into the snapshot, and the `pending`/`failed`/`pool` rows
/// returned straight from that response and never cached. It **writes** `q.wallet_height`
/// from its own `get_height` and `q.adjusted_now` from the local clock, so no caller can
/// hand it the NODE's height. With `w: None` it serves the snapshot alone, issues nothing,
/// leaves both `None`, and both derived fields render `null` (§2l (d)).
pub fn query(
    _ctx: &Context,
    _w: Option<&WalletClient>,
    _q: &mut HistoryQuery,
) -> Result<Vec<Transfer>, Error> {
    Err(Error::unimplemented("history::query"))
}

/// The ONLY implementation of `confirmations` (§1b.4, `wallet_rpc_server.cpp:162-167`):
/// `0` when `height == 0` or `height >= wh`, otherwise `wh - height`.
///
/// `wh` is the **WALLET's** height; `None` (a `--cached` read) renders JSON `null` and `—`
/// in a table, never `0` (§5's shape rules).
pub fn confirmations(_t: &Transfer, _wh: Option<u64>) -> Option<u64> {
    None
}

/// The ONLY implementation of `locked` (§1b.4, `wallet2.cpp:7308-7344`).
///
/// A **conjunction of two independent conditions**, not a maximum:
///
/// ```text
/// age_ok       = height != 0 && height + SPENDABLE_AGE (10) <= wh
/// spendtime_ok = if unlock_time == 0                   { true }
///                else if unlock_time < MAX_BLOCK_NUMBER { wh >= unlock_time }  // a HEIGHT
///                else                                   { adjusted_now + 120 >= unlock_time }
/// locked       = !(age_ok && spendtime_ok)
/// ```
///
/// `unlock_time` is therefore **never a delta added to `height`**: treating it as one marks
/// every timestamp-unlocked incoming transfer locked until the year 55 000. `pending` and
/// `pool` rows are unconditionally locked (`wallet_rpc_server.cpp:551`, `:581`).
pub fn locked(_t: &Transfer, _wh: Option<u64>, _adjusted_now: Option<i64>) -> Option<bool> {
    None
}

/// `Ok(None)` when the file is absent; never an error. Exists because the doctor's
/// `history_cache_fresh` check (§7.1) must read the header and may not touch core's
/// internals.
pub fn header(_ctx: &Context) -> Result<Option<CacheHeader>, Error> {
    Err(Error::unimplemented("history::header"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5b rule 3: the wire spelling is part of the contract, and `block` is the variant a
    /// five-value enum would miss.
    #[test]
    fn the_six_transfer_kinds_spell_themselves_in_lowercase() {
        assert_eq!(TransferKind::ALL.len(), 6);
        for (kind, wire) in [
            (TransferKind::In, "in"),
            (TransferKind::Out, "out"),
            (TransferKind::Pending, "pending"),
            (TransferKind::Failed, "failed"),
            (TransferKind::Pool, "pool"),
            (TransferKind::Block, "block"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), wire);
            let back: TransferKind = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(back, kind);
            assert_eq!(kind.as_str(), wire);
        }
    }

    /// A row of the shape §1b.4 warns about: `subaddr_index` is an object, and a coinbase
    /// row arrives typed `block` inside the `in` list.
    #[test]
    fn a_coinbase_row_with_an_object_subaddr_index_deserializes() {
        let t: Transfer = serde_json::from_value(serde_json::json!({
            "txid": "a".repeat(64),
            "kind": "block",
            "height": 3_500_000,
            "timestamp": 1_700_000_000i64,
            "amount": 600_000_000_000u64,
            "subaddr_index": {"major": 0, "minor": 3},
            "destinations": [],
            "unlock_time": 3_500_060u64,
        }))
        .expect("a `block` row must deserialize");
        assert_eq!(t.kind, TransferKind::Block);
        assert_eq!(t.subaddr_index.major, 0);
        assert_eq!(t.subaddr_index.minor, 3);
        assert_eq!(t.amount, Piconero(600_000_000_000));
    }

    #[test]
    fn the_unlock_constants_are_the_ones_from_source() {
        assert_eq!(SPENDABLE_AGE, 10);
        assert_eq!(MAX_BLOCK_NUMBER, 500_000_000);
        assert_eq!(LOCK_LEEWAY_SEC, 120);
    }
}
