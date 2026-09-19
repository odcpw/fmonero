//! MD5 HTTP Digest, the only scheme epee speaks (DESIGN.md §1b.7, §2c).
//!
//! Written against the four facts of §1b.7, which are restated in the doc comments because
//! each of them is a way to get this wrong that still *looks* like a client bug:
//!
//! 1. a 401 carries **two** `WWW-Authenticate` headers, `algorithm=MD5` and `MD5-sess`, and
//!    `MD5-sess` is a different HA1 (`http_auth.cpp:548-551`), so a client that reads "the"
//!    header and lands on the second one fails every request;
//! 2. `qop=auth` is unconditional (`:671`), so the RFC 2069 no-`qop` branch is server-side
//!    dead code and is not implemented (§2h);
//! 3. `nc` belongs to the **nonce**, not to the client: the server zeroes its own counter
//!    whenever it issues one (`:749`) and then requires equality (`:335-345`);
//! 4. the server never sends `opaque` (`:676-683`).

use crate::error::Error;
use crate::rpc::Login;

/// One connection's Digest session.
///
/// Carries no `opaque` (fact 4). `nc` is a field of this type and the type is replaced
/// wholesale by [`challenge`] on every new nonce, which is what makes fact 3 structural
/// rather than a rule somebody has to remember. The session lives behind the client's
/// `Mutex` (§2c).
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
    /// `qop`, always `auth` against `monerod`/`monero-wallet-rpc` (fact 2).
    pub fn qop(&self) -> &str {
        &self.qop
    }
    /// The count the next request will carry.
    pub fn next_nc(&self) -> u32 {
        self.nc + 1
    }
}

/// Split one `Digest k=v,k="v",…` header value into `(key, value)` pairs.
///
/// Quote-aware rather than a `split(',')`: epee's nonce is base64 of 128 random bits
/// (`http_auth.cpp:751-753`) and a future realm could carry a comma. Keys are lowercased;
/// values keep their case, because `nonce` and `realm` are hashed verbatim.
fn params(header: &str) -> Vec<(String, String)> {
    let rest = match header.trim_start().split_once(char::is_whitespace) {
        Some((_scheme, rest)) => rest,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut key = String::new();
    let mut value = String::new();
    let mut in_value = false;
    let mut quoted = false;
    let mut push = |key: &mut String, value: &mut String| {
        let k = key.trim().to_ascii_lowercase();
        if !k.is_empty() {
            out.push((k, value.trim().trim_matches('"').to_string()));
        }
        key.clear();
        value.clear();
    };
    for c in rest.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                if in_value {
                    value.push(c);
                }
            }
            '=' if !quoted && !in_value => in_value = true,
            ',' if !quoted => {
                push(&mut key, &mut value);
                in_value = false;
            }
            _ if in_value => value.push(c),
            _ => key.push(c),
        }
    }
    push(&mut key, &mut value);
    out
}

