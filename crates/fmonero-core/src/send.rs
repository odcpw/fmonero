//! The **only** send orchestrator (DESIGN.md §2a, §5b).
//!
//! Types frozen at the Phase 1a gate; bodies are Phase 1b's (§8).
//!
//! [`build`] and [`relay`] are the only two functions in the codebase that may call
//! `WalletClient::build_transfer`, `build_sweep_all` or `relay_tx`, and `relay` is the only
//! writer of the `pending.json` record that creates it or marks an entry. The CLI
//! (`wallet send`, `wallet sweep`) and the TUI (§7a) are two independently written callers:
//! every §2a guarantee lives here so that one unit test proves it for both.

use serde::{Deserialize, Serialize};

use crate::amount::Piconero;
use crate::context::Context;
use crate::error::Error;
use crate::proc::CancelToken;
use crate::rpc::{NodeClient, WalletClient};
use crate::state::{self, PendingKind, PendingSend, PendingTx};

/// What the CLI and the TUI hand to [`build`].
///
/// No `unlock_time`, no `payment_id`, no `ring_size`, no `outputs`, no
/// `subtract_fee_from_outputs` — by §2a.8 those fields do not exist here, and the
/// serializer emits none of them. `on_transfer` rejects any non-zero `unlock_time` with
/// `-50` before validating anything else, a standalone payment id with `-5`, and
/// `subtract_fee_from_outputs` would make the number fmonero printed different from the
/// number that arrives.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendRequest {
    /// A Monero address, or `@label` — resolved by [`build`] at step 5a of §2a.1's refusal
    /// order, never by `main.rs` (§4).
    pub dest: String,
    pub amount: Piconero,
    pub account_index: u32,
    /// 0–4. `0` is an **alias of the lowest tier**, not a "wallet default" (§1b.4).
    pub priority: u32,
    pub note: Option<String>,
    /// §2a.2: refuse the relay when the BUILT total fee exceeds this. The one fee bound that
    /// survives the build-discard-rebuild flow, and it is enforced in [`relay`], never by a
    /// caller.
    pub max_fee: Option<Piconero>,
}

/// What `wallet sweep` hands to [`build_sweep`].
///
/// **No subaddress selector, deliberately**: the wire request always carries
/// `subaddr_indices_all: true` (§2a.10) and there is no field that could disable it. With
/// it absent, `create_transactions_all` picks **one subaddress at random**
/// (`wallet2.cpp:11846-11853`) — a sweep that moves one subaddress's funds, reports success
/// and leaves the rest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SweepRequest {
    pub dest: String,
    pub account_index: u32,
    pub priority: u32,
    pub below: Option<Piconero>,
    pub max_fee: Option<Piconero>,
}

/// One `destinations[]` entry of a `transfer` request (`:596-627`).
#[derive(Clone, Debug, Serialize)]
struct WireDest {
    amount: Piconero,
    address: String,
}

/// The wire shape of `transfer` / `sweep_all`, built **only** inside this module from the
/// two request types above (§2a.8, §2a.10).
///
/// Every field is private and there is no public constructor, which is what makes "no
/// `unlock_time`, no `payment_id`, no `ring_size`, no `outputs`, no
/// `subtract_fee_from_outputs`" a property of the **type** rather than a rule every caller
/// has to remember — and what makes `subaddr_indices_all: true` impossible to switch off
/// (§2a.10).
#[derive(Clone, Debug, Serialize)]
pub struct TransferRequest {
    /// `transfer` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    destinations: Option<Vec<WireDest>>,
    /// `sweep_all` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
    account_index: u32,
    priority: u32,
    /// `transfer`: the transaction secret key is never wanted (§2a.2, §2b.2).
    #[serde(skip_serializing_if = "Option::is_none")]
    get_tx_key: Option<bool>,
    /// `sweep_all` spells it plural (`:880`).
    #[serde(skip_serializing_if = "Option::is_none")]
    get_tx_keys: Option<bool>,
    /// **Always true** here: `build` never relays (§2a.2).
    do_not_relay: bool,
    /// The wire blob is not what `relay_tx` wants, so it is never asked for (§1b.4).
    get_tx_hex: bool,
    /// **Always true**: the metadata IS the thing `relay_tx` takes (§1b.4).
    get_tx_metadata: bool,
    /// `sweep_all` only, and **always `Some(true)`** (§2a.10): with it absent the server
    /// hands an empty subaddress set to `create_transactions_all`, which then picks **one
    /// subaddress at random** (`wallet2.cpp:11846-11853`).
    #[serde(skip_serializing_if = "Option::is_none")]
    subaddr_indices_all: Option<bool>,
    /// `sweep_all`'s optional `--below`.
    #[serde(skip_serializing_if = "Option::is_none")]
    below_amount: Option<Piconero>,
}

