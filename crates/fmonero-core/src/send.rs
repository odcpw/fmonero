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
/// 7. the daemon-currency gate, `require_node_current`, which switches on the `NodeGate` the
///    caller passed: locally one `get_info.synchronized` → exit 10 `node_not_synchronized`,
///    remotely one bounded `refresh` that must succeed → exit 4 `unreachable` or exit 10
///    `wallet_catching_up` (§2m.3);
/// 8. the build.
///
/// Step 9, `--max-fee` → exit 3 `fee_above_max`, is [`check_max_fee`], which the caller runs on
/// the returned plan immediately and **before** its `--yes` check: §5c requires that refusal to
/// carry the built transaction's key set, and an `Err` from here would destroy the `Plan` it has
/// to be rendered from. [`relay`] re-checks it as the enforcing gate.
///
/// The ONLY state it writes is the §2a.4 resolve step's clear (`state::clear_pending_send`):
/// it never creates, extends or marks an entry. `gate` names the situation at step 7 — see
/// [`NodeGate`], whose three variants replaced an `Option<&NodeClient>` in v5.2 because `None`
/// already meant the opposite of what remote mode needs (§2m.3).
pub fn build(
    ctx: &Context,
    w: &WalletClient,
    gate: NodeGate<'_>,
    req: &SendRequest,
    cancel: &CancelToken,
) -> Result<Plan, Error> {
    // 1. amount rules (§2a.5). `Piconero::parse` has already run in the caller; what is left
    //    is the one value it cannot reject, because it is a perfectly good number.
    if req.amount.0 == 0 {
        return Err(Error::usage("0 XMR is not an amount.")
            .with_reason("bad_amount")
            .with_hint("fmonero wallet sweep <address> --dry-run   # to move everything"));
    }
    let lock = preflight(ctx, w, &req.dest, cancel)?;
    // 5a. `@label` resolution, then the address the label resolved to is parsed (§4).
    let dest = resolve_dest(w, &req.dest)?;
    // 6. + 7.
    check_recipient(ctx, w, &dest)?;
    require_node_current(ctx, w, gate, cancel)?;
    // 8. The build, once, with do_not_relay (§2a.2, §2a.3).
    let tx = w.build_transfer(&TransferRequest::for_send(req, &dest), cancel)?;
    // **8a. The amount that was built must be the amount that was asked for** (round 4, S2).
    //
    // Every screen downstream of here renders `plan.total_amount` — the wallet service's own
    // reply — and nothing compared it with `req.amount`, so the confirm a person types `send`
    // into could show a different figure from the form he typed it on, in one frame, with
    // nothing flagging the difference. A single-destination `transfer` returns the sum of its
    // destinations (`wallet_rpc_server.cpp`'s `single_transfer_response.amount`), so for this
    // request the two are equal or something is wrong with the answer — and §2a's rule is that
    // fmonero refuses rather than sends what it cannot explain. Exit 3, before `--yes` is even
    // consulted, and the message names both figures because which one is wrong is not ours to
    // guess.
    if tx.amount != req.amount {
        return Err(Error::safety(format!(
            "You asked to send {} XMR and the wallet service prepared {} XMR. Nothing has been \
             sent.",
            req.amount.to_xmr_string(),
            tx.amount.to_xmr_string()
        ))
        .with_reason("amount_mismatch")
        .with_action("Press Esc, then check the amount and try again.")
        .with_hint("fmonero wallet balance"));
    }
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
    // Step 9, `--max-fee`, is `check_max_fee(&plan)` in the caller — before its `--yes` check,
    // so a no-`--yes` run with `--max-fee` still says so, and with the plan in hand so §5c's
    // refusal can carry the built fee (§2a.2, §5c).
    Ok(plan)
}

