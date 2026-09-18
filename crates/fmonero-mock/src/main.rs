//! `fmonero-mock` — a fixture-serving stand-in for `monerod` and `monero-wallet-rpc`
//! (DESIGN.md §8, Phase 0). Test-only: `publish = false`, no dependency of either shipped crate, and
//! `scripts/install.sh` builds `-p fmonero` only, so it can never reach `~/.local/bin`. It proves our
//! wire format and our parsing, never that a real daemon agrees: every row it backs is AMBER in §4b.
//!
//! **`tests/fixtures/README.md` is the reference.** The argv and why each flag exists, the
//! fixture-lookup order, the four Digest rules of §1b.7, the request-log shape and every deviation
//! from §8 are documented there, so this file can stay near §8's 500-line budget without the prose and
//! the code drifting apart. What is left here are the `file:line` citations a reader of a given line
//! needs on the spot.
//!
//! Serves `POST /json_rpc` by dispatching on `method`, and legacy `POST /<name>` endpoints, from
//! `<--fixtures>/<--role>/`; fixture names are wire names. Stateless apart from the per-connection
//! Digest session and the request log, so a `-13` comes from `--fail` and never from mock state.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: fmonero-mock --role node|wallet --port N --fixtures DIR --log FILE \
[--restricted] [--auth USER:PASS] [--delay-ms N] [--rotate-nonce-after N] [--scenario NAME] \
[--fail METHOD=CODE:MESSAGE]...
   or: fmonero-mock --static DIR --port N [--log FILE]      # v5.5, §2n.7";

/// json_rpc methods `monerod` does not REGISTER under `--restricted-rpc`
/// (`MAP_JON_RPC_WE_IF(..., !m_restricted)`, src/rpc/core_rpc_server.h:166, :169, :170, :179); they
/// fall through to `END_JSON_RPC_MAP()`, which answers 200 with -32601
/// (http_server_handlers_map2.h:280-285).
const RESTRICTED_JSONRPC: &str = "sync_info get_connections get_bans set_bans";

/// Legacy endpoints ditto (core_rpc_server.h:121, :124, :131, :134, :136, :137, :138) — but they leave
/// `handled = false` (http_server_handlers_map2.h:144) and epee answers **404**, never -32601
/// (http_protocol_handler.inl:664).
const RESTRICTED_LEGACY: &str =
    "get_peer_list get_net_stats set_limit out_peers in_peers stop_daemon set_log_level";

/// Wire method -> default fixture basename. One entry: §8 names the transfer fixture
/// `transfer-do-not-relay.json` while the wire method is `transfer`. `--scenario` still wins.
const FIXTURE_ALIAS: &[(&str, &str)] = &[("transfer", "transfer-do-not-relay")];

/// epee's Digest realm, verbatim (`auth_realm`, contrib/epee/src/http_auth.cpp:97).
const REALM: &str = "monero-rpc";

struct Routes {
    role: String,
    port: u16,
    dir: PathBuf,
    restricted: bool,
    auth: Option<(String, String)>,
    delay: Duration,
    rotate_after: u32,
    scenario: Option<String>,
    fails: BTreeMap<String, (i64, String)>,
    /// v5.5, §2n.7: serve `DIR/<path>` for a GET as `application/json`, 404 otherwise. It is
    /// what points `FMONERO_NODES_URL` at the captured monero.fail fixtures, so nothing in §2n
    /// needs the internet to be tested — and it keeps the suite to **one** mock binary instead
    /// of adding `python3 -m http.server` to `tests/e2e.sh`.
    static_dir: Option<PathBuf>,
    log: Mutex<File>,
    seq: AtomicU64,
    conns: AtomicU64,
    nonces: AtomicU64,
}

impl Routes {
    fn gated(&self, list: &str, name: &str) -> bool {
        self.restricted && list.split(' ').any(|m| m == name)
    }
}

fn main() {
    let r = parse_args(std::env::args().skip(1).collect()).unwrap_or_else(|e| die(&e));
    if let Err(e) = serve(Arc::new(r)) {
        eprintln!("fmonero-mock: {e}");
        std::process::exit(1);
    }
}

fn die(msg: &str) -> ! {
    eprintln!("fmonero-mock: {msg}\n{USAGE}");
    std::process::exit(2)
}

