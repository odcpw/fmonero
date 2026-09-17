//! The JSON-RPC 2.0 envelope, the legacy `POST /<name>` form, and the one HTTP transport
//! both clients share (DESIGN.md §1b.2, §1b.3, §2c, §2d).
//!
//! Three properties live here rather than in the two clients, because a property that lives
//! in two clients holds in one (§2a.3):
//!
//! * **`status` is inspected before the payload is deserialized.** Every daemon response
//!   carries `status` (`core_rpc_server_commands_defs.h:101-112`), and
//!   `CHECK_CORE_READY()` answers `"BUSY"` with a **value-initialised** body — every numeric
//!   field zero, no error object (`core_rpc_server.cpp:477`). That is the answer for the
//!   whole initial sync, so deserializing first would render a block 56 years old for days
//!   (§1b.3). `"BUSY"` becomes [`RpcError::Busy`]; any other non-`"OK"` value becomes
//!   [`RpcError::Status`] with the text verbatim.
//! * **One 401 retry, under one lock.** epee's auth session is per connection
//!   (`http_protocol_handler.h:186`), the agent keeps an idle pool of one, and the whole
//!   authorize → send → re-challenge → retry sequence holds the session mutex, so the TUI's
//!   several threads cannot race `nc` (§2c).
//! * **The §2d breaker is consulted at the first request, not at construction.** §2a.1
//!   requires steps 1–4 of the refusal order to run before any daemon is contacted, so no
//!   constructor may decide a money-path exit code; and `send::relay` must never be blocked
//!   by a breaker, which is why a transport that has already completed a request stops
//!   consulting it (a `Plan` can only be relayed on the client that built it).

use std::sync::Mutex;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::context::Context;
use crate::error::Error;
use crate::rpc::{digest, Login, RpcError};
use crate::state::{self, Daemon};

/// Which deadline a call gets. One agent serves them all — a second agent would be a second
/// pooled connection and therefore a second server-side Digest session (§2c) — and the
/// deadline is applied per request through `ureq`'s request-scoped config.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Deadline {
    /// The client's own probe deadline: `timeouts.node` (3 s) or `timeouts.wallet` (5 s).
    Probe,
    /// `timeouts.transfer` — a `transfer`/`sweep_all` build (120 s).
    Transfer,
    /// `timeouts.refresh` (120 s).
    Refresh,
    /// `timeouts.open` — `open_wallet` (300 s).
    Open,
    /// `timeouts.relay` — `relay_tx` (30 s), a fixed constant with no settings key.
    Relay,
}

/// What the transport remembers between requests. Behind one mutex, so the whole
/// authorize/send/retry sequence is atomic against other threads (§2c).
#[derive(Default)]
struct Live {
    session: Option<digest::Session>,
    /// What the **wire** did on the last completed request (§5b `authenticated()`).
    authenticated: Option<bool>,
    /// Set by the first request that came back 200. After that the §2d breaker is no longer
    /// consulted on this client, which is what keeps `send::relay` out of its reach (§2d).
    any_success: bool,
    /// Set by [`Transport::disable_breaker`], which `send::relay` calls as its first
    /// statement. §2d: "**`send::relay` likewise never consults a breaker**: once a plan is
    /// built the §2a.11 `send.lock` is held and the transaction exists, so exiting 5 there
    /// would abandon a built transaction rather than protect anything." `any_success` alone
    /// made that true only for a caller that happened to reuse the client that built the
    /// plan — the CLI does, the TUI builds a fresh client for `Action::Relay` — so the
    /// exemption lives on the transport and is switched on by `relay` itself, for every
    /// caller. Arming is unaffected: a `relay_tx` killed at its deadline still records the
    /// stall, it is only never *blocked* by one.
    breaker_off: bool,
}

/// One daemon's HTTP transport.
pub struct Transport {
    agent: crate::rpc::Agent,
    base: String,
    auth: Option<Login>,
    probe: Duration,
    transfer: Duration,
    refresh: Duration,
    open: Duration,
    relay: Duration,
    live: Mutex<Live>,
    who: Daemon,
    /// `None` for a client built by `NodeClient::new`/`WalletClient::new` (no `Context`, so
    /// no state directory) and for the clients `status` builds — which must never arm a
    /// breaker and are never blocked by one (§2d).
    breaker: Option<Context>,
}