/// Sweep's counterpart; the resulting [`Plan`] may hold several transactions. Its wire
/// request always carries `subaddr_indices_all: true` (§2a.10). One caller only:
/// `wallet sweep` — the TUI has no sweep path in v0.1 (§0, §7a).
pub fn build_sweep(
    ctx: &Context,
    w: &WalletClient,
    gate: NodeGate<'_>,
    req: &SweepRequest,
    cancel: &CancelToken,
) -> Result<Plan, Error> {
    let lock = preflight(ctx, w, &req.dest, cancel)?;
    let dest = resolve_dest(w, &req.dest)?;
    check_recipient(ctx, w, &dest)?;
    require_node_current(ctx, w, gate, cancel)?;
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
    // Step 9 is `check_max_fee(&plan)` in the caller, as in `build` above (§2a.2, §5c).
    Ok(plan)
}

/// Relays a plan, in this order and no other (§2a.2, §2a.4, §2a.9):
///
/// stop consulting the §2d breaker (§2d) → re-check `max_fee` (the enforcing gate) →
/// `announce` the built recipient, amount, fee,
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
    // §2d, in one line and for every caller: "`send::relay` likewise never consults a
    // breaker: once a plan is built the §2a.11 `send.lock` is held and the transaction exists,
    // so exiting 5 there would abandon a built transaction rather than protect anything."
    // Leaving this to the transport's `any_success` flag made it true only for a caller that
    // relays on the client that built — the CLI does, the TUI does not.
    w.disable_breaker();
    // The enforcing gate: the caller checked it immediately after the build, so a caller that
    // built a plan by other means still cannot relay past it (§2a.2).
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
                    "The node reported a different payment id ({hash}) than the one fmonero \
                     prepared ({}).",
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
                // **X2: this string is pasted into the error's own SENTENCE** by
                // `relay_failure`, so `broadcast` (rule 7) reached a person on the money path.
                // The ids move to `detail`, where `--json` and `-v` keep them whole (W8).
                let ids = relayed.join(", ");
                let detail = if relayed.is_empty() {
                    format!("none of the {total} payments has gone out")
                } else {
                    format!("{i} of {total} payments have gone out")
                };
                return Err(relay_failure(e, &detail, total).with_detail(ids));
            }
        }
    }

    // One `store` after the last relay (§2a.4): wallet-rpc writes the wallet file only on a
    // graceful stop or on this call, so without it a SIGKILL loses the wallet's own record of
    // the outgoing transfer and the resolve step then blocks every later send.
    if let Err(e) = w.store() {
        // S6's grep convention: what is interpolated into a sentence a person reads is already a
        // sentence, and the binding says so. `{e}` is reserved for the technical text, which
        // `with_detail` is the only home for.
        let why = e.to_string();
        announce(&format!(
            // X2: rule 7 (`broadcast`) and an implementation name as the subject.
            "The payments have gone out, but this wallet could not write them down. {why} It \
             will find them again on its next refresh."
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
            let why = e.to_string();
            announce(&format!(
                "The payment has gone out. Its note could not be saved. {why}"
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
    refuse_if_send_in_flight(ctx)?;
    if state::pending_send(ctx)?.is_some() {
        // Every entry relayed: resolve. A positive "not found" is the only answer allowed to
        // block; every other failure leaves the record and does NOT block (§2a.4).
        if let Some(missing) = resolve_record(ctx, w)? {
            return Err(Error::safety(format!(
                "This wallet has no record of {missing}, which fmonero sent. That payment may \
                 still be on its way."
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
            Error::not_found(format!("No address book entry is named \"{label}\"."))
                .with_hint("fmonero wallet book list"),
        ),
        1 => {
            let address = matches[0].address.clone();
            // The resolved address goes through the structural parse step 2 skipped.
            crate::address::parse(&address)?;
            Ok(address)
        }
        n => Err(Error::ambiguous(format!(
            "\"{label}\" matches {n} entries in your address book. They are: {}.",
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
        return Err(
            Error::invalid_address(format!("{} is not a valid address.", short(dest)))
                .with_hint(format!("fmonero wallet validate {dest}")),
        );
    }
    // The wallet's own nettype, recorded by `wallet_session::open` from its primary address,
    // so this costs no second RPC on the normal path (§2a.6).
    let wallet_net = match state::last_open(ctx)? {
        Some((_, net)) => net,
        None => crate::address::parse(&w.get_address(ctx.settings.wallet.account)?.primary)?.net,
    };
    if !check.nettype.is_empty() && check.nettype != wallet_net.as_str() {
        return Err(Error::invalid_address(format!(
            "{} is a {} address. This wallet is {}.",
            short(dest),
            check.nettype,
            wallet_net.as_str()
        ))
        .with_hint(format!("fmonero wallet validate {dest}")));
    }
    Ok(())
}

/// Step 7 of §2a.1's refusal order, **as data** (v5.2, §2m.3).
///
/// The argument was `Option<&NodeClient>` until v5.2, and it could not stay one: `None` already
/// means "the caller proved there is no node ⇒ refuse", and remote mode means the opposite —
/// proceed, by a different gate. Two opposite behaviours behind one `None` is the arrangement
/// §2a.3 rejects, on the one code path where getting it wrong either refuses every remote send
/// or accepts every local send against a node that is down. Three variants make the caller name
/// the situation and the compiler enumerate the cases.
pub enum NodeGate<'a> {
    /// Local mode, node answering: one `get_info`; not `synchronized` ⇒ exit 10
    /// `reason: node_not_synchronized`.
    Local(&'a NodeClient),
    /// Local mode, node proved unreachable by the caller ⇒ exit 4 `reason: unreachable`.
    Unreachable,
    /// Remote mode: one bounded `refresh` through `wallet_session::refresh` under
    /// `timeouts.refresh_sec`, which must succeed. It carries no client because fmonero
    /// constructs none in remote mode (§2c).
    Remote,
}

/// The one implementation of step 7, switching on the gate. Called by [`build`] and
/// [`build_sweep`] and by nothing else.
fn require_node_current(
    ctx: &Context,
    w: &WalletClient,
    gate: NodeGate<'_>,
    cancel: &CancelToken,
) -> Result<(), Error> {
    let n = match gate {
        NodeGate::Local(n) => n,
        NodeGate::Unreachable => {
            return Err(Error::backend(
                "Your node is not answering, so fmonero will not prepare a payment.",
            )
            .with_reason("unreachable")
            .with_hint("fmonero node up"))
        }
        // **One bounded `refresh { start_height: 0 }`, which must succeed** (§2m.3). What it
        // buys: a `refresh` that returns proves the remote daemon answered **and** that the
        // wallet holds every block that daemon offered (§1b.9 item 6) — strictly more than the
        // local gate's `get_info.synchronized`, which is the node's opinion of itself.
        //
        // What it does not buy, stated because a send is the one place a half-truth costs
        // money: a remote node's own sync state cannot be observed through `monero-wallet-rpc`
        // at all (§1b.9 item 5), so a node that is itself hours behind — or lying — answers the
        // refresh and the gate passes. The transaction is then built against a stale tip and is
        // **rejected on relay**: `commit_tx` throws and becomes `-4 GENERIC_TRANSFER_ERROR`
        // (exit 16), with the funds unaffected, because outputs are marked spent only after a
        // successful relay (§2a.11). A failed command, never a loss.
        //
        // It goes through `wallet_session::refresh` so that `state::note_refresh` — the one
        // writer of §5's `wallet.last_refresh_at` — has one home (§5b).
        NodeGate::Remote => {
            return match crate::session::wallet_session::refresh(ctx, w, 0, cancel) {
                Ok(_) => Ok(()),
                // Killed at `timeouts.refresh_sec`: the instruction is identical to local
                // mode's ("wait, then retry"), so it is exit 10 and not exit 5 — and **no §2d
                // breaker is armed**, because `blocks_behind`, the breaker's only
                // discriminator, is `null` in remote mode (§2d, §2m.3).
                Err(e) if e.reason() == Some("killed_at_deadline") => {
                    Err(Error::node_syncing(format!(
                        "The wallet did not finish checking against {} within {}s. A payment \
                         prepared now could be refused.",
                        ctx.settings.node.remote,
                        ctx.timeouts.refresh.as_secs()
                    ))
                    .with_reason("wallet_catching_up")
                    .with_hint("fmonero wallet refresh"))
                }
                Err(e) => Err(e),
            };
        }
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
        "Your node is {}% synced. A payment prepared now could be refused.",
        pct
    ))
    .with_reason("node_not_synchronized")
    .with_hint("fmonero status --json"))
}

/// What a failed `relay_tx` means, and only what it means (§2a.9, §2j).
///
/// `relay_tx` returning is the only positive evidence the daemon accepted a transaction, but a
/// *failure* is not evidence of rejection: a call killed at its 30 s deadline, a 401, or a
/// broken socket may all have delivered the transaction first. Flattening every one of those
/// into exit 16 `tx_rejected`/`relay_refused` states that the daemon refused it, which §2j does
/// not allow a check to claim beyond its mechanism — and an agent whose reflex on exit 16 is
/// "build a smaller transaction and retry" would then attempt a second spend.
///
/// So the underlying error's class and `reason` survive — exit 5 `killed_at_deadline`/`stalled`,
/// exit 4 `auth_refused`/`unreachable` — and the message says the outcome is unknown. Exit 16
/// `relay_refused` is reserved for a JSON-RPC answer, which is the daemon actually refusing.
/// The hint is the same in both cases: only `wallet history` can settle it.
fn relay_failure(e: Error, detail: &str, total: usize) -> Error {
    let hint = format!(
        "fmonero wallet history --limit {total}   # then: fmonero wallet clear-pending --yes"
    );
    let unknown = matches!(
        e.reason(),
        Some("killed_at_deadline" | "stalled" | "unreachable" | "auth_refused")
    );
    // S6's convention: the binding holding a sentence a person reads is named for that.
    let why = e.to_string();
    if !unknown {
        return Error::tx_rejected(format!("{why} {detail}"))
            .with_reason("relay_refused")
            .with_hint(hint);
    }
    let text = format!(
        "{why} {detail} The node never answered, so it is not known whether it received the \
         payment."
    );
    let out = if e.exit_code() == 4 {
        Error::backend(text)
    } else {
        Error::upstream(text)
    };
    let out = match e.reason() {
        Some(r) => out.with_reason(r),
        None => out,
    };
    out.with_hint(hint)
}

/// §2a.2's `--max-fee` gate. Public because §5c requires the `fee_above_max` refusal to carry
/// the **built** transaction's key set, and `build`'s frozen signature (§5b) is
/// `Result<Plan, Error>` — an `Err` destroys the `Plan` the payload has to be rendered from.
/// The caller therefore raises it immediately after `build` returns and before its `--yes`
/// check, which is §2a.1's step order; `relay` re-checks it as the enforcing gate, so a caller
/// that skips this still cannot relay past it (§2a.2's own words).
pub fn check_max_fee(plan: &Plan) -> Result<(), Error> {
    let Some(max) = plan.max_fee else {
        return Ok(());
    };
    if plan.total_fee <= max {
        return Ok(());
    }
    Err(Error::safety(format!(
        "The fee came to {} XMR, above the --max-fee of {} XMR.",
        plan.total_fee.to_xmr_string(),
        max.to_xmr_string()
    ))
    .with_reason("fee_above_max")
    .with_hint(format!(
        "fmonero wallet send <to> <amount> --max-fee {}   # or drop --max-fee",
        plan.total_fee.to_xmr_string()
    )))
}

/// §2o.2's four words for a `--priority`, in core so the CLI, the TUI and this line agree (X2).
///
/// `0` is an alias of `1` on the wire (§1b.4), so both read `cheapest`.
pub fn fee_level_words(priority: u32) -> &'static str {
    match priority {
        0 | 1 => "cheapest",
        2 => "normal",
        3 => "fast",
        _ => "fastest",
    }
}

/// The one line `relay` announces before the first `relay_tx` (§2a.2), so the number that is
/// paid is always displayed even though the fee may differ from a previous invocation's.
fn announce_line(plan: &Plan) -> String {
    let total = plan
        .total()
        .map(|t| t.to_xmr_string())
        .unwrap_or_else(|_| "?".to_string());
    // **X2 and X8: `priority` and `transactions` on the line printed immediately before the
    // money moves.** §2o.2 fixes `fee level` and its four words, and `payment`.
    format!(
        "{} {} XMR to {} · fee {} XMR · total {} XMR · {} fee level · account {}{}",
        if plan.is_sweep { "sweeping" } else { "sending" },
        plan.total_amount.to_xmr_string(),
        plan.dest,
        plan.total_fee.to_xmr_string(),
        total,
        fee_level_words(plan.priority),
        plan.account_index,
        if plan.txs.len() > 1 {
            format!(" · {} payments", plan.txs.len())
        } else {
            String::new()
        }
    )
}

/// The **unrelayed** half of §2a.4's entry check: any entry with `relayed == false` means a
/// send may be in flight, so refuse with exit 3 `pending_send`.
///
/// Public because `node_mode::{set_remote, set_local}` need exactly this half and **not** the
/// resolve step that follows it (§2m.5 step 6): resolving asks the wallet daemon, which those
/// two commands are about to restart, and a resolve answered by a *different* node than the one
/// that relayed would say "not found" for a transaction that really was broadcast (§2a.4). One
/// check, one message, three callers — the arrangement §2a.3 asks for.
pub fn refuse_if_send_in_flight(ctx: &Context) -> Result<(), Error> {
    let Some(record) = state::pending_send(ctx)? else {
        return Ok(());
    };
    let unrelayed = record.txs.iter().filter(|t| !t.relayed).count();
    if unrelayed == 0 {
        return Ok(());
    }
    let first = record
        .txs
        .iter()
        .find(|t| !t.relayed)
        .map(|t| t.txid.clone())
        .unwrap_or_default();
    // **X2: `in flight`, `built` and `relayed` in one sentence, about money** — and
    // `record.kind.as_str()` is the internal word (`send` / `sweep`) where §2o.2 wants the verb.
    Err(Error::safety(format!(
        "An earlier {} may still be on its way: {unrelayed} of {} payments were prepared and \
         fmonero never saw them go out.",
        match record.kind {
            PendingKind::Sweep => "sweep",
            PendingKind::Send => "payment",
        },
        record.txs.len()
    ))
    .with_reason("pending_send")
    .with_hint(format!(
        "fmonero wallet history --limit 5   # confirm whether {first} landed, then: fmonero wallet clear-pending --yes"
    )))
}

/// §2a.11: an exclusive `flock(LOCK_EX|LOCK_NB)` on `state_dir/send.lock`, owned by the
/// [`Plan`] and released only when it drops — after `relay`, after a `--dry-run` or a
/// no-`--yes` discard, or when the process dies, which is what makes it crash-safe with no
/// marker to clean up.
///
/// Public because `wallet clear-pending` is §2a.4's third writer of the record and must enter
/// the same critical section: without the lock it can delete the record between two `relay_tx`
/// calls of a sweep, `state::mark_relayed` then fails for a transaction that really went out,
/// and **no** record is left — the lost update §2a.4 put the record in its own file to prevent.
/// The returned file is the lock; dropping it releases it.
pub fn take_send_lock(ctx: &Context) -> Result<std::fs::File, Error> {
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
        .map_err(|e| Error::io("open", "the file that stops two sends at once", &path, &e))?;
    // **`flock` fails for more than one reason, and only one of them means "somebody else has
    // it".** `EINTR` is the one that bites: `flock` is interruptible, and a signal arriving on
    // this thread — `SIGCHLD` from any reaped child, which this process produces on every
    // `proc::run_with_deadline` — makes the call return `-1` with nothing held by anybody.
    // Treating every non-zero return as `send_in_progress` therefore refuses a legitimate send
    // at random, on the money path, with a message naming a process that does not exist.
    //
    // So: retry on `EINTR`, report `EWOULDBLOCK`/`EAGAIN` as the interlock, and give anything
    // else its own exit code — a lock fmonero cannot take for an I/O reason is a failure (exit
    // 1), not a claim that a transaction may be in flight (exit 3), and §4's whole argument for
    // `reason` is that an agent acts on the difference.
    //
    // **`EWOULDBLOCK` does not mean "somebody else has it" either, and that is the second
    // defect.** A `flock` belongs to the *open file description*, not to the process, and
    // `fork(2)` — by **any** thread — copies the descriptor table, so a child holds a
    // reference to the description until it `exec`s. Rust opens every file `O_CLOEXEC`, which
    // bounds that window to fork→exec rather than to the child's whole life; it does not close
    // it. So a `std::process::Command` on one thread (a `systemctl` probe, the mock server a
    // test starts) keeps *this* thread's already-dropped lock alive for a few microseconds,
    // and a bare non-blocking `flock` then reports `send_in_progress` with nobody holding
    // anything: the verifier measured **8 122 spurious `EWOULDBLOCK` in 20 000** on a private
    // lock file with four sibling threads doing nothing but spawning `/bin/true`, and
    // `/proc/locks` is empty by the time anyone looks. The CLI is single-threaded on the money
    // path today; the TUI is not, so a legitimate send can be refused.
    //
    // **The fix is a bounded retry, not a weaker interlock**, and the two timescales are what
    // make that safe: a fork window lasts microseconds, while a genuine holder holds
    // `send.lock` across a build, a human's confirm modal and every `relay_tx` — seconds at
    // best. Retrying for [`LOCK_RETRY_BUDGET`] therefore cannot let two real senders through
    // (the second still meets a lock that will be held long after the budget expires) and
    // costs a real refusal only that budget in latency. The alternatives were considered and
    // rejected: `O_CLOEXEC` is already set and is exactly what leaves the fork→exec window;
    // `fcntl(F_OFD_SETLK)` is an open-file-description lock too and is inherited by `fork` in
    // the same way, so it closes nothing; and process-associated `F_SETLK` would close it only
    // by moving to the lock family §2i's `flock`-vs-`fcntl` choice deliberately keeps clear of,
    // while adding POSIX record locking's "any close drops the lock" footgun on the one path
    // that must not lose a lock silently.
    let started = std::time::Instant::now();
    let mut backoff = std::time::Duration::from_micros(200);
    loop {
        // SAFETY: `f` is an open descriptor for the whole call.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(f);
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(e) if e == libc::EWOULDBLOCK || e == libc::EAGAIN => {
                if started.elapsed() < LOCK_RETRY_BUDGET {
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(std::time::Duration::from_millis(20));
                    continue;
                }
                // `flock` does not name the holder and the lock file stays empty, so the
                // message carries no pid; the holder is identified through `status`'s
                // `pending` record, which is what the hint points at (§2a.11).
                return Err(
                    Error::safety("Another fmonero send is already in progress.")
                        .with_reason("send_in_progress")
                        .with_hint(
                            "fmonero status --json   # wait for pending to clear, then retry",
                        ),
                );
            }
            _ => {
                return Err(
                    Error::io("lock", "the file that stops two sends at once", &path, &err)
                        .with_hint("fmonero doctor"),
                )
            }
        }
    }
}

/// How long [`take_send_lock`] retries a non-blocking `flock` that answered `EWOULDBLOCK`
/// before it declares `send_in_progress` (§2a.11).
///
/// Sized between the two timescales it has to separate: the fork→exec window that produces a
/// spurious refusal is microseconds, and a real holder holds the lock for seconds. 500 ms is
/// three orders of magnitude clear of the first and one clear of the second, and it is the
/// whole extra latency a genuine refusal pays.
pub const LOCK_RETRY_BUDGET: std::time::Duration = std::time::Duration::from_millis(500);

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

    /// §2a.9/§2j: a `relay_tx` that never came back is not a rejection. The only failure
    /// fmonero may report as `tx_rejected`/`relay_refused` is a JSON-RPC answer; a deadline
    /// kill, an armed breaker, a 401 or a broken socket keep their own class and say the
    /// outcome is unknown, because an agent whose reflex on exit 16 is "retry smaller" would
    /// otherwise attempt a second spend of the same outputs.
    #[test]
    fn a_relay_that_never_answered_is_not_reported_as_a_rejection() {
        // The daemon answered and refused: -4 GENERIC_TRANSFER_ERROR (§1b.4).
        let refused = relay_failure(
            Error::from_wallet_rpc(-4, "Transaction was rejected by daemon"),
            "0 of 1 broadcast",
            1,
        );
        assert_eq!(refused.exit_code(), 16);
        assert_eq!(refused.reason(), Some("relay_refused"));
        assert!(refused.hint().contains("clear-pending"));

        // We never heard back. Three shapes, one per §4 reason that means "unknown".
        for (e, code, reason) in [
            (
                Error::upstream("killed").with_reason("killed_at_deadline"),
                5,
                "killed_at_deadline",
            ),
            (
                Error::upstream("stalled").with_reason("stalled"),
                5,
                "stalled",
            ),
            (
                Error::backend("no socket").with_reason("unreachable"),
                4,
                "unreachable",
            ),
            (
                Error::backend("401").with_reason("auth_refused"),
                4,
                "auth_refused",
            ),
        ] {
            let out = relay_failure(e, "0 of 1 broadcast", 1);
            assert_eq!(out.exit_code(), code, "{reason}");
            assert_eq!(out.reason(), Some(reason));
            assert!(
                out.to_string().contains("it is not known whether"),
                "{reason}: the report must not assert a rejection that may not have happened"
            );
            assert!(out.hint().contains("wallet history"), "{reason}");
        }
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

    /// §2a.11, and the third defect the remote-node verification found: `take_send_lock` must
    /// not answer `send_in_progress` because a **sibling thread forked**.
    ///
    /// The shape is the verifier's 20 000-iteration probe, shrunk to fit a test suite: four
    /// threads doing nothing but `std::process::Command::spawn`, while this thread takes the
    /// lock on its own private state directory, drops it and immediately takes it again. No
    /// other process ever touches this file, so **every** refusal here is spurious. On the
    /// bare non-blocking `flock` the verifier measured 8 122 in 20 000 (≈ 40 %); 400
    /// iterations therefore fail it with overwhelming margin, and the bounded retry makes them
    /// all succeed.
    #[test]
    fn a_fork_in_a_sibling_thread_never_refuses_a_send() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = std::env::temp_dir().join(format!("fmonero-send-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = Context::for_test(&dir).expect("test context");

        let stop = Arc::new(AtomicBool::new(false));
        let spawners: Vec<_> = (0..4)
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        if let Ok(mut c) = std::process::Command::new("/bin/true").spawn() {
                            let _ = c.wait();
                        }
                    }
                })
            })
            .collect();

        let mut refused = 0usize;
        let mut other = 0usize;
        for _ in 0..400 {
            match take_send_lock(&ctx) {
                Ok(lock) => drop(lock),
                Err(e) if e.reason() == Some("send_in_progress") => refused += 1,
                Err(_) => other += 1,
            }
        }
        stop.store(true, Ordering::Relaxed);
        for t in spawners {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            (refused, other),
            (0, 0),
            "{refused} spurious send_in_progress (and {other} other errors) on a private lock \
             file no other process ever opened"
        );
    }

    /// The other half of the same change: the retry must not weaken the interlock. Two open
    /// file descriptions in one process conflict exactly as two processes do, so a lock that is
    /// really held is still refused — after the budget, not before it.
    #[test]
    fn a_lock_that_is_really_held_is_still_refused_after_the_budget() {
        let dir = std::env::temp_dir().join(format!("fmonero-send-held-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = Context::for_test(&dir).expect("test context");

        let held = take_send_lock(&ctx).expect("the first lock");
        let started = std::time::Instant::now();
        let err = take_send_lock(&ctx).expect_err("the second must be refused");
        let waited = started.elapsed();
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(err.exit_code(), 3, "{err}");
        assert_eq!(err.reason(), Some("send_in_progress"));
        assert!(
            waited >= LOCK_RETRY_BUDGET,
            "it gave up after {waited:?}, before the {LOCK_RETRY_BUDGET:?} budget"
        );
    }
}