fn parse_args(args: Vec<String>) -> Result<Routes, String> {
    let (mut role, mut port, mut fix, mut logp) = (None, None, None, None);
    let (mut restricted, mut auth, mut ms, mut rot, mut scen) = (false, None, 0u64, 0u32, None);
    let mut static_dir: Option<PathBuf> = None;
    let mut fails = BTreeMap::new();
    let mut it = args.into_iter();
    let num = |v: String| v.parse::<u64>().map_err(|e| e.to_string());
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--role" => role = Some(val()?),
            "--port" => port = Some(num(val()?)? as u16),
            "--fixtures" => fix = Some(PathBuf::from(val()?)),
            "--log" => logp = Some(PathBuf::from(val()?)),
            "--scenario" => scen = Some(val()?),
            "--restricted" => restricted = true,
            "--static" => static_dir = Some(PathBuf::from(val()?)),
            "--delay-ms" => ms = num(val()?)?,
            "--rotate-nonce-after" => rot = num(val()?)? as u32,
            "--auth" => auth = Some(split2(&val()?, ':', "--auth wants USER:PASS")?),
            // METHOD=CODE:MESSAGE, split at the FIRST '=' then the FIRST ':', so a message may
            // contain either character.
            "--fail" => {
                let bad = "--fail wants METHOD=CODE:MESSAGE";
                let (m, rest) = split2(&val()?, '=', bad)?;
                let (c, msg) = split2(&rest, ':', bad)?;
                let c = c
                    .parse::<i64>()
                    .map_err(|_| format!("--fail: {c} is not a code"))?;
                fails.insert(m, (c, msg));
            }
            "-h" | "--help" => die(""),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    // `--static` is its own mode: no role, no fixture directory, no Digest, no dispatcher.
    // Refusing the combination beats ignoring half of it (the same rule `--restricted` follows
    // below): a `--static` that also accepted `--role` would read as coverage it does not give.
    if static_dir.is_some() {
        if role.is_some() || fix.is_some() || restricted || auth.is_some() {
            return Err("--static takes only --port and --log".into());
        }
        let logp = logp.unwrap_or_else(|| PathBuf::from("/dev/null"));
        let log = File::create(&logp).map_err(|e| format!("{}: {e}", logp.display()))?;
        return Ok(Routes {
            role: "static".into(),
            port: port.ok_or("--port is required")?,
            dir: PathBuf::new(),
            restricted: false,
            auth: None,
            delay: Duration::from_millis(ms),
            rotate_after: 0,
            scenario: None,
            fails,
            static_dir,
            log: Mutex::new(log),
            seq: AtomicU64::new(0),
            conns: AtomicU64::new(0),
            nonces: AtomicU64::new(0),
        });
    }
    let role = role.ok_or("--role is required")?;
    if role != "node" && role != "wallet" {
        return Err(format!("--role must be node or wallet, not {role}"));
    }
    // Refusing beats ignoring: a silently accepted flag reads as coverage it does not give.
    if restricted && role == "wallet" {
        return Err("--restricted is a node flag; the wallet unit never sets it".into());
    }
    let dir = fix.ok_or("--fixtures is required")?.join(&role);
    let logp = logp.ok_or("--log is required (§2a's central tests read it)")?;
    let log = File::create(&logp).map_err(|e| format!("{}: {e}", logp.display()))?;
    Ok(Routes {
        role,
        port: port.ok_or("--port is required")?,
        dir,
        restricted,
        auth,
        delay: Duration::from_millis(ms),
        rotate_after: rot,
        scenario: scen,
        fails,
        static_dir: None,
        log: Mutex::new(log),
        seq: AtomicU64::new(0),
        conns: AtomicU64::new(0),
        nonces: AtomicU64::new(0),
    })
}

fn split2(v: &str, sep: char, err: &str) -> Result<(String, String), String> {
    let (a, b) = v.split_once(sep).ok_or_else(|| err.to_string())?;
    Ok((a.to_string(), b.to_string()))
}

/// Bind loopback; ONE THREAD PER ACCEPTED SOCKET, which is load-bearing (§8): fmonero keeps one pooled
/// socket for the life of the process (§2c), so a single-threaded accept loop would park every second
/// client behind a connection that is never closed — and §2a.4/§2a.11's two-process tests are the ones
/// that prove `pending.json` survives concurrent writers.
fn serve(r: Arc<Routes>) -> std::io::Result<()> {
    let l = TcpListener::bind(("127.0.0.1", r.port))?;
    let port = l.local_addr()?.port();
    eprintln!(
        "fmonero-mock: role={} dir={} on 127.0.0.1:{port}",
        r.role,
        r.dir.display()
    );
    accept_loop(l, r);
    Ok(())
}