impl Transport {
    /// `timeout` is the probe deadline; the long deadlines fall back to it, which is what a
    /// test's `WalletClient::new(url, auth, Duration::from_secs(2))` wants.
    pub fn new(url: &str, auth: Option<Login>, timeout: Duration, who: Daemon) -> Transport {
        Transport {
            agent: crate::rpc::agent(timeout),
            base: url.trim_end_matches('/').to_string(),
            auth,
            probe: timeout,
            transfer: timeout,
            refresh: timeout,
            open: timeout,
            relay: timeout,
            live: Mutex::new(Live::default()),
            who,
            breaker: None,
        }
    }

    /// The transport a real command uses: every §2d deadline from the context, and the
    /// breaker armed and consulted (§2d). `status` and the two `new` constructors do not
    /// take this path.
    pub(crate) fn from_ctx(
        ctx: &Context,
        url: &str,
        auth: Option<Login>,
        who: Daemon,
        breaker: bool,
    ) -> Transport {
        let t = &ctx.timeouts;
        let probe = match who {
            Daemon::Node => t.node,
            Daemon::Wallet => t.wallet,
        };
        // The agent's own global timeout is the LONGEST any call may take; each request then
        // lowers it to its own deadline. Built once per client (§2d).
        let longest = [probe, t.transfer, t.refresh, t.open, t.relay]
            .into_iter()
            .max()
            .unwrap_or(probe);
        Transport {
            agent: crate::rpc::agent(longest),
            base: url.trim_end_matches('/').to_string(),
            auth,
            probe,
            transfer: t.transfer,
            refresh: t.refresh,
            open: t.open,
            relay: t.relay,
            live: Mutex::new(Live::default()),
            who,
            breaker: breaker.then(|| ctx.clone()),
        }
    }

    fn deadline(&self, d: Deadline) -> Duration {
        match d {
            Deadline::Probe => self.probe,
            Deadline::Transfer => self.transfer,
            Deadline::Refresh => self.refresh,
            Deadline::Open => self.open,
            Deadline::Relay => self.relay,
        }
    }

    /// What the wire did on the last **completed** request (§5b).
    pub fn authenticated(&self) -> Option<bool> {
        self.live.lock().map(|l| l.authenticated).unwrap_or(None)
    }

    /// Whether this transport carries a credential at all — the doctor's `wallet_rpc_auth`
    /// and §2c's `auth_refused`/`unreachable` split read it.
    pub fn has_login(&self) -> bool {
        self.auth.is_some()
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Stop consulting the §2d breaker on this transport, for good. `send::relay` calls it as
    /// its first statement so that the exemption §2d states in words is a property of the
    /// relay itself rather than of whichever client the caller happened to pass (§2d).
    pub(crate) fn disable_breaker(&self) {
        match self.live.lock() {
            Ok(mut l) => l.breaker_off = true,
            Err(p) => p.into_inner().breaker_off = true,
        }
    }

    /// One POST, with at most one 401 retry. Returns the response body.
    fn post(&self, path: &str, body: &[u8], d: Deadline) -> Result<Vec<u8>, RpcError> {
        let mut live = match self.live.lock() {
            Ok(l) => l,
            // A poisoned mutex means another thread panicked mid-request; the session is
            // not trustworthy, so start a fresh one rather than propagating a panic.
            Err(p) => {
                let mut l = p.into_inner();
                l.session = None;
                l
            }
        };
        if !live.any_success && !live.breaker_off {
            if let Some(ctx) = &self.breaker {
                if !ctx.force {
                    if let Ok(Some(left)) = state::stall_remaining(ctx, self.who) {
                        return Err(RpcError::Stalled(left));
                    }
                }
            }
        }
        let url = format!("{}{path}", self.base);
        let timeout = self.deadline(d);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let authorization = match (&self.auth, live.session.as_mut()) {
                (Some(login), Some(session)) => {
                    Some(digest::authorization(session, login, "POST", path))
                }
                _ => None,
            };
            let sent_auth = authorization.is_some();
            let answer = match crate::rpc::post_once(
                &self.agent,
                &url,
                authorization.as_deref(),
                timeout,
                body,
            ) {
                Ok(a) => a,
                Err(RpcError::Killed) => {
                    drop(live);
                    self.arm_breaker();
                    return Err(RpcError::Killed);
                }
                Err(e) => return Err(e),
            };
            if answer.status == 401 {
                // A second 401 in a row is exit 4 (§2c): either we have no credential, or
                // the one we have is wrong.
                if attempt >= 2 || self.auth.is_none() {
                    live.authenticated = None;
                    return Err(RpcError::Unauthorized);
                }
                let refs: Vec<&str> = answer.challenges.iter().map(String::as_str).collect();
                match digest::challenge(&refs) {
                    Ok(s) => live.session = Some(s),
                    Err(_) => return Err(RpcError::Unauthorized),
                }
                continue;
            }
            if answer.status == 404 {
                // §1b.2: a gated **legacy** endpoint leaves `handled = false` and epee
                // answers 404 — the restricted-RPC answer for that transport.
                return Err(RpcError::NotRegistered(path.to_string()));
            }
            if answer.status != 200 {
                return Err(RpcError::Transport(format!(
                    "HTTP {} from {path}: {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                        .chars()
                        .take(200)
                        .collect::<String>()
                )));
            }
            live.authenticated = Some(sent_auth);
            live.any_success = true;
            return Ok(answer.body);
        }
    }

