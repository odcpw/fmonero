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
///
/// The message is the user's (§2o) and the serde text — which names Rust types and JSON paths —
/// is the `detail` an agent and `-v` get.
pub fn typed<T: DeserializeOwned>(what: &str, v: Value) -> Result<T, Error> {
    serde_json::from_value(v).map_err(|e| {
        Error::failure("fmonero could not read the answer it was given.")
            .with_detail(format!("{what}: {e}"))
    })
}

/// Which side of the wire an error came from, in the **user's** words (§2o.2): `monerod` is
/// *the node*, `monero-wallet-rpc` is *the wallet service*, and neither binary is ever named in
/// a sentence a person reads.
///
/// The enum exists because the message, the hint and the in-app action all differ between the
/// two, and passing the binary name as a string — which is what v5.5 did — is how `monerod` and
/// `monero-wallet-rpc` got into the TUI's status bar, the CLI's stderr and the bar widget's
/// panel from one function (DECISIONS, V8).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Side {
    Node,
    Wallet,
}

impl Side {
    /// The subject of every sentence below, capitalised because it starts one.
    fn subject(self) -> &'static str {
        match self {
            Side::Node => "The node",
            Side::Wallet => "The wallet service",
        }
    }

    /// The CLI's runnable command (§9, §2o's exemption for the CLI's own hints).
    fn hint(self) -> &'static str {
        match self {
            Side::Node => "fmonero node up",
            Side::Wallet => "fmonero wallet open <name>   # starts the service, then opens",
        }
    }

    /// The in-app route, for the TUI and the widget (§2o.1). Both are keys on the screen the
    /// user is already looking at.
    fn action(self) -> &'static str {
        match self {
            Side::Node => "Press u on the Node screen to start your node.",
            Side::Wallet => "Press o on the Wallet screen to open a wallet.",
        }
    }
}

/// Map a **node** [`RpcError`] to the §4a error it means (§1b.2, §2d).
pub fn node_error(what: &str, e: RpcError) -> Error {
    match e {
        RpcError::Rpc { code, message } => Error::from_node_rpc(code, &message),
        // §2o.2: `restricted` is never the word. What it means to the user is that this node
        // answers the basic questions and not the rest, and the path it refused is the detail.
        RpcError::NotRegistered(path) => {
            Error::upstream("This node only answers basic questions, so it cannot answer that.")
                .with_reason("restricted")
                .with_detail(path)
                .with_hint("fmonero doctor --only node")
                .with_action("Press f on the Node screen to choose another node.")
        }
        other => shared_error(Side::Node, what, other),
    }
}

/// Map a **wallet** [`RpcError`] through the §1b.4 code table.
pub fn wallet_error(what: &str, e: RpcError) -> Error {
    match e {
        RpcError::Rpc { code, message } => Error::from_wallet_rpc(code, &message),
        RpcError::NotRegistered(path) => Error::upstream("The wallet service cannot answer that.")
            .with_reason("rpc_error")
            .with_detail(path),
        other => shared_error(Side::Wallet, what, other),
    }
}