/// The accept loop itself, so `serve` and [`spawn_for_test`] share it rather than each owning
/// a copy of the one thing §8 calls load-bearing.
fn accept_loop(l: TcpListener, r: Arc<Routes>) {
    for stream in l.incoming() {
        let Ok(stream) = stream else { break };
        let r = Arc::clone(&r);
        std::thread::spawn(move || drop(connection(stream, &r)));
    }
}

/// Bind an ephemeral loopback port, spawn the accept loop on a thread and return the port.
///
/// This is how `fmonero-core`'s in-process tests get a mock **without a second
/// implementation**: they include this file with `#[path]` and call this, so the request
/// parsing, the Digest server and the request log are the same 150 lines the binary serves
/// (§8 Phase 0's "a thin `main()` over a `serve(routes)` function"). Sharing it as a library
/// would need a `[dev-dependencies]` entry, and no `Cargo.toml` may be edited after the 1a
/// gate (§2k) — the deviation is recorded in DECISIONS.md.
///
/// `#[allow(dead_code)]`: nothing in the **binary** calls it.
#[allow(dead_code)]
pub fn spawn_for_test(args: &[&str]) -> Result<u16, String> {
    let mut r = parse_args(args.iter().map(|a| (*a).to_string()).collect())?;
    let l = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = l.local_addr().map_err(|e| e.to_string())?.port();
    r.port = port;
    let r = Arc::new(r);
    std::thread::spawn(move || accept_loop(l, r));
    Ok(port)
}

/// Per-connection Digest state, mirroring epee's `m_auth` — a member of the per-connection protocol
/// handler (http_protocol_handler.h:186, :223), not a server-wide one. `counter` is epee's own:
/// incremented before each verify (http_auth.cpp:734), required equal to the request's `nc` (:340),
/// zeroed with every new nonce (:749) — so the first request on a new nonce carries `nc=00000001`.
#[derive(Default)]
struct Session {
    nonce: String,
    counter: u32,
    authed: u32,
    force_stale: bool,
}

/// One socket, HTTP/1.1 keep-alive: successive requests on the same stream until the client closes it.
/// `Content-Length` always, `Connection: close` never — without keep-alive §2c's "the second request
/// on one nonce carries `nc=00000002`" is unwritable.
fn connection(mut out: TcpStream, r: &Routes) -> std::io::Result<()> {
    // Loopback, small frames, request/response: Nagle can only add latency here.
    let _ = out.set_nodelay(true);
    let conn = r.conns.fetch_add(1, Ordering::SeqCst);
    let mut sess = Session::default();
    let mut input = BufReader::new(out.try_clone()?);
    while let Some((verb, path, head, body)) = read_request(&mut input)? {
        std::thread::sleep(r.delay);
        let chal = r
            .auth
            .as_ref()
            .and_then(|a| check_auth(r, &mut sess, a, &verb, &head));
        let (code, extra, payload) = match chal {
            Some(c) => (401u16, c, b"<h1>401 Unauthorized</h1>".to_vec()),
            None if r.static_dir.is_some() => {
                let (c, b) = serve_static(r, &verb, &path);
                (c, String::new(), b)
            }
            None => {
                let (c, b) = dispatch(r, &path, &body);
                (c, String::new(), b)
            }
        };
        log_request(r, conn, &path, &body, code, &head);
        let (why, ctype) = match code {
            200 => ("OK", "application/json"),
            401 => ("Unauthorized", "text/html"),
            _ => ("Not Found", "text/html"),
        };
        let n = payload.len();
        // ONE write, and TCP_NODELAY below. A header write followed by a body write is two small
        // segments on a keep-alive connection, and Nagle holds the second until the client's ACK
        // of the first — about 40 ms per request on loopback. That is invisible in a functional
        // test and it moved DESIGN.md §2l (b)'s `status` figure from ~60 ms to ~240 ms, i.e. the
        // AMBER budget would have been "missed" by the mock rather than by fmonero.
        let mut frame = format!(
            "HTTP/1.1 {code} {why}\r\nContent-Type: {ctype}\r\nContent-Length: {n}\r\n{extra}\r\n"
        )
        .into_bytes();
        frame.extend_from_slice(&payload);
        out.write_all(&frame)?;
        out.flush()?;
    }
    Ok(())
}

