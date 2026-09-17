//! Typed errors: one variant per exit code, a stable `code_name`, a machine-readable
//! `reason` and a runnable hint (DESIGN.md §4a, §4, §5b).
//!
//! Ported from `fproton/crates/fproton-core/src/error.rs` and adapted to §5b's names.
//! Two things are new here: `reason` — the stable discriminator *within* a code, which
//! §4 makes a mandatory key of every failure envelope — and the two RPC mappers of
//! §1b.4/§1b.2, so that `main.rs` never interprets a daemon's error code itself.

/// Exit-code dictionary (§4a). Printed verbatim by `capabilities` and `robot-docs`.
pub const EXIT_CODES: &[(i32, &str, &str)] = &[
    (0, "ok", "success"),
    (1, "failure", "unexpected failure (bug or I/O); retrying rarely helps"),
    (
        2,
        "usage",
        "bad arguments, unknown settings key, non-loopback URL, bad amount",
    ),
    (
        3,
        "safety",
        "refused without --yes, refused because a pending send exists, refused to start a second node; `reason` says which",
    ),
    (
        4,
        "backend",
        "monerod/monero-wallet-rpc not installed, unit missing, daemon unreachable, auth refused",
    ),
    (
        5,
        "upstream",
        "a daemon errored, or was killed at its deadline (a stall lands here and arms the breaker)",
    ),
    (
        6,
        "not_found",
        "unknown wallet name, address-book label, txid or settings key",
    ),
    (
        7,
        "ambiguous",
        "an address-book label matched several entries",
    ),
    (
        8,
        "timeout",
        "a bounded wait ran out (node up, node up --wait); nothing is broken",
    ),
    (
        9,
        "wallet_locked",
        "no wallet open, wrong password, or a background-sync wallet",
    ),
    (
        10,
        "node_syncing",
        "the node is too far behind for this operation (send/sweep only)",
    ),
    (11, "doctor", "the doctor found failures"),
    (12, "doctor_partial", "a fixer failed, or fixed only part"),
    (
        13,
        "doctor_refused",
        "unsafe precondition or undo integrity failure",
    ),
    (
        14,
        "insufficient_funds",
        "NOT_ENOUGH_MONEY / NOT_ENOUGH_UNLOCKED_MONEY",
    ),
    (
        15,
        "invalid_address",
        "structural failure, validate_address said no, or the wrong nettype",
    ),
    (
        16,
        "tx_rejected",
        "the transaction could not be built or was refused on relay",
    ),
    (130, "interrupted", "SIGINT, or a cancelled TUI action"),
];

/// (exit code, reason) — the §4 table, printed by `capabilities`.
///
/// `reason` is a stable machine-readable discriminator **within** a code, because exit 3
/// alone covers eight unrelated conditions and an agent's reflex on exit 3 (append
/// `--yes` and retry) is right for one of them. `capabilities.reasons` is generated from
/// this table, so the documented set and the constructible set cannot drift.
pub const REASONS: &[(i32, &str)] = &[
    // 1 — the one exit-1 path where retrying is the wrong reflex (§2a.9).
    (1, "txid_mismatch"),
    // 2
    (2, "bad_args"),
    (2, "unknown_key"),
    (2, "bad_amount"),
    (2, "non_loopback_url"),
    (2, "unsupported_flag"),
    // v5.2, §2m: the remote-address grammar and the mode/address invariant.
    (2, "bad_remote_address"),
    (2, "remote_unset"),
    // 3
    (3, "needs_yes"),
    (3, "pending_send"),
    (3, "send_in_progress"),
    (3, "fee_above_max"),
    (3, "foreign_node"),
    (3, "data_dir_in_use"),
    (3, "wallet_exists"),
    (3, "full_chain_refused"),
    // 4
    (4, "not_installed"),
    (4, "unit_missing"),
    (4, "unreachable"),
    (4, "auth_refused"),
    // v5.2, §2m.3: a node command that needs the local monerod while `node.mode` is remote,
    // and a `set_daemon` whose address epee's own parser refused.
    (4, "remote_node"),
    (4, "remote_address_rejected"),
    // 5
    (5, "stalled"),
    (5, "rpc_error"),
    (5, "restricted"),
    (5, "killed_at_deadline"),
    // 9
    (9, "not_open"),
    (9, "bad_password"),
    (9, "background_sync"),
    // 10 — exit 10 carried no reason before v5.2, and the two situations take different
    // actions: the first waits for OUR node, the second for a refresh against someone else's
    // (§2m.3).
    (10, "node_not_synchronized"),
    (10, "wallet_catching_up"),
    // 16
    (16, "too_large"),
    (16, "not_possible"),
    (16, "relay_refused"),
];