impl TransferRequest {
    /// The `transfer` body for one recipient (§1b.4, §2a.8).
    fn for_send(req: &SendRequest, dest: &str) -> TransferRequest {
        TransferRequest {
            destinations: Some(vec![WireDest {
                amount: req.amount,
                address: dest.to_string(),
            }]),
            address: None,
            account_index: req.account_index,
            priority: req.priority,
            get_tx_key: Some(false),
            get_tx_keys: None,
            do_not_relay: true,
            get_tx_hex: false,
            get_tx_metadata: true,
            subaddr_indices_all: None,
            below_amount: None,
        }
    }

    /// The `sweep_all` body (§2a.10). `subaddr_indices_all` is set here, unconditionally, and
    /// `SweepRequest` carries no field that could unset it.
    fn for_sweep(req: &SweepRequest, dest: &str) -> TransferRequest {
        TransferRequest {
            destinations: None,
            address: Some(dest.to_string()),
            account_index: req.account_index,
            priority: req.priority,
            get_tx_key: None,
            get_tx_keys: Some(false),
            do_not_relay: true,
            get_tx_hex: false,
            get_tx_metadata: true,
            subaddr_indices_all: Some(true),
            below_amount: req.below,
        }
    }
}

/// A transaction that exists but has **not** been broadcast.
///
/// `metadata` is the hex `relay_tx` wants — a serialized `wallet2::pending_tx`, **not**
/// `tx_blob` (§1b.4). It is never serialized, never logged and never written to disk
/// (§2a.4): it carries the transaction secret key.
pub struct BuiltTx {
    pub txid: String,
    pub amount: Piconero,
    pub fee: Piconero,
    pub weight: u64,
    pub metadata: String,
}

impl std::fmt::Debug for BuiltTx {
    /// Hand-written, as §5b requires: the metadata prints as `<redacted>`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltTx")
            .field("txid", &self.txid)
            .field("amount", &self.amount)
            .field("fee", &self.fee)
            .field("weight", &self.weight)
            .field("metadata", &"<redacted>")
            .finish()
    }
}

/// A built, unbroadcast plan. Holds [`BuiltTx`], so it is never serialized.
pub struct Plan {
    pub txs: Vec<BuiltTx>,
    pub total_amount: Piconero,
    pub total_fee: Piconero,
    pub total_weight: u64,
    pub dest: String,
    pub account_index: u32,
    pub priority: u32,
    pub is_sweep: bool,
    /// Carried from the request so that [`relay`] — not each caller — applies it (§5b).
    pub note: Option<String>,
    pub max_fee: Option<Piconero>,
    /// The §2a.11 `send.lock` file descriptor. Private and never serialized: the lock lives
    /// exactly as long as the plan does, so a discarded plan, a `--dry-run` and a crash all
    /// release it with nothing to clean up.
    _lock: std::fs::File,
}

impl Plan {
    /// amount + fee, checked (§2a.5).
    pub fn total(&self) -> Result<Piconero, Error> {
        self.total_amount.checked_add(self.total_fee)
    }
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("txs", &self.txs)
            .field("total_amount", &self.total_amount)
            .field("total_fee", &self.total_fee)
            .field("total_weight", &self.total_weight)
            .field("dest", &self.dest)
            .field("account_index", &self.account_index)
            .field("priority", &self.priority)
            .field("is_sweep", &self.is_sweep)
            .field("max_fee", &self.max_fee)
            .finish_non_exhaustive()
    }
}