/// `--static` (v5.5, §2n.7). One path segment, no traversal, no directory listing: the fixture
/// server exists to answer `GET /nodes.json` and `GET /health.json` and nothing else, and a
/// test server that can be talked into reading `../../` is a test server that teaches the wrong
/// habit even where it cannot be reached.
fn serve_static(r: &Routes, verb: &str, path: &str) -> (u16, Vec<u8>) {
    let not_found = || (404u16, b"<h1>404 Not Found</h1>".to_vec());
    if verb != "GET" {
        return not_found();
    }
    let Some(dir) = r.static_dir.as_ref() else {
        return not_found();
    };
    let name = path
        .trim_start_matches('/')
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return not_found();
    }
    match std::fs::read(dir.join(name)) {
        Ok(bytes) => (200, bytes),
        Err(_) => not_found(),
    }
}

type Headers = Vec<(String, String)>;
/// (verb, path, headers, body).
type Request = (String, String, Headers, Vec<u8>);

/// `Ok(None)` at a clean EOF, which is how a keep-alive connection ends.
fn read_request(input: &mut BufReader<TcpStream>) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let verb = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut head = Headers::new();
    loop {
        let mut h = String::new();
        if input.read_line(&mut h)? == 0 {
            return Ok(None);
        }
        match h.trim_end_matches(['\r', '\n']).split_once(':') {
            Some((k, v)) => head.push((k.trim().to_ascii_lowercase(), v.trim().to_string())),
            None => break,
        }
    }
    let n: usize = header(&head, "content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; n];
    input.read_exact(&mut body)?;
    Ok(Some((verb, path, head, body)))
}

fn header<'a>(h: &'a Headers, name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// `Some(challenge headers)` to answer 401, `None` to let the request through. Only the MD5 form is
/// accepted, so a client that answered the MD5-sess challenge — whose HA1 is `md5(key:nonce:cnonce)`,
/// a different formula (http_auth.cpp:548-551) — is refused. README, "Digest", has all four rules.
fn check_auth(
    r: &Routes,
    s: &mut Session,
    login: &(String, String),
    verb: &str,
    h: &Headers,
) -> Option<String> {
    let (user, pass) = (login.0.as_str(), login.1.as_str());
    let stale = match header(h, "authorization") {
        _ if s.force_stale => true,
        None => false,
        Some(a) => {
            s.counter += 1; // epee increments BEFORE verifying (http_auth.cpp:734)
            let p = auth_params(a);
            let ok = a.trim_start().to_ascii_lowercase().starts_with("digest")
                && p.get("username").map(String::as_str) == Some(user)
                && p.get("nonce").map(String::as_str) == Some(s.nonce.as_str())
                && p.get("nc").and_then(|n| u32::from_str_radix(n, 16).ok()) == Some(s.counter)
                && p.get("algorithm")
                    .is_none_or(|a| a.eq_ignore_ascii_case("MD5"))
                && expected(user, REALM, pass, verb, &p).eq_ignore_ascii_case(p.get("response")?);
            if ok {
                s.authed += 1;
                // Rotate LAZILY: the next request presents a nonce we no longer accept and takes the
                // ordinary stale path, exactly as against a real epee server.
                s.force_stale = r.rotate_after > 0 && s.authed >= r.rotate_after;
                return None;
            }
            // A wrong nonce is `kStale`, a wrong response `kFail` (:326-348); report stale only when
            // a nonce was offered at all, which is what makes the client retry.
            p.contains_key("nonce")
        }
    };
    (s.counter, s.authed, s.force_stale) = (0, 0, false); // epee zeroes with every nonce (:749)
    s.nonce = nonce(r);
    // TWO headers (:665-691). `add_field` pushes a bare comma with NO space (:206-210) and the name is
    // spelled `WWW-authenticate` (:96); both are reproduced exactly, so a client that splits on ", "
    // or matches the header name case-sensitively fails here as it would against a real daemon.
    let one = |alg: &str| {
        format!(
            "WWW-authenticate: Digest qop=\"auth\",algorithm={alg},\
             realm=\"{REALM}\",nonce=\"{}\",stale={stale}\r\n",
            s.nonce
        )
    };
    Some(one("MD5") + &one("MD5-sess"))
}

