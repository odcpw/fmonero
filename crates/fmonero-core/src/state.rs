//! `state.json`, `pending.json` and the two locks (DESIGN.md §5, §2a.4, §2a.11, §5b).
//!
//! The **types** are Phase 1a's and are frozen here; every body below is Phase 1b's (§8).
//! Three rules that the bodies must honour, restated so they cannot be lost between
//! phases:
//!
//! 1. Every read-modify-write of `state.json` goes through [`with_state`], which holds
//!    `flock(LOCK_EX)` on `paths.state_lock()` for its whole duration. `rename(2)` alone
//!    makes a *reader* safe; it does not make two read-modify-write cycles safe, and
//!    `status` runs from the plugin every 3 s and from the TUI every second (§5).
//! 2. The money record is **not** in that file. `pending.json` is its own 0600 file with
//!    exactly three writers — `send::relay`, [`mark_relayed`] and [`clear_pending_send`] —
//!    because `status` rewrites the whole of `state.json` on every invocation and a lost
//!    update there is the one that can double-spend (§2a.4).
//! 3. Everything fmonero writes is created at mode 0600 through tmp + `rename(2)`, never
//!    `chmod`ed afterwards (§2b.7).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::address::NetType;
use crate::amount::Piconero;
use crate::context::Context;
use crate::error::Error;
use crate::rpc::node::NetStats;
use crate::status::{FeeStatus, LastError};

/// Which daemon a breaker or a probe is about.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Daemon {
    Node,
    Wallet,
}

impl Daemon {
    /// The key this daemon's breaker is stored under in [`StateDoc::stalls`].
    pub fn key(self) -> &'static str {
        match self {
            Daemon::Node => "node",
            Daemon::Wallet => "wallet",
        }
    }
}

/// An armed §2d breaker: a call that a command *needed* was killed at its deadline, so
/// every later command that needs the same daemon exits 5 immediately for 60 s rather than
/// queueing another 120 s wait. **`status` never arms one — it only reports one.**
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Stall {
    pub at: i64,
    pub backoff_until: i64,
}

/// How long a breaker holds, in seconds (§2d).
pub const STALL_BACKOFF_SEC: i64 = 60;

/// The previous `get_net_stats` sample. Two of them make `rx_bps`/`tx_bps` meaningful; one
/// does not, which is why §5 renders `null` until the second (§5).
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NetSample {
    pub at: i64,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

/// The 60 s cold set of §5: the fields `status` refreshes at most once a minute and serves
/// from here in between. `staleness.node_extras_age_sec` is `now - built_at`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColdCache {
    pub built_at: i64,
    /// `hard_fork_info{version:0}.version` — the node's **current** fork version (§1b.3).
    pub hard_fork_version: Option<u8>,
    pub fee: FeeStatus,
    pub total_bytes_in: Option<u64>,
    pub total_bytes_out: Option<u64>,
    pub limit_up_kbps: Option<i64>,
    pub limit_down_kbps: Option<i64>,
    /// `get_accounts.subaddress_accounts.len()`.
    pub accounts: Option<u32>,
    /// Wallet `get_version`, the only source of §5's `wallet.rpc_version`.
    pub wallet_rpc_version: Option<String>,
}

/// The `state.json` document.
///
/// Public and named here because §5b rule 2 requires it (it appears in [`with_state`]'s
/// closure) and because the doctor's `state_valid` fixer must be able to render its
/// replacement bytes through `mutate(WriteFile{..})` (§7.1).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StateDoc {
    /// The open wallet's name **and its own nettype**, which is the §2a.6 send-path
    /// authority. Written by `wallet_session::open` and by nothing else.
    pub last_open: Option<(String, NetType)>,
    pub last_error: Option<LastError>,
    /// Keyed by [`Daemon::key`].
    pub stalls: BTreeMap<String, Stall>,
    pub net_sample: Option<NetSample>,
    pub cold: Option<ColdCache>,
}

impl StateDoc {
    /// The document's bytes, for the doctor's `state_valid` fixer — which may not call
    /// [`with_state`], because that writes the file itself and would bypass the backup, the
    /// sha256 pair and `actions.jsonl`, and therefore `doctor undo` (§7.1).
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::failure(format!("cannot serialise state.json: {e}")))?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// §2a.4's on-disk money record, serialized to `paths.pending_file()` (0600) and nowhere
/// else. The tx metadata is **never** part of it: it carries the transaction secret key,
/// and crash safety needs only the txid. It is never serialized into `Status` —
/// [`crate::status::PendingStatus`] is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingSend {
    pub kind: PendingKind,
    pub account_index: u32,
    pub dest: String,
    pub built_at: i64,
    /// One entry per built transaction, **in relay order**.
    pub txs: Vec<PendingTx>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingKind {
    Send,
    Sweep,
}

impl PendingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingKind::Send => "send",
            PendingKind::Sweep => "sweep",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingTx {
    pub txid: String,
    pub amount: Piconero,
    pub fee: Piconero,
    pub relayed: bool,
    pub relayed_at: Option<i64>,
}

// ---- state.json -------------------------------------------------------------

/// Load `state.json` under `flock(LOCK_EX)` on `paths.state_lock()` with a 2 s deadline,
/// apply `f`, write `.tmp.<pid>` at 0600, fsync, rename, release.
///
/// A lock that cannot be taken within the deadline **skips the write** rather than blocking
/// `status`, which is a read-only document with a stale cache at worst (§5).
pub fn with_state<T>(_ctx: &Context, _f: impl FnOnce(&mut StateDoc) -> T) -> Result<T, Error> {
    Err(Error::unimplemented("state::with_state"))
}

