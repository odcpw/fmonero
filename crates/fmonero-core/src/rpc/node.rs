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
use crate::rpc::jsonrpc::{self, Deadline, Transport};
use crate::rpc::Login;
use crate::state::Daemon;

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
    t: Transport,
}

impl NodeClient {
    /// Rejects any non-http scheme and any non-loopback host with `Error::Usage`, through
    /// `rpc::require_loopback` — the same function `WalletClient::new` calls (§2c). That
    /// check is this constructor's **first statement**.
    ///
    /// A client built this way consults and arms **no** §2d breaker: it has no `Context`,
    /// so it has no state directory. `from_ctx` is the one that does.
    pub fn new(url: &str, auth: Option<Login>, timeout: Duration) -> Result<NodeClient, Error> {
        crate::rpc::require_loopback(url)?;
        Ok(NodeClient {
            t: Transport::new(url, auth, timeout, Daemon::Node),
        })
    }

    /// The client `settings.node.url` and `timeouts.node` describe, with the credential
    /// from `~/.config/fmonero/monerod.conf` when there is one (§2c). A node with no
    /// credentials still works, unauthenticated: Arch's system unit has none.
    ///
    /// **This is the remote-mode gate, in one place** (v5.2, §2m.3): with
    /// `settings.node.mode == remote` it returns `Error::Backend{reason:"remote_node"}` with
    /// `hint: fmonero node local --yes`, **before constructing anything**. `node info`,
    /// `node peers`, `node bans`, `node ban`, `node unban` and `node limit` reach the daemon
    /// only through this constructor, so none of them needs a check of its own
    /// (`node info` takes a settings-only path in remote mode and never calls it);
    /// `node up|down|restart` meet the same error in `node_session`'s `Layout::Remote` arm.
    /// A per-command check would be the arrangement §2a.3 rejects, on nine commands across
    /// three phases — and it is also what keeps §2c's "fmonero never speaks to a remote node"
    /// enforceable rather than merely stated.
    pub fn from_ctx(ctx: &Context) -> Result<NodeClient, Error> {
        Self::build(ctx, true)
    }

    /// The client `status` uses: **it never arms a §2d breaker and is never blocked by one**
    /// (§2d). Crate-private, so the frozen API is unchanged and no other caller can pick it.
    pub(crate) fn for_status(ctx: &Context) -> Result<NodeClient, Error> {
        Self::build(ctx, false)
    }

    fn build(ctx: &Context, breaker: bool) -> Result<NodeClient, Error> {
        // The §2m.3 gate, before anything is constructed: in remote mode there is no local
        // daemon and fmonero's own `ureq` agent must never be handed a public address (§2c).
        if ctx.settings.node.mode.is_remote() {
            return Err(crate::units::remote_node_error(ctx, None));
        }
        let url = &ctx.settings.node.url;
        crate::rpc::require_loopback(url)?;
        let auth = Login::from_monerod_conf(&ctx.paths.monerod_conf())?;
        Ok(NodeClient {
            t: Transport::from_ctx(ctx, url, auth, Daemon::Node, breaker),
        })
    }

    fn rpc<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, Error> {
        let v = self
            .t
            .rpc(method, params, Deadline::Probe)
            .map_err(|e| jsonrpc::node_error(method, e))?;
        jsonrpc::typed(method, v)
    }

    fn legacy<T: serde::de::DeserializeOwned>(
        &self,
        name: &str,
        params: serde_json::Value,
    ) -> Result<T, Error> {
        let v = self
            .t
            .legacy(name, params, Deadline::Probe)
            .map_err(|e| jsonrpc::node_error(name, e))?;
        jsonrpc::typed(name, v)
    }

    pub fn get_info(&self) -> Result<GetInfo, Error> {
        self.rpc("get_info", serde_json::json!({}))
    }

    pub fn get_last_block_header(&self) -> Result<BlockHeader, Error> {
        #[derive(Deserialize)]
        struct Wrap {
            block_header: BlockHeader,
        }
        // `fill_pow_hash` stays false: it is expensive (§1b.3).
        let w: Wrap = self.rpc(
            "get_last_block_header",
            serde_json::json!({"fill_pow_hash": false}),
        )?;
        Ok(w.block_header)
    }