/// A failure with an exit code (§4a), a stable name, a `reason` and a runnable hint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// 1 — unexpected (bug, I/O).
    #[error("{msg}")]
    Failure {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 2 — bad arguments, unknown settings key, non-loopback URL, bad amount.
    #[error("{msg}")]
    Usage {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 3 — refused without `--yes`, or refused for one of the other §4 reasons.
    #[error("{msg}")]
    Safety {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 4 — a daemon is not installed, its unit is missing, it is unreachable or it refused auth.
    #[error("{msg}")]
    Backend {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 5 — a daemon errored, or was killed at its deadline.
    #[error("{msg}")]
    Upstream {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 6 — unknown wallet, label, txid or settings key.
    #[error("{msg}")]
    NotFound {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 7 — an address-book label matched several entries.
    #[error("{msg}")]
    Ambiguous {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 8 — a bounded wait ran out; nothing is broken.
    #[error("{msg}")]
    Timeout {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 9 — no wallet open, a wrong password, or a background-sync wallet.
    #[error("{msg}")]
    WalletLocked {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 10 — the node is too far behind for this operation (`send`/`sweep` only, §4a).
    #[error("{msg}")]
    NodeSyncing {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 11 — the doctor found failures.
    #[error("{msg}")]
    Doctor {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 12 — a fixer failed, or fixed only part.
    #[error("{msg}")]
    DoctorPartial {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 13 — an unsafe precondition, or an undo integrity failure.
    #[error("{msg}")]
    DoctorRefused {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 14 — `NOT_ENOUGH_MONEY` / `NOT_ENOUGH_UNLOCKED_MONEY` (§1b.4).
    #[error("{msg}")]
    InsufficientFunds {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 15 — structural failure, `validate_address` said no, or the wrong nettype.
    #[error("{msg}")]
    InvalidAddress {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 16 — the transaction could not be built or was refused on relay.
    #[error("{msg}")]
    TxRejected {
        msg: String,
        hint: Option<String>,
        reason: Option<&'static str>,
    },
    /// 130 — SIGINT, or a cancelled action (`CancelToken`).
    ///
    /// A unit variant, because §2d and §7a both write `Err(Error::Interrupted)` and
    /// nothing has a message, hint or reason to carry for it.
    #[error("interrupted")]
    Interrupted,
}

macro_rules! per_variant {
    ($self:expr, $field:ident) => {
        match $self {
            Error::Failure { $field, .. }
            | Error::Usage { $field, .. }
            | Error::Safety { $field, .. }
            | Error::Backend { $field, .. }
            | Error::Upstream { $field, .. }
            | Error::NotFound { $field, .. }
            | Error::Ambiguous { $field, .. }
            | Error::Timeout { $field, .. }
            | Error::WalletLocked { $field, .. }
            | Error::NodeSyncing { $field, .. }
            | Error::Doctor { $field, .. }
            | Error::DoctorPartial { $field, .. }
            | Error::DoctorRefused { $field, .. }
            | Error::InsufficientFunds { $field, .. }
            | Error::InvalidAddress { $field, .. }
            | Error::TxRejected { $field, .. } => Some($field),
            Error::Interrupted => None,
        }
    };
}

macro_rules! constructors {
    ($($name:ident => $variant:ident),* $(,)?) => {
        $(
            #[doc = concat!("Build an `Error::", stringify!($variant), "` with no hint and no reason.")]
            pub fn $name(msg: impl Into<String>) -> Error {
                Error::$variant { msg: msg.into(), hint: None, reason: None }
            }
        )*
    };
}

impl Error {
    constructors! {
        failure => Failure,
        usage => Usage,
        safety => Safety,
        backend => Backend,
        upstream => Upstream,
        not_found => NotFound,
        ambiguous => Ambiguous,
        timeout => Timeout,
        wallet_locked => WalletLocked,
        node_syncing => NodeSyncing,
        doctor => Doctor,
        doctor_partial => DoctorPartial,
        doctor_refused => DoctorRefused,
        insufficient_funds => InsufficientFunds,
        invalid_address => InvalidAddress,
        tx_rejected => TxRejected,
    }

    /// The (exit code, reason) table, reachable as `Error::REASONS` as well as
    /// `error::REASONS` — §5b names it both ways.
    pub const REASONS: &'static [(i32, &'static str)] = crate::error::REASONS;

    /// A function that the frozen API declares but this build does not implement (§2h).
    /// Never presented as success: the caller sees exit 1 and a message naming the gap.
    pub fn unimplemented(what: &str) -> Error {
        Error::failure(format!("{what} is not implemented in this build"))
            .with_hint("fmonero capabilities   # what this build actually does")
    }

    /// Attach a runnable hint, overriding the default for this class.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Error {
        if let Some(slot) = per_variant!(&mut self, hint) {
            *slot = Some(hint.into());
        }
        self
    }

    /// Attach the §4 machine-readable discriminator.
    #[must_use]
    pub fn with_reason(mut self, reason: &'static str) -> Error {
        if let Some(slot) = per_variant!(&mut self, reason) {
            *slot = Some(reason);
        }
        self
    }

    /// Re-point a `sandboxed_units` refusal's hint at the command the **user** typed (v5.3,
    /// §4.2).
    ///
    /// `units::install`'s sandbox refusal is raised in core, which cannot know whether the
    /// caller was `setup --units`, `node remote <addr> --yes` or `node local --yes`, so it
    /// ships a hint naming `fmonero setup --units --dry-run`. That is runnable and safe and is
    /// still somebody else's command; §4's rule is that a hint is the user's own. Every CLI
    /// entry point that can reach `install` calls this with its own invocation plus
    /// `--dry-run`, and every other error passes through untouched — the reason is the guard,
    /// so this can never rewrite a hint it was not meant for.
    #[must_use]
    pub fn retarget_sandbox_hint(self, own_dry_run: impl Into<String>) -> Error {
        if self.reason() == Some("sandboxed_units") {
            return self.with_hint(own_dry_run);
        }
        self
    }

    /// The process exit code for this error (§4a).
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Failure { .. } => 1,
            Error::Usage { .. } => 2,
            Error::Safety { .. } => 3,
            Error::Backend { .. } => 4,
            Error::Upstream { .. } => 5,
            Error::NotFound { .. } => 6,
            Error::Ambiguous { .. } => 7,
            Error::Timeout { .. } => 8,
            Error::WalletLocked { .. } => 9,
            Error::NodeSyncing { .. } => 10,
            Error::Doctor { .. } => 11,
            Error::DoctorPartial { .. } => 12,
            Error::DoctorRefused { .. } => 13,
            Error::InsufficientFunds { .. } => 14,
            Error::InvalidAddress { .. } => 15,
            Error::TxRejected { .. } => 16,
            Error::Interrupted => 130,
        }
    }

    /// The stable machine name for this error class (§4a).
    pub fn code_name(&self) -> &'static str {
        let code = self.exit_code();
        EXIT_CODES
            .iter()
            .find(|(c, _, _)| *c == code)
            .map(|(_, n, _)| *n)
            .unwrap_or("failure")
    }

    /// The §4 discriminator within this code, or `None` for codes that have none.
    pub fn reason(&self) -> Option<&'static str> {
        per_variant!(self, reason).copied().flatten()
    }

    /// A runnable next step. Always non-empty: the caller's own hint when one was
    /// attached, otherwise the default for this class.
    pub fn hint(&self) -> String {
        per_variant!(self, hint)
            .cloned()
            .flatten()
            .unwrap_or_else(|| self.default_hint().to_string())
    }

    fn default_hint(&self) -> &'static str {
        match self {
            Error::Failure { .. } => "fmonero doctor",
            Error::Usage { .. } => "fmonero --help",
            Error::Safety { .. } => "fmonero status --json   # then re-run with --yes",
            Error::Backend { .. } => "omarchy pkg add monero",
            Error::Upstream { .. } => "fmonero doctor",
            Error::NotFound { .. } => "fmonero wallet list",
            Error::Ambiguous { .. } => "fmonero wallet book list",
            Error::Timeout { .. } => "fmonero status --json",
            Error::WalletLocked { .. } => "fmonero wallet open <name>",
            Error::NodeSyncing { .. } => "fmonero status --json",
            Error::Doctor { .. } => "fmonero doctor --fix",
            Error::DoctorPartial { .. } => "fmonero doctor",
            Error::DoctorRefused { .. } => "fmonero doctor --json",
            Error::InsufficientFunds { .. } => "fmonero wallet balance",
            Error::InvalidAddress { .. } => "fmonero wallet validate <address>",
            Error::TxRejected { .. } => "fmonero node info",
            Error::Interrupted => "fmonero status --json",
        }
    }

    /// True for a cancelled action (`CancelToken`) or SIGINT.
    pub fn is_interrupted(&self) -> bool {
        matches!(self, Error::Interrupted)
    }

    /// The `--json` failure envelope (§4): the same keys for every command, `reason`
    /// included (`null` when this code has none).
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "ok": false,
            "schema": crate::SCHEMA_VERSION,
            "code": self.exit_code(),
            "code_name": self.code_name(),
            "error": self.to_string(),
            "hint": self.hint(),
            "reason": self.reason(),
        })
    }

    /// Map a wallet-RPC error code to the right variant, message verbatim (§1b.4).
    ///
    /// `-8` with "Transaction not found." is **not** mapped here: it is the normal
    /// "not in this wallet yet" answer and `WalletClient::get_transfer_by_txid` turns it
    /// into `Ok(None)` (§5b) before this function is ever reached.
    pub fn from_wallet_rpc(code: i32, message: &str) -> Error {
        let msg = message.to_string();
        match code {
            -2 => Error::invalid_address(msg).with_hint("fmonero wallet validate <address>"),
            -4 => Error::tx_rejected(msg).with_reason("relay_refused"),
            -5 => Error::usage(msg)
                .with_hint("fmonero wallet send <integrated-address> <amount>   # a payment id travels inside an integrated address"),
            -8 => Error::usage(msg).with_hint("fmonero wallet history --limit 20"),
            -12 | -14 | -15 | -20 | -46 => {
                Error::usage(msg).with_hint("fmonero wallet balance --subaddresses")
            }
            -13 => Error::wallet_locked(msg).with_reason("not_open"),
            -16 | -19 => Error::tx_rejected(msg).with_reason("not_possible"),
            -17 | -37 => Error::insufficient_funds(msg),
            -18 => Error::tx_rejected(msg).with_reason("too_large").with_hint(
                "fmonero wallet sweep <same address> --below <amount> --dry-run   # v0.1 builds one transaction; this send needs more",
            ),
            -21 => Error::safety(msg).with_reason("wallet_exists"),
            -22 => Error::wallet_locked(msg).with_reason("bad_password"),
            -23 => Error::backend(msg).with_hint("fmonero setup --units --yes"),
            -26 | -27 | -50 => Error::failure(msg),
            -29 => Error::safety(msg),
            -38 => Error::backend(msg)
                .with_reason("unreachable")
                .with_hint("fmonero status --json"),
            -7 | -48 => Error::backend(msg),
            -51 | -52 => Error::wallet_locked(msg).with_reason("background_sync"),
            // epee's catch-all: `MAP_JON_RPC_WE_IF` serialises an uncaught exception as
            // `{"error":{"code":0,"message":""}}`, and `on_get_transfers` /
            // `on_get_transfer_by_txid` have no try/catch of their own, so this is the
            // ordinary answer when wallet-rpc cannot reach the daemon (§1b.4). A verbatim
            // empty message would be an error report that reports nothing (§2j).
            0 => Error::upstream(
                "monero-wallet-rpc failed without a message (usually: it cannot reach the daemon)",
            )
            .with_reason("rpc_error")
            .with_hint("fmonero status --json"),
            // -1 from `open_wallet` means a bad password or unreadable keys, because
            // `on_open_wallet` overwrites whatever code was mapped (§1b.4). The caller
            // pre-checks that `<wallet.dir>/<NAME>.keys` exists (exit 6), which is what
            // makes that reading unambiguous.
            -1 => Error::wallet_locked(msg).with_reason("bad_password"),
            _ => Error::upstream(msg).with_reason("rpc_error"),
        }
    }

    /// Map a node JSON-RPC error code (§1b.2). `-32601` is the restricted-RPC answer:
    /// the method is not registered at all, so the data has no source on this node.
    pub fn from_node_rpc(code: i32, message: &str) -> Error {
        if code == -32601 {
            return Error::upstream(format!(
                "this monerod is running with restricted RPC, so the method is not available ({message})"
            ))
            .with_reason("restricted")
            .with_hint("fmonero doctor --only node");
        }
        Error::upstream(message.to_string()).with_reason("rpc_error")
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::failure(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::failure(format!("JSON: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_per_code(code: i32) -> Error {
        match code {
            1 => Error::failure("x"),
            2 => Error::usage("x"),
            3 => Error::safety("x"),
            4 => Error::backend("x"),
            5 => Error::upstream("x"),
            6 => Error::not_found("x"),
            7 => Error::ambiguous("x"),
            8 => Error::timeout("x"),
            9 => Error::wallet_locked("x"),
            10 => Error::node_syncing("x"),
            11 => Error::doctor("x"),
            12 => Error::doctor_partial("x"),
            13 => Error::doctor_refused("x"),
            14 => Error::insufficient_funds("x"),
            15 => Error::invalid_address("x"),
            16 => Error::tx_rejected("x"),
            130 => Error::Interrupted,
            other => panic!("undocumented code {other}"),
        }
    }

    #[test]
    fn codes_and_names_match_the_dictionary() {
        assert_eq!(EXIT_CODES.len(), 18, "§4a has 18 rows");
        for (code, name, _) in EXIT_CODES {
            if *code == 0 {
                continue;
            }
            let e = one_per_code(*code);
            assert_eq!(e.exit_code(), *code);
            assert_eq!(e.code_name(), *name);
            assert!(!e.hint().is_empty(), "every error needs a hint");
        }
    }

    #[test]
    fn every_documented_reason_is_constructible_on_its_own_code() {
        for (code, reason) in REASONS {
            let e = one_per_code(*code).with_reason(reason);
            assert_eq!(e.reason(), Some(*reason));
            assert_eq!(e.exit_code(), *code);
            assert_eq!(e.json()["reason"], *reason);
        }
    }

    #[test]
    fn a_custom_hint_and_reason_survive_into_the_envelope() {
        let e = Error::usage("unknown setting 'nope'")
            .with_reason("unknown_key")
            .with_hint("fmonero settings list");
        let v = e.json();
        assert_eq!(v["ok"], false);
        assert_eq!(v["code"], 2);
        assert_eq!(v["code_name"], "usage");
        assert_eq!(v["reason"], "unknown_key");
        assert_eq!(v["hint"], "fmonero settings list");
        assert_eq!(v["error"], "unknown setting 'nope'");
        assert_eq!(v["schema"], crate::SCHEMA_VERSION);
    }

    #[test]
    fn interrupted_carries_no_reason_and_exits_130() {
        let e = Error::Interrupted;
        assert_eq!(e.exit_code(), 130);
        assert_eq!(e.reason(), None);
        assert!(e.is_interrupted());
        assert_eq!(e.json()["reason"], serde_json::Value::Null);
    }

    #[test]
    fn the_wallet_rpc_map_is_the_one_in_the_design() {
        for (code, exit) in [
            (-2, 15),
            (-4, 16),
            (-5, 2),
            (-12, 2),
            (-13, 9),
            (-16, 16),
            (-17, 14),
            (-18, 16),
            (-19, 16),
            (-21, 3),
            (-22, 9),
            (-23, 4),
            (-26, 1),
            (-29, 3),
            (-37, 14),
            (-38, 4),
            (-7, 4),
            (-48, 4),
            (-50, 1),
            (-51, 9),
            (-52, 9),
            (-1, 9),
            (-999, 5),
        ] {
            let e = Error::from_wallet_rpc(code, "upstream text");
            assert_eq!(e.exit_code(), exit, "wallet-rpc {code}");
        }
        // The message is verbatim except for epee's empty catch-all (§1b.4).
        assert_eq!(
            Error::from_wallet_rpc(-17, "not enough money").to_string(),
            "not enough money"
        );
        let catch_all = Error::from_wallet_rpc(0, "");
        assert_eq!(catch_all.exit_code(), 5);
        assert_eq!(catch_all.reason(), Some("rpc_error"));
        assert!(catch_all.to_string().contains("cannot reach the daemon"));
        assert_eq!(
            Error::from_wallet_rpc(-18, "Transaction would be too large.").reason(),
            Some("too_large")
        );
    }

    #[test]
    fn a_gated_node_method_is_the_restricted_reason() {
        let e = Error::from_node_rpc(-32601, "Method not found");
        assert_eq!(e.exit_code(), 5);
        assert_eq!(e.reason(), Some("restricted"));
        assert!(e.to_string().contains("restricted RPC"));
        assert_eq!(
            Error::from_node_rpc(-32000, "boom").reason(),
            Some("rpc_error")
        );
    }

    #[test]
    fn unimplemented_is_exit_one_and_names_the_gap() {
        let e = Error::unimplemented("status");
        assert_eq!(e.exit_code(), 1);
        assert!(e.to_string().contains("status"));
    }
}