/// A read-only load of the whole document (§5b) — the doctor's `state_valid` fixer needs
/// the bytes without writing the file.
pub fn load(_ctx: &Context) -> Result<StateDoc, Error> {
    Err(Error::unimplemented("state::load"))
}

/// Records the wallet name `status` reports as `wallet.name` **and the wallet's own
/// nettype** (§2a.6). Written by `wallet_session::open` and by nothing else.
pub fn set_last_open(_ctx: &Context, _name: &str, _net: NetType) -> Result<(), Error> {
    Err(Error::unimplemented("state::set_last_open"))
}

/// Cleared by `wallet_session::close` and by any hot-path `get_height` that answers
/// `-13 NOT_OPEN` — so a wallet closed behind fmonero's back leaves no stale name on the
/// bar widget (§5).
pub fn clear_last_open(_ctx: &Context) -> Result<(), Error> {
    Err(Error::unimplemented("state::clear_last_open"))
}

pub fn last_open(_ctx: &Context) -> Result<Option<(String, NetType)>, Error> {
    Err(Error::unimplemented("state::last_open"))
}

pub fn set_last_error(_ctx: &Context, _e: &Error, _text: Option<String>) -> Result<(), Error> {
    Err(Error::unimplemented("state::set_last_error"))
}

/// Arm a §2d breaker. **Never called by `status`**, and never by `send::relay` (§2d).
pub fn note_stall(_ctx: &Context, _who: Daemon) -> Result<(), Error> {
    Err(Error::unimplemented("state::note_stall"))
}

/// Seconds left on an armed breaker, or `None`.
pub fn stall_remaining(_ctx: &Context, _who: Daemon) -> Result<Option<u64>, Error> {
    Err(Error::unimplemented("state::stall_remaining"))
}

/// Store one `get_net_stats` sample and return `(rx_bps, tx_bps)` against the previous one,
/// or `None` when there is no previous sample (§5).
pub fn net_sample(_ctx: &Context, _s: NetStats) -> Result<Option<(u64, u64)>, Error> {
    Err(Error::unimplemented("state::net_sample"))
}

// ---- pending.json: its own file, three writers, `status` reads it only (§2a.4) ----

pub fn pending_send(_ctx: &Context) -> Result<Option<PendingSend>, Error> {
    Err(Error::unimplemented("state::pending_send"))
}

/// Writes the WHOLE record to `paths.pending_file()` and **fsyncs**. Called exactly once
/// per send, by `send::relay`, before the first `relay_tx` (§2a.3, §2a.4).
pub fn set_pending_send(_ctx: &Context, _p: &PendingSend) -> Result<(), Error> {
    Err(Error::unimplemented("state::set_pending_send"))
}

/// Flips one entry of the list to `relayed: true` and re-fsyncs before the next `relay_tx`.
pub fn mark_relayed(_ctx: &Context, _txid: &str) -> Result<(), Error> {
    Err(Error::unimplemented("state::mark_relayed"))
}

/// Called from `send::build`'s resolve step and from `wallet clear-pending` — and from
/// nowhere else (§2a.3).
pub fn clear_pending_send(_ctx: &Context) -> Result<(), Error> {
    Err(Error::unimplemented("state::clear_pending_send"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_document_round_trips_and_defaults_to_empty() {
        let mut doc = StateDoc::default();
        assert!(doc.last_open.is_none());
        assert!(doc.stalls.is_empty());
        doc.last_open = Some(("stagetest".to_string(), NetType::Stagenet));
        doc.stalls.insert(
            Daemon::Wallet.key().to_string(),
            Stall {
                at: 1,
                backoff_until: 61,
            },
        );
        let bytes = doc.to_bytes().unwrap();
        let back: StateDoc = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.last_open.as_ref().unwrap().0, "stagetest");
        assert_eq!(back.last_open.as_ref().unwrap().1, NetType::Stagenet);
        assert_eq!(back.stalls["wallet"].backoff_until, 61);
        // An older document with fewer keys still loads (§7.1's state_valid is about
        // parse failures, not about missing keys).
        let sparse: StateDoc = serde_json::from_str("{}").unwrap();
        assert!(sparse.cold.is_none());
    }

    #[test]
    fn the_pending_record_spells_its_kind_in_lowercase() {
        let p = PendingSend {
            kind: PendingKind::Sweep,
            account_index: 0,
            dest: "4".repeat(95),
            built_at: 1_700_000_000,
            txs: vec![PendingTx {
                txid: "b".repeat(64),
                amount: Piconero(1),
                fee: Piconero(2),
                relayed: false,
                relayed_at: None,
            }],
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "sweep");
        assert_eq!(v["txs"][0]["relayed"], false);
        assert_eq!(v["txs"][0]["relayed_at"], serde_json::Value::Null);
        // The record never carries the transaction metadata (§2a.4).
        assert!(v["txs"][0].get("metadata").is_none());
        assert_eq!(serde_json::to_value(PendingKind::Send).unwrap(), "send");
    }

    #[test]
    fn the_daemon_keys_are_stable() {
        assert_eq!(Daemon::Node.key(), "node");
        assert_eq!(Daemon::Wallet.key(), "wallet");
        assert_eq!(STALL_BACKOFF_SEC, 60);
    }
}
