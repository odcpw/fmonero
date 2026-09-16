//! `monerod` JSON-RPC and the legacy `POST /<name>` endpoints (DESIGN.md §1b.2, §1b.3, §5b).
//!
//! **Signature-only at the Phase 1a gate** (§8): every type here is frozen and every body
//! is Phase 1b's. `NodeClient::new` is the exception — it is the §2c loopback gate, which
//! is a local check with a local test, so it works from day one.
//!
//! Field types follow §5b rule 2: snake_case, `u64` for counts and heights, [`Piconero`]
//! for amounts, `Option<T>` only where the RPC marks a field optional — with the exception
//! §1b.3 names twice, `connection_info.ip`/`.port`/`.peer_id`/`.connection_id`, which are
//! **strings** upstream while `peer.ip`/`.port` are **numbers**. Applying "u64 for counts"
//! blindly to `Connection` fails to deserialize every row of `get_connections`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::amount::Piconero;
use crate::context::Context;
use crate::error::Error;
use crate::rpc::Login;

/// `GET_INFO` (`core_rpc_server_commands_defs.h:709-745`). The node's hot-path call and
/// the reachability probe: it is **not** gated by `CHECK_CORE_READY()`, which is why
/// `status` uses it rather than a block header (§1b.3).
///
/// A **restricted** node poisons part of this response rather than omitting it
/// (`core_rpc_server.cpp:517-569`): `start_time: 0`, `free_space: 2^64-1`, `version: ""`,
/// `update_available: false`, `alt_blocks_count: 0`, all four connection/peer-list counts
/// `0`, and `database_size` rounded up to a 5 GiB multiple. §5 turns every one of those
/// into `null`; `tx_pool_size` is a **lower bound** under restriction, not poison.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GetInfo {
    pub height: u64,
    /// **0 when synced**, not equal to `height` — the sync percentage must special-case it.
    pub target_height: u64,
    pub difficulty: u64,
    pub wide_difficulty: String,
    pub difficulty_top64: u64,
    pub target: u64,
    pub tx_count: u64,
    pub tx_pool_size: u64,
    pub alt_blocks_count: u64,
    pub outgoing_connections_count: u64,
    pub incoming_connections_count: u64,
    pub rpc_connections_count: u64,
    pub white_peerlist_size: u64,
    pub grey_peerlist_size: u64,
    pub mainnet: bool,
    pub testnet: bool,
    pub stagenet: bool,
    pub nettype: String,
    pub top_block_hash: String,
    pub cumulative_difficulty: u64,
    pub block_size_limit: u64,
    pub block_weight_limit: u64,
    pub block_size_median: u64,
    pub block_weight_median: u64,
    pub adjusted_time: u64,
    pub start_time: u64,
    pub free_space: u64,
    pub offline: bool,
    pub bootstrap_daemon_address: String,
    pub height_without_bootstrap: u64,
    pub was_bootstrap_ever_used: bool,
    pub database_size: u64,
    pub update_available: bool,
    pub busy_syncing: bool,
    pub version: String,
    pub synchronized: bool,
    pub restricted: bool,
}

/// `block_header_response` (`:1166-1215`). `timestamp` gives "last block N s ago".
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockHeader {
    pub major_version: u8,
    pub minor_version: u8,
    pub timestamp: u64,
    pub prev_hash: String,
    pub nonce: u64,
    pub orphan_status: bool,
    pub height: u64,
    pub depth: u64,
    pub hash: String,
    pub difficulty: u64,
    pub cumulative_difficulty: u64,
    pub reward: Piconero,
    pub block_size: u64,
    pub block_weight: u64,
    pub num_txes: u64,
    pub pow_hash: String,
    pub long_term_weight: u64,
    pub miner_tx_hash: String,
}