/// The five arms both clients share — **rewritten in v5.6, because this one function was the
/// biggest single violation of §2o in the program** (DECISIONS, V8).
///
/// It used to interpolate the binary name (`monerod`, `monero-wallet-rpc`), the RPC method
/// (`get_info`) and the raw transport text into one string, and that string reached all three
/// surfaces at once: the TUI's status bar, the CLI's stderr and — through
/// `status.last_error.text` and a failed action's stderr — the bar widget's panel. The owner's
/// complaint, verbatim, was *"most of the text in the TUI is slop"*, and
/// `monero-wallet-rpc is not answering: io: Connection refused (os error 111)` is the sentence
/// that earned it. Every arm now says what happened in one short sentence, names the in-app key
/// that fixes it, and puts the wire's own words in `detail`, where `--json` and `-v` keep them
/// verbatim and nothing a person reads carries them.
fn shared_error(side: Side, what: &str, e: RpcError) -> Error {
    let subject = side.subject();
    match e {
        // §2o.2: "synced" / "catching up", never "synchronizing", and BUSY is a wire value.
        RpcError::Busy => Error::upstream(format!(
            "{subject} is still catching up, so it cannot answer that yet."
        ))
        .with_reason("rpc_error")
        .with_detail(format!("{what}: status BUSY"))
        .with_hint("fmonero status --json")
        .with_action("The Node screen shows how far along it is."),
        RpcError::Status(s) => Error::upstream(format!("{subject} refused that request."))
            .with_reason("rpc_error")
            .with_detail(format!("{what}: status {s:?}")),
        // "credential" is the program's word for it; the user's is password.
        RpcError::Unauthorized => Error::backend(format!(
            "{subject} did not accept the password fmonero uses for it."
        ))
        .with_reason("auth_refused")
        .with_detail(format!("{what}: 401 after one retry"))
        .with_hint("fmonero doctor --only security"),
        RpcError::Killed => Error::upstream(format!("{subject} did not answer in time."))
            .with_reason("killed_at_deadline")
            .with_detail(format!("{what}: killed at its deadline"))
            .with_hint("fmonero doctor   # or re-run with --force"),
        // §2o rule 7 forbids "breaker", which is what this is: the §2d backoff after a kill.
        RpcError::Stalled(left) => Error::upstream(format!(
            "{subject} was too slow a moment ago. fmonero will try again in {left}s."
        ))
        .with_reason("stalled")
        .with_detail(format!("{what}: {left}s of backoff left"))
        .with_hint("fmonero doctor   # or re-run with --force"),
        // The most frequently hit error path in the tool — every command against a stopped
        // service — and the one V8 quotes. The hint is per side because §7.1 names a different
        // repair for each (`node_reachable` → `fmonero node up`, `wallet_rpc_reachable` → open a
        // wallet, which starts the service on demand); `omarchy pkg add monero` belongs to
        // `reason: not_installed` alone and used to answer here by falling through to the
        // default hint.
        RpcError::Transport(msg) => Error::backend(format!("{subject} is not answering."))
            .with_reason("unreachable")
            .with_detail(format!("{what}: {msg}"))
            .with_hint(side.hint())
            .with_action(side.action()),
        RpcError::Rpc { code, message } => {
            Error::upstream(format!("{subject} reported a problem."))
                .with_reason("rpc_error")
                .with_detail(format!("{what}: {message} ({code})"))
        }
        RpcError::NotRegistered(path) => Error::upstream(format!("{subject} cannot answer that."))
            .with_reason("rpc_error")
            .with_detail(format!("{what}: {path}")),
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

    /// v5.6, §2o: the **message** is the user's ("still catching up") and the wire's own word
    /// is in `detail`, where `--json` and `-v` keep it. The exit code and reason are unchanged,
    /// which is the half agents branch on.
    #[test]
    fn busy_maps_to_exit_five_and_never_to_a_zero() {
        let e = node_error("get_last_block_header", RpcError::Busy);
        assert_eq!(e.exit_code(), 5);
        assert_eq!(e.reason(), Some("rpc_error"));
        assert!(e.to_string().contains("still catching up"), "{e}");
        assert!(!e.to_string().contains("BUSY"), "{e}");
        let detail = e.detail().unwrap_or_default();
        assert!(detail.contains("BUSY"), "{detail}");
        assert!(detail.contains("get_last_block_header"), "{detail}");
    }

    /// **V8, as a test on the whole family.** Until v5.6 this function interpolated the binary
    /// name, the RPC method and the transport's own words into one sentence, and that sentence
    /// reached the TUI's status bar, the CLI's stderr and the bar widget's panel:
    /// `monero-wallet-rpc is not answering: io: Connection refused (os error 111)`. Every arm,
    /// both sides: nothing a person reads may carry an implementation name, a method name or a
    /// transport string, and every one of those has to survive in `detail`.
    #[test]
    fn no_arm_of_the_shared_family_says_a_binary_name_a_method_or_a_transport_string() {
        let arms = || {
            vec![
                RpcError::Busy,
                RpcError::Status("Invalid parameter".into()),
                RpcError::Unauthorized,
                RpcError::Killed,
                RpcError::Stalled(42),
                RpcError::Transport("io: Connection refused (os error 111)".into()),
                RpcError::NotRegistered("/get_peer_list".into()),
                RpcError::Rpc {
                    code: -32601,
                    message: "Method not found".into(),
                },
            ]
        };
        for (side, expect) in [
            (Side::Node, "The node"),
            (Side::Wallet, "The wallet service"),
        ] {
            for e in arms() {
                let err = shared_error(side, "get_info", e.clone());
                let text = err.to_string();
                for banned in [
                    "monerod",
                    "monero-wallet-rpc",
                    "get_info",
                    "daemon",
                    "unit",
                    "RPC",
                    "os error",
                    "BUSY",
                ] {
                    assert!(
                        !text.contains(banned),
                        "{e:?} on {side:?} still says {banned:?}: {text}"
                    );
                }
                assert!(text.starts_with(expect), "{e:?} on {side:?}: {text}");
                assert!(text.ends_with('.'), "one sentence, one period: {text}");
                // The wire's own words are kept, verbatim, where only `--json` and `-v` look.
                let detail = err.detail().unwrap_or_default();
                assert!(
                    detail.starts_with("get_info: "),
                    "{e:?}: the detail must name the call: {detail}"
                );
            }
        }
        // And the one the owner met carries the in-app route as well as the CLI's command.
        let wallet = shared_error(
            Side::Wallet,
            "get_balance",
            RpcError::Transport("io".into()),
        );
        assert_eq!(
            wallet.to_string(),
            "The wallet service is not answering.",
            "the string V8 quotes, rewritten"
        );
        assert_eq!(
            wallet.action().as_deref(),
            Some("Press o on the Wallet screen to open a wallet.")
        );
        assert!(wallet.hint().starts_with("fmonero wallet open"), "{wallet}");
        let node = shared_error(Side::Node, "get_info", RpcError::Transport("io".into()));
        assert_eq!(node.to_string(), "The node is not answering.");
        assert_eq!(
            node.action().as_deref(),
            Some("Press u on the Node screen to start your node.")
        );
        assert_eq!(node.hint(), "fmonero node up");
    }

    /// The other two entry points, which have their own arms before the shared ones: a gated
    /// method on a node that only answers basic questions, and a path the wallet service does
    /// not serve. Neither may name the path in the sentence (§2o rule 5).
    #[test]
    fn the_two_not_registered_arms_are_plain_and_keep_the_path_in_detail() {
        let e = node_error(
            "get_peer_list",
            RpcError::NotRegistered("/get_peer_list".into()),
        );
        assert_eq!(
            e.to_string(),
            "This node only answers basic questions, so it cannot answer that."
        );
        assert_eq!(e.reason(), Some("restricted"));
        assert_eq!(e.detail().as_deref(), Some("/get_peer_list"));
        let e = wallet_error(
            "get_accounts",
            RpcError::NotRegistered("/get_accounts".into()),
        );
        assert_eq!(e.to_string(), "The wallet service cannot answer that.");
        assert_eq!(e.detail().as_deref(), Some("/get_accounts"));
    }

    /// `typed`'s message named the daemon and pasted serde's own text at the user; the text is
    /// what an agent needs and `detail` is where it belongs.
    #[test]
    fn a_shape_mismatch_is_plain_and_keeps_serdes_words_in_detail() {
        let e = typed::<u64>("get_height", serde_json::json!({"height": "not a number"}))
            .expect_err("a string is not a u64");
        assert_eq!(
            e.to_string(),
            "fmonero could not read the answer it was given."
        );
        let detail = e.detail().unwrap_or_default();
        assert!(detail.starts_with("get_height: "), "{detail}");
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
        assert!(e.to_string().contains("cannot reach the node"), "{e}");
    }
}
