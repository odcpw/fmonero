//! MD5 HTTP Digest, the only scheme epee speaks (DESIGN.md §1b.7, §2c).
//!
//! **Signature-only at the Phase 1a gate** (§8): Phase 1b writes the bodies against the
//! four facts of §1b.7, which are restated here so they cannot be lost between phases.
//! Nothing in this module is reachable from a shipped command until then — every caller
//! is a `NodeClient`/`WalletClient` method whose own body returns
//! `Error::unimplemented` (§2h's sanctioned handover).

use crate::error::Error;
use crate::rpc::Login;

/// One connection's Digest session.
///
/// Carries no `opaque`: epee's server never sends one (`http_auth.cpp:676-683`). `nc`
/// belongs to `nonce` and is **reset to 1 whenever `nonce` changes**, because the server
/// zeroes its own counter every time it issues a nonce and then requires equality
/// (`http_auth.cpp:734`, `:749`, `:335-345`). A monotonic counter that is not reset costs
/// a `stale` 401 plus a retry on every request — two round trips forever, silently
/// doubling the §2l budget. The session lives behind the client's `Mutex` (§2c).
#[derive(Clone, Debug, Default)]
pub struct Session {
    realm: String,
    nonce: String,
    qop: String,
    nc: u32,
}

impl Session {
    /// The nonce this session is counting against, for tests and diagnostics.
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
    /// The realm the server advertised.
    pub fn realm(&self) -> &str {
        &self.realm
    }
    /// `qop`, always `auth` against `monerod`/`monero-wallet-rpc`: the challenge is built
    /// from `fvalue = "Digest qop=\"auth\""` unconditionally (`http_auth.cpp:671`), so the
    /// RFC 2069 no-`qop` branch is server-side dead code and fmonero does not implement
    /// it (§2h).
    pub fn qop(&self) -> &str {
        &self.qop
    }
    /// The count the next request will carry.
    pub fn next_nc(&self) -> u32 {
        self.nc + 1
    }
}

/// Build a session from a 401's challenges.
///
/// Takes **every** `WWW-Authenticate` header of the response, in order, and selects the one
/// whose `algorithm` is exactly `MD5` (case-insensitive) or absent. A 401 from epee carries
/// **two** — `algorithm=MD5` and `algorithm=MD5-sess`, emitted by a
/// `for (unsigned i = 0; i < 2; ++i)` loop (`http_auth.cpp:665-691`) — and `MD5-sess` is a
/// *different* HA1 (`key = md5(key:nonce:cnonce)`, `:548-551`), so a client that reads "the"
/// header and lands on the second one fails every request.
pub fn challenge(_www_authenticate: &[&str]) -> Result<Session, Error> {
    Err(Error::unimplemented("rpc::digest::challenge"))
}

/// The `Authorization` header value for one request.
///
/// `HA1 = md5(user:realm:pass)`, `HA2 = md5(method:uri)` where `uri` is the request path
/// exactly as sent (`/json_rpc`, `/get_info`, …), and
/// `response = md5(HA1:nonce:nc:cnonce:auth:HA2)` under `qop=auth`. A fresh 128-bit
/// `cnonce` per request; `nc` as 8 lowercase hex digits, unquoted; `cnonce` quoted.
///
/// Phase 1b writes the body (§8). Until then this returns an empty header rather than a
/// forged one: `NodeClient`/`WalletClient` cannot reach it, because their own bodies
/// return `Error::unimplemented` first.
pub fn authorization(_s: &mut Session, _login: &Login, _method: &str, _uri: &str) -> String {
    String::new()
}