/// `HARD_FORK_INFO` (`:1961`). **Two different fork versions arrive in one response**
/// (§1b.3): `version` is always `get_current_hard_fork_version()`, while
/// `enabled`/`window`/`votes`/`threshold`/`earliest_height`/`voting` describe the version
/// the request named — and `req.version == 0` means the **next** fork, not the current one
/// (`core_rpc_server.cpp:2738-2741`). `status` reads only `version` and `state`; `node info`
/// publishes the voting fields under their own `voting_on` object.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HardForkInfo {
    pub version: u8,
    pub enabled: bool,
    pub window: u64,
    pub votes: u64,
    pub threshold: u64,
    pub voting: u8,
    pub state: u32,
    pub earliest_height: u64,
}

/// One row of `get_version.hard_forks[]` (`:2173`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HardForkEntry {
    pub hf_version: u8,
    pub height: u64,
}

/// `GET_VERSION` (`:2173`). `version` is the **RPC** version packed `major<<16|minor`, not
/// the software version — that one is `get_info.version` (§5c `node info`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeVersion {
    pub version: u32,
    pub release: bool,
    pub current_height: u64,
    pub target_height: u64,
    pub hard_forks: Vec<HardForkEntry>,
}

/// `GET_BASE_FEE_ESTIMATE`, json_rpc name `get_fee_estimate` (`:2253`).
///
/// Since HF 15 `on_get_base_fee_estimate` sets `res.fee = res.fees[0]`
/// (`core_rpc_server.cpp:3011-3012`), so **`fee` is the cheapest tier, not a "normal" one**:
/// §5 renders it as `per_byte_pico` beside `fees_pico` and no code may treat it as a
/// default. `fees[]` — four entries, in order — is the authority for the four priority
/// levels, and needs no local multiplier table.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FeeEstimate {
    pub fee: Piconero,
    pub quantization_mask: u64,
    pub fees: Vec<Piconero>,
}

/// One bucket of `txpool_stats.histo[]` (`:1680-1712`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TxpoolHisto {
    pub txs: u64,
    pub bytes: u64,
}

/// `txpool_stats` (`:1680-1712`), the payload of `GET_TRANSACTION_POOL_STATS` (`:1715`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TxpoolStats {
    pub bytes_total: u64,
    pub bytes_min: u64,
    pub bytes_max: u64,
    pub bytes_med: u64,
    pub fee_total: Piconero,
    pub oldest: u64,
    pub txs_total: u64,
    pub num_failing: u64,
    pub num_10m: u64,
    pub num_not_relayed: u64,
    pub histo_98pc: u64,
    pub histo: Vec<TxpoolHisto>,
    pub num_double_spends: u64,
}

/// `connection_info` (`cryptonote_protocol_defs.h:47-88`).
///
/// **`ip`, `port`, `peer_id` and `connection_id` are STRINGS here** (`:56`, `:57`, `:61`,
/// `:81`) while the same-named fields of [`Peer`] are numbers. §5c therefore also says that
/// `connections[].port` is parsed from the daemon's string and **rendered as an integer**,
/// so both arrays of `node peers --json` agree.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    pub incoming: bool,
    pub localhost: bool,
    pub local_ip: bool,
    pub ssl: bool,
    pub address: String,
    pub host: String,
    pub ip: String,
    pub port: String,
    pub rpc_port: u16,
    pub rpc_credits_per_hash: u32,
    pub peer_id: String,
    pub recv_count: u64,
    pub recv_idle_time: u64,
    pub send_count: u64,
    pub send_idle_time: u64,
    pub state: String,
    pub live_time: u64,
    pub avg_download: u64,
    pub current_download: u64,
    pub avg_upload: u64,
    pub current_upload: u64,
    pub support_flags: u32,
    pub connection_id: String,
    pub height: u64,
    pub pruning_seed: u32,
    pub address_type: u8,
}

/// One `sync_info.peers[]` entry (`:2386`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncPeer {
    pub info: Connection,
}