/// Runs §2a.1's refusal order — which is the order the exit code depends on — and then
/// builds **once** with `do_not_relay: true, get_tx_metadata: true`. Never relays.
///
/// 1. amount rules (§2a.5) → exit 2;
/// 2. `address::parse` → exit 15, **skipped when `dest` begins with `@`**;
/// 3. the §2a.11 `send.lock` → exit 3 `send_in_progress`;
/// 4. the §2a.4 `pending_send` entry check and resolve step → exit 3 `pending_send`;
/// 5. the wallet-daemon reachability probe → exit 4;
/// 5. (a) `@label` resolution through one `get_address_book` (exit 6 on no match, 7 on
///    several), after which the resolved **address** — never the label — is what
///    `Plan.dest`, every line `relay` announces and the record carry;
/// 6. `validate_address` and the nettype check (§2a.6) → exit 15;
/// 7. the `get_info.synchronized` gate → exit 10;
/// 8. the build;
/// 9. `--max-fee` → exit 3 `fee_above_max`, **before** the caller's `--yes` check.
///
/// The ONLY state it writes is the §2a.4 resolve step's clear (`state::clear_pending_send`):
/// it never creates, extends or marks an entry. `n` is `None` only when the caller has
/// already proved the node is unreachable, in which case this returns `Error::Backend`
/// before building.
pub fn build(
    ctx: &Context,
    w: &WalletClient,
    n: Option<&NodeClient>,
    req: &SendRequest,
    cancel: &CancelToken,
) -> Result<Plan, Error> {
    // 1. amount rules (§2a.5). `Piconero::parse` has already run in the caller; what is left
    //    is the one value it cannot reject, because it is a perfectly good number.
    if req.amount.0 == 0 {
        return Err(Error::usage("0 is not an amount to send")
            .with_reason("bad_amount")
            .with_hint("fmonero wallet sweep <address> --dry-run   # to move everything"));
    }
    let lock = preflight(ctx, w, &req.dest, cancel)?;
    // 5a. `@label` resolution, then the address the label resolved to is parsed (§4).
    let dest = resolve_dest(w, &req.dest)?;
    // 6. + 7.
    check_recipient(ctx, w, &dest)?;
    require_synchronized(n)?;
    // 8. The build, once, with do_not_relay (§2a.2, §2a.3).
    let tx = w.build_transfer(&TransferRequest::for_send(req, &dest), cancel)?;
    let plan = Plan {
        total_amount: tx.amount,
        total_fee: tx.fee,
        total_weight: tx.weight,
        txs: vec![tx],
        dest,
        account_index: req.account_index,
        priority: req.priority,
        is_sweep: false,
        note: req.note.clone(),
        max_fee: req.max_fee,
        _lock: lock,
    };
    // 9. `--max-fee`, raised HERE — before the caller's `--yes` check — so a no-`--yes` run
    //    with `--max-fee` says so rather than printing a fee it would have refused (§2a.2).
    check_max_fee(&plan)?;
    Ok(plan)
}

