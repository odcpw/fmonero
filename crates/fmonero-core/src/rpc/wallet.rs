//! `monero-wallet-rpc` JSON-RPC (DESIGN.md §1b.4, §5b).
//!
//! **Signature-only at the Phase 1a gate** (§8), except `WalletClient::new`, which is the
//! §2c loopback gate and needs no daemon. `WALLET_RPC_VERSION` is 1.31
//! (`wallet_rpc_server_commands_defs.h:49-50`).
//!
//! Two methods are callable from **exactly one** place each, and the doc comments say so
//! because §2a.3 rests on it: `build_transfer`/`build_sweep_all` only from `send::build`,
//! and `relay_tx`/`store` only from `send::relay`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::amount::Piconero;
use crate::context::Context;
use crate::error::Error;
use crate::history::Transfer;
use crate::proc::CancelToken;
use crate::rpc::jsonrpc::{self, Deadline, Transport};
use crate::rpc::Login;
use crate::send::{BuiltTx, TransferRequest};
use crate::settings::SslSupport;
use crate::state::Daemon;

/// `get_balance` (`:60`), with the per-subaddress rows of `:77-99`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Balance {
    pub balance: Piconero,
    pub unlocked: Piconero,
    pub blocks_to_unlock: u64,
    pub time_to_unlock_sec: u64,
    /// Only the subaddresses holding unspent, unfrozen outputs — `wallet2::balance_per_subaddress`
    /// builds it from those alone (`wallet2.cpp:7037-7060`), which is why §5's
    /// `subaddress_count` comes from `get_address`, not from here.
    pub per_subaddress: Vec<SubaddressBalance>,
}

/// One row of `get_balance.per_subaddress[]` (`:77-99`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SubaddressBalance {
    pub address_index: u32,
    pub address: String,
    pub label: String,
    pub balance: Piconero,
    pub unlocked: Piconero,
    pub num_unspent_outputs: u64,
    pub blocks_to_unlock: u64,
}

/// `get_address` (`:123`): the account's primary address and **every** generated
/// subaddress of it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AddressSet {
    pub primary: String,
    pub addresses: Vec<SubaddressRow>,
}

/// One row of `get_address.addresses[]` (`:123-160`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SubaddressRow {
    pub address: String,
    pub label: String,
    pub address_index: u32,
    pub used: bool,
}

/// One row of `get_accounts.subaddress_accounts[]` (`:263`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountRow {
    pub account_index: u32,
    pub base_address: String,
    pub balance: Piconero,
    pub unlocked_balance: Piconero,
    pub label: String,
    pub tag: String,
}

/// `create_address` (`:208`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NewAddress {
    pub address: String,
    pub address_index: u32,
    pub addresses: Vec<String>,
    pub address_indices: Vec<u32>,
}

/// `validate_address` (`:2576`), always asked with `any_net_type: true` and
/// `allow_openalias: false` (§2a.6) — so the answer names the address's **real** network
/// and the call needs no open wallet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AddressCheck {
    pub valid: bool,
    pub integrated: bool,
    pub subaddress: bool,
    pub nettype: String,
    pub openalias_address: String,
}

/// `refresh` (`:2054`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Refreshed {
    pub blocks_fetched: u64,
    pub received_money: bool,
}

/// One row of `get_address_book` (`:1981`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BookEntry {
    pub index: u64,
    pub address: String,
    pub description: String,
}

/// What `get_transfers` (`:1601`) is asked for.
///
/// **`filter_by_height` is a field of this frozen type, not a serializer detail.**
/// `on_get_transfers` reads `min_height`/`max_height` only inside
/// `if (req.filter_by_height)` (`wallet_rpc_server.cpp:2890-2895`), and the key is not an
/// `_OPT`, so an omitted flag value-initialises to `false` and the bounds silently become
/// `0 .. CRYPTONOTE_MAX_BLOCK_NUMBER` — the whole history, correct-looking either way
/// (§1b.4). `max_height` is deliberately absent: its `KV_SERIALIZE_OPT` default is already
/// the maximum (`:1626`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TransferQuery {
    pub account_index: u32,
    /// The `in` list, which also carries coinbase rows typed `block` (§1b.4).
    pub r#in: bool,
    pub out: bool,
    pub pending: bool,
    pub failed: bool,
    /// Sent **only** when the caller asked for pool rows: that branch alone runs
    /// `update_pool_state`/`process_pool_state`, a daemon round trip that throws when the
    /// node is unreachable and turns the whole response into epee's `code: 0` (§1b.4).
    pub pool: bool,
    pub filter_by_height: bool,
    pub min_height: u64,
    pub subaddr_indices: Vec<u32>,
}