/// One `sync_info.spans[]` entry (`:2386`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Span {
    pub start_block_height: u64,
    pub nblocks: u64,
    pub connection_id: String,
    pub rate: u64,
    pub speed: u64,
    pub size: u64,
    pub remote_address: String,
}

/// `SYNC_INFO` (`:2386`) — a `!m_restricted` method, so a restricted node answers
/// `-32601` and this is fmonero's restricted-RPC probe (§1b.2).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncInfo {
    pub height: u64,
    pub target_height: u64,
    pub next_needed_pruning_seed: u32,
    pub peers: Vec<SyncPeer>,
    pub spans: Vec<Span>,
    pub overview: String,
}

/// One row of `get_peer_list`'s `white_list[]`/`gray_list[]` (`:1338-1346`).
/// **`ip` and `port` are NUMBERS here**, unlike [`Connection`]'s strings (§1b.3).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Peer {
    pub id: u64,
    pub host: String,
    pub ip: u32,
    pub port: u16,
    pub rpc_port: u16,
    pub rpc_credits_per_hash: u32,
    pub last_seen: u64,
    pub pruning_seed: u32,
}

/// `GET_PEER_LIST` (`:1372`) — `public_only` defaults to **true**.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PeerLists {
    pub white_list: Vec<Peer>,
    pub gray_list: Vec<Peer>,
}

/// One row of `GET_BANS` (`:2000`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ban {
    pub host: String,
    pub ip: u32,
    pub seconds: u64,
}

/// One row of `SET_BANS` (`:2035`). Unban is `ban: false`; send `host` and leave `ip` 0,
/// because the server accepts either (§1b.3).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BanRequest {
    pub host: String,
    pub ip: u32,
    pub ban: bool,
    pub seconds: u64,
}

/// `GET_NET_STATS` (`:796`). These are lifetime counters: the **rates** are fmonero's job,
/// from two samples kept in `state.json` (§5).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NetStats {
    pub start_time: u64,
    pub total_packets_in: u64,
    pub total_bytes_in: u64,
    pub total_packets_out: u64,
    pub total_bytes_out: u64,
}

/// `GET_LIMIT` (`:1855`) / `SET_LIMIT` (`:1879`). **kB/s** (the comment at `:1882`);
/// `-1` resets to the default and `0` means "unchanged", which is why these are signed.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub limit_up: i64,
    pub limit_down: i64,
}

/// A client for one `monerod`.
///
/// The five **mutators** (`stop_daemon`, `set_log_level`, `set_bans`, `set_limit`,
/// `set_peers_count`) deliberately do not exist here: they live on [`NodeMutators`], and
/// the only way to obtain one is [`NodeClient::mutators`], which refuses unless the §2i
/// layout is `UserUnit`. A caller that did not pass that gate cannot name them, so a
/// forgotten check is a compile error rather than a working `node down` against Arch's
/// system daemon (§2i).
pub struct NodeClient {
    /* Phase 1b adds the private state §5b names: agent, base, auth, timeout, and the
    Digest session behind a Mutex (§2c). Nothing here is part of the frozen API. */
}

impl NodeClient {
    /// Rejects any non-http scheme and any non-loopback host with `Error::Usage`, through
    /// `rpc::require_loopback` — the same function `WalletClient::new` calls (§2c). That
    /// check is this constructor's **first statement**, and it is implemented at the 1a
    /// gate because it needs no daemon.
    pub fn new(url: &str, _auth: Option<Login>, _timeout: Duration) -> Result<NodeClient, Error> {
        crate::rpc::require_loopback(url)?;
        Ok(NodeClient {})
    }

    /// The client `settings.node.url` and `timeouts.node` describe, with the credential
    /// from `~/.config/fmonero/monerod.conf` when there is one (§2c).
    pub fn from_ctx(_ctx: &Context) -> Result<NodeClient, Error> {
        Err(Error::unimplemented("NodeClient::from_ctx"))
    }

    pub fn get_info(&self) -> Result<GetInfo, Error> {
        Err(Error::unimplemented("NodeClient::get_info"))
    }