/// Sweep's counterpart; the resulting [`Plan`] may hold several transactions. Its wire
/// request always carries `subaddr_indices_all: true` (§2a.10). One caller only:
/// `wallet sweep` — the TUI has no sweep path in v0.1 (§0, §7a).
pub fn build_sweep(
    ctx: &Context,
    w: &WalletClient,
    n: Option<&NodeClient>,
    req: &SweepRequest,
    cancel: &CancelToken,
) -> Result<Plan, Error> {
    let lock = preflight(ctx, w, &req.dest, cancel)?;
    let dest = resolve_dest(w, &req.dest)?;
    check_recipient(ctx, w, &dest)?;
    require_synchronized(n)?;
    let txs = w.build_sweep_all(&TransferRequest::for_sweep(req, &dest), cancel)?;
    let mut total_amount = Piconero(0);
    let mut total_fee = Piconero(0);
    let mut total_weight = 0u64;
    for tx in &txs {
        total_amount = total_amount.checked_add(tx.amount)?;
        total_fee = total_fee.checked_add(tx.fee)?;
        total_weight = total_weight.saturating_add(tx.weight);
    }
    let plan = Plan {
        txs,
        total_amount,
        total_fee,
        total_weight,
        dest,
        account_index: req.account_index,
        priority: req.priority,
        is_sweep: true,
        note: None,
        max_fee: req.max_fee,
        _lock: lock,
    };
    check_max_fee(&plan)?;
    Ok(plan)
}

/// Relays a plan, in this order and no other (§2a.2, §2a.4, §2a.9):
///
/// re-check `max_fee` (the enforcing gate) → `announce` the built recipient, amount, fee,
/// total, priority and account **before the first `relay_tx`**, through the caller's sink
/// because core stays silent (§2g) → write and fsync the **whole** `pending.json` record →
/// `relay_tx` each transaction in order, comparing every returned `tx_hash` with the built
/// txid and marking that entry relayed with an fsync before the next call → one wallet
/// `store` → the §2a.4 resolve step → when `plan.note` is `Some`, one `set_tx_notes` over
/// every relayed txid, **whose failure is a warning and never a failure of the send**.
///
/// Returns the txids in relay order. The [`Plan`] is consumed, so its `send.lock` is
/// released here.
pub fn relay(
    ctx: &Context,
    w: &WalletClient,
    plan: Plan,
    announce: &dyn Fn(&str),
    cancel: &CancelToken,
) -> Result<Vec<String>, Error> {
    // The enforcing gate: `build` raised it first, so a caller that built a plan by other
    // means still cannot relay past it (§2a.2).
    check_max_fee(&plan)?;
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    // The number that is paid is always displayed, immediately before the first relay_tx
    // (§2a.2). Core stays silent (§2g): this goes through the caller's sink.
    announce(&announce_line(&plan));

    let total = plan.txs.len();
    let record = PendingSend {
        kind: if plan.is_sweep {
            PendingKind::Sweep
        } else {
            PendingKind::Send
        },
        account_index: plan.account_index,
        dest: plan.dest.clone(),
        built_at: crate::now_unix(),
        txs: plan
            .txs
            .iter()
            .map(|t| PendingTx {
                txid: t.txid.clone(),
                amount: t.amount,
                fee: t.fee,
                relayed: false,
                relayed_at: None,
            })
            .collect(),
    };
    // The WHOLE record, fsynced, BEFORE the first relay_tx (§2a.4). Every txid is already
    // final: the transactions are fully built, so their hashes are known.
    state::set_pending_send(ctx, &record)?;

    let mut relayed: Vec<String> = Vec::new();
    for (i, tx) in plan.txs.iter().enumerate() {
        match w.relay_tx(&tx.metadata) {
            Ok(hash) if hash == tx.txid => {
                relayed.push(hash.clone());
                state::mark_relayed(ctx, &hash)?;
            }
            Ok(hash) => {
                // §2a.9: the entry is NOT marked relayed, and this is the one exit-1 path
                // where retrying is the wrong reflex.
                return Err(Error::failure(format!(
                    "relay_tx returned {hash} for the transaction fmonero built as {}",
                    tx.txid
                ))
                .with_reason("txid_mismatch")
                .with_hint(
                    "fmonero wallet history --limit 5   # two transaction ids differ; check which one landed",
                ));
            }
            Err(e) => {
                // §2a.4's partial sweep: the metadata of the remaining entries is dropped
                // (it was never written to disk), the record stays, and the message names
                // what did go out.
                let ids = relayed.join(", ");
                let detail = if relayed.is_empty() {
                    format!("0 of {total} broadcast")
                } else {
                    format!("{i} of {total} broadcast ({ids})")
                };
                return Err(Error::tx_rejected(format!("{e}; {detail}"))
                    .with_reason("relay_refused")
                    .with_hint(format!(
                        "fmonero wallet history --limit {total}   # then: fmonero wallet clear-pending --yes"
                    )));
            }
        }
    }

    // One `store` after the last relay (§2a.4): wallet-rpc writes the wallet file only on a
    // graceful stop or on this call, so without it a SIGKILL loses the wallet's own record of
    // the outgoing transfer and the resolve step then blocks every later send.
    if let Err(e) = w.store() {
        announce(&format!(
            "warning: the transactions were broadcast but `store` failed ({e}); the wallet will \
             re-derive them on its next refresh"
        ));
        let _ = state::set_last_error(ctx, &e, None);
    }

    // The resolve step, so a normal successful send leaves nothing behind (§2a.4).
    let _ = resolve_record(ctx, w);

    // The note travels on the request and is applied HERE, not by a caller (§5b). Its
    // failure is a warning and never a failure of the send: the money has moved.
    if let Some(note) = plan.note.as_ref().filter(|n| !n.is_empty()) {
        let notes: Vec<String> = relayed.iter().map(|_| note.clone()).collect();
        if let Err(e) = w.set_tx_notes(&relayed, &notes) {
            announce(&format!(
                "warning: the send succeeded but the note could not be stored ({e})"
            ));
            let _ = state::set_last_error(ctx, &e, None);
        }
    }
    Ok(relayed)
}