fn lookup(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// Build a session from a 401's challenges.
///
/// Takes **every** `WWW-Authenticate` header of the response, in order, and selects the one
/// whose `algorithm` is exactly `MD5` (case-insensitive) or absent; `MD5-sess` is never
/// selected (fact 1). A challenge with no `nonce` is not usable and is skipped.
pub fn challenge(www_authenticate: &[&str]) -> Result<Session, Error> {
    let mut saw_digest = false;
    for header in www_authenticate {
        if !header
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("digest")
        {
            continue;
        }
        saw_digest = true;
        let pairs = params(header);
        let algorithm = lookup(&pairs, "algorithm");
        match algorithm.as_deref() {
            None => {}
            Some(a) if a.eq_ignore_ascii_case("MD5") => {}
            // MD5-sess, or anything epee does not implement.
            Some(_) => continue,
        }
        let Some(nonce) = lookup(&pairs, "nonce") else {
            continue;
        };
        if nonce.is_empty() {
            continue;
        }
        return Ok(Session {
            realm: lookup(&pairs, "realm").unwrap_or_default(),
            nonce,
            // fact 2: the server advertises `qop="auth"` unconditionally. Default to it
            // rather than implementing the RFC 2069 branch (§2h).
            qop: lookup(&pairs, "qop")
                .filter(|q| !q.is_empty())
                .unwrap_or_else(|| "auth".to_string()),
            nc: 0,
        });
    }
    // §2o: the wire's own vocabulary is `detail`, never the sentence. Both of these reached a
    // person, the first of them 120 characters of `epee`, `MD5-sess` and `401`.
    let (why, detail) = if saw_digest {
        (
            "That node asked fmonero to prove who it is in a way fmonero cannot.",
            "only MD5 Digest is implemented; the server offered MD5-sess alone",
        )
    } else {
        (
            "That node refused fmonero and did not say how to prove who it is.",
            "401 with no Digest challenge in the WWW-Authenticate header",
        )
    };
    Err(Error::backend(why)
        .with_detail(detail)
        .with_reason("auth_refused")
        .with_hint("fmonero doctor --only security"))
}

fn md5_hex(data: &str) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(data.as_bytes());
    h.finalize().iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The `Authorization` header value for one request, and the one place `nc` advances.
///
/// `HA1 = md5(user:realm:pass)`, `HA2 = md5(method:uri)` where `uri` is the request path
/// exactly as sent (`/json_rpc`, `/get_info`, …), and
/// `response = md5(HA1:nonce:nc:cnonce:qop:HA2)` under `qop=auth`. A fresh 128-bit `cnonce`
/// per request; `nc` as 8 lowercase hex digits, **unquoted**; `cnonce` quoted.
pub fn authorization(s: &mut Session, login: &Login, method: &str, uri: &str) -> String {
    s.nc += 1;
    let nc = format!("{:08x}", s.nc);
    // /dev/urandom, falling back to the clock and the address of the session: a cnonce only
    // has to be unique per (nonce, nc) pair, and a client that refused to authenticate
    // because /dev/urandom was unreadable would be worse than one that used a weaker one.
    let cnonce = crate::rpc::random_hex(16).unwrap_or_else(|_| {
        md5_hex(&format!(
            "{:?}:{:p}:{}",
            std::time::SystemTime::now(),
            s,
            s.nc
        ))
    });
    let ha1 = md5_hex(&format!("{}:{}:{}", login.user, s.realm, login.pass));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let response = md5_hex(&format!("{ha1}:{}:{nc}:{cnonce}:{}:{ha2}", s.nonce, s.qop));
    format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{uri}\", \
         algorithm=MD5, qop={}, nc={nc}, cnonce=\"{cnonce}\", response=\"{response}\"",
        login.user, s.realm, s.nonce, s.qop
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// epee's own challenge shape: **two** headers, no space after the commas, the header
    /// name spelled `WWW-authenticate`, and `MD5-sess` second (`http_auth.cpp:665-691`).
    const MD5: &str =
        "Digest qop=\"auth\",algorithm=MD5,realm=\"monero-rpc\",nonce=\"aaaa1111\",stale=false";
    const SESS: &str =
        "Digest qop=\"auth\",algorithm=MD5-sess,realm=\"monero-rpc\",nonce=\"bbbb2222\",stale=false";

    #[test]
    fn the_md5_challenge_is_selected_and_md5_sess_never_is() {
        // In epee's own order, and reversed: selection must not depend on position.
        let s = challenge(&[MD5, SESS]).expect("an MD5 challenge is usable");
        assert_eq!(s.nonce(), "aaaa1111");
        let s = challenge(&[SESS, MD5]).expect("order must not matter");
        assert_eq!(
            s.nonce(),
            "aaaa1111",
            "MD5-sess has a different HA1 (§1b.7)"
        );
        assert_eq!(s.realm(), "monero-rpc");
        assert_eq!(s.qop(), "auth");
        // MD5-sess alone is not usable: answering it with the MD5 HA1 fails every request.
        let e = challenge(&[SESS]).unwrap_err();
        assert_eq!(e.exit_code(), 4);
        assert_eq!(e.reason(), Some("auth_refused"));
        // No Digest at all.
        let e = challenge(&["Basic realm=\"x\""]).unwrap_err();
        assert!(e.to_string().contains("did not say how to prove"), "{e}");
    }

    #[test]
    fn a_challenge_with_no_algorithm_is_accepted() {
        let s = challenge(&["Digest qop=\"auth\",realm=\"r\",nonce=\"n\""]).unwrap();
        assert_eq!(s.nonce(), "n");
    }

    /// RFC 2617 §3.5's worked example, which is also what the mock verifies with — so a
    /// wrong HA1/HA2/response formula fails here rather than looking like a server bug.
    #[test]
    fn the_response_matches_rfc2617() {
        let mut s = Session {
            realm: "testrealm@host.com".into(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".into(),
            qop: "auth".into(),
            nc: 0,
        };
        let login = Login {
            user: "Mufasa".into(),
            pass: "Circle Of Life".into(),
        };
        let header = authorization(&mut s, &login, "GET", "/dir/index.html");
        // The cnonce is random, so recompute the published response with the one we sent.
        let pairs = params(&header);
        let cnonce = lookup(&pairs, "cnonce").unwrap();
        let ha1 = md5_hex("Mufasa:testrealm@host.com:Circle Of Life");
        let ha2 = md5_hex("GET:/dir/index.html");
        let want = md5_hex(&format!(
            "{ha1}:dcd98b7102dd2f0e8b11d0f600bfb0c093:00000001:{cnonce}:auth:{ha2}"
        ));
        assert_eq!(lookup(&pairs, "response").unwrap(), want);
        assert_eq!(lookup(&pairs, "nc").unwrap(), "00000001");
        assert_eq!(lookup(&pairs, "algorithm").unwrap(), "MD5");
        assert_eq!(lookup(&pairs, "uri").unwrap(), "/dir/index.html");
        // `nc` is unquoted and `cnonce` is quoted, as RFC 2617 requires.
        assert!(header.contains("nc=00000001,"), "{header}");
        assert!(header.contains(&format!("cnonce=\"{cnonce}\"")), "{header}");
        assert_eq!(ha1, "939e7578ed9e3c518a452acee763bce9", "the published HA1");
    }

    /// Fact 3: `nc` advances within one nonce and **resets** with a new one, because the
    /// server zeroes its own counter and then requires equality.
    #[test]
    fn nc_advances_within_a_nonce_and_resets_with_a_new_one() {
        let login = Login {
            user: "fmonero".into(),
            pass: "t3st".into(),
        };
        let mut s = challenge(&[MD5, SESS]).unwrap();
        assert_eq!(s.next_nc(), 1);
        let first = authorization(&mut s, &login, "POST", "/json_rpc");
        assert!(first.contains("nc=00000001"), "{first}");
        let second = authorization(&mut s, &login, "POST", "/json_rpc");
        assert!(second.contains("nc=00000002"), "{second}");
        // A rotated nonce arrives as a fresh challenge, which is a fresh Session.
        let rotated =
            "Digest qop=\"auth\",algorithm=MD5,realm=\"monero-rpc\",nonce=\"cccc3333\",stale=true";
        let mut s = challenge(&[rotated]).unwrap();
        let third = authorization(&mut s, &login, "POST", "/json_rpc");
        assert!(third.contains("nc=00000001"), "{third}");
        assert!(third.contains("nonce=\"cccc3333\""), "{third}");
    }

    #[test]
    fn a_quoted_value_may_contain_a_comma_and_an_equals_sign() {
        let p = params("Digest realm=\"a,b\",nonce=\"z=+/\",algorithm=MD5");
        assert_eq!(lookup(&p, "realm").unwrap(), "a,b");
        assert_eq!(lookup(&p, "nonce").unwrap(), "z=+/");
        assert_eq!(lookup(&p, "algorithm").unwrap(), "MD5");
    }
}
