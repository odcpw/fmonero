//! Shared HTTP plumbing for both daemons (DESIGN.md §2c, §5b).
//!
//! Phase 1a freezes the surface; Phase 1b writes `jsonrpc.rs`, fills `digest.rs` in and
//! gives `NodeClient`/`WalletClient` their bodies. **One body is implemented here and
//! now**: [`require_loopback`], because `Settings::set` — which Phase 1a owns and
//! implements in full — validates `node.url` and `wallet.url` through it, and a
//! `settings set node.url …` that exited 1 "not implemented" would make the §4 key table
//! unusable at the 1a gate. It touches no daemon, so it is testable here in the same way
//! `amount.rs` and `address.rs` are.

pub mod digest;
pub mod jsonrpc;
#[cfg(test)]
mod mock_tests;
pub mod node;
pub mod wallet;

use std::path::Path;

pub use node::{NodeClient, NodeMutators};
pub use wallet::WalletClient;

use crate::error::Error;

/// What a daemon answered, before it becomes an [`Error`].
///
/// `Busy` is the one that changes a rendering rather than a result: `rpc/jsonrpc.rs`
/// inspects the daemon's `status` field on every node response *before* deserializing the
/// payload, so a `CHECK_CORE_READY()`-gated call answering `"BUSY"` with a
/// value-initialised (all-zero) body becomes `null` in §5 and never a block 56 years old
/// (§1b.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcError {
    /// `status: "BUSY"` — the whole initial sync answers this.
    Busy,
    /// Any other non-`"OK"` `status`, verbatim.
    Status(String),
    /// A JSON-RPC `error` object.
    Rpc { code: i32, message: String },
    /// The daemon required a credential we do not have, or refused the one we sent.
    Unauthorized,
    /// The call was killed at its deadline (§2d). Distinct from [`RpcError::Transport`]
    /// because only this one arms a breaker and only this one is `killed_at_deadline`.
    Killed,
    /// A §2d breaker is armed for this daemon: `n` seconds of backoff left.
    Stalled(u64),
    /// A **legacy** endpoint answered HTTP 404, which is how epee reports a `!m_restricted`
    /// endpoint that was never registered (`http_protocol_handler.inl:664`, §1b.2). The
    /// json_rpc form of the same refusal is `Rpc { code: -32601 }`.
    NotRegistered(String),
    /// Nothing answered, or the answer was not usable.
    Transport(String),
}

/// The §2c loopback rule, in one place.
///
/// `Ok(())` only for scheme `http` with a **literal** host in `127.0.0.0/8` or `::1`, or a
/// `localhost` whose **every** resolved address is loopback — `localhost` is a name, not
/// an address, and an edited `/etc/hosts` or a name-service module can point it anywhere.
/// `NodeClient::new` and `WalletClient::new` each call it as their first statement, and
/// `settings set node.url` / `settings set wallet.url` validate through it. The wallet
/// client is the one that carries a credential and moves money, so exempting it would
/// leave the rule true only where it costs nothing.
pub fn require_loopback(url: &str) -> Result<(), Error> {
    let refuse = |why: &str| {
        Error::usage(format!("{url} is not a loopback http URL: {why}"))
            .with_reason("non_loopback_url")
            .with_hint(
                "fmonero wallet set-daemon <addr>   # a remote node is reached by monero-wallet-rpc, not by fmonero",
            )
    };
    let Some(rest) = url.strip_prefix("http://") else {
        return Err(refuse(
            "fmonero speaks plaintext http to loopback only, and carries no TLS stack",
        ));
    };
    // `host[:port]`, with an optional path we ignore here.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(refuse("it names no host"));
    }
    if authority.contains('@') {
        return Err(refuse(
            "credentials belong in the daemon's own login file (§2c)",
        ));
    }
    let host = if let Some(end) = authority.strip_prefix('[') {
        // [::1]:18081
        match end.split_once(']') {
            Some((h, _)) => h.to_string(),
            None => return Err(refuse("its IPv6 host is not bracketed")),
        }
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h.to_string())
            .unwrap_or_else(|| authority.to_string())
    };
    if host.is_empty() {
        return Err(refuse("it names no host"));
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return if ip.is_loopback() {
            Ok(())
        } else {
            Err(refuse("only 127.0.0.0/8 and ::1 are accepted"))
        };
    }
    if host.eq_ignore_ascii_case("localhost") {
        use std::net::ToSocketAddrs;
        let resolved: Vec<std::net::SocketAddr> = (host.as_str(), 0u16)
            .to_socket_addrs()
            .map_err(|e| refuse(&format!("'localhost' does not resolve ({e})")))?
            .collect();
        if resolved.is_empty() {
            return Err(refuse("'localhost' resolves to nothing"));
        }
        if resolved.iter().all(|a| a.ip().is_loopback()) {
            return Ok(());
        }
        return Err(refuse(
            "'localhost' resolves to an address that is not loopback",
        ));
    }
    Err(refuse(
        "only a literal loopback address (or localhost) is accepted",
    ))
}

