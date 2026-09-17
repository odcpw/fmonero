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
    /// The last `blocks_behind` `status` observed (§5), which is what makes §2d's wallet
    /// exemption decidable: a wallet call killed at its deadline while the wallet is more
    /// than [`crate::WALLET_BEHIND_BLOCKS`] behind is `wallet_syncing`, not a stall, and the
    /// classification needs a number no command on the wallet path has itself.
    ///
    /// DESIGN.md §5b lists this document's fields without it; §2d's sentence
    /// ("while `status.wallet.blocks_behind > 100`") names a value only `status` computes, and
    /// `status` already rewrites this file on every invocation. The deviation is recorded in
    /// DECISIONS.md.
    pub wallet_behind: Option<i64>,
    /// v5.2: unix seconds of the last `refresh` **fmonero** issued (§5's
    /// `wallet.last_refresh_at`). Written only by [`note_refresh`], read only by `status`.
    ///
    /// An absent key deserializes to `None` through `#[derive(Default)]`, so state files
    /// written by a pre-v5.2 build load unchanged.
    pub last_refresh: Option<i64>,
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
pub fn with_state<T>(ctx: &Context, f: impl FnOnce(&mut StateDoc) -> T) -> Result<T, Error> {
    ctx.paths.ensure_dirs()?;
    let guard = FileLock::take(&ctx.paths.state_lock(), LOCK_DEADLINE)?;
    let mut doc = read_doc(&ctx.paths.state_file());
    let out = f(&mut doc);
    // A lock fmonero could not take never blocks `status`, which is a read-only document
    // with a stale cache at worst (§5). The mutation still happened in memory, so the
    // caller's answer is the same; only the write is skipped.
    if guard.is_some() {
        crate::write_private(&ctx.paths.state_file(), &doc.to_bytes()?)?;
    }
    Ok(out)
}

/// A read-only load of the whole document (§5b) — the doctor's `state_valid` fixer needs
/// the bytes without writing the file.
pub fn load(ctx: &Context) -> Result<StateDoc, Error> {
    Ok(read_doc(&ctx.paths.state_file()))
}

/// Records the wallet name `status` reports as `wallet.name` **and the wallet's own
/// nettype** (§2a.6). Written by `wallet_session::open` and by nothing else.
pub fn set_last_open(ctx: &Context, name: &str, net: NetType) -> Result<(), Error> {
    with_state(ctx, |s| s.last_open = Some((name.to_string(), net)))
}

/// Cleared by `wallet_session::close` and by any hot-path `get_height` that answers
/// `-13 NOT_OPEN` — so a wallet closed behind fmonero's back leaves no stale name on the
/// bar widget (§5).
pub fn clear_last_open(ctx: &Context) -> Result<(), Error> {
    with_state(ctx, |s| s.last_open = None)
}

pub fn last_open(ctx: &Context) -> Result<Option<(String, NetType)>, Error> {
    Ok(read_doc(&ctx.paths.state_file()).last_open)
}

pub fn set_last_error(ctx: &Context, e: &Error, text: Option<String>) -> Result<(), Error> {
    let record = LastError {
        at: crate::now_rfc3339(),
        code: e.exit_code(),
        code_name: e.code_name().to_string(),
        text: text.unwrap_or_else(|| e.to_string()),
        hint: e.hint(),
    };
    with_state(ctx, |s| s.last_error = Some(record))
}

/// Record "now" as the last refresh **fmonero** issued (v5.2, §5).
///
/// Its only caller is `wallet_session::refresh`; `status` reads it and never writes it (§2e).
/// The qualifier matters: `monero-wallet-rpc`'s own 20 s auto-refresh (§2d) is invisible to
/// fmonero, so a field that counted it would be a claim about the daemon we cannot make (§2j).
/// It matters most in remote mode, where `blocks_behind` is `null` and this is the only
/// freshness signal left (§2m.3).
pub fn note_refresh(ctx: &Context) -> Result<(), Error> {
    let now = crate::now_unix();
    with_state(ctx, |s| s.last_refresh = Some(now))
}

/// Arm a §2d breaker. **Never called by `status`**, and never by `send::relay` (§2d).
pub fn note_stall(ctx: &Context, who: Daemon) -> Result<(), Error> {
    let now = crate::now_unix();
    with_state(ctx, |s| {
        s.stalls.insert(
            who.key().to_string(),
            Stall {
                at: now,
                backoff_until: now + STALL_BACKOFF_SEC,
            },
        );
    })
}

/// Seconds left on an armed breaker, or `None`.
///
/// Read-only: it neither arms nor disarms, so `status` can report a breaker without taking
/// the write lock (§2e), and an expired one simply stops being reported.
pub fn stall_remaining(ctx: &Context, who: Daemon) -> Result<Option<u64>, Error> {
    let doc = read_doc(&ctx.paths.state_file());
    let Some(stall) = doc.stalls.get(who.key()) else {
        return Ok(None);
    };
    let left = stall.backoff_until - crate::now_unix();
    Ok((left > 0).then_some(left as u64))
}