/// `HA1 = md5(user:realm:pass)`, `HA2 = md5(method:uri)`,
/// `response = md5(HA1:nonce:nc:cnonce:qop:HA2)` — the `qop=auth` form, the only one epee advertises.
fn expected(user: &str, realm: &str, pw: &str, verb: &str, p: &BTreeMap<String, String>) -> String {
    let g = |k: &str| p.get(k).cloned().unwrap_or_default();
    let a1 = md5_hex(format!("{user}:{realm}:{pw}").as_bytes());
    let a2 = md5_hex(format!("{verb}:{}", g("uri")).as_bytes());
    md5_hex(
        format!(
            "{a1}:{}:{}:{}:{}:{a2}",
            g("nonce"),
            g("nc"),
            g("cnonce"),
            g("qop")
        )
        .as_bytes(),
    )
}

/// Split `Digest k=v,k="v",…` into a map; values may be quoted or bare, keys are lowercased.
fn auth_params(h: &str) -> BTreeMap<String, String> {
    let clean = |(k, v): (&str, &str)| {
        (
            k.trim().to_ascii_lowercase(),
            v.trim().trim_matches('"').to_string(),
        )
    };
    let rest = h.trim_start().split_once(' ').map_or("", |(_, r)| r);
    rest.split(',')
        .filter_map(|s| s.split_once('='))
        .map(clean)
        .collect()
}

/// epee's nonce is base64 of 128 random bits (:751-753); a mock needs uniqueness, not
/// unpredictability, so this is a hash of the clock and a counter of its OWN -- sharing
/// `conns` made the accept-order connection ids in the log skip.
fn nonce(r: &Routes) -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    md5_hex(format!("{n}:{}", r.nonces.fetch_add(1, Ordering::SeqCst)).as_bytes())
}

/// `POST /json_rpc` by `method`, or a legacy `POST /<name>`. Returns (HTTP status, body).
fn dispatch(r: &Routes, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let req: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let not_found = || (404u16, b"<h1>404 Not Found</h1>".to_vec());
    if path != "/json_rpc" {
        let name = path.trim_start_matches('/');
        if r.gated(RESTRICTED_LEGACY, name) {
            return not_found();
        }
        if let Some((_code, msg)) = r.fails.get(name) {
            // A legacy endpoint has no JSON-RPC error channel: `status` IS the daemon's error channel
            // for handlers that answer 200 with no error object — "BUSY", or free text such as
            // set_limit's "Invalid parameter" (§1b.3). The CODE is ignored here, deliberately.
            return (200, bytes(&json!({"status": msg, "untrusted": false})));
        }
        return fixture(r, name, &json!({})).map_or_else(not_found, |v| (200, bytes(&v)));
    }
    let id = req.get("id").cloned().unwrap_or(json!(0));
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = req.get("params").cloned().unwrap_or(json!({}));
    if r.gated(RESTRICTED_JSONRPC, &method) {
        // The fixture, with the request's own id: monerod echoes whatever it was sent.
        let mut e = load(r, "error-restricted").unwrap_or_else(|| rpc_err(&id, -32601));
        e["id"] = id;
        return (200, bytes(&e));
    }
    if let Some((code, msg)) = r.fails.get(&method) {
        let mut e = rpc_err(&id, *code);
        e["error"]["message"] = json!(msg);
        return (200, bytes(&e));
    }
    match fixture(r, &method, &params) {
        Some(v) => (
            200,
            bytes(&json!({"jsonrpc": "2.0", "id": id, "result": v})),
        ),
        // Which is also what monerod answers for a method it does not have.
        None => (200, bytes(&rpc_err(&id, -32601))),
    }
}

fn rpc_err(id: &Value, code: i64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": "Method not found"}})
}

fn bytes(v: &Value) -> Vec<u8> {
    v.to_string().into_bytes()
}

