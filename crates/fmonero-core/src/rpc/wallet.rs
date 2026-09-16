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
use crate::rpc::Login;
use crate::send::{BuiltTx, TransferRequest};

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
    /* Phase 1b adds the private state §5b names: agent, base, auth, timeout, and the
    Digest session behind a Mutex (§2c). Nothing here is part of the frozen API. */
}

impl WalletClient {
    /// Rejects any non-http scheme and any non-loopback host with `Error::Usage`, through
    /// the same `rpc::require_loopback` `NodeClient::new` calls (§2c). The wallet client is
    /// the one that carries a credential and moves money, so the rule is not relaxed here.
    pub fn new(url: &str, _auth: Option<Login>, _timeout: Duration) -> Result<WalletClient, Error> {
        crate::rpc::require_loopback(url)?;
        Ok(WalletClient {})
    }

    /// Reads `user:pass` from the login file (§2c). **An absent file is `Ok` with
    /// `auth: None`, never an error**: steps 1–4 of §2a.1's refusal order must still run in
    /// order, so no constructor may decide a money-path exit code. The absence surfaces at
    /// the first request as exit 4 — `reason: unreachable` for a refused connect,
    /// `auth_refused` for a 401.
    pub fn from_ctx(_ctx: &Context) -> Result<WalletClient, Error> {
        Err(Error::unimplemented("WalletClient::from_ctx"))
    }

    /// As `NodeClient::authenticated`: what the wire did on the last completed request.
    pub fn authenticated(&self) -> Option<bool> {
        None
    }

    /// `(major, minor, release)` — `WALLET_RPC_VERSION` is packed `major<<16|minor` (`:2554`).
    pub fn get_version(&self) -> Result<(u16, u16, bool), Error> {
        Err(Error::unimplemented("WalletClient::get_version"))
    }

    /// The wallet's own blockchain height (`:455`) — simultaneously the reachability probe
    /// and the open/closed probe (§5). `Error::WalletLocked` on `-13 NOT_OPEN`.
    pub fn get_height(&self) -> Result<u64, Error> {
        Err(Error::unimplemented("WalletClient::get_height"))
    }

    /// Maps **any** `-1` to `Error::WalletLocked` (exit 9): `on_open_wallet` overwrites the
    /// mapped code, so a wrong password never arrives as `-22` (§1b.4). The caller
    /// pre-checks that the `.keys` file exists (exit 6), which is what makes that mapping
    /// unambiguous. Called only from `wallet_session::open`.
    pub fn open_wallet(&self, _name: &str, _password: &str) -> Result<(), Error> {
        Err(Error::unimplemented("WalletClient::open_wallet"))
    }

    pub fn close_wallet(&self) -> Result<(), Error> {
        Err(Error::unimplemented("WalletClient::close_wallet"))
    }

    /// `language` is **required** by the daemon and an unknown value comes back as a bare
    /// `-1`, so it is validated locally first (`settings::validate_seed_language`, §1b.4).
    pub fn create_wallet(
        &self,
        _name: &str,
        _password: &str,
        _language: &str,
    ) -> Result<(), Error> {
        Err(Error::unimplemented("WalletClient::create_wallet"))
    }

    pub fn get_balance(&self, _account: u32) -> Result<Balance, Error> {
        Err(Error::unimplemented("WalletClient::get_balance"))
    }

    pub fn get_accounts(&self) -> Result<Vec<AccountRow>, Error> {
        Err(Error::unimplemented("WalletClient::get_accounts"))
    }

    pub fn get_address(&self, _account: u32) -> Result<AddressSet, Error> {
        Err(Error::unimplemented("WalletClient::get_address"))
    }

    pub fn create_address(&self, _account: u32, _label: &str) -> Result<NewAddress, Error> {
        Err(Error::unimplemented("WalletClient::create_address"))
    }

    pub fn label_address(&self, _account: u32, _index: u32, _label: &str) -> Result<(), Error> {
        Err(Error::unimplemented("WalletClient::label_address"))
    }

    pub fn get_transfers(&self, _q: &TransferQuery) -> Result<Vec<Transfer>, Error> {
        Err(Error::unimplemented("WalletClient::get_transfers"))
    }

    /// `Ok(None)` when wallet-rpc answers `-8` with "Transaction not found." — the normal
    /// "not in this wallet yet" answer, and the whole basis of the §2a.4 resolve step.
    /// `Err(Usage)` for the malformed-txid variants of `-8` (§1b.4).
    pub fn get_transfer_by_txid(
        &self,
        _txid: &str,
        _account: u32,
    ) -> Result<Option<Transfer>, Error> {
        Err(Error::unimplemented("WalletClient::get_transfer_by_txid"))
    }