/// Store one `get_net_stats` sample and return `(rx_bps, tx_bps)` against the previous one,
/// or `None` when there is no previous sample (§5).
pub fn net_sample(ctx: &Context, s: NetStats) -> Result<Option<(u64, u64)>, Error> {
    let now = crate::now_unix();
    let sample = NetSample {
        at: now,
        bytes_in: s.total_bytes_in,
        bytes_out: s.total_bytes_out,
    };
    with_state(ctx, move |doc| {
        let rates = doc.net_sample.and_then(|prev| {
            let dt = now - prev.at;
            // A counter that went backwards means the daemon restarted: two samples of two
            // different lifetimes are not a rate, and a wrapped subtraction would render a
            // gigabyte per second. `None` is the honest answer (§5).
            if dt <= 0 || sample.bytes_in < prev.bytes_in || sample.bytes_out < prev.bytes_out {
                return None;
            }
            let dt = dt as u64;
            Some((
                (sample.bytes_in - prev.bytes_in) / dt,
                (sample.bytes_out - prev.bytes_out) / dt,
            ))
        });
        doc.net_sample = Some(sample);
        rates
    })
}

// ---- pending.json: its own file, three writers, `status` reads it only (§2a.4) ----

pub fn pending_send(ctx: &Context) -> Result<Option<PendingSend>, Error> {
    let path = ctx.paths.pending_file();
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
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    // A record fmonero cannot parse must **block**, not vanish: it is the interlock that
    // stops a double send, and "unparsable" is not "absent" (§2a.4).
    serde_json::from_slice(&bytes).map(Some).map_err(|e| {
        Error::safety(format!(
            "{} exists but cannot be read ({e}), so a send may be in flight",
            path.display()
        ))
        .with_reason("pending_send")
        .with_hint("fmonero wallet history --limit 5   # then: fmonero wallet clear-pending --yes")
    })
}

/// Writes the WHOLE record to `paths.pending_file()` and **fsyncs**. Called exactly once
/// per send, by `send::relay`, before the first `relay_tx` (§2a.3, §2a.4).
pub fn set_pending_send(ctx: &Context, p: &PendingSend) -> Result<(), Error> {
    ctx.paths.ensure_dirs()?;
    let mut bytes = serde_json::to_vec_pretty(p)
        .map_err(|e| Error::failure(format!("cannot serialise the pending record: {e}")))?;
    bytes.push(b'\n');
    crate::write_private(&ctx.paths.pending_file(), &bytes)?;
    // `write_private` fsyncs the file; the rename itself is only durable once the directory
    // entry is, and the whole point of this record is surviving a power cut mid-relay.
    fsync_dir(&ctx.paths.state_dir);
    Ok(())
}

/// Flips one entry of the list to `relayed: true` and re-fsyncs before the next `relay_tx`.
pub fn mark_relayed(ctx: &Context, txid: &str) -> Result<(), Error> {
    let Some(mut record) = pending_send(ctx)? else {
        return Err(Error::failure(format!(
            "no pending record to mark {txid} relayed in"
        )));
    };
    let now = crate::now_unix();
    let mut found = false;
    for tx in &mut record.txs {
        if tx.txid == txid {
            tx.relayed = true;
            tx.relayed_at = Some(now);
            found = true;
        }
    }
    if !found {
        return Err(Error::failure(format!(
            "{txid} is not in the pending record"
        )));
    }
    set_pending_send(ctx, &record)
}

/// Called from `send::build`'s resolve step and from `wallet clear-pending` — and from
/// nowhere else (§2a.3).
pub fn clear_pending_send(ctx: &Context) -> Result<(), Error> {
    let path = ctx.paths.pending_file();
    match std::fs::remove_file(&path) {
        Ok(()) => {
            fsync_dir(&ctx.paths.state_dir);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::failure(format!(
            "cannot remove {}: {e}",
            path.display()
        ))),
    }
}

// ---- the plumbing ------------------------------------------------------------

/// How long [`with_state`] waits for the `flock` before skipping the write (§5).
const LOCK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// A state document that cannot be parsed is **not** an error: `state.json` is a cache plus
/// three breakers, the doctor's `state_valid` check exists to report it, and refusing to run
/// `status` because a cache file is corrupt would make one unreadable file break the whole
/// tool (§7.1).
fn read_doc(path: &std::path::Path) -> StateDoc {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// `fsync` a directory, so a `rename(2)` into it survives a power cut. Best effort: a
/// read-only state dir degrades to a stale cache, never to a failure (§5).
fn fsync_dir(dir: &std::path::Path) {
    if let Ok(f) = std::fs::File::open(dir) {
        let _ = f.sync_all();
    }
}

/// `flock(LOCK_EX)` on `state_dir/.lock`, released on drop.
///
/// `rename(2)` alone makes a **reader** safe; it does not make two read-modify-write cycles
/// safe, and `status` runs from the plugin every 3 s and from the TUI every second (§5). This
/// is a different lock from §2a.11's `send.lock`, which the money path holds from build to
/// relay, and from `pending.json`, which is not in this file at all.
struct FileLock(std::fs::File);

impl FileLock {
    /// `Ok(None)` when the lock could not be taken within `deadline`: the write is then
    /// skipped rather than blocking (§5).
    fn take(
        path: &std::path::Path,
        deadline: std::time::Duration,
    ) -> Result<Option<FileLock>, Error> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| Error::failure(format!("cannot open {}: {e}", path.display())))?;
        let started = std::time::Instant::now();
        loop {
            // SAFETY: `f` is an open file descriptor for the whole call.
            let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                return Ok(Some(FileLock(f)));
            }
            if started.elapsed() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // SAFETY: the descriptor is still open; it is closed when `self.0` drops after this.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
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
