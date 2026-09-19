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
use crate::rpc::wallet::TransferQuery;
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
    /// The kind as a **person** reads it (§2o rule 5, §2o.2): `as_str` is the wire value that
    /// `--json` carries and agents branch on, and it was printed as the `WHAT` column of both
    /// history tables — where `block` means a mining reward and `pool` means waiting to be mined,
    /// neither of which a reader can work out. One implementation, so the TUI's table and
    /// `wallet history`'s cannot word one payment two ways.
    pub fn words(self) -> &'static str {
        match self {
            TransferKind::In => "in",
            TransferKind::Out => "out",
            TransferKind::Pending => "sending",
            TransferKind::Failed => "failed",
            TransferKind::Pool => "waiting",
            TransferKind::Block => "mined",
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

    /// **What "my payments" means when nobody asked for a subset** (X17).
    ///
    /// `wallet history` used `in`/`block`/`out`/`pending` and the TUI's Payments pane used those
    /// plus `failed`, so one wallet read `5 payments` in the terminal and six rows on the screen —
    /// the same question, two answers, and the row that differed was a payment that did not go
    /// through. One constant, and both callers read it.
    ///
    /// `pool` is **not** here and that is deliberate on both surfaces: §4.1 sends the daemon round
    /// trip `pool` needs only when the caller asked for pool rows. `--pool` asks for it in the CLI
    /// and `t` reaches it in the TUI.
    pub const DEFAULT_ROWS: &'static [TransferKind] = &[
        TransferKind::In,
        TransferKind::Block,
        TransferKind::Out,
        TransferKind::Pending,
        TransferKind::Failed,
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
pub fn load(ctx: &Context, account: u32) -> Result<Vec<Transfer>, Error> {
    let snap = read_snapshot(ctx);
    if !snap.usable_for(None) {
        return Ok(Vec::new());
    }
    Ok(snap.accounts.get(&account).cloned().unwrap_or_default())
}

/// The incremental top-up. **Its only caller is [`query`]** (§5b rule 1): the top-up, the
/// `get_height` and the row merge are one operation, and splitting them across callers is
/// how the node's height ends up in `confirmations`.
pub fn refresh(ctx: &Context, w: &WalletClient, account: u32) -> Result<usize, Error> {
    let before = load(ctx, account)?.len();
    let mut q = HistoryQuery {
        account_index: account,
        limit: usize::MAX,
        kinds: vec![TransferKind::In, TransferKind::Block, TransferKind::Out],
        ..HistoryQuery::default()
    };
    query(ctx, Some(w), &mut q)?;
    Ok(load(ctx, account)?.len().saturating_sub(before))
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
    ctx: &Context,
    w: Option<&WalletClient>,
    q: &mut HistoryQuery,
) -> Result<Vec<Transfer>, Error> {
    // Ignored on input, always written here (§5b): that is what makes handing this function
    // the NODE's height unrepresentable.
    q.wallet_height = None;
    q.adjusted_now = None;
    q.pool_unavailable = false;

    let mut snap = read_snapshot(ctx);
    let mut live: Vec<Transfer> = Vec::new();

    if let Some(w) = w {
        // The wallet's own height, which is the clock wallet-rpc recomputes `confirmations`
        // and `locked` against (§1b.4).
        let wh = w.get_height()?;
        q.wallet_height = Some(wh);
        q.adjusted_now = Some(crate::now_unix());

        let primary = w.get_address(q.account_index)?.primary;
        let fp = fingerprint(&primary);
        if !snap.usable_for(Some(fp)) {
            snap = Snapshot::fresh(fp);
        }

        // Re-read a day's worth rather than starting at the high-water mark, so a reorg
        // inside that window is corrected instead of being cached forever (§5b).
        let min_height = snap.high_water.saturating_sub(REFETCH_WINDOW_BLOCKS);
        // `pool` only when the caller asked for pool rows: that branch alone runs
        // `update_pool_state`, a daemon round trip that throws when the node is unreachable
        // and turns the WHOLE response into epee's `code: 0` (§1b.4).
        let want_pool = q.kinds.contains(&TransferKind::Pool);
        let mut wire = TransferQuery {
            account_index: q.account_index,
            r#in: true,
            out: true,
            pending: true,
            failed: true,
            pool: want_pool,
            // Mandatory: without it the bound is silently ignored and the daemon returns the
            // whole history (§1b.4).
            filter_by_height: true,
            min_height,
            subaddr_indices: Vec::new(),
        };
        let rows = match w.get_transfers(&wire) {
            Ok(rows) => rows,
            // Retried EXACTLY ONCE without `pool`: the rows are then complete except for the
            // pool, which is not a failure of a read-only command (§4.1).
            Err(e) if want_pool => {
                wire.pool = false;
                q.pool_unavailable = true;
                w.get_transfers(&wire).map_err(|_| e)?
            }
            Err(e) => return Err(e),
        };

        // Confirmed `in`/`out`/`block` rows are merged into the snapshot; `pending`, `failed`
        // and `pool` are returned from this response and NEVER cached (§5b).
        let confirmed_below = wh.saturating_sub(SPENDABLE_AGE);
        let cached = snap.accounts.entry(q.account_index).or_default();
        for row in rows {
            let cacheable = matches!(
                row.kind,
                TransferKind::In | TransferKind::Out | TransferKind::Block
            ) && row.height != 0
                && row.height <= confirmed_below;
            if cacheable {
                match cached.iter().position(|c| c.txid == row.txid) {
                    Some(i) => cached[i] = row,
                    None => cached.push(row),
                }
            } else {
                live.push(row);
            }
        }
        cached.sort_by(|a, b| {
            b.height
                .cmp(&a.height)
                .then_with(|| b.timestamp.cmp(&a.timestamp))
                .then_with(|| a.txid.cmp(&b.txid))
        });
        // `high_water` is the highest CONFIRMED height in the snapshot, never the tip (§5b).
        snap.high_water = cached.iter().map(|t| t.height).max().unwrap_or(0);
        snap.built_at = crate::now_unix();
        write_snapshot(ctx, &snap)?;
    } else if !snap.usable_for(None) {
        // `--cached` with a stale or absent snapshot: an empty answer, not an error (§5b).
        snap = Snapshot::default();
    }

    let mut rows: Vec<Transfer> = snap
        .accounts
        .get(&q.account_index)
        .cloned()
        .unwrap_or_default();
    rows.extend(live);
    // The kind filter, then `--since`, then the newest `limit` rows.
    if !q.kinds.is_empty() {
        rows.retain(|t| q.kinds.contains(&t.kind));
    }
    if let Some(since) = q.since_height {
        // A pending/pool row has height 0 and is by definition not below any height the user
        // asked about, so it survives the filter (it is the newest thing there is).
        rows.retain(|t| t.height == 0 || t.height >= since);
    }
    rows.sort_by(|a, b| {
        // Unconfirmed rows (height 0) are the newest; below them, newest height first.
        let key = |t: &Transfer| (t.height == 0, t.height, t.timestamp);
        key(b).cmp(&key(a))
    });
    if rows.len() > q.limit {
        rows.truncate(q.limit);
    }
    Ok(rows)
}

/// The ONLY implementation of `confirmations` (§1b.4, `wallet_rpc_server.cpp:162-167`):
/// `0` when `height == 0` or `height >= wh`, otherwise `wh - height`.
///
/// `wh` is the **WALLET's** height; `None` (a `--cached` read) renders JSON `null` and `—`
/// in a table, never `0` (§5's shape rules).
pub fn confirmations(t: &Transfer, wh: Option<u64>) -> Option<u64> {
    let wh = wh?;
    if t.height == 0 || t.height >= wh {
        return Some(0);
    }
    Some(wh - t.height)
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
pub fn locked(t: &Transfer, wh: Option<u64>, adjusted_now: Option<i64>) -> Option<bool> {
    let wh = wh?;
    let adjusted_now = adjusted_now?;
    // `pending` and `pool` rows are unconditionally locked (`wallet_rpc_server.cpp:551`, `:581`).
    if matches!(t.kind, TransferKind::Pending | TransferKind::Pool) {
        return Some(true);
    }
    // A `failed` row was never accepted; it is not spendable and not "unlocked".
    if t.kind == TransferKind::Failed {
        return Some(true);
    }
    let age_ok = t.height != 0 && t.height + SPENDABLE_AGE <= wh;
    let spendtime_ok = if t.unlock_time == 0 {
        true
    } else if t.unlock_time < MAX_BLOCK_NUMBER {
        // An absolute BLOCK INDEX, never a delta added to `height` (`wallet2.cpp:7319-7344`).
        wh >= t.unlock_time
    } else {
        // A unix TIMESTAMP, with the V2 leeway of 120 s (§1b.4).
        adjusted_now + LOCK_LEEWAY_SEC >= t.unlock_time as i64
    };
    Some(!(age_ok && spendtime_ok))
}

/// `Ok(None)` when the file is absent; never an error. Exists because the doctor's
/// `history_cache_fresh` check (§7.1) must read the header and may not touch core's
/// internals.
pub fn header(ctx: &Context) -> Result<Option<CacheHeader>, Error> {
    let path = ctx.paths.history_cache();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Error::failure(format!(
                "cannot read {}: {e}",
                path.display()
            )))
        }
    };
    // A file that does not decode has no header to report, and postcard is not
    // self-describing so a version bump lands here: `magic_ok: false` says so without
    // pretending the numbers beside it mean anything.
    Ok(Some(
        postcard::from_bytes::<Snapshot>(&bytes)
            .map(|s| s.header())
            .unwrap_or_default(),
    ))
}