    /// ALWAYS called with `do_not_relay: true` and `get_tx_metadata: true` (§2a.2); the
    /// type makes the other combination unrepresentable. **Callable only from
    /// `send::build`** (§2a.3).
    pub fn build_transfer(
        &self,
        _req: &TransferRequest,
        _cancel: &CancelToken,
    ) -> Result<BuiltTx, Error> {
        Err(Error::unimplemented("WalletClient::build_transfer"))
    }

    /// `sweep_all` returns a **list** (`split_transfer_response`, §1b.4), and its request
    /// always carries `subaddr_indices_all: true` (§2a.10). **Callable only from
    /// `send::build_sweep`** (§2a.3).
    pub fn build_sweep_all(
        &self,
        _req: &TransferRequest,
        _cancel: &CancelToken,
    ) -> Result<Vec<BuiltTx>, Error> {
        Err(Error::unimplemented("WalletClient::build_sweep_all"))
    }

    /// Returns the server's `tx_hash`, which the caller compares with the built txid
    /// (§2a.9). `metadata_hex` is a serialized `wallet2::pending_tx` — **not** `tx_blob`
    /// (§1b.4). **Callable only from `send::relay`** (§2a.3).
    pub fn relay_tx(&self, _metadata_hex: &str) -> Result<String, Error> {
        Err(Error::unimplemented("WalletClient::relay_tx"))
    }

    /// Persists the wallet file. Its ONLY caller is `send::relay`, after the last
    /// successful `relay_tx` (§2a.4): wallet-rpc writes the wallet only on a graceful
    /// `stop()` (`wallet_rpc_server.cpp:294-302`) or on this call (`:2060-2077`) — the
    /// auto-refresh idle handler never stores (`:210-285`).
    pub fn store(&self) -> Result<(), Error> {
        Err(Error::unimplemented("WalletClient::store"))
    }

    /// Always `{ any_net_type: true, allow_openalias: false }` (§2a.6).
    pub fn validate_address(&self, _address: &str) -> Result<AddressCheck, Error> {
        Err(Error::unimplemented("WalletClient::validate_address"))
    }

    pub fn refresh(&self, _start_height: u64, _cancel: &CancelToken) -> Result<Refreshed, Error> {
        Err(Error::unimplemented("WalletClient::refresh"))
    }

    /// Issued once immediately after a successful `open_wallet`, with the upstream default
    /// period of 20 s — explicit rather than inherited (§2d).
    pub fn auto_refresh(&self, _enable: bool, _period_sec: u32) -> Result<(), Error> {
        Err(Error::unimplemented("WalletClient::auto_refresh"))
    }

    pub fn get_address_book(&self) -> Result<Vec<BookEntry>, Error> {
        Err(Error::unimplemented("WalletClient::get_address_book"))
    }

    pub fn add_address_book(&self, _address: &str, _description: &str) -> Result<u64, Error> {
        Err(Error::unimplemented("WalletClient::add_address_book"))
    }

    pub fn delete_address_book(&self, _index: u64) -> Result<(), Error> {
        Err(Error::unimplemented("WalletClient::delete_address_book"))
    }

    pub fn set_tx_notes(&self, _txids: &[String], _notes: &[String]) -> Result<(), Error> {
        Err(Error::unimplemented("WalletClient::set_tx_notes"))
    }

    pub fn get_tx_notes(&self, _txids: &[String]) -> Result<Vec<String>, Error> {
        Err(Error::unimplemented("WalletClient::get_tx_notes"))
    }

    /// `login` carries the **node's** `rpc-login` credential (§4.2). Without it a wallet
    /// pointed at fmonero's own authenticated node gets a 401 on every daemon call and can
    /// never sync. **Requires an already-open wallet**: `on_set_daemon` answers
    /// `-13 NOT_OPEN` with no wallet loaded (`wallet_rpc_server.cpp:4692`), so it is called
    /// immediately *after* `open_wallet`, never before. Called only from
    /// `wallet_session::open` and from `wallet set-daemon`.
    pub fn set_daemon(
        &self,
        _address: &str,
        _trusted: bool,
        _login: Option<&Login>,
    ) -> Result<(), Error> {
        Err(Error::unimplemented("WalletClient::set_daemon"))
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