// ---- the refusal order, in one place (§2a.1) ----------------------------------

/// Steps 2–5 of §2a.1's refusal order: `address::parse`, the `send.lock`, the `pending_send`
/// entry check and the wallet-daemon probe, in that order and no other — the order is what
/// decides the exit code, and it lives here rather than in `build` and `build_sweep`
/// separately so the two cannot drift.
fn preflight(
    ctx: &Context,
    w: &WalletClient,
    dest: &str,
    cancel: &CancelToken,
) -> Result<std::fs::File, Error> {
    // 2. Structural address parse — skipped for a `@label`, which step 5a resolves and parses.
    if !dest.starts_with('@') {
        crate::address::parse(dest)?;
    }
    // 3. The §2a.11 lock, BEFORE the entry check, so that the check, the build, the record
    //    write, every relay_tx, the store and the resolve step are one critical section.
    let lock = take_send_lock(ctx)?;
    // 4. The §2a.4 entry check and resolve step.
    if let Some(record) = state::pending_send(ctx)? {
        let unrelayed = record.txs.iter().filter(|t| !t.relayed).count();
        if unrelayed > 0 {
            let first = record
                .txs
                .iter()
                .find(|t| !t.relayed)
                .map(|t| t.txid.clone())
                .unwrap_or_default();
            return Err(Error::safety(format!(
                "a previous {} may still be in flight: {unrelayed} of {} transaction(s) were built but not confirmed relayed",
                record.kind.as_str(),
                record.txs.len()
            ))
            .with_reason("pending_send")
            .with_hint(format!(
                "fmonero wallet history --limit 5   # confirm whether {first} landed, then: fmonero wallet clear-pending --yes"
            )));
        }
        // Every entry relayed: resolve. A positive "not found" is the only answer allowed to
        // block; every other failure leaves the record and does NOT block (§2a.4).
        if let Some(missing) = resolve_record(ctx, w)? {
            return Err(Error::safety(format!(
                "the wallet has no record of {missing}, which fmonero relayed, so a send may still be in flight"
            ))
            .with_reason("pending_send")
            .with_hint(format!(
                "fmonero wallet history --limit 5   # confirm whether {missing} landed, then: fmonero wallet clear-pending --yes"
            )));
        }
    }
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    // 5. The wallet-daemon reachability probe. `-13 NOT_OPEN` arrives as exit 9, which is the
    //    right answer: reachable, but nothing to spend from.
    w.get_height()?;
    Ok(lock)
}