// ---- the snapshot ------------------------------------------------------------

/// The whole on-disk snapshot: a header plus the confirmed rows, postcard-encoded.
///
/// One struct rather than a hand-rolled header and a body, because postcard is **not
/// self-describing**: a field added to either half changes the byte layout, which is exactly
/// what [`CACHE_FORMAT_VERSION`] exists to notice.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Snapshot {
    magic: [u8; 4],
    format_version: u32,
    wallet_fp: [u8; 16],
    high_water: u64,
    built_at: i64,
    /// Keyed by account index, because one `history.bin` serves every account of the wallet
    /// and `wallet history --account 1` must not discard account 0's rows.
    accounts: std::collections::BTreeMap<u32, Vec<Transfer>>,
}

impl Snapshot {
    fn header(&self) -> CacheHeader {
        CacheHeader {
            magic_ok: &self.magic == CACHE_MAGIC,
            format_version: self.format_version,
            wallet_fp: self.wallet_fp,
            high_water: self.high_water,
            built_at: self.built_at,
        }
    }
    fn fresh(fp: [u8; 16]) -> Snapshot {
        Snapshot {
            magic: *CACHE_MAGIC,
            format_version: CACHE_FORMAT_VERSION,
            wallet_fp: fp,
            ..Snapshot::default()
        }
    }
    /// `true` when the header says this file is about this wallet and this format. A mismatch
    /// is **STALE, not corrupt**: ignored and refetched, never an error (§5b).
    fn usable_for(&self, fp: Option<[u8; 16]>) -> bool {
        &self.magic == CACHE_MAGIC
            && self.format_version == CACHE_FORMAT_VERSION
            && fp.is_none_or(|f| f == self.wallet_fp)
    }
}

