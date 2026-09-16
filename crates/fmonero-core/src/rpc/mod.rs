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

/// The **one** place a `ureq::Agent` is constructed (§2d), with the deadline baked in, so
/// there is no code path that can issue a request without one — `scripts/check.sh` greps
/// for `ureq::` outside this file and fails on any other hit. The idle pool is one
/// connection, because epee's auth session is per connection (`http_protocol_handler.h:186`,
/// §1b.7) and a second pooled connection would start with no nonce.
pub fn agent(timeout: std::time::Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_idle_connections_per_host(1)
        .build()
        .into()
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
    pub fn from_wallet_login_file(_path: &Path) -> Result<Option<Login>, Error> {
        Err(Error::unimplemented("rpc::Login::from_wallet_login_file"))
    }

    /// Read `rpc-login=` from a monerod config file (§2c). `Ok(None)` when absent — the
    /// Arch `system_unit` layout has no credential at all.
    pub fn from_monerod_conf(_path: &Path) -> Result<Option<Login>, Error> {
        Err(Error::unimplemented("rpc::Login::from_monerod_conf"))
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