    /// `version: 0` asks about the **next** fork for the voting fields and about the
    /// **current** one for `version` — the opposite of "0 = the current one" (§1b.3).
    pub fn hard_fork_info(&self, version: u8) -> Result<HardForkInfo, Error> {
        self.rpc("hard_fork_info", serde_json::json!({"version": version}))
    }

    pub fn get_version(&self) -> Result<NodeVersion, Error> {
        self.rpc("get_version", serde_json::json!({}))
    }

    /// `grace_blocks` is **always 10**, the `FEE_ESTIMATE_GRACE_BLOCKS` wallet2 itself uses
    /// (`wallet2.cpp:123`, applied at `:8597`), so the chips fmonero draws are computed
    /// exactly as the fee the build will charge (§1b.3).
    pub fn get_fee_estimate(&self, grace_blocks: u64) -> Result<FeeEstimate, Error> {
        self.rpc(
            "get_fee_estimate",
            serde_json::json!({"grace_blocks": grace_blocks}),
        )
    }

    pub fn get_txpool_stats(&self) -> Result<TxpoolStats, Error> {
        #[derive(Deserialize)]
        struct Wrap {
            pool_stats: TxpoolStats,
        }
        // The wire name is `get_transaction_pool_stats` on BOTH transports (§1b.3); the
        // legacy endpoint is the open one, so that is the one fmonero uses.
        let w: Wrap = self.legacy("get_transaction_pool_stats", serde_json::json!({}))?;
        Ok(w.pool_stats)
    }

    /// `Upstream(-32601)` on a restricted node (§1b.2).
    pub fn sync_info(&self) -> Result<SyncInfo, Error> {
        self.rpc("sync_info", serde_json::json!({}))
    }

    pub fn get_connections(&self) -> Result<Vec<Connection>, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wrap {
            connections: Vec<Connection>,
        }
        let w: Wrap = self.rpc("get_connections", serde_json::json!({}))?;
        Ok(w.connections)
    }

    pub fn get_peer_list(&self, public_only: bool) -> Result<PeerLists, Error> {
        self.legacy(
            "get_peer_list",
            serde_json::json!({"public_only": public_only, "include_blocked": false}),
        )
    }

    pub fn get_bans(&self) -> Result<Vec<Ban>, Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Wrap {
            bans: Vec<Ban>,
        }
        let w: Wrap = self.rpc("get_bans", serde_json::json!({}))?;
        Ok(w.bans)
    }

    pub fn get_net_stats(&self) -> Result<NetStats, Error> {
        self.legacy("get_net_stats", serde_json::json!({}))
    }

    pub fn get_limit(&self) -> Result<Limits, Error> {
        self.legacy("get_limit", serde_json::json!({}))
    }

    /// Read-only: always sends `set: false` (§1b.3). The writing form lives on
    /// [`NodeMutators`]. Returns `(out_peers, in_peers)`.
    pub fn peers_count(&self) -> Result<(u32, u32), Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Count {
            out_peers: u32,
            in_peers: u32,
        }
        let out: Count = self.legacy(
            "out_peers",
            serde_json::json!({"set": false, "out_peers": 0u32}),
        )?;
        let inc: Count = self.legacy(
            "in_peers",
            serde_json::json!({"set": false, "in_peers": 0u32}),
        )?;
        Ok((out.out_peers, inc.in_peers))
    }

    /// One call, classifying the answer: `Ok(false)` = unrestricted, `Ok(true)` = the
    /// `-32601` of a gated `sync_info` (§1b.2). The cheapest unambiguous probe there is.
    pub fn is_restricted(&self) -> Result<bool, Error> {
        match self
            .t
            .rpc("sync_info", serde_json::json!({}), Deadline::Probe)
        {
            Ok(_) => Ok(false),
            Err(crate::rpc::RpcError::Rpc { code: -32601, .. }) => Ok(true),
            Err(e) => Err(jsonrpc::node_error("sync_info", e)),
        }
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
        self.t.authenticated()
    }

    /// **The §2i layout gate, as a type rather than as a comment.** `Error::Safety` with
    /// the `sudo systemctl …` hint unless `units::detect_layout(ctx) == Layout::UserUnit`.
    /// There is no `--force` override.
    pub fn mutators<'a>(&'a self, ctx: &Context) -> Result<NodeMutators<'a>, Error> {
        if crate::units::may_mutate_node(ctx)? {
            return Ok(NodeMutators { client: self });
        }
        // Layout::None is its own answer, and this is a deviation from §5b's one-sentence
        // description of this method ("Returns `Error::Safety` … unless UserUnit"), recorded in
        // DECISIONS.md by Phase 3's ergonomics pass. §2i's own layout table gives `none` "exits
        // **4 `backend`**: hint: `fmonero setup --units`", which is what `node_session::up` and
        // `down` already answer — with `--yes`, because `fmonero setup --units` alone exits 3
        // `needs_yes`; collapsing it into the `system_unit` refusal told a user with
        // no monerod installed at all — this machine, and every fresh one — to run
        // `sudo systemctl <verb> monerod` about a unit that does not exist, and called it
        // `foreign_node` when there is no node. `system_unit` and `remote`, the two layouts §2i
        // names for this refusal, keep exit 3 `foreign_node` verbatim.
        if crate::units::detect_layout(ctx)? == crate::units::Layout::None {
            return Err(Error::backend(
                "fmonero has not set your node up yet, so there is no node of yours to change.",
            )
            .with_reason("unit_missing")
            .with_hint("fmonero setup --units --yes"));
        }
        Err(Error::safety(
            "The node on this machine is not the one fmonero set up, so fmonero will not change it.",
        )
        .with_reason("foreign_node")
        .with_hint("sudo systemctl <verb> monerod   # whoever set that node up is who may change it"))
    }
}