    pub fn get_last_block_header(&self) -> Result<BlockHeader, Error> {
        Err(Error::unimplemented("NodeClient::get_last_block_header"))
    }

    pub fn hard_fork_info(&self, _version: u8) -> Result<HardForkInfo, Error> {
        Err(Error::unimplemented("NodeClient::hard_fork_info"))
    }

    pub fn get_version(&self) -> Result<NodeVersion, Error> {
        Err(Error::unimplemented("NodeClient::get_version"))
    }

    /// `grace_blocks` is **always 10**, the `FEE_ESTIMATE_GRACE_BLOCKS` wallet2 itself uses
    /// (`wallet2.cpp:123`, applied at `:8597`), so the chips fmonero draws are computed
    /// exactly as the fee the build will charge (§1b.3).
    pub fn get_fee_estimate(&self, _grace_blocks: u64) -> Result<FeeEstimate, Error> {
        Err(Error::unimplemented("NodeClient::get_fee_estimate"))
    }

    pub fn get_txpool_stats(&self) -> Result<TxpoolStats, Error> {
        Err(Error::unimplemented("NodeClient::get_txpool_stats"))
    }

    /// `Upstream(-32601)` on a restricted node (§1b.2).
    pub fn sync_info(&self) -> Result<SyncInfo, Error> {
        Err(Error::unimplemented("NodeClient::sync_info"))
    }

    pub fn get_connections(&self) -> Result<Vec<Connection>, Error> {
        Err(Error::unimplemented("NodeClient::get_connections"))
    }

    pub fn get_peer_list(&self, _public_only: bool) -> Result<PeerLists, Error> {
        Err(Error::unimplemented("NodeClient::get_peer_list"))
    }

    pub fn get_bans(&self) -> Result<Vec<Ban>, Error> {
        Err(Error::unimplemented("NodeClient::get_bans"))
    }

    pub fn get_net_stats(&self) -> Result<NetStats, Error> {
        Err(Error::unimplemented("NodeClient::get_net_stats"))
    }

    pub fn get_limit(&self) -> Result<Limits, Error> {
        Err(Error::unimplemented("NodeClient::get_limit"))
    }

    /// Read-only: always sends `set: false` (§1b.3). The writing form lives on
    /// [`NodeMutators`]. Returns `(out_peers, in_peers)`.
    pub fn peers_count(&self) -> Result<(u32, u32), Error> {
        Err(Error::unimplemented("NodeClient::peers_count"))
    }

    /// One call, classifying the answer: `Ok(false)` = unrestricted, `Ok(true)` = the
    /// `-32601` of a gated `sync_info` (§1b.2). The cheapest unambiguous probe there is.
    pub fn is_restricted(&self) -> Result<bool, Error> {
        Err(Error::unimplemented("NodeClient::is_restricted"))
    }

    /// Whether the last **completed** request went through a Digest handshake
    /// (`Some(true)`), was accepted with no challenge at all (`Some(false)`), or has not
    /// been attempted (`None`).
    ///
    /// It reports **what the wire did, never what a config file says**: inferring it from
    /// `Login::from_monerod_conf` returning `Some` would report `authenticated: true`
    /// against a node that never challenged us, which is the false claim §2j forbids. It
    /// feeds §5's `node.authenticated`, the `auth_missing` warning and the doctor's
    /// `node_auth` (§7.1).
    pub fn authenticated(&self) -> Option<bool> {
        None
    }

    /// **The §2i layout gate, as a type rather than as a comment.** `Error::Safety` with
    /// the `sudo systemctl …` hint unless `units::detect_layout(ctx) == Layout::UserUnit`.
    /// There is no `--force` override.
    pub fn mutators<'a>(&'a self, _ctx: &Context) -> Result<NodeMutators<'a>, Error> {
        Err(Error::unimplemented("NodeClient::mutators"))
    }
}