/// A client for one `monero-wallet-rpc`.
pub struct WalletClient {
    t: Transport,
}

impl WalletClient {
    /// Rejects any non-http scheme and any non-loopback host with `Error::Usage`, through
    /// the same `rpc::require_loopback` `NodeClient::new` calls (§2c). The wallet client is
    /// the one that carries a credential and moves money, so the rule is not relaxed here.
    pub fn new(url: &str, auth: Option<Login>, timeout: Duration) -> Result<WalletClient, Error> {
        crate::rpc::require_loopback(url)?;
        Ok(WalletClient {
            t: Transport::new(url, auth, timeout, Daemon::Wallet),
        })
    }

    /// Reads `user:pass` from the login file (§2c). **An absent file is `Ok` with
    /// `auth: None`, never an error**: steps 1–4 of §2a.1's refusal order must still run in
    /// order, so no constructor may decide a money-path exit code. The absence surfaces at
    /// the first request as exit 4 — `reason: unreachable` for a refused connect,
    /// `auth_refused` for a 401.
    pub fn from_ctx(ctx: &Context) -> Result<WalletClient, Error> {
        Self::build(ctx, true)
    }

    /// The client `status` uses: it never arms a §2d breaker and is never blocked by one
    /// (§2d). Crate-private, so the frozen API is unchanged.
    pub(crate) fn for_status(ctx: &Context) -> Result<WalletClient, Error> {
        Self::build(ctx, false)
    }

    /// §2d's "`send::relay` never consults a breaker", as a property of the relay rather than
    /// of the caller. Called by `send::relay` on whatever client it was handed — the CLI
    /// reuses the one that built the plan, the TUI builds a fresh one for `Action::Relay`, and
    /// only the first of those was exempt by `any_success` alone. Crate-private, so the frozen
    /// API (§5b) is unchanged.
    pub(crate) fn disable_breaker(&self) {
        self.t.disable_breaker();
    }

    fn build(ctx: &Context, breaker: bool) -> Result<WalletClient, Error> {
        let url = &ctx.settings.wallet.url;
        crate::rpc::require_loopback(url)?;
        // An absent login file is `None`, not an error (§2c): the absence must surface at
        // the first request, so that §2a.1's refusal order decides the exit code.
        let auth =
            Login::from_wallet_login_file(&ctx.paths.wallet_login_file(ctx.settings.wallet.port))
                .unwrap_or(None);
        Ok(WalletClient {
            t: Transport::from_ctx(ctx, url, auth, Daemon::Wallet, breaker),
        })
    }

    fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
        d: Deadline,
    ) -> Result<T, Error> {
        let v = self
            .t
            .rpc(method, params, d)
            .map_err(|e| jsonrpc::wallet_error(method, e))?;
        jsonrpc::typed(method, v)
    }

    /// The methods whose answer is `{}` and whose only information is "it did not fail".
    fn ack(&self, method: &str, params: serde_json::Value, d: Deadline) -> Result<(), Error> {
        let _: serde_json::Value = self.call(method, params, d)?;
        Ok(())
    }

    /// As `NodeClient::authenticated`: what the wire did on the last completed request.
    pub fn authenticated(&self) -> Option<bool> {
        self.t.authenticated()
    }

    /// `(major, minor, release)` — `WALLET_RPC_VERSION` is packed `major<<16|minor` (`:2554`).
    pub fn get_version(&self) -> Result<(u16, u16, bool), Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct V {
            version: u32,
            release: bool,
        }
        let v: V = self.call("get_version", serde_json::json!({}), Deadline::Probe)?;
        Ok((
            (v.version >> 16) as u16,
            (v.version & 0xffff) as u16,
            v.release,
        ))
    }

    /// The wallet's own blockchain height (`:455`) — simultaneously the reachability probe
    /// and the open/closed probe (§5). `Error::WalletLocked` on `-13 NOT_OPEN`.
    pub fn get_height(&self) -> Result<u64, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct H {
            height: u64,
        }
        let h: H = self.call("get_height", serde_json::json!({}), Deadline::Probe)?;
        Ok(h.height)
    }

    /// Maps **any** `-1` to `Error::WalletLocked` (exit 9): `on_open_wallet` overwrites the
    /// mapped code, so a wrong password never arrives as `-22` (§1b.4). The caller
    /// pre-checks that the `.keys` file exists (exit 6), which is what makes that mapping
    /// unambiguous. Called only from `wallet_session::open`.
    pub fn open_wallet(&self, name: &str, password: &str) -> Result<(), Error> {
        self.ack(
            "open_wallet",
            serde_json::json!({
                "filename": name, "password": password, "autosave_current": true
            }),
            // Opening a large wallet is slow: this is the one long deadline (§2d).
            Deadline::Open,
        )
    }

    pub fn close_wallet(&self) -> Result<(), Error> {
        self.ack(
            "close_wallet",
            serde_json::json!({"autosave_current": true}),
            Deadline::Open,
        )
    }

    /// `language` is **required** by the daemon and an unknown value comes back as a bare
    /// `-1`, so it is validated locally first (`settings::validate_seed_language`, §1b.4).
    pub fn create_wallet(&self, name: &str, password: &str, language: &str) -> Result<(), Error> {
        self.ack(
            "create_wallet",
            serde_json::json!({"filename": name, "password": password, "language": language}),
            Deadline::Open,
        )
    }

    pub fn get_balance(&self, account: u32) -> Result<Balance, Error> {
        // The wire names differ from fmonero's (`unlocked_balance` vs `unlocked`,
        // `time_to_unlock` vs `time_to_unlock_sec`), so the wire shape is its own type.
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct WireSub {
            address_index: u32,
            address: String,
            label: String,
            balance: Piconero,
            unlocked_balance: Piconero,
            num_unspent_outputs: u64,
            blocks_to_unlock: u64,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wire {
            balance: Piconero,
            unlocked_balance: Piconero,
            blocks_to_unlock: u64,
            time_to_unlock: u64,
            per_subaddress: Vec<WireSub>,
        }
        let w: Wire = self.call(
            "get_balance",
            serde_json::json!({
                "account_index": account, "all_accounts": false, "strict": false
            }),
            Deadline::Probe,
        )?;
        Ok(Balance {
            balance: w.balance,
            unlocked: w.unlocked_balance,
            blocks_to_unlock: w.blocks_to_unlock,
            time_to_unlock_sec: w.time_to_unlock,
            per_subaddress: w
                .per_subaddress
                .into_iter()
                .map(|s| SubaddressBalance {
                    address_index: s.address_index,
                    address: s.address,
                    label: s.label,
                    balance: s.balance,
                    unlocked: s.unlocked_balance,
                    num_unspent_outputs: s.num_unspent_outputs,
                    blocks_to_unlock: s.blocks_to_unlock,
                })
                .collect(),
        })
    }

    pub fn get_accounts(&self) -> Result<Vec<AccountRow>, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wire {
            subaddress_accounts: Vec<AccountRow>,
        }
        let w: Wire = self.call("get_accounts", serde_json::json!({}), Deadline::Probe)?;
        Ok(w.subaddress_accounts)
    }

    pub fn get_address(&self, account: u32) -> Result<AddressSet, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wire {
            address: String,
            addresses: Vec<SubaddressRow>,
        }
        let w: Wire = self.call(
            "get_address",
            serde_json::json!({"account_index": account}),
            Deadline::Probe,
        )?;
        Ok(AddressSet {
            primary: w.address,
            addresses: w.addresses,
        })
    }

    pub fn create_address(&self, account: u32, label: &str) -> Result<NewAddress, Error> {
        self.call(
            "create_address",
            serde_json::json!({"account_index": account, "count": 1, "label": label}),
            Deadline::Probe,
        )
    }

    pub fn label_address(&self, account: u32, index: u32, label: &str) -> Result<(), Error> {
        self.ack(
            "label_address",
            // `index` is the `{major, minor}` object `subaddress_index.h:38-55` serializes
            // (§1b.4) — never a scalar.
            serde_json::json!({"index": {"major": account, "minor": index}, "label": label}),
            Deadline::Probe,
        )
    }

    pub fn get_transfers(&self, q: &TransferQuery) -> Result<Vec<Transfer>, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Lists {
            r#in: Vec<WireTransfer>,
            out: Vec<WireTransfer>,
            pending: Vec<WireTransfer>,
            failed: Vec<WireTransfer>,
            pool: Vec<WireTransfer>,
        }
        let lists: Lists = self.call(
            "get_transfers",
            serde_json::to_value(q).map_err(Error::from)?,
            Deadline::Refresh,
        )?;
        let mut rows = Vec::new();
        for list in [
            lists.r#in,
            lists.out,
            lists.pending,
            lists.failed,
            lists.pool,
        ] {
            rows.extend(list.into_iter().map(WireTransfer::into_transfer));
        }
        Ok(rows)
    }

    /// `Ok(None)` when wallet-rpc answers `-8` with "Transaction not found." — the normal
    /// "not in this wallet yet" answer, and the whole basis of the §2a.4 resolve step.
    /// `Err(Usage)` for the malformed-txid variants of `-8` (§1b.4).
    pub fn get_transfer_by_txid(
        &self,
        txid: &str,
        account: u32,
    ) -> Result<Option<Transfer>, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wire {
            transfer: WireTransfer,
        }
        let params = serde_json::json!({"txid": txid, "account_index": account});
        match self
            .t
            .rpc("get_transfer_by_txid", params, Deadline::Refresh)
        {
            Ok(v) => {
                let w: Wire = jsonrpc::typed("get_transfer_by_txid", v)?;
                Ok(Some(w.transfer.into_transfer()))
            }
            // The one `-8` that is an answer rather than an error (§1b.4).
            Err(crate::rpc::RpcError::Rpc { code: -8, message })
                if message.contains("not found") =>
            {
                Ok(None)
            }
            Err(e) => Err(jsonrpc::wallet_error("get_transfer_by_txid", e)),
        }
    }

    /// ALWAYS called with `do_not_relay: true` and `get_tx_metadata: true` (§2a.2); the
    /// type makes the other combination unrepresentable. **Callable only from
    /// `send::build`** (§2a.3).
    pub fn build_transfer(
        &self,
        req: &TransferRequest,
        cancel: &CancelToken,
    ) -> Result<BuiltTx, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wire {
            tx_hash: String,
            amount: Piconero,
            fee: Piconero,
            weight: u64,
            tx_metadata: String,
        }
        let w: Wire = self.call(
            "transfer",
            serde_json::to_value(req).map_err(Error::from)?,
            Deadline::Transfer,
        )?;
        if w.tx_metadata.is_empty() {
            // Without it there is nothing to relay: `relay_tx` wants a serialized
            // `wallet2::pending_tx`, not `tx_blob` (§1b.4).
            return Err(Error::failure(
                "monero-wallet-rpc built a transaction but returned no tx_metadata, so it cannot be relayed",
            ));
        }
        Ok(BuiltTx {
            txid: w.tx_hash,
            amount: w.amount,
            fee: w.fee,
            weight: w.weight,
            metadata: w.tx_metadata,
        })
    }

    /// `sweep_all` returns a **list** (`split_transfer_response`, §1b.4), and its request
    /// always carries `subaddr_indices_all: true` (§2a.10). **Callable only from
    /// `send::build_sweep`** (§2a.3).
    pub fn build_sweep_all(
        &self,
        req: &TransferRequest,
        cancel: &CancelToken,
    ) -> Result<Vec<BuiltTx>, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wire {
            tx_hash_list: Vec<String>,
            amount_list: Vec<Piconero>,
            fee_list: Vec<Piconero>,
            weight_list: Vec<u64>,
            tx_metadata_list: Vec<String>,
        }
        let w: Wire = self.call(
            "sweep_all",
            serde_json::to_value(req).map_err(Error::from)?,
            Deadline::Transfer,
        )?;
        // Five parallel arrays that must stay in lockstep: a shorter `tx_metadata_list`
        // would silently drop a transaction the record already promises to relay (§2a.4).
        let n = w.tx_hash_list.len();
        if n == 0 {
            return Err(Error::tx_rejected("the sweep built no transaction")
                .with_reason("not_possible")
                .with_hint("fmonero wallet balance"));
        }
        if w.tx_metadata_list.len() != n
            || w.amount_list.len() != n
            || w.fee_list.len() != n
            || w.weight_list.len() != n
        {
            return Err(Error::failure(format!(
                "monero-wallet-rpc answered sweep_all with {n} txids but {} metadata, {} amounts, {} fees and {} weights",
                w.tx_metadata_list.len(),
                w.amount_list.len(),
                w.fee_list.len(),
                w.weight_list.len()
            )));
        }
        Ok((0..n)
            .map(|i| BuiltTx {
                txid: w.tx_hash_list[i].clone(),
                amount: w.amount_list[i],
                fee: w.fee_list[i],
                weight: w.weight_list[i],
                metadata: w.tx_metadata_list[i].clone(),
            })
            .collect())
    }

    /// Returns the server's `tx_hash`, which the caller compares with the built txid
    /// (§2a.9). `metadata_hex` is a serialized `wallet2::pending_tx` — **not** `tx_blob`
    /// (§1b.4). **Callable only from `send::relay`** (§2a.3).
    pub fn relay_tx(&self, metadata_hex: &str) -> Result<String, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wire {
            tx_hash: String,
        }
        let w: Wire = self.call(
            "relay_tx",
            serde_json::json!({"hex": metadata_hex}),
            Deadline::Relay,
        )?;
        Ok(w.tx_hash)
    }

    /// Persists the wallet file. Its ONLY caller is `send::relay`, after the last
    /// successful `relay_tx` (§2a.4): wallet-rpc writes the wallet only on a graceful
    /// `stop()` (`wallet_rpc_server.cpp:294-302`) or on this call (`:2060-2077`) — the
    /// auto-refresh idle handler never stores (`:210-285`).
    pub fn store(&self) -> Result<(), Error> {
        self.ack("store", serde_json::json!({}), Deadline::Relay)
    }

    /// Always `{ any_net_type: true, allow_openalias: false }` (§2a.6).
    pub fn validate_address(&self, address: &str) -> Result<AddressCheck, Error> {
        self.call(
            "validate_address",
            serde_json::json!({
                "address": address,
                // Load-bearing: with it false a foreign address comes back
                // `valid: false, nettype: ""` and the user is told "invalid address" when
                // the truth is "that is a stagenet address" (§2a.6).
                "any_net_type": true,
                // Always false: it does DNS (§2a.6).
                "allow_openalias": false
            }),
            Deadline::Probe,
        )
    }

    pub fn refresh(&self, start_height: u64, cancel: &CancelToken) -> Result<Refreshed, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        self.call(
            "refresh",
            serde_json::json!({"start_height": start_height}),
            Deadline::Refresh,
        )
    }

    /// Issued once immediately after a successful `open_wallet`, with the upstream default
    /// period of 20 s — explicit rather than inherited (§2d).
    pub fn auto_refresh(&self, enable: bool, period_sec: u32) -> Result<(), Error> {
        self.ack(
            "auto_refresh",
            serde_json::json!({"enable": enable, "period": period_sec}),
            Deadline::Probe,
        )
    }

    pub fn get_address_book(&self) -> Result<Vec<BookEntry>, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wire {
            entries: Vec<BookEntry>,
        }
        // An empty `entries` request means "all of them" (§1b.4).
        let w: Wire = self.call(
            "get_address_book",
            serde_json::json!({"entries": []}),
            Deadline::Probe,
        )?;
        Ok(w.entries)
    }

    pub fn add_address_book(&self, address: &str, description: &str) -> Result<u64, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wire {
            index: u64,
        }
        let w: Wire = self.call(
            "add_address_book",
            serde_json::json!({"address": address, "description": description}),
            Deadline::Probe,
        )?;
        Ok(w.index)
    }

    pub fn delete_address_book(&self, index: u64) -> Result<(), Error> {
        self.ack(
            "delete_address_book",
            serde_json::json!({"index": index}),
            Deadline::Probe,
        )
    }

    pub fn set_tx_notes(&self, txids: &[String], notes: &[String]) -> Result<(), Error> {
        self.ack(
            "set_tx_notes",
            serde_json::json!({"txids": txids, "notes": notes}),
            Deadline::Probe,
        )
    }

    pub fn get_tx_notes(&self, txids: &[String]) -> Result<Vec<String>, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wire {
            notes: Vec<String>,
        }
        let w: Wire = self.call(
            "get_tx_notes",
            serde_json::json!({"txids": txids}),
            Deadline::Probe,
        )?;
        Ok(w.notes)
    }

    /// `login` carries the **node's** `rpc-login` credential (§4.2). Without it a wallet
    /// pointed at fmonero's own authenticated node gets a 401 on every daemon call and can
    /// never sync. **Requires an already-open wallet**: `on_set_daemon` answers
    /// `-13 NOT_OPEN` with no wallet loaded (`wallet_rpc_server.cpp:4692`), so it is called
    /// immediately *after* `open_wallet`, never before. Called only from
    /// `wallet_session::open` and from `wallet set-daemon`.
    /// `ssl` is a typed enum rather than a string because epee accepts exactly three spellings
    /// and answers `-38 "Invalid ssl support mode"` for a fourth (`net_ssl.cpp:654-665`,
    /// `wallet_rpc_server.cpp:4732-4737`) — a typo would be a runtime failure on the send path's
    /// own precondition. It is `Disabled` for a loopback address and `Autodetect` otherwise
    /// (`units::ssl_for_address`), and it is **always sent**: an omitted key means `autodetect`,
    /// not plaintext (`KV_SERIALIZE_OPT(ssl_support, "autodetect")`, §1b.4).
    ///
    /// `login` is `None` in remote mode, **always** (§2m.3): fmonero's `rpc-login` belongs to
    /// fmonero's own node and means nothing to a stranger's.
    ///
    /// Never sends `proxy`, `ssl_private_key_path`, `ssl_certificate_path`, `ssl_ca_file`,
    /// `ssl_allowed_fingerprints` or `ssl_allow_any_cert` (§0's cut list).
    pub fn set_daemon(
        &self,
        address: &str,
        trusted: bool,
        login: Option<&Login>,
        ssl: SslSupport,
    ) -> Result<(), Error> {
        let mut params = serde_json::json!({
            "address": address,
            "trusted": trusted,
            "ssl_support": ssl.as_str(),
        });
        // The two credential fields are **omitted** when there is none — the §2i
        // `system_unit` layout has no `rpc-login` at all (§4.2), and remote mode never has one.
        if let (Some(l), Some(map)) = (login, params.as_object_mut()) {
            map.insert("username".into(), serde_json::json!(l.user));
            map.insert("password".into(), serde_json::json!(l.pass));
        }
        // **`Deadline::Refresh`, not `Deadline::Probe`** — v5.3, and the defect that forced it
        // (§4.2, §2m.3). "`set_daemon` opens no socket" (§1b.9 item 1) is true of the call and
        // false of its latency: `wallet2::set_daemon` takes `m_daemon_rpc_mutex` as its first
        // statement, and in remote mode the auto-refresh a freshly created or opened wallet
        // starts holds that mutex across internet round trips. Under the 5 s
        // `timeouts.wallet_sec` probe deadline the very first `wallet create` against a real
        // public node exited **5 `killed_at_deadline`** for a wallet that had already been
        // written to disk.
        //
        // `timeouts.refresh_sec` (120 s) rather than `timeouts.open_sec` (300 s) because the
        // latency being waited on **is a refresh**: this design's one budget for "the wallet is
        // fetching blocks from a daemon" is the right ceiling for waiting behind one, while
        // `open_sec` is the budget for decrypting a wallet file — five minutes, during which a
        // `wallet open` against a wedged daemon would look like a hang. `--timeout N` lowers it
        // like every other deadline (§4).
        match self.t.rpc("set_daemon", params, Deadline::Refresh) {
            Ok(_) => Ok(()),
            // **`-38` here does not mean "cannot reach the node"**, and mapping it through the
            // §1b.4 table's `unreachable` would say so. `set_daemon` opens no socket: it is
            // `m_http_client->set_server(...)` (`wallet2.cpp:1409`), a URL **parse**
            // (`abstract_http_client.cpp:134-148`), and `-38 "Unable to set daemon"`
            // (`wallet_rpc_server.cpp:4758-4763`) is raised when that parse failed — which is
            // a disagreement between fmonero's §2m.2 grammar and epee's parser, not a network
            // condition. It gets the reason §4 documents for exactly that (§2m.3); the caller
            // decides whether it is fatal, and only `wallet_session::open` in **local** mode
            // swallows it (§4.2).
            Err(crate::rpc::RpcError::Rpc { code: -38, message }) => Err(Error::backend(message)
                .with_reason("remote_address_rejected")
                .with_hint(format!("fmonero node remote {address} --yes"))),
            Err(e) => Err(jsonrpc::wallet_error("set_daemon", e)),
        }
    }
}