    /// Arm the §2d breaker for this daemon, unless this client is one `status` built.
    ///
    /// The wallet exemption of §2d is applied here: a wallet call killed at its deadline
    /// while the last observed `blocks_behind` is above [`crate::WALLET_BEHIND_BLOCKS`] is
    /// **`wallet_syncing`, not a stall**, because `monero-wallet-rpc` serves RPC on one
    /// thread and deliberately blocks it while it catches up (`wallet_rpc_server.cpp:211-280`).
    fn arm_breaker(&self) {
        let Some(ctx) = &self.breaker else { return };
        if self.who == Daemon::Wallet {
            // **In remote mode no wallet call killed at its deadline ever arms the breaker**
            // (v5.2, §2d, §2m.3). `blocks_behind` — the one discriminator this exemption has
            // between "catching up" and "stuck" — is `node.height - wallet.height`, and
            // `node.height` is `null` in remote mode because no wallet-RPC method reports the
            // daemon's height (§1b.9 item 5). The rule below already chooses "do not arm" for
            // the other case where `blocks_behind` is unknown (a wallet blocked inside
            // `open_wallet`), and the alternative here is a breaker that fires on the ordinary
            // first sync against a public node and then refuses every wallet command for 60 s —
            // the breaker an agent disables. The §2m.3 send gate's own deadline miss is exit 10
            // `wallet_catching_up`: the same instruction, with no 60 s penalty attached.
            if ctx.settings.node.mode.is_remote() {
                return;
            }
            let behind = state::load(ctx).ok().and_then(|s| s.wallet_behind);
            if behind.is_some_and(|b| b > crate::WALLET_BEHIND_BLOCKS) {
                return;
            }
        }
        let _ = state::note_stall(ctx, self.who);
    }