/// The five mutating node calls, reachable only through [`NodeClient::mutators`] (§2i).
pub struct NodeMutators<'a> {
    /// Private state, not part of the frozen API: Phase 1b replaces it with the
    /// `&'a NodeClient` §5b names. Allowed dead for exactly as long as the bodies below
    /// are `Error::unimplemented` (§8's sanctioned handover).
    #[allow(dead_code)]
    client: std::marker::PhantomData<&'a NodeClient>,
}

impl NodeMutators<'_> {
    pub fn stop_daemon(&self) -> Result<(), Error> {
        Err(Error::unimplemented("NodeMutators::stop_daemon"))
    }

    pub fn set_log_level(&self, _level: i8) -> Result<(), Error> {
        Err(Error::unimplemented("NodeMutators::set_log_level"))
    }

    pub fn set_bans(&self, _bans: &[BanRequest]) -> Result<(), Error> {
        Err(Error::unimplemented("NodeMutators::set_bans"))
    }

    /// kB/s; `-1` restores the default, `0` leaves a side unchanged (§1b.3).
    pub fn set_limit(&self, _down_kbps: i64, _up_kbps: i64) -> Result<Limits, Error> {
        Err(Error::unimplemented("NodeMutators::set_limit"))
    }

    pub fn set_peers_count(
        &self,
        _out: Option<u32>,
        _inc: Option<u32>,
    ) -> Result<(u32, u32), Error> {
        Err(Error::unimplemented("NodeMutators::set_peers_count"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §2c cases, which hold from the 1a gate onwards because `new` is the gate.
    #[test]
    fn a_non_loopback_client_cannot_be_constructed() {
        for url in [
            "https://example.com",
            "http://10.0.0.5:18081",
            "http://example.com:18081",
        ] {
            let e = match NodeClient::new(url, None, Duration::from_secs(3)) {
                Err(e) => e,
                Ok(_) => panic!("{url} must be refused by the §2c gate"),
            };
            assert_eq!(e.exit_code(), 2, "{url}");
            assert_eq!(e.reason(), Some("non_loopback_url"), "{url}");
        }
        assert!(NodeClient::new("http://127.0.0.1:18081", None, Duration::from_secs(3)).is_ok());
    }

    #[test]
    fn the_two_ip_and_port_shapes_are_not_one_type() {
        // §1b.3: `connection_info.ip`/`.port` are strings, `peer.ip`/`.port` are numbers.
        // A single shape here fails to deserialize one of the two arrays of `node peers`.
        let c: Connection = serde_json::from_value(serde_json::json!({
            "ip": "3232235777", "port": "18080", "peer_id": "1234567890abcdef",
            "connection_id": "83bb47d6b8914f9a", "address": "1.2.3.4:18080", "height": 3541122
        }))
        .expect("connection_info uses strings");
        assert_eq!(c.port, "18080");
        assert_eq!(c.height, 3_541_122);
        let p: Peer = serde_json::from_value(serde_json::json!({
            "id": 7, "host": "1.2.3.4", "ip": 3232235777u32, "port": 18080, "last_seen": 1
        }))
        .expect("peer uses numbers");
        assert_eq!(p.port, 18080);
        assert_eq!(p.ip, 3_232_235_777);
    }

    #[test]
    fn a_sparse_daemon_response_still_deserializes() {
        // Every wire struct carries `#[serde(default)]`, so a field a future daemon drops
        // is a zero rather than a failed command.
        let i: GetInfo = serde_json::from_value(serde_json::json!({"height": 1})).unwrap();
        assert_eq!(i.height, 1);
        assert_eq!(i.target_height, 0);
        let f: FeeEstimate = serde_json::from_value(serde_json::json!({
            "fee": 20000, "fees": [20000, 80000, 320000, 4000000], "quantization_mask": 10000
        }))
        .unwrap();
        assert_eq!(f.fees.len(), 4);
        assert_eq!(f.fee, Piconero(20000));
    }
}