/// `<method>-<scenario>.json` when `--scenario` names one that exists, else the method's own file
/// (through `FIXTURE_ALIAS`). `validate_address` dispatches on its request instead.
fn fixture(r: &Routes, method: &str, params: &Value) -> Option<Value> {
    if method == "validate_address" {
        return validate_address(r, params);
    }
    if let Some(v) = r
        .scenario
        .as_ref()
        .and_then(|s| load(r, &format!("{method}-{s}")))
    {
        return Some(v);
    }
    // After the scenario lookup, so `--scenario mismatch` still selects
    // relay_tx-mismatch.json and DESIGN.md §2a.9's txid comparison keeps its one test.
    if method == "relay_tx" {
        return relay_tx(r, params);
    }
    load(
        r,
        FIXTURE_ALIAS
            .iter()
            .find(|(m, _)| *m == method)
            .map_or(method, |(_, f)| f),
    )
}

/// Read `<dir>/<name>.json`, dropping every top-level key that begins with `_`: those are fixture
/// metadata — `_provenance` (§1.1: "a top-level `_provenance` string that the loader deletes") and
/// `_request_address` — and no Monero RPC response field starts with an underscore.
fn load(r: &Routes, name: &str) -> Option<Value> {
    let mut v = raw(r, name)?;
    if let Some(map) = v.as_object_mut() {
        map.retain(|k, _| !k.starts_with('_'));
    }
    Some(v)
}

fn raw(r: &Routes, name: &str) -> Option<Value> {
    serde_json::from_reader(File::open(r.dir.join(format!("{name}.json"))).ok()?).ok()
}

/// The fixture whose `_request_address` equals the request's `address`, else
/// `validate_address-invalid.json` (§8). A mock cannot verify a Keccak checksum, so "unknown" and
/// "invalid" are one answer — which is right: §2a.6 validates twice because `address::parse` cannot
/// check a checksum either.
fn validate_address(r: &Routes, params: &Value) -> Option<Value> {
    let want = params.get("address").and_then(Value::as_str).unwrap_or("");
    let stem = |n: String| n.strip_suffix(".json").map(str::to_string);
    let mut names: Vec<String> = std::fs::read_dir(&r.dir)
        .ok()?
        .filter_map(|e| stem(e.ok()?.file_name().to_str()?.to_string()))
        .filter(|n| n.starts_with("validate_address-"))
        .collect();
    names.sort(); // deterministic, rather than whatever order readdir happened to give
    for n in &names {
        if raw(r, n)?["_request_address"].as_str() == Some(want) {
            return load(r, n);
        }
    }
    load(r, "validate_address-invalid")
}

/// `relay_tx` answers the txid of the transaction whose `tx_metadata` it was handed.
///
/// That is what a real wallet-rpc does — `res.tx_hash = pod_to_hex(get_transaction_hash(ptx.tx))`
/// over the `pending_tx` parsed from the request's `hex` (wallet_rpc_server.cpp:1963) — and a
/// mock cannot hash a transaction, but it CAN look the pair up in the very fixtures that
/// produced the metadata. It **must**: sweep_all.json carries two transactions with their own
/// txids, so a single static answer makes DESIGN.md §2a.9's txid comparison fire on every
/// correct sweep, and the one test of that check becomes "the mock disagrees with itself".
/// A `hex` no fixture produced still falls back to relay_tx.json.
fn relay_tx(r: &Routes, params: &Value) -> Option<Value> {
    let hex = params
        .get("hex")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(v) = raw(r, "transfer-do-not-relay") {
        if v["tx_metadata"].as_str() == Some(hex) {
            return Some(json!({"tx_hash": v["tx_hash"]}));
        }
    }
    if let Some(v) = raw(r, "sweep_all") {
        let meta = v["tx_metadata_list"].as_array()?;
        let hashes = v["tx_hash_list"].as_array()?;
        if let Some(i) = meta.iter().position(|m| m.as_str() == Some(hex)) {
            return hashes.get(i).map(|h| json!({"tx_hash": h}));
        }
    }
    load(r, "relay_tx")
}