/// `transfer_entry` as it arrives (`wallet_rpc_server_commands_defs.h:1446-1487`).
///
/// A separate type from [`Transfer`] for one reason: `confirmations`, `locked` and
/// `suggested_confirmations_threshold` are on the wire and are **dropped here**, because
/// wallet-rpc recomputes them per response from the wallet's own height and a cached copy is
/// a wrong answer about money one block later (§1b.4, §5b).
#[derive(Default, Deserialize)]
#[serde(default)]
struct WireTransfer {
    txid: String,
    payment_id: String,
    height: u64,
    timestamp: i64,
    amount: Piconero,
    fee: Piconero,
    note: String,
    destinations: Vec<crate::history::Destination>,
    r#type: crate::history::TransferKind,
    unlock_time: u64,
    subaddr_index: crate::history::SubaddrIndex,
    address: String,
    double_spend_seen: bool,
}

impl WireTransfer {
    fn into_transfer(self) -> Transfer {
        Transfer {
            txid: self.txid,
            kind: self.r#type,
            height: self.height,
            timestamp: self.timestamp,
            amount: self.amount,
            fee: self.fee,
            note: self.note,
            address: self.address,
            subaddr_index: self.subaddr_index,
            destinations: self.destinations,
            unlock_time: self.unlock_time,
            payment_id: self.payment_id,
            double_spend_seen: self.double_spend_seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §2c cases for the client that carries the credential and moves the money.
    #[test]
    fn a_non_loopback_wallet_client_cannot_be_constructed() {
        for url in [
            "https://example.com",
            "http://10.0.0.5:18083",
            "http://example.com:18083",
        ] {
            let e = match WalletClient::new(url, None, Duration::from_secs(5)) {
                Err(e) => e,
                Ok(_) => panic!("{url} must be refused by the §2c gate"),
            };
            assert_eq!(e.exit_code(), 2, "{url}");
            assert_eq!(e.reason(), Some("non_loopback_url"), "{url}");
        }
        assert!(WalletClient::new("http://127.0.0.1:18083", None, Duration::from_secs(5)).is_ok());
    }

    /// `filter_by_height` must be a field, because an omitted key silently returns the
    /// whole history (§1b.4).
    #[test]
    fn a_transfer_query_carries_the_height_filter_flag() {
        let q = TransferQuery {
            account_index: 0,
            r#in: true,
            out: true,
            filter_by_height: true,
            min_height: 3_500_000,
            ..TransferQuery::default()
        };
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(v["filter_by_height"], true);
        assert_eq!(v["min_height"], 3_500_000);
        assert_eq!(v["in"], true, "the field is `r#in`, the key is `in`");
        assert_eq!(v["pool"], false, "pool is sent only when asked for");
        assert!(v.get("max_height").is_none(), "left unset (§1b.4)");
    }
}