/// The HTTP agent, behind a name of fmonero's own so that `scripts/check.sh` can grep for
/// `ureq::` outside this file and fail on **any** hit — an exception list is a grep that stops
/// protecting anything. The idle pool is one connection, because epee's auth session is per
/// connection (`http_protocol_handler.h:186`, §1b.7) and a second pooled connection would start
/// with no nonce.
pub type Agent = ureq::Agent;

/// The **one** place a `ureq::Agent` is constructed (§2d), with the deadline baked in.
pub fn agent(timeout: std::time::Duration) -> Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_idle_connections_per_host(1)
        .build()
        .into()
}

/// One HTTP answer, with everything the caller needs and **no `ureq` type in sight**.
pub(crate) struct HttpAnswer {
    pub status: u16,
    /// Every `WWW-Authenticate` header value, in order — a 401 from epee carries **two**
    /// (§1b.7), and `digest::challenge` picks the `MD5` one.
    pub challenges: Vec<String>,
    pub body: Vec<u8>,
}

/// Send one POST and read its answer. Together with [`agent`] this is the **only** place
/// `ureq` is named, which is what makes §2d's `grep -rn 'ureq::' crates/*/src` a real check
/// rather than one that has to list exceptions: no code path can issue a request without a
/// deadline, because there is no other code path.
///
/// The body is **always** read to completion, 401 included. `ureq` returns a connection to the
/// pool only once its body has been consumed, and epee's auth session is per connection
/// (`http_protocol_handler.h:186`): a retry on a fresh socket would present a nonce the server
/// never issued and fail forever, which looks exactly like a wrong password (§2c).
pub(crate) fn post_once(
    agent: &Agent,
    url: &str,
    authorization: Option<&str>,
    timeout: std::time::Duration,
    body: &[u8],
) -> Result<HttpAnswer, RpcError> {
    let mut req = agent
        .post(url)
        .config()
        // Per request, so one agent — and therefore one pooled connection and one server-side
        // Digest session — serves every deadline of §2d.
        .timeout_global(Some(timeout))
        .build()
        .header("Content-Type", "application/json");
    if let Some(value) = authorization {
        req = req.header("Authorization", value);
    }
    let mut resp = match req.send(body) {
        Ok(r) => r,
        // The one failure that arms a §2d breaker, and the only one that is `killed_at_deadline`.
        Err(ureq::Error::Timeout(_)) => return Err(RpcError::Killed),
        Err(e) => return Err(RpcError::Transport(e.to_string())),
    };
    let status = resp.status().as_u16();
    let challenges: Vec<String> = resp
        .headers()
        .get_all("www-authenticate")
        .into_iter()
        .filter_map(|v| v.to_str().ok().map(str::to_string))
        .collect();
    let body = resp
        .body_mut()
        .read_to_vec()
        .map_err(|e| RpcError::Transport(e.to_string()))?;
    Ok(HttpAnswer {
        status,
        challenges,
        body,
    })
}

/// `n` bytes from `/dev/urandom`, as lowercase hex.
///
/// The one randomness source in the crate, and it lives beside [`Login`] because both its
/// consumers are credentials: Digest's per-request `cnonce` (§1b.7, 16 bytes) and the node's
/// `rpc-login` token (§2c, 16 bytes → the 32 hex characters §4.2 specifies).
pub fn random_hex(n: usize) -> Result<String, Error> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| Error::failure(format!("cannot read /dev/urandom: {e}")))?;
    Ok(buf.iter().fold(String::with_capacity(n * 2), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    }))
}

/// A `user:pass` pair for HTTP Digest (§2c). fmonero never generates, writes or rotates
/// either daemon's credential: it reads them.
#[derive(Clone)]
pub struct Login {
    pub user: String,
    pub pass: String,
}

impl std::fmt::Debug for Login {
    /// Hand-written: the password never reaches a log line, a panic message or a debug dump
    /// (§2b.2).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("user", &self.user)
            .field("pass", &"<redacted>")
            .finish()
    }
}

