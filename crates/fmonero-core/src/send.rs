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

/// The wire shape of `transfer` / `sweep_all`, built **only** inside this module from the
/// two request types above (§2a.8, §2a.10).
#[derive(Clone, Debug, Serialize)]
pub struct TransferRequest {
    /* Private: Phase 1b gives it the exact key set of §1b.4, and nothing else can
    construct one, which is what makes "no unlock_time, no payment_id" a property of the
    type rather than of every caller. */
    #[serde(skip)]
    _private: (),
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
    _ctx: &Context,
    _w: &WalletClient,
    _n: Option<&NodeClient>,
    _req: &SendRequest,
    _cancel: &CancelToken,
) -> Result<Plan, Error> {
    Err(Error::unimplemented("send::build"))
}

/// Sweep's counterpart; the resulting [`Plan`] may hold several transactions. Its wire
/// request always carries `subaddr_indices_all: true` (§2a.10). One caller only:
/// `wallet sweep` — the TUI has no sweep path in v0.1 (§0, §7a).
pub fn build_sweep(
    _ctx: &Context,
    _w: &WalletClient,
    _n: Option<&NodeClient>,
    _req: &SweepRequest,
    _cancel: &CancelToken,
) -> Result<Plan, Error> {
    Err(Error::unimplemented("send::build_sweep"))
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
    _ctx: &Context,
    _w: &WalletClient,
    _plan: Plan,
    _announce: &dyn Fn(&str),
    _cancel: &CancelToken,
) -> Result<Vec<String>, Error> {
    Err(Error::unimplemented("send::relay"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5b and §2b.2: the metadata carries the transaction secret key, so it must not reach
    /// a panic message, a `dbg!` or a future log line.
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
}