/// The five mutating node calls, reachable only through [`NodeClient::mutators`] (§2i).
pub struct NodeMutators<'a> {
    client: &'a NodeClient,
}

impl NodeMutators<'_> {
    pub fn stop_daemon(&self) -> Result<(), Error> {
        // The answer is the bare `{status, untrusted}` base; `check_status` has already
        // vetted it, so there is nothing to deserialize.
        let _: serde_json::Value = self.client.legacy("stop_daemon", serde_json::json!({}))?;
        Ok(())
    }

    pub fn set_log_level(&self, level: i8) -> Result<(), Error> {
        let _: serde_json::Value = self
            .client
            .legacy("set_log_level", serde_json::json!({"level": level}))?;
        Ok(())
    }

    pub fn set_bans(&self, bans: &[BanRequest]) -> Result<(), Error> {
        let _: serde_json::Value = self
            .client
            .rpc("set_bans", serde_json::json!({"bans": bans}))?;
        Ok(())
    }

    /// kB/s; `-1` restores the default, `0` leaves a side unchanged (§1b.3).
    pub fn set_limit(&self, down_kbps: i64, up_kbps: i64) -> Result<Limits, Error> {
        self.client.legacy(
            "set_limit",
            serde_json::json!({"limit_down": down_kbps, "limit_up": up_kbps}),
        )
    }

    /// `None` leaves that side alone: the read form (`set: false`) is asked for instead, so
    /// `node limit --out-peers 16` does not silently rewrite `in_peers` (§1b.3).
    pub fn set_peers_count(&self, out: Option<u32>, inc: Option<u32>) -> Result<(u32, u32), Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Count {
            out_peers: u32,
            in_peers: u32,
        }
        let o: Count = self.client.legacy(
            "out_peers",
            serde_json::json!({"set": out.is_some(), "out_peers": out.unwrap_or(0)}),
        )?;
        let i: Count = self.client.legacy(
            "in_peers",
            serde_json::json!({"set": inc.is_some(), "in_peers": inc.unwrap_or(0)}),
        )?;
        Ok((o.out_peers, i.in_peers))
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