impl Login {
    /// Read `user:pass` from `monero-wallet-rpc`'s own 0400 login file (§2c).
    /// `Ok(None)` when the file is absent, which means "the daemon is not running" —
    /// never an error, because no constructor may decide a money-path exit code (§2a.1).
    pub fn from_wallet_login_file(path: &Path) -> Result<Option<Login>, Error> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(
                    Error::backend(format!("cannot read {}: {e}", path.display()))
                        .with_reason("auth_refused")
                        .with_hint("systemctl --user restart fmonero-wallet-rpc.service"),
                )
            }
        };
        // `monero-wallet-rpc` writes exactly `<user>:<pass>` with no newline
        // (`wallet_rpc_server.cpp:351-380`); tolerate a trailing newline anyway.
        let line = text.trim_end_matches(['\n', '\r']);
        match line.split_once(':') {
            Some((user, pass)) if !user.is_empty() => Ok(Some(Login {
                user: user.to_string(),
                pass: pass.to_string(),
            })),
            // A file that exists but says nothing usable is not "the daemon is not running":
            // it is a credential we cannot use, and §2c's answer to that is exit 4.
            _ => Err(
                Error::backend(format!("{} is not user:pass", path.display()))
                    .with_reason("auth_refused")
                    .with_hint("systemctl --user restart fmonero-wallet-rpc.service"),
            ),
        }
    }

    /// Read `rpc-login=` from a monerod config file (§2c). `Ok(None)` when absent — the
    /// Arch `system_unit` layout has no credential at all.
    pub fn from_monerod_conf(path: &Path) -> Result<Option<Login>, Error> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(Error::failure(format!(
                    "cannot read {}: {e}",
                    path.display()
                )))
            }
        };
        Ok(Login::from_monerod_conf_text(&text))
    }

    /// The parsing half of [`Login::from_monerod_conf`], so `units::existing_or_new_node_login`
    /// can decide whether an existing line is valid without reading the file twice.
    pub(crate) fn from_monerod_conf_text(text: &str) -> Option<Login> {
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some(value) = line.strip_prefix("rpc-login") else {
                continue;
            };
            let value = value.trim_start().strip_prefix('=')?.trim();
            let (user, pass) = value.split_once(':')?;
            if user.is_empty() || pass.is_empty() {
                return None;
            }
            return Some(Login {
                user: user.to_string(),
                pass: pass.to_string(),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_urls_are_accepted() {
        for url in [
            "http://127.0.0.1:18081",
            "http://127.0.0.1:18083/json_rpc",
            "http://127.0.0.2:38081",
            "http://[::1]:18081",
            "http://localhost:18083",
            "http://LOCALHOST:18083",
        ] {
            require_loopback(url).unwrap_or_else(|e| panic!("{url} must be accepted: {e}"));
        }
    }

    #[test]
    fn everything_else_is_exit_two_with_the_non_loopback_reason() {
        for url in [
            "https://127.0.0.1:18081",
            "https://example.com",
            "http://10.0.0.5:18081",
            "http://example.com:18081",
            "http://[2001:db8::1]:18081",
            "http://user:pass@127.0.0.1:18081",
            "http://",
            "127.0.0.1:18081",
            "",
        ] {
            let e = require_loopback(url).unwrap_err();
            assert_eq!(e.exit_code(), 2, "{url}");
            assert_eq!(e.reason(), Some("non_loopback_url"), "{url}");
            assert!(e.hint().contains("set-daemon"), "{url}");
        }
    }

    #[test]
    fn the_agent_carries_a_global_deadline_and_one_idle_connection() {
        let a = agent(std::time::Duration::from_secs(3));
        assert_eq!(
            a.config().timeouts().global,
            Some(std::time::Duration::from_secs(3))
        );
        assert_eq!(a.config().max_idle_connections_per_host(), 1);
        assert!(!a.config().http_status_as_error());
    }

    #[test]
    fn the_wallet_login_file_is_read_and_an_absent_one_is_not_an_error() {
        let dir = std::env::temp_dir().join(format!("fmonero-login-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("monero-wallet-rpc.18083.login");
        // §2c: an absent file means "the daemon is not running", never a construction error.
        assert!(Login::from_wallet_login_file(&path).unwrap().is_none());
        // The daemon writes `<user>:<pass>` with no newline.
        std::fs::write(&path, b"monero:Zm9vYmFy").unwrap();
        let l = Login::from_wallet_login_file(&path).unwrap().unwrap();
        assert_eq!((l.user.as_str(), l.pass.as_str()), ("monero", "Zm9vYmFy"));
        std::fs::write(&path, b"fmonero:t3st\n").unwrap();
        let l = Login::from_wallet_login_file(&path).unwrap().unwrap();
        assert_eq!((l.user.as_str(), l.pass.as_str()), ("fmonero", "t3st"));
        // A file that exists but is unusable is exit 4 `auth_refused`, not "not running".
        std::fs::write(&path, b"nonsense").unwrap();
        let e = Login::from_wallet_login_file(&path).unwrap_err();
        assert_eq!(e.exit_code(), 4);
        assert_eq!(e.reason(), Some("auth_refused"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_node_credential_is_read_from_the_conf_and_a_bad_line_is_none() {
        let good = "data-dir=/home/x/.bitmonero\nrpc-login=fmonero:0123456789abcdef0123456789abcdef\nno-zmq=1\n";
        let l = Login::from_monerod_conf_text(good).expect("a valid line");
        assert_eq!(l.user, "fmonero");
        assert_eq!(l.pass.len(), 32);
        // A commented-out line is not a credential, and neither is a half-written one.
        assert!(Login::from_monerod_conf_text("# rpc-login=fmonero:dead").is_none());
        assert!(Login::from_monerod_conf_text("rpc-login=fmonero:").is_none());
        assert!(Login::from_monerod_conf_text("rpc-login=nocolon").is_none());
        // The Arch `system_unit` layout has no credential at all (§2i).
        assert!(Login::from_monerod_conf_text("data-dir=/var/lib/monero\n").is_none());
    }

    #[test]
    fn a_login_never_prints_its_password() {
        let l = Login {
            user: "fmonero".into(),
            pass: "not-a-real-password".into(),
        };
        let shown = format!("{l:?}");
        assert!(shown.contains("fmonero"));
        assert!(!shown.contains("not-a-real-password"), "{shown}");
    }
}