    /// `POST /json_rpc`, dispatching on `method`. The daemon's `status` is inspected before
    /// the payload is touched (§1b.3).
    pub fn rpc(&self, method: &str, params: Value, d: Deadline) -> Result<Value, RpcError> {
        let body = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0", "id": "0", "method": method, "params": params,
        }))
        .map_err(|e| RpcError::Transport(e.to_string()))?;
        let bytes = self.post("/json_rpc", &body, d)?;
        let envelope: Value = serde_json::from_slice(&bytes)
            .map_err(|e| RpcError::Transport(format!("{method}: unparsable answer: {e}")))?;
        if let Some(err) = envelope.get("error").filter(|e| !e.is_null()) {
            return Err(RpcError::Rpc {
                code: err.get("code").and_then(Value::as_i64).unwrap_or(0) as i32,
                message: err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        let result = envelope
            .get("result")
            .cloned()
            .ok_or_else(|| RpcError::Transport(format!("{method}: no result and no error")))?;
        check_status(&result)?;
        Ok(result)
    }

    /// The legacy `POST /<name>` form, whose body **is** the response — it inherits
    /// `rpc_response_base`, so `status` is inspected the same way (§1b.3).
    pub fn legacy(&self, name: &str, params: Value, d: Deadline) -> Result<Value, RpcError> {
        let body = serde_json::to_vec(&params).map_err(|e| RpcError::Transport(e.to_string()))?;
        let bytes = self.post(&format!("/{name}"), &body, d)?;
        let result: Value = serde_json::from_slice(&bytes)
            .map_err(|e| RpcError::Transport(format!("{name}: unparsable answer: {e}")))?;
        check_status(&result)?;
        Ok(result)
    }
}

/// **A `status` other than `"OK"` is not a result** (§1b.3). Absent is fine: the wallet RPC
/// has no `rpc_response_base` and none of its responses carries the key.
fn check_status(result: &Value) -> Result<(), RpcError> {
    match result.get("status").and_then(Value::as_str) {
        None | Some("OK") => Ok(()),
        Some("BUSY") => Err(RpcError::Busy),
        Some(other) => Err(RpcError::Status(other.to_string())),
    }
}

/// Deserialize a payload, turning a shape mismatch into a message that names the call.
pub fn typed<T: DeserializeOwned>(what: &str, v: Value) -> Result<T, Error> {
    serde_json::from_value(v)
        .map_err(|e| Error::failure(format!("{what}: the daemon's answer did not fit: {e}")))
}

/// Map a **node** [`RpcError`] to the §4a error it means (§1b.2, §2d).
pub fn node_error(what: &str, e: RpcError) -> Error {
    match e {
        RpcError::Rpc { code, message } => Error::from_node_rpc(code, &message),
        RpcError::NotRegistered(path) => Error::upstream(format!(
            "this monerod is running with restricted RPC, so {path} is not available"
        ))
        .with_reason("restricted")
        .with_hint("fmonero doctor --only node"),
        other => shared_error("monerod", what, other),
    }
}

/// Map a **wallet** [`RpcError`] through the §1b.4 code table.
pub fn wallet_error(what: &str, e: RpcError) -> Error {
    match e {
        RpcError::Rpc { code, message } => Error::from_wallet_rpc(code, &message),
        RpcError::NotRegistered(path) => {
            Error::upstream(format!("monero-wallet-rpc does not serve {path}"))
                .with_reason("rpc_error")
        }
        other => shared_error("monero-wallet-rpc", what, other),
    }
}

fn shared_error(daemon: &str, what: &str, e: RpcError) -> Error {
    match e {
        RpcError::Busy => Error::upstream(format!(
            "{daemon} answered BUSY to {what}: it is still synchronizing, and this call is gated until it is"
        ))
        .with_reason("rpc_error")
        .with_hint("fmonero status --json"),
        RpcError::Status(s) => {
            Error::upstream(format!("{daemon} answered {what} with status {s:?}"))
                .with_reason("rpc_error")
        }
        RpcError::Unauthorized => Error::backend(format!(
            "{daemon} refused the credential fmonero has for it"
        ))
        .with_reason("auth_refused")
        .with_hint("fmonero doctor --only security"),
        RpcError::Killed => Error::upstream(format!(
            "{daemon} did not answer {what} within its deadline"
        ))
        .with_reason("killed_at_deadline")
        .with_hint("fmonero doctor   # or re-run with --force"),
        RpcError::Stalled(left) => Error::upstream(format!(
            "{daemon} was killed at its deadline less than a minute ago; {left}s of backoff left"
        ))
        .with_reason("stalled")
        .with_hint("fmonero doctor   # or re-run with --force"),
        // The most frequently hit error path in the tool — every command against a stopped
        // daemon — and the only exit-4 reason that used to carry no hint of its own, so it fell
        // through to `Error::default_hint`'s `omarchy pkg add monero`: an install command for a
        // daemon that is installed and merely not running. §7.1 names the right one for each of
        // these two conditions (`node_reachable` → `fmonero node up`, `wallet_rpc_reachable` →
        // start the unit); `omarchy pkg add monero` belongs to `reason: not_installed` alone.
        RpcError::Transport(msg) => Error::backend(format!("{daemon} is not answering: {msg}"))
            .with_reason("unreachable")
            .with_hint(if daemon == "monerod" {
                "fmonero node up"
            } else {
                "fmonero wallet open <name>   # starts the unit on demand, then opens"
            }),
        RpcError::Rpc { code, message } => {
            Error::upstream(format!("{daemon}: {message} ({code})")).with_reason("rpc_error")
        }
        RpcError::NotRegistered(path) => {
            Error::upstream(format!("{daemon} does not serve {path}")).with_reason("rpc_error")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §1b.3's central rule, as a test on the one function that enforces it: a
    /// `CHECK_CORE_READY()`-gated call answers `"BUSY"` with an all-zero body, and that
    /// must **not** deserialize into a block header (which would render one 56 years old
    /// for the whole initial sync).
    #[test]
    fn a_busy_answer_is_not_a_result() {
        let busy = serde_json::json!({
            "block_header": {"timestamp": 0, "height": 0, "hash": ""},
            "status": "BUSY", "untrusted": false
        });
        assert_eq!(check_status(&busy), Err(RpcError::Busy));
        let ok = serde_json::json!({"height": 3541122, "status": "OK", "untrusted": false});
        assert_eq!(check_status(&ok), Ok(()));
        // The daemon's free-text error channel for handlers that answer 200 with no error
        // object — `set_limit`'s "Invalid parameter" is the example (§1b.3).
        assert_eq!(
            check_status(&serde_json::json!({"status": "Invalid parameter"})),
            Err(RpcError::Status("Invalid parameter".into()))
        );
        // A wallet response has no `status` at all.
        assert_eq!(check_status(&serde_json::json!({"height": 1})), Ok(()));
    }

    #[test]
    fn busy_maps_to_exit_five_and_never_to_a_zero() {
        let e = node_error("get_last_block_header", RpcError::Busy);
        assert_eq!(e.exit_code(), 5);
        assert_eq!(e.reason(), Some("rpc_error"));
        assert!(e.to_string().contains("BUSY"), "{e}");
    }

    #[test]
    fn a_gated_legacy_endpoint_is_the_restricted_answer() {
        let e = node_error(
            "get_peer_list",
            RpcError::NotRegistered("/get_peer_list".into()),
        );
        assert_eq!(e.exit_code(), 5);
        assert_eq!(e.reason(), Some("restricted"));
        // And so is a gated json_rpc method, which arrives as -32601 instead.
        let e = node_error(
            "sync_info",
            RpcError::Rpc {
                code: -32601,
                message: "Method not found".into(),
            },
        );
        assert_eq!(e.reason(), Some("restricted"));
    }

    #[test]
    fn the_transport_error_split_is_the_one_section_two_c_promises() {
        let unreachable = wallet_error("get_height", RpcError::Transport("refused".into()));
        assert_eq!(unreachable.exit_code(), 4);
        assert_eq!(unreachable.reason(), Some("unreachable"));
        let refused = wallet_error("get_height", RpcError::Unauthorized);
        assert_eq!(refused.exit_code(), 4);
        assert_eq!(refused.reason(), Some("auth_refused"));
        let killed = wallet_error("open_wallet", RpcError::Killed);
        assert_eq!(killed.exit_code(), 5);
        assert_eq!(killed.reason(), Some("killed_at_deadline"));
        let stalled = wallet_error("get_balance", RpcError::Stalled(42));
        assert_eq!(stalled.exit_code(), 5);
        assert_eq!(stalled.reason(), Some("stalled"));
        assert!(stalled.hint().contains("--force"), "{}", stalled.hint());
    }

    /// The §1b.4 map, one case per mapped code, through the function the clients call.
    #[test]
    fn the_wallet_error_map_is_the_one_from_source() {
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
            (-20, 2),
            (-21, 3),
            (-23, 4),
            (-26, 1),
            (-29, 3),
            (-37, 14),
            (-38, 4),
            (-48, 4),
            (-50, 1),
            (-51, 9),
            (-1, 9),
            (0, 5),
            (-999, 5),
        ] {
            let e = wallet_error(
                "transfer",
                RpcError::Rpc {
                    code,
                    message: "boom".into(),
                },
            );
            assert_eq!(e.exit_code(), exit, "wallet-rpc {code} must exit {exit}");
        }
        // epee's catch-all must never report an empty message (§2j).
        let e = wallet_error(
            "get_transfers",
            RpcError::Rpc {
                code: 0,
                message: String::new(),
            },
        );
        assert!(e.to_string().contains("cannot reach the daemon"), "{e}");
    }
}