/// The §2a.4 resolve step. `Ok(None)` = nothing left to resolve (the record is gone, or every
/// entry was found and it has been cleared). `Ok(Some(txid))` = that entry was answered
/// **not found**, which is the only answer allowed to block.
///
/// Any other failure of a resolve call — the wallet unreachable, `-38`, epee's `code: 0` —
/// leaves the record and returns `Ok(None)`: none of them is evidence of a second spend, and
/// blocking on them would arm the interlock exactly when the node hiccups after a send.
fn resolve_record(ctx: &Context, w: &WalletClient) -> Result<Option<String>, Error> {
    let Some(record) = state::pending_send(ctx)? else {
        return Ok(None);
    };
    if record.txs.iter().any(|t| !t.relayed) {
        return Ok(None);
    }
    for tx in &record.txs {
        match w.get_transfer_by_txid(&tx.txid, record.account_index) {
            // An answer of ANY type — in, out, pending, pool or failed — counts as found.
            Ok(Some(_)) => {}
            Ok(None) => return Ok(Some(tx.txid.clone())),
            Err(_) => return Ok(None),
        }
    }
    state::clear_pending_send(ctx)?;
    Ok(None)
}

/// Step 5a (§2a.1, §4): one `get_address_book` on the open wallet, matched
/// case-insensitively against `description`. The resolved **address** — never the label — is
/// what everything downstream carries.
fn resolve_dest(w: &WalletClient, dest: &str) -> Result<String, Error> {
    let Some(label) = dest.strip_prefix('@') else {
        return Ok(dest.to_string());
    };
    let wanted = label.to_lowercase();
    let matches: Vec<crate::rpc::wallet::BookEntry> = w
        .get_address_book()?
        .into_iter()
        .filter(|e| e.description.to_lowercase() == wanted)
        .collect();
    match matches.len() {
        0 => Err(
            Error::not_found(format!("no address-book entry is described '{label}'"))
                .with_hint("fmonero wallet book list"),
        ),
        1 => {
            let address = matches[0].address.clone();
            // The resolved address goes through the structural parse step 2 skipped.
            crate::address::parse(&address)?;
            Ok(address)
        }
        n => Err(Error::ambiguous(format!(
            "'{label}' matches {n} address-book entries: {}",
            matches
                .iter()
                .map(|e| format!("{} ({})", e.index, short(&e.address)))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .with_hint("fmonero wallet book list")),
    }
}

/// Step 6 (§2a.6): `validate_address { any_net_type: true, allow_openalias: false }`, then the
/// nettype check against the **wallet's own** nettype — never against `settings.node.network`,
/// which can disagree with the running daemon after a hand-edited config.
fn check_recipient(ctx: &Context, w: &WalletClient, dest: &str) -> Result<(), Error> {
    let check = w.validate_address(dest)?;
    if !check.valid {
        return Err(Error::invalid_address(format!(
            "monero-wallet-rpc says {} is not a valid address",
            short(dest)
        ))
        .with_hint(format!("fmonero wallet validate {dest}")));
    }
    // The wallet's own nettype, recorded by `wallet_session::open` from its primary address,
    // so this costs no second RPC on the normal path (§2a.6).
    let wallet_net = match state::last_open(ctx)? {
        Some((_, net)) => net,
        None => crate::address::parse(&w.get_address(ctx.settings.wallet.account)?.primary)?.net,
    };
    if !check.nettype.is_empty() && check.nettype != wallet_net.as_str() {
        return Err(Error::invalid_address(format!(
            "{} is a {} address; this wallet is {}",
            short(dest),
            check.nettype,
            wallet_net.as_str()
        ))
        .with_hint(format!("fmonero wallet validate {dest}")));
    }
    Ok(())
}

/// Step 7 (§2a.1): a transaction built against a chain tip we do not have may be rejected.
fn require_synchronized(n: Option<&NodeClient>) -> Result<(), Error> {
    let Some(n) = n else {
        return Err(Error::backend(
            "the node is not reachable, so a transaction cannot be built safely",
        )
        .with_reason("unreachable")
        .with_hint("fmonero node up"));
    };
    let info = n.get_info()?;
    if info.synchronized {
        return Ok(());
    }
    let pct = if info.target_height > info.height {
        info.height * 100 / info.target_height.max(1)
    } else {
        100
    };
    Err(Error::node_syncing(format!(
        "the node is at {}% and not synchronized, so a transaction built now may be rejected",
        pct
    ))
    .with_hint("fmonero status --json"))
}

fn check_max_fee(plan: &Plan) -> Result<(), Error> {
    let Some(max) = plan.max_fee else {
        return Ok(());
    };
    if plan.total_fee <= max {
        return Ok(());
    }
    Err(Error::safety(format!(
        "the built fee is {} XMR, above the --max-fee of {} XMR",
        plan.total_fee.to_xmr_string(),
        max.to_xmr_string()
    ))
    .with_reason("fee_above_max")
    .with_hint(format!(
        "fmonero wallet send <to> <amount> --max-fee {}   # or drop --max-fee",
        plan.total_fee.to_xmr_string()
    )))
}

/// The one line `relay` announces before the first `relay_tx` (§2a.2), so the number that is
/// paid is always displayed even though the fee may differ from a previous invocation's.
fn announce_line(plan: &Plan) -> String {
    let total = plan
        .total()
        .map(|t| t.to_xmr_string())
        .unwrap_or_else(|_| "?".to_string());
    format!(
        "{} {} XMR to {} · fee {} XMR · total {} XMR · priority {} · account {}{}",
        if plan.is_sweep { "sweeping" } else { "sending" },
        plan.total_amount.to_xmr_string(),
        plan.dest,
        plan.total_fee.to_xmr_string(),
        total,
        plan.priority,
        plan.account_index,
        if plan.txs.len() > 1 {
            format!(" · {} transactions", plan.txs.len())
        } else {
            String::new()
        }
    )
}

/// §2a.11: an exclusive `flock(LOCK_EX|LOCK_NB)` on `state_dir/send.lock`, owned by the
/// [`Plan`] and released only when it drops — after `relay`, after a `--dry-run` or a
/// no-`--yes` discard, or when the process dies, which is what makes it crash-safe with no
/// marker to clean up.
fn take_send_lock(ctx: &Context) -> Result<std::fs::File, Error> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    ctx.paths.ensure_dirs()?;
    let path = ctx.paths.send_lock();
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| Error::failure(format!("cannot open {}: {e}", path.display())))?;
    // SAFETY: `f` is an open descriptor for the whole call.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(f);
    }
    // `flock` does not name the holder and the lock file stays empty, so the message carries
    // no pid; the holder is identified through `status`'s `pending` record (§2a.11).
    Err(
        Error::safety("another fmonero send is building or relaying")
            .with_reason("send_in_progress")
            .with_hint("fmonero status --json   # wait for pending to clear, then retry"),
    )
}

