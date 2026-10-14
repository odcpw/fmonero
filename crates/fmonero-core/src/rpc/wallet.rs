//! `monero-wallet-rpc` JSON-RPC (DESIGN.md §1b.4, §5b).
//!
//! Typed wallet calls with protected serialization for secrets and transaction
//! metadata. Normal payments prepare through transfer_split; send owns review,
//! pending persistence and relay. Advanced methods are exposed through small domain
//! modules so callers cannot bypass their identity and payment guards.

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
    /// Apply the same validated transport used by wallet startup. Credential
    /// serialization uses the protected path and never appears in diagnostics.
    pub fn set_connection(
        &self,
        connection: &crate::transport::DaemonConnection,
        login: Option<&Login>,
    ) -> Result<(), Error> {
        #[derive(serde::Serialize)]
        struct Params<'a> {
            address: &'a str,
            trusted: bool,
            ssl_support: &'a str,
            ssl_ca_file: String,
            ssl_allowed_fingerprints: Vec<&'a str>,
            ssl_allow_any_cert: bool,
            proxy: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            username: Option<crate::secrets::SecretRef<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            password: Option<crate::secrets::SecretRef<'a>>,
        }
        #[derive(serde::Deserialize)]
        struct Ack {}
        let _: Ack = self
            .t
            .sensitive(
                "set_daemon",
                &Params {
                    address: &connection.address,
                    trusted: connection.trusted,
                    ssl_support: connection.ssl.as_str(),
                    ssl_ca_file: connection
                        .ca_file
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    ssl_allowed_fingerprints: connection
                        .fingerprint
                        .as_deref()
                        .into_iter()
                        .collect(),
                    ssl_allow_any_cert: false,
                    proxy: connection.rpc_proxy(),
                    username: login.map(|l| crate::secrets::SecretRef(&l.user)),
                    password: login.map(|l| crate::secrets::SecretRef(&l.pass)),
                },
                Deadline::Refresh,
            )
            .map_err(|e| match e {
                crate::rpc::RpcError::Rpc { code: -38, .. } => {
                    Error::backend("Monero refused the configured wallet connection.")
                        .with_reason("remote_address_rejected")
                        .with_hint("fmonero settings list")
                }
                other => jsonrpc::wallet_error("set_daemon", other),
            })?;
        Ok(())
    }
    pub(crate) fn restore_protected(
        &self,
        request: &crate::restore::RestoreRequest,
    ) -> Result<String, Error> {
        use crate::secrets::SecretRef;
        #[derive(serde::Serialize)]
        struct Params<'a> {
            filename: &'a str,
            seed: SecretRef<'a>,
            password: SecretRef<'a>,
            seed_offset: SecretRef<'a>,
            restore_height: u64,
            language: &'a str,
            autosave_current: bool,
            enable_multisig_experimental: bool,
        }
        // Deliberately do not deserialize returned seed/info into an owned string.
        #[derive(serde::Deserialize)]
        struct Restored {
            address: String,
        }
        let result: Restored = self
            .t
            .sensitive(
                "restore_deterministic_wallet",
                &Params {
                    filename: &request.name,
                    seed: SecretRef(request.seed.expose_secret()),
                    password: SecretRef(request.password.expose_secret()),
                    seed_offset: SecretRef(
                        request
                            .seed_offset
                            .as_ref()
                            .map_or("", |s| s.expose_secret()),
                    ),
                    restore_height: request.restore_height,
                    language: request.language.as_deref().unwrap_or(""),
                    autosave_current: true,
                    enable_multisig_experimental: false,
                },
                Deadline::Open,
            )
            .map_err(|e| jsonrpc::wallet_error("restore_deterministic_wallet", e))?;
        Ok(result.address)
    }
    /// Rejects any non-http scheme and any non-loopback host with `Error::Usage`, through
    /// the same `rpc::require_loopback` `NodeClient::new` calls (§2c). The wallet client is
    /// the one that carries a credential and moves money, so the rule is not relaxed here.
    pub fn new(url: &str, auth: Option<Login>, timeout: Duration) -> Result<WalletClient, Error> {
        crate::rpc::require_loopback(url)?;
        Ok(WalletClient {
            t: Transport::new(url, auth, timeout, Daemon::Wallet),
        })
    }

    /// Reads `user:pass` from the login file for each request (§2c), including after startup.
    /// **An absent file is `Ok` with
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
        Ok(WalletClient {
            t: Transport::from_ctx(ctx, url, None, Daemon::Wallet, breaker)
                .with_wallet_login_file(ctx.paths.wallet_login_file(ctx.settings.wallet.port)),
        })
    }

    pub(crate) fn call<T: serde::de::DeserializeOwned>(
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
    pub(crate) fn ack(
        &self,
        method: &str,
        params: serde_json::Value,
        d: Deadline,
    ) -> Result<(), Error> {
        let _: serde_json::Value = self.call(method, params, d)?;
        Ok(())
    }

    pub(crate) fn protected<P: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        method: &'static str,
        params: &P,
        deadline: Deadline,
    ) -> Result<R, Error> {
        self.t
            .sensitive(method, params, deadline)
            .map_err(|e| jsonrpc::wallet_error(method, e))
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

    /// At this opening boundary only, `-1` maps to `Error::WalletLocked` (exit 9):
    /// Monero overwrites the mapped opening error, including a wrong password,
    /// with UNKNOWN_ERROR. Other operations keep the generic upstream failure.
    pub fn open_wallet(&self, name: &str, password: &str) -> Result<(), Error> {
        #[derive(serde::Serialize)]
        struct Params<'a> {
            filename: &'a str,
            password: crate::secrets::SecretRef<'a>,
            autosave_current: bool,
        }
        #[derive(serde::Deserialize)]
        struct Ack {}
        let _: Ack = self
            .t
            .sensitive(
                "open_wallet",
                &Params {
                    filename: name,
                    password: crate::secrets::SecretRef(password),
                    autosave_current: true,
                },
                Deadline::Open,
            )
            .map_err(|e| jsonrpc::wallet_error("open_wallet", e))?;
        Ok(())
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
        #[derive(serde::Serialize)]
        struct Params<'a> {
            filename: &'a str,
            password: crate::secrets::SecretRef<'a>,
            language: &'a str,
        }
        #[derive(serde::Deserialize)]
        struct Ack {}
        let _: Ack = self
            .t
            .sensitive(
                "create_wallet",
                &Params {
                    filename: name,
                    password: crate::secrets::SecretRef(password),
                    language,
                },
                Deadline::Open,
            )
            .map_err(|e| jsonrpc::wallet_error("create_wallet", e))?;
        Ok(())
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
            // **X2, and it is on the money path.** Rules 6 and 7 in one sentence, in a file
            // `check.sh` did not read. What the user needs to know is that nothing was sent.
            return Err(Error::failure(
                "The wallet service prepared this payment but did not give fmonero what it needs \
                 to send it. Nothing has gone out.",
            )
            .with_detail("transfer: empty tx_metadata"));
        }
        Ok(BuiltTx {
            txid: w.tx_hash,
            amount: w.amount,
            fee: w.fee,
            weight: w.weight,
            metadata: w.tx_metadata,
        })
    }

    /// Normal payments may span several transactions. Always prepare without relay.
    pub(crate) fn build_transfer_split(
        &self,
        req: &TransferRequest,
        cancel: &CancelToken,
    ) -> Result<Vec<BuiltTx>, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        #[derive(Deserialize)]
        struct Wire {
            tx_hash_list: Vec<String>,
            amount_list: Vec<Piconero>,
            fee_list: Vec<Piconero>,
            weight_list: Vec<u64>,
            tx_metadata_list: Vec<crate::secrets::ProtectedText>,
        }
        let result: Wire = self.protected("transfer_split", req, Deadline::Transfer)?;
        let count = result.tx_hash_list.len();
        if count == 0
            || count > 1024
            || result.amount_list.len() != count
            || result.fee_list.len() != count
            || result.weight_list.len() != count
            || result.tx_metadata_list.len() != count
        {
            return Err(Error::upstream(
                "Monero returned inconsistent split payment data. Nothing was sent.",
            ));
        }
        if result
            .tx_hash_list
            .iter()
            .any(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
            || result.tx_metadata_list.iter().any(|s| s.0.is_empty())
            || result
                .tx_hash_list
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != count
        {
            return Err(Error::upstream(
                "Monero returned incomplete split payment data. Nothing was sent.",
            ));
        }
        Ok(result
            .tx_hash_list
            .into_iter()
            .zip(result.amount_list)
            .zip(result.fee_list)
            .zip(result.weight_list)
            .zip(result.tx_metadata_list)
            .map(|((((txid, amount), fee), weight), mut metadata)| BuiltTx {
                txid,
                amount,
                fee,
                weight,
                metadata: std::mem::take(&mut *metadata.0),
            })
            .collect())
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
            tx_metadata_list: Vec<crate::secrets::ProtectedText>,
        }
        let w: Wire = self.protected("sweep_all", req, Deadline::Transfer)?;
        // Five parallel arrays that must stay in lockstep: a shorter `tx_metadata_list`
        // would silently drop a transaction the record already promises to relay (§2a.4).
        let n = w.tx_hash_list.len();
        if n == 0 {
            return Err(
                Error::tx_rejected("There is nothing in this account to sweep.")
                    .with_reason("not_possible")
                    .with_hint("fmonero wallet balance"),
            );
        }
        if n > 1024
            || w.tx_metadata_list.len() != n
            || w.amount_list.len() != n
            || w.fee_list.len() != n
            || w.weight_list.len() != n
        {
            // X2: the five array lengths are the detail an agent and `-v` want; what a person
            // needs is that the answer did not add up and nothing was sent.
            return Err(Error::failure(
                "The wallet service gave an answer fmonero cannot use. Nothing has gone out.",
            )
            .with_detail(format!(
                "sweep_all: {n} ids but {} metadata, {} amounts, {} fees and {} weights",
                w.tx_metadata_list.len(),
                w.amount_list.len(),
                w.fee_list.len(),
                w.weight_list.len()
            )));
        }
        if w.tx_hash_list
            .iter()
            .any(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
            || w.tx_hash_list
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != n
            || w.tx_metadata_list.iter().any(|s| s.0.is_empty())
        {
            return Err(Error::upstream(
                "Monero returned incomplete sweep data. Nothing was sent.",
            ));
        }
        Ok(w.tx_hash_list
            .into_iter()
            .zip(w.amount_list)
            .zip(w.fee_list)
            .zip(w.weight_list)
            .zip(w.tx_metadata_list)
            .map(|((((txid, amount), fee), weight), mut metadata)| BuiltTx {
                txid,
                amount,
                fee,
                weight,
                metadata: std::mem::take(&mut *metadata.0),
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
        #[derive(Serialize)]
        struct Params<'a> {
            hex: crate::secrets::SecretRef<'a>,
        }
        let w: Wire = self.protected(
            "relay_tx",
            &Params {
                hex: crate::secrets::SecretRef(metadata_hex),
            },
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
        let node = crate::settings::NodeSettings {
            tls: match ssl {
                SslSupport::Enabled => crate::transport::TlsPolicy::Required,
                SslSupport::Disabled => crate::transport::TlsPolicy::Disabled,
                SslSupport::Autodetect => crate::transport::TlsPolicy::Auto,
            },
            ..Default::default()
        };
        let connection = crate::transport::DaemonConnection::for_address(&node, address, trusted)?;
        self.set_connection(&connection, login)
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