/// Read the snapshot, or an empty one. Never an error: a cache is not a result.
fn read_snapshot(ctx: &Context) -> Snapshot {
    std::fs::read(ctx.paths.history_cache())
        .ok()
        .and_then(|b| postcard::from_bytes::<Snapshot>(&b).ok())
        .unwrap_or_default()
}

fn write_snapshot(ctx: &Context, s: &Snapshot) -> Result<(), Error> {
    ctx.paths.ensure_dirs()?;
    let bytes = postcard::to_stdvec(s)
        .map_err(|e| Error::failure(format!("cannot encode the history snapshot: {e}")))?;
    // 0600: the snapshot holds the whole transfer history, with addresses and notes (§2b.7).
    crate::write_private(&ctx.paths.history_cache(), &bytes)
}

/// The first 16 bytes of `sha256(primary_address)`. The address is public (§2b); the
/// fingerprint only has to notice that this is a **different wallet**.
fn fingerprint(primary_address: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(primary_address.as_bytes());
    let mut fp = [0u8; 16];
    fp.copy_from_slice(&digest[..16]);
    fp
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

/// The two derived fields of §1b.4, tested on their own — the module is named so that
/// `cargo test -p fmonero-core history::locked` (§8's gate line) selects exactly these.
///
/// The formula is a **conjunction of two independent conditions**, not a maximum, and
/// `unlock_time` is an absolute height **or** a unix timestamp — never a delta added to
/// `height`. Treating it as a delta marks every timestamp-unlocked incoming transfer locked
/// until the year 55 000, and that is the mistake this module exists to catch.
#[cfg(test)]
mod locked {
    use super::*;

    fn row(height: u64, unlock_time: u64, kind: TransferKind) -> Transfer {
        Transfer {
            txid: "d".repeat(64),
            kind,
            height,
            timestamp: 1_700_000_000,
            amount: Piconero(1),
            fee: Piconero(2),
            unlock_time,
            ..Transfer::default()
        }
    }

    /// §8's gate line, verbatim: `locked` is false for `unlock_time == 0` at
    /// `wh = height + 10`, true at `wh = height + 9`, and `unlock_time = 1_900_000_000` is read
    /// as a **timestamp** and not as a height.
    #[test]
    fn the_age_rule_is_ten_blocks_and_a_big_unlock_time_is_a_timestamp() {
        let h = 3_500_000;
        let t = row(h, 0, TransferKind::In);
        let now = Some(1_700_000_000);
        assert_eq!(
            super::locked(&t, Some(h + SPENDABLE_AGE), now),
            Some(false),
            "at height + 10 it is unlocked (wallet2.cpp:7313)"
        );
        assert_eq!(
            super::locked(&t, Some(h + SPENDABLE_AGE - 1), now),
            Some(true),
            "one block short is still locked"
        );

        // 1_900_000_000 is above CRYPTONOTE_MAX_BLOCK_NUMBER (500_000_000), so it is a unix
        // TIMESTAMP. Read as a height it would be "locked until block 1.9 billion", i.e.
        // forever; read as a timestamp it unlocks when the clock passes it, minus the leeway.
        let t = row(h, 1_900_000_000, TransferKind::In);
        let wh = Some(h + SPENDABLE_AGE);
        assert_eq!(
            super::locked(&t, wh, Some(1_899_999_000)),
            Some(true),
            "before the timestamp (minus the 120 s leeway) it is locked"
        );
        assert_eq!(
            super::locked(&t, wh, Some(1_900_000_000 - LOCK_LEEWAY_SEC)),
            Some(false),
            "the V2 leeway is 120 s (wallet2.cpp:7336-7337)"
        );
        assert_eq!(
            super::locked(&t, wh, Some(1_900_000_000 - LOCK_LEEWAY_SEC - 1)),
            Some(true),
            "one second before that, still locked"
        );
        // And the boundary the other way: just under MAX_BLOCK_NUMBER is a HEIGHT.
        let t = row(h, MAX_BLOCK_NUMBER - 1, TransferKind::In);
        assert_eq!(super::locked(&t, wh, now), Some(true), "a height far ahead");
        assert_eq!(
            super::locked(&t, Some(MAX_BLOCK_NUMBER), now),
            Some(false),
            "once the wallet reaches it, unlocked"
        );
    }

    /// A **coinbase** row needs no special age rule: its 60-block window arrives inside
    /// `unlock_time` as an absolute height, which the height branch already handles. A second,
    /// coinbase-specific 60 would double-count it (§1b.4).
    #[test]
    fn a_coinbase_row_uses_the_same_age_rule_and_its_own_unlock_height() {
        let h = 3_500_000;
        let t = row(h, h + 60, TransferKind::Block);
        // Old enough by age, but its own unlock height has not been reached.
        assert_eq!(super::locked(&t, Some(h + 10), Some(0)), Some(true));
        assert_eq!(super::locked(&t, Some(h + 59), Some(0)), Some(true));
        assert_eq!(super::locked(&t, Some(h + 60), Some(0)), Some(false));
    }

    /// `pending` and `pool` rows are unconditionally locked
    /// (`wallet_rpc_server.cpp:551`, `:581`), and a `failed` row was never accepted.
    #[test]
    fn unconfirmed_rows_are_always_locked() {
        for kind in [
            TransferKind::Pending,
            TransferKind::Pool,
            TransferKind::Failed,
        ] {
            let t = row(0, 0, kind);
            assert_eq!(
                super::locked(&t, Some(3_500_000), Some(0)),
                Some(true),
                "{kind}"
            );
        }
    }

    /// With no wallet height — a `--cached` read — **both** fields render `null`, never `0`
    /// (§5's shape rules, §4.1).
    #[test]
    fn with_no_wallet_height_both_fields_are_null() {
        let t = row(3_500_000, 0, TransferKind::In);
        assert_eq!(super::locked(&t, None, Some(0)), None);
        assert_eq!(super::locked(&t, Some(3_500_010), None), None);
        assert_eq!(super::confirmations(&t, None), None);
    }

    /// `confirmations` (`wallet_rpc_server.cpp:162-167`): `0` when `height == 0` or
    /// `height >= wh`, otherwise `wh - height` — against the **wallet's** height, never the
    /// node's.
    #[test]
    fn confirmations_is_the_wallets_height_minus_the_rows() {
        let t = row(3_500_000, 0, TransferKind::In);
        assert_eq!(super::confirmations(&t, Some(3_501_122)), Some(1_122));
        assert_eq!(super::confirmations(&t, Some(3_500_000)), Some(0));
        assert_eq!(
            super::confirmations(&t, Some(3_499_000)),
            Some(0),
            "a wallet behind the row never reports a negative count"
        );
        let unconfirmed = row(0, 0, TransferKind::Pending);
        assert_eq!(super::confirmations(&unconfirmed, Some(3_500_000)), Some(0));
    }
}