/// Enough of an address to recognise, for a message that must not wrap a terminal.
fn short(address: &str) -> String {
    if address.len() <= 20 {
        return address.to_string();
    }
    format!("{}…{}", &address[..10], &address[address.len() - 6..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5b and §2b.2: the metadata carries the transaction secret key, so it must not reach
    /// a panic message, a debug dump or a future log line.
    #[test]
    fn a_built_transaction_never_debug_prints_its_metadata() {
        let tx = BuiltTx {
            txid: "c".repeat(64),
            amount: Piconero(1),
            fee: Piconero(2),
            weight: 3,
            metadata: "deadbeefnotarealblob".to_string(),
        };
        let shown = format!("{tx:?}");
        assert!(shown.contains(&"c".repeat(64)));
        assert!(!shown.contains("deadbeefnotarealblob"), "{shown}");
        assert!(shown.contains("<redacted>"));
    }

    /// §2a.8 as a property of the type: there is no field to put an `unlock_time` or a
    /// `payment_id` in, on either request.
    #[test]
    fn the_request_types_have_no_field_the_design_forbids() {
        let req = SendRequest {
            dest: "4".repeat(95),
            amount: Piconero(10_000_000_000),
            account_index: 0,
            priority: 2,
            note: None,
            max_fee: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 6, "{keys:?}");
        for forbidden in [
            "unlock_time",
            "payment_id",
            "ring_size",
            "outputs",
            "subtract_fee_from_outputs",
        ] {
            assert!(v.get(forbidden).is_none(), "{forbidden} must not exist");
        }
        let sweep = SweepRequest {
            dest: "4".repeat(95),
            account_index: 0,
            priority: 2,
            below: None,
            max_fee: None,
        };
        let v = serde_json::to_value(&sweep).unwrap();
        // No subaddress selector: `subaddr_indices_all: true` cannot be switched off (§2a.10).
        for forbidden in ["subaddr_indices", "subaddr_indices_all", "unlock_time"] {
            assert!(v.get(forbidden).is_none(), "{forbidden} must not exist");
        }
    }

    /// §2a.10: **the sweep constructor sets `subaddr_indices_all: true` unconditionally**, and
    /// `SweepRequest` carries no field that could unset it.
    ///
    /// With it absent (it defaults to `false`) and `subaddr_indices` empty, `on_sweep_all`
    /// hands an EMPTY set to `create_transactions_all`, which then picks **one subaddress at
    /// random** (`wallet2.cpp:11846-11853`) — a sweep that moves one subaddress's funds,
    /// reports success and leaves the rest, invisible to any fixture-driven mock.
    #[test]
    fn the_sweep_constructor_always_sends_subaddr_indices_all() {
        for below in [None, Some(Piconero(500_000_000_000))] {
            for (account, priority) in [(0u32, 0u32), (7, 4)] {
                let req = SweepRequest {
                    dest: "8".repeat(95),
                    account_index: account,
                    priority,
                    below,
                    max_fee: None,
                };
                let wire = serde_json::to_value(TransferRequest::for_sweep(&req, &req.dest))
                    .expect("the wire request serialises");
                assert_eq!(
                    wire["subaddr_indices_all"], true,
                    "unconditionally, for every argument combination: {wire}"
                );
                assert_eq!(wire["do_not_relay"], true, "§2a.2");
                assert_eq!(wire["get_tx_metadata"], true, "§1b.4");
                assert_eq!(wire["address"], req.dest);
                assert_eq!(wire["account_index"], account);
                assert_eq!(wire["priority"], priority);
                // Nothing §2a.8 forbids, and no `destinations` (that is `transfer`'s key).
                for forbidden in [
                    "unlock_time",
                    "payment_id",
                    "ring_size",
                    "outputs",
                    "subtract_fee_from_outputs",
                    "subaddr_indices",
                    "destinations",
                ] {
                    assert!(wire.get(forbidden).is_none(), "{forbidden}: {wire}");
                }
                match below {
                    Some(b) => assert_eq!(wire["below_amount"], b.0),
                    None => assert!(wire.get("below_amount").is_none(), "{wire}"),
                }
            }
        }
        // And `transfer`'s own body is the other shape: one destination, no sweep keys.
        let req = SendRequest {
            dest: "4".repeat(95),
            amount: Piconero(10_000_000_000),
            account_index: 0,
            priority: 2,
            note: Some("a note travels on the request, not on the wire".into()),
            max_fee: Some(Piconero(1)),
        };
        let wire = serde_json::to_value(TransferRequest::for_send(&req, &req.dest)).unwrap();
        assert_eq!(wire["destinations"][0]["amount"], 10_000_000_000u64);
        assert_eq!(wire["get_tx_key"], false);
        for forbidden in [
            "address",
            "subaddr_indices_all",
            "below_amount",
            "note",
            "max_fee",
            "unlock_time",
            "payment_id",
        ] {
            assert!(wire.get(forbidden).is_none(), "{forbidden}: {wire}");
        }
    }
}