/// One JSON object per request. §8 fixes `{seq, ts_ms, path, method, body}`; `conn`, `status` and `nc`
/// are a documented superset — README, "Deviations", says why each is needed. `body` is parsed JSON
/// when it parses, else the raw text as a JSON string. One `Mutex<File>` guards it — the only state
/// shared between connection threads — so lines stay whole and `seq` stays ordered.
fn log_request(r: &Routes, conn: u64, path: &str, body: &[u8], status: u16, h: &Headers) {
    let text = || Value::String(String::from_utf8_lossy(body).into_owned());
    let parsed: Value = serde_json::from_slice(body).unwrap_or_else(|_| text());
    let method = match path {
        "/json_rpc" => parsed
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => path.trim_start_matches('/').to_string(),
    };
    let nc = header(h, "authorization")
        .and_then(|a| auth_params(a).get("nc").cloned())
        .map_or(Value::Null, Value::String);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let line = json!({
        "seq": r.seq.fetch_add(1, Ordering::SeqCst), "ts_ms": ms, "conn": conn, "path": path,
        "method": method, "body": parsed, "status": status, "nc": nc,
    });
    if let Ok(mut f) = r.log.lock() {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

// ---- MD5 (RFC 1321) --------------------------------------------------------------------------
// epee's `digest_algorithms` is a ONE-ELEMENT tuple holding `md5_` (http_auth.cpp:155), advertised as
// "MD5" (:108-110) and implemented with EVP_md5() (:144) — there is no SHA-256 variant, so verifying a
// Digest response needs MD5 and nothing else. Inline because §8 caps this binary at serde_json plus
// std. No security property rests on it: it authenticates a test daemon.

/// MD5's 64 shifts are these sixteen, four per round, cycling every four.
const SHIFT: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];

/// `K[i] = floor(2^32 * abs(sin(i + 1)))`, written out rather than computed: `f64::sin` is not
/// guaranteed bit-identical across platforms and a hash constant may not vary.
const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

fn md5_hex(data: &[u8]) -> String {
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_le_bytes());
    let mut st = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    for chunk in msg.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (i, w) in chunk.as_chunks::<4>().0.iter().enumerate() {
            m[i] = u32::from_le_bytes(*w);
        }
        let [mut a, mut b, mut c, mut d] = st;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            (a, d, c) = (d, c, b);
            b = b.wrapping_add(f.rotate_left(SHIFT[i / 16 * 4 + i % 4]));
        }
        for (s, v) in st.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
    st.iter()
        .flat_map(|v| v.to_le_bytes())
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 2617 §3.5's worked `qop=auth` example, through the same `expected` the server verifies with,
    /// plus proof that the MD5-sess HA1 (`md5(md5(a1):nonce:cnonce)`, http_auth.cpp:548-551) gives a
    /// DIFFERENT answer — without which "never pick MD5-sess" is a vacuous claim. It exercises
    /// `md5_hex` on six inputs against a published constant, so a wrong MD5 fails here too, which is
    /// where it must fail: otherwise every Digest handshake breaks in a way that looks like a client bug.
    #[test]
    fn digest_matches_rfc2617_and_differs_from_md5_sess() {
        let n = "dcd98b7102dd2f0e8b11d0f600bfb0c093";
        let pairs = [
            ("uri", "/dir/index.html"),
            ("nonce", n),
            ("nc", "00000001"),
            ("cnonce", "0a4f113b"),
            ("qop", "auth"),
        ];
        let p = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let md5 = expected("Mufasa", "testrealm@host.com", "Circle Of Life", "GET", &p);
        assert_eq!(md5, "6629fae49393a05397450978507c4ef1");
        let a1 = md5_hex(b"Mufasa:testrealm@host.com:Circle Of Life");
        let sess_a1 = md5_hex(format!("{a1}:{n}:0a4f113b").as_bytes());
        let a2 = md5_hex(b"GET:/dir/index.html");
        let sess = md5_hex(format!("{sess_a1}:{n}:00000001:0a4f113b:auth:{a2}").as_bytes());
        assert_ne!(md5, sess);
    }

    /// RFC 1321 appendix A.5's edge cases: the empty input (padding only) and an input that spans two
    /// 64-byte blocks. Between them they cover the two places the padding loop can be wrong.
    #[test]
    fn md5_matches_rfc1321_edges() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        let long = "1234567890".repeat(8);
        assert_eq!(md5_hex(long.as_bytes()), "57edf4a22be3c955ac49da2e2107b67a");
    }

    #[test]
    fn auth_params_splits_quoted_and_bare_values() {
        let p = auth_params("Digest username=\"fmonero\", uri=\"/json_rpc\", algorithm=MD5, nc=2");
        assert_eq!(p["username"], "fmonero");
        assert_eq!(p["uri"], "/json_rpc");
        assert_eq!(
            (&p["algorithm"], &p["nc"]),
            (&"MD5".to_string(), &"2".to_string())
        );
    }
}
