//! Public node discovery (DESIGN.md §2n, v5.5).
//!
//! **This is the only module in the workspace that spawns `curl`**, and `scripts/check.sh`'s
//! grep (vi) (§2c) is what keeps that true. Everything it does leaves through
//! [`crate::proc::run_with_deadline`] in one of exactly two argv shapes (§2n.3): an HTTPS GET
//! of monero.fail's two documents, and a `get_info` POST to a node the user has not chosen
//! yet. No credential, wallet password, wallet name, monetary amount or txid may ever appear
//! in either — `/proc/<pid>/cmdline` is world-readable, which is §2c's own reason for keeping
//! `rpc-login` out of argv.
//!
//! Three rules that are the module's whole safety story, restated because each is one line of
//! code away from being false:
//!
//! 1. **A candidate is only a candidate if [`crate::Settings::parse_remote_address`] accepted
//!    it** (§2n.1). monero.fail is an untrusted publisher: a string beginning with `-` is read
//!    by curl as an *option*, a userinfo form makes the row's rendered address and the host
//!    curl contacts two different hosts, and a trailing `#` turns the probe into a request for
//!    `/`. Anything the grammar refuses is dropped and counted.
//! 2. **The probe URL is reconstructed, never carried.** It is
//!    `format!("{scheme}://{address}/json_rpc")` from the two validated fields, which is why
//!    [`probe_one`] takes a [`ProbeTarget`] and not a `&str`: a signature that cannot be handed
//!    the raw string is the enforceable form of that sentence.
//! 3. **Nothing here is on a render path.** [`cached`] spawns nothing, ever; [`list`] is the
//!    only entry point that can fetch, and it fetches only when asked or when the cache is a
//!    day old (§2n.6).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::context::Context;
use crate::error::Error;
use crate::proc::{self, CancelToken};
use crate::settings::Settings;

/// How old a cache may be before [`list`] refetches without being asked (§2n.6).
pub const CACHE_MAX_AGE_SEC: u64 = 86_400;
/// How many probes are in flight at once. **Not a tuning knob**: 158 concurrent `curl` forks
/// is the fork storm §2a.11 has already paid for once.
pub const PROBE_CONCURRENCY: usize = 8;
/// `node list --limit`'s default, which bounds both what is printed and what is probed (§4.1).
pub const DEFAULT_LIMIT: usize = 20;
/// The one origin argv shape 1 may name (§2n.3).
pub const SOURCE: &str = "https://monero.fail";
/// Test-only (§2n.7), read here and nowhere else — `scripts/check.sh` asserts that.
pub const URL_ENV: &str = "FMONERO_NODES_URL";
/// The cache document's schema. A mismatch is STALE, not corrupt (§2n.6).
pub const SCHEMA: u32 = 1;

/// The fetch deadline curl enforces on itself, in seconds (§2n.3).
const FETCH_MAX_TIME_SEC: u64 = 20;
/// The probe deadline curl enforces on itself, in seconds (§2n.4).
const PROBE_MAX_TIME_SEC: u64 = 5;
/// `run_with_deadline` gets `--max-time + this`, so curl's own error — which has a message —
/// normally wins over our SIGTERM, which does not (§2n.3).
const DEADLINE_SLACK_SEC: u64 = 2;
/// The byte ceiling `nodes.rs` enforces on a list document itself, because `--max-filesize`
/// is only effective against an unknown-length response from curl 8.4.0 (§2n.3, revision 10).
const LIST_MAX_BYTES: usize = 8 * 1024 * 1024;
/// The same, for a `get_info` reply.
const PROBE_MAX_BYTES: usize = 64 * 1024;
/// The probe's request body — a constant, never built from anything (§2n.3).
const PROBE_BODY: &str = r#"{"jsonrpc":"2.0","id":"0","method":"get_info"}"#;

// ---------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------

/// One candidate, as it lives in `cache_dir/public-nodes.json` and as §5c prints it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublicNode {
    /// monero.fail's own string, kept verbatim so the cache can be diffed against a refetch.
    /// **A display and diff field only**: it is a third party's bytes and it reaches no `curl`
    /// argv (§2n.1, revision 2).
    pub url: String,
    /// The `host:port` [`Settings::parse_remote_address`] accepted (§2m.2) — derived once, so
    /// the picker, the confirm modal and `node remote` cannot disagree about what was chosen.
    pub address: String,
    /// `"http"` or `"https"`, lower-case. monero.fail publishes both in `monero.clear`.
    pub scheme: String,
    /// In monero.fail's `web_compatible` bucket, which is a **flag on a row** and not a second
    /// list — all 42 of its entries were also in `clear` on 2026-09-18 (§2n.1).
    pub web_compatible: bool,
    /// The user typed this one into the picker's `a` modal (§7a.4). Preserved across a list
    /// refresh (§2n.6) and rendered with `—` in the `monero.fail says` columns.
    #[serde(default)]
    pub custom: bool,
    pub health_available: Option<bool>,
    /// `checks.iter().filter(..).count() / checks.len()`. `None` for an empty or absent list —
    /// never `0.0`, and never against a fixed denominator: the list is 3..94 long (§2n.1).
    pub health_ratio: Option<f64>,
    /// The sample size behind `health_ratio`, because 100 % of 3 and 72 % of 94 are not
    /// comparable and a bare percent invites the comparison.
    pub health_checks: Option<u32>,
    pub health_last_height: Option<u64>,
    /// The HTTP-date, verbatim. Kept even when it did not parse: the row is useful without
    /// the arithmetic (§2n.1).
    pub health_checked_at_text: Option<String>,
    /// Parsed via `jiff::fmt::rfc2822`; `None` when it did not parse.
    pub health_checked_at: Option<i64>,
    /// This machine's own measurement, or `None` until it made one. Survives a list refresh.
    pub probe: Option<Probe>,
}

/// What one `get_info` measured (§2n.4).
///
/// `health_ratio` above is the one `f64` in this design's money-free half and it is allowed
/// for the reason §2a.5 forbids one elsewhere: it is a display ratio of two integers, never an
/// amount. `scripts/check.sh`'s `as f64` grep names `nodes.rs` in its audited list.
#[derive(Clone, Debug, Serialize)]
pub struct Probe {
    pub reachable: bool,
    pub height: Option<u64>,
    pub synchronized: Option<bool>,
    pub restricted: Option<bool>,
    pub nettype: Option<String>,
    /// `None` when the node returned `""`, which is what a restricted node does (§1b.3).
    pub version: Option<String>,
    pub latency_ms: Option<u64>,
    /// `None` on success; else one of [`PROBE_REASONS`] (§2n.4).
    pub reason: Option<&'static str>,
    pub probed_at: i64,
}

/// Every value [`Probe::reason`] can hold (§2n.4). `cancelled` is the sixth and is v5.5's
/// implementation note: `probe_many` returns one result per input in the input's order, so a
/// sweep the user stopped with `Esc` has to say *why* those rows carry no measurement rather
/// than render them as unreachable, which would be a claim about a node nobody contacted.
pub const PROBE_REASONS: &[&str] = &[
    "timed_out",
    "refused",
    "tls",
    "http",
    "bad_json",
    "cancelled",
];

/// `reason` is an `Option<&'static str>` in the frozen signature (§5b) and a JSON string in the
/// cache, and serde's derive cannot bridge the two: a `&'static str` field makes the generated
/// `Deserialize<'de>` demand `'de: 'static`, which no borrowed deserializer satisfies. So the
/// read side is hand-written over a wire struct. The set is closed, and a cache row naming
/// something outside it deserialises to `None` — the same "STALE, not corrupt" rule §2n.6
/// applies to the document as a whole.
impl<'de> Deserialize<'de> for Probe {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Probe, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            reachable: bool,
            #[serde(default)]
            height: Option<u64>,
            #[serde(default)]
            synchronized: Option<bool>,
            #[serde(default)]
            restricted: Option<bool>,
            #[serde(default)]
            nettype: Option<String>,
            #[serde(default)]
            version: Option<String>,
            #[serde(default)]
            latency_ms: Option<u64>,
            #[serde(default)]
            reason: Option<String>,
            #[serde(default)]
            probed_at: i64,
        }
        let w = Wire::deserialize(d)?;
        Ok(Probe {
            reachable: w.reachable,
            height: w.height,
            synchronized: w.synchronized,
            restricted: w.restricted,
            nettype: w.nettype,
            version: w.version,
            latency_ms: w.latency_ms,
            reason: w.reason.and_then(|s| {
                PROBE_REASONS
                    .iter()
                    .find(|known| **known == s.as_str())
                    .copied()
            }),
            probed_at: w.probed_at,
        })
    }
}

impl Probe {
    fn failed(reason: &'static str, latency_ms: Option<u64>) -> Probe {
        Probe {
            reachable: false,
            height: None,
            synchronized: None,
            restricted: None,
            nettype: None,
            version: None,
            latency_ms,
            reason: Some(reason),
            probed_at: crate::now_unix(),
        }
    }
    /// An answer arrived but carried no readable measurement — a `"BUSY"` node in initial
    /// sync, which is `reachable: true` with every measured field `null` and never a row of
    /// zeros (§2n.4).
    fn answered(latency_ms: Option<u64>) -> Probe {
        Probe {
            reachable: true,
            height: None,
            synchronized: None,
            restricted: None,
            nettype: None,
            version: None,
            latency_ms,
            reason: None,
            probed_at: crate::now_unix(),
        }
    }
}

/// monero.fail's health entry for one exact URL string (§2n.1).
#[derive(Clone, Debug, Default)]
pub struct Health {
    pub available: Option<bool>,
    pub ratio: Option<f64>,
    pub checks: Option<u32>,
    pub last_height: Option<u64>,
    pub checked_at_text: Option<String>,
    pub checked_at: Option<i64>,
}

/// `health.json` reduced to what §2n.1 says it is: an exact-URL lookup, told nothing about
/// buckets (its `monero.clear` held 1 116 keys of which 890 were onion or i2p hosts).
pub type HealthMap = BTreeMap<String, Health>;

/// The cache document (§2n.6).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeList {
    pub schema: u32,
    pub fetched_at: i64,
    pub source: String,
    /// How many candidates were dropped for carrying no port (37 of 195 on 2026-09-18).
    pub skipped_no_port: u32,
    /// Dropped for any **other** §2m.2 grammar refusal — a leading `-`, userinfo, a path, a
    /// query, a fragment, a bad label (§2n.1, revision 2).
    #[serde(default)]
    pub skipped_invalid: u32,
    /// Unix seconds when a surface displayed §2n.5's notice. The consent record — **not** the
    /// file's existence (revision 7).
    #[serde(default)]
    pub notice_ack: Option<i64>,
    pub nodes: Vec<PublicNode>,
    /// Age at render time, filled by [`list`]/[`cached`]; not part of the file.
    #[serde(skip_deserializing, default)]
    pub cache_age_sec: Option<u64>,
}

impl NodeList {
    fn empty() -> NodeList {
        NodeList {
            schema: SCHEMA,
            fetched_at: 0,
            source: source_base(),
            skipped_no_port: 0,
            skipped_invalid: 0,
            notice_ack: None,
            nodes: Vec::new(),
            cache_age_sec: None,
        }
    }
    /// True when [`list`] must fetch even though the caller did not ask (§2n.6).
    pub fn is_stale(&self) -> bool {
        self.cache_age_sec.unwrap_or(u64::MAX) > CACHE_MAX_AGE_SEC
    }
}

/// What [`parse_nodes_json`] found. A struct rather than a tuple because two of its three
/// fields are counters and an unnamed `(_, u32, u32)` is a call site nobody can read.
#[derive(Clone, Debug, Default)]
pub struct ParsedNodes {
    pub nodes: Vec<PublicNode>,
    pub skipped_no_port: u32,
    pub skipped_invalid: u32,
}

/// `node list`'s flags (§4.1).
#[derive(Copy, Clone, Debug, Default)]
pub struct ListOpts {
    pub refresh: bool,
    pub probe: bool,
    pub limit: usize,
    pub all: bool,
}

/// What a probe is allowed to be told to contact: a validated `address` and the `scheme` it
/// came with, **never** monero.fail's `url` (§2n.1, revision 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeTarget {
    pub scheme: String,
    pub address: String,
}

impl ProbeTarget {
    /// From a cache row. Infallible: `address` was validated at parse time.
    pub fn from_node(n: &PublicNode) -> ProbeTarget {
        ProbeTarget {
            scheme: n.scheme.clone(),
            address: n.address.clone(),
        }
    }
    /// From `settings.node.remote` — the configured-node probe of §2n.4. Validates through
    /// [`Settings::parse_remote_address`], so a hand-edited `config.toml` cannot reach curl
    /// either.
    pub fn parse(s: &str) -> Result<ProbeTarget, Error> {
        let normalised = Settings::parse_remote_address(s)?;
        Ok(split_scheme(&normalised))
    }
    /// The one place a probe URL is built (§2n.3 argv shape 2).
    fn json_rpc_url(&self) -> String {
        format!("{}://{}/json_rpc", self.scheme, self.address)
    }
}

/// Split a normalised §2m.2 address into a scheme and a bare `host:port`. A value with no
/// scheme is `http`, which is what `monero-wallet-rpc` would reach it as anyway (§1b.9 item 1
/// — the scheme's whole effect there is a default port fmonero never lets it choose).
fn split_scheme(normalised: &str) -> ProbeTarget {
    match normalised.split_once("://") {
        Some((scheme, rest)) => ProbeTarget {
            scheme: scheme.to_ascii_lowercase(),
            address: rest.to_string(),
        },
        None => ProbeTarget {
            scheme: "http".to_string(),
            address: normalised.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------------------

/// The base of argv shape 1's URL — `https://monero.fail`, or the test seam (§2n.7).
fn source_base() -> String {
    match std::env::var(URL_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim().trim_end_matches('/').to_string(),
        _ => SOURCE.to_string(),
    }
}

/// True when the test seam is in force, which is the only thing that relaxes argv shape 1's
/// `--proto =https` (§2n.7): a local fixture server is `http://127.0.0.1:<port>`, and a seam
/// that cannot be reached is not a seam.
fn seam_in_force() -> bool {
    source_base() != SOURCE
}

/// Read the cache only. **Spawns nothing** — this is what `node list` with a warm cache, the
/// TUI's Node screen on entry, the doctor's `public_node_cache` and every offline test call.
///
/// A missing, unreadable or stale-schema file is `Ok(None)`, never an error (§2n.6): it is a
/// third party's document reduced to a cache, and one unparsable file must not break the tool.
pub fn cached(ctx: &Context) -> Result<Option<NodeList>, Error> {
    let path = ctx.paths.public_nodes();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Ok(None),
    };
    let Ok(mut doc) = serde_json::from_slice::<NodeList>(&bytes) else {
        return Ok(None);
    };
    if doc.schema != SCHEMA {
        return Ok(None);
    }
    doc.cache_age_sec = Some(crate::now_unix().saturating_sub(doc.fetched_at).max(0) as u64);
    Ok(Some(doc))
}

fn save(ctx: &Context, doc: &NodeList) -> Result<(), Error> {
    ctx.paths.ensure_dirs()?;
    let mut bytes = serde_json::to_vec_pretty(doc)
        .map_err(|e| Error::failure(format!("cannot serialise the public node list: {e}")))?;
    bytes.push(b'\n');
    crate::write_private(&ctx.paths.public_nodes(), &bytes)
}

/// Write `notice_ack` into the cache (§2n.5 rule 2, §2n.6).
///
/// A missing cache file is created holding nothing but the acknowledgement, because the answer
/// must survive a fetch that then failed — which is the whole reason the record is a field and
/// not the file's existence.
pub fn ack_notice(ctx: &Context) -> Result<(), Error> {
    let mut doc = cached(ctx)?.unwrap_or_else(NodeList::empty);
    doc.notice_ack = Some(crate::now_unix());
    save(ctx, &doc)
}

/// Has §2n.5's notice been acknowledged? An absent or unreadable cache is "no", which is the
/// safe direction.
pub fn notice_acknowledged(ctx: &Context) -> bool {
    cached(ctx)
        .ok()
        .flatten()
        .and_then(|d| d.notice_ack)
        .is_some()
}

/// Add a user-typed address to the cache with `custom: true` (§7a.4's `a` modal).
///
/// Validates through [`Settings::parse_remote_address`], so the TUI cannot store a string the
/// CLI would refuse. A duplicate `address` is a no-op returning the row that is already there,
/// not an error: pressing `a` twice with the same address is a user repeating themselves.
pub fn add_custom(ctx: &Context, address: &str) -> Result<PublicNode, Error> {
    let normalised = Settings::parse_remote_address(address)?;
    let target = split_scheme(&normalised);
    let mut doc = cached(ctx)?.unwrap_or_else(NodeList::empty);
    if let Some(existing) = doc.nodes.iter().find(|n| n.address == target.address) {
        return Ok(existing.clone());
    }
    let node = PublicNode {
        url: normalised.clone(),
        address: target.address.clone(),
        scheme: target.scheme.clone(),
        web_compatible: false,
        custom: true,
        health_available: None,
        health_ratio: None,
        health_checks: None,
        health_last_height: None,
        health_checked_at_text: None,
        health_checked_at: None,
        probe: None,
    };
    doc.nodes.push(node.clone());
    save(ctx, &doc)?;
    Ok(node)
}

// ---------------------------------------------------------------------------------------
// The parsers — pure, public, and driven by the captured fixtures with no process at all
// ---------------------------------------------------------------------------------------

/// `monero.clear` is the candidate set; `monero.web_compatible` is a **flag** on a row and not
/// a second list (§2n.1). Every candidate must pass [`Settings::parse_remote_address`]; the
/// ones that do not are dropped and counted, portless entries separately from the rest.
pub fn parse_nodes_json(bytes: &[u8]) -> Result<ParsedNodes, Error> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(bad_doc)?;
    let monero = v.get("monero").and_then(|m| m.as_object()).ok_or_else(|| {
        Error::upstream("monero.fail's list is not in the shape fmonero expects.")
            .with_reason("rpc_error")
            .with_hint("fmonero node list   # the cached list still works")
    })?;
    let clear = monero
        .get("clear")
        .and_then(|c| c.as_array())
        .ok_or_else(|| {
            Error::upstream("monero.fail's list is not in the shape fmonero expects.")
                .with_reason("rpc_error")
                .with_hint("fmonero node list   # the cached list still works")
        })?;
    let web: std::collections::BTreeSet<&str> = monero
        .get("web_compatible")
        .and_then(|w| w.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
        .unwrap_or_default();

    let mut out = ParsedNodes::default();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entry in clear {
        let Some(url) = entry.as_str() else {
            out.skipped_invalid += 1;
            continue;
        };
        let url = url.trim();
        // `Settings::parse_remote_address` is the gate, and the portless case is one of its
        // refusals — counted separately only because §2n.1 quotes the figure (37 of 195) and a
        // reader comparing 158 to 195 needs it.
        let normalised = match Settings::parse_remote_address(url) {
            Ok(n) => n,
            Err(e) => {
                if e.to_string().contains("the port is required") {
                    out.skipped_no_port += 1;
                } else {
                    out.skipped_invalid += 1;
                }
                continue;
            }
        };
        let target = split_scheme(&normalised);
        if !seen.insert(target.address.clone()) {
            continue;
        }
        out.nodes.push(PublicNode {
            url: url.to_string(),
            address: target.address,
            scheme: target.scheme,
            web_compatible: web.contains(url),
            custom: false,
            health_available: None,
            health_ratio: None,
            health_checks: None,
            health_last_height: None,
            health_checked_at_text: None,
            health_checked_at: None,
            probe: None,
        });
    }
    Ok(out)
}

/// An exact-URL lookup over every bucket `monero` holds, because the bucket name does not
/// partition what is in it (§2n.1) and the question asked per candidate is only ever "what
/// does monero.fail know about this exact string?".
pub fn parse_health_json(bytes: &[u8]) -> Result<HealthMap, Error> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(bad_doc)?;
    let mut map = HealthMap::new();
    // A missing `monero` key is not an error here: health is optional per candidate by
    // construction (a candidate with no entry renders six nulls, §2n.1), so an absent
    // document degrades to "no health for anyone" rather than failing the whole fetch.
    let Some(monero) = v.get("monero").and_then(|m| m.as_object()) else {
        return Ok(map);
    };
    for bucket in monero.values() {
        let Some(rows) = bucket.as_object() else {
            continue;
        };
        for (url, row) in rows {
            let Some(obj) = row.as_object() else { continue };
            let checks = obj.get("checks").and_then(|c| c.as_array());
            // `checks` has NO fixed length — 3 to 94 across `monero.clear` on 2026-09-18 — so
            // the denominator is the list's own length and an empty list is `None`, never 0.0.
            let (ratio, n) = match checks {
                Some(list) if !list.is_empty() => {
                    // `samples`, not `total`: §2a.5's money-shaped-float grep matches
                    // `total … as f64`, and it is right to — a variable called `total` beside an
                    // `as f64` is the shape that grep exists to catch. This one is a count of
                    // health checks, and naming it so keeps the grep sharp instead of widening it.
                    let samples = list.len();
                    let ok = list.iter().filter(|c| c.as_bool() == Some(true)).count();
                    (Some(ok as f64 / samples as f64), Some(samples as u32))
                }
                _ => (None, None),
            };
            let text = obj
                .get("datetime_checked")
                .and_then(|d| d.as_str())
                .map(str::to_string);
            let parsed = text.as_deref().and_then(parse_http_date);
            map.insert(
                url.clone(),
                Health {
                    available: obj.get("available").and_then(|a| a.as_bool()),
                    ratio,
                    checks: n,
                    last_height: obj.get("last_height").and_then(|h| h.as_u64()),
                    checked_at_text: text,
                    checked_at: parsed,
                },
            );
        }
    }
    Ok(map)
}

/// `"Fri, 18 Sep 2026 05:12:02 GMT"` — an HTTP-date, not RFC 3339 (§2n.1). `jiff`'s RFC 2822
/// parser reads it (`b"ut" | b"gmt" | b"z" => Offset::UTC`), so this costs no dependency, and a
/// value that does not parse leaves the age `null` while the string is still rendered.
fn parse_http_date(s: &str) -> Option<i64> {
    jiff::fmt::rfc2822::DateTimeParser::new()
        .parse_timestamp(s)
        .ok()
        .map(|t| t.as_second())
}

fn bad_doc(e: serde_json::Error) -> Error {
    Error::upstream(format!(
        "monero.fail returned something that is not JSON: {e}"
    ))
    .with_reason("rpc_error")
    .with_hint("fmonero node list   # the cached list still works")
}

/// Join the candidate set to the health lookup. Separated so the fixtures can drive it.
fn merge(parsed: ParsedNodes, health: &HealthMap) -> NodeList {
    let mut nodes = parsed.nodes;
    for n in &mut nodes {
        if let Some(h) = health.get(&n.url) {
            n.health_available = h.available;
            n.health_ratio = h.ratio;
            n.health_checks = h.checks;
            n.health_last_height = h.last_height;
            n.health_checked_at_text = h.checked_at_text.clone();
            n.health_checked_at = h.checked_at;
        }
    }
    NodeList {
        schema: SCHEMA,
        fetched_at: crate::now_unix(),
        source: source_base(),
        skipped_no_port: parsed.skipped_no_port,
        skipped_invalid: parsed.skipped_invalid,
        notice_ack: None,
        nodes,
        cache_age_sec: Some(0),
    }
}

// ---------------------------------------------------------------------------------------
// curl — the entire outbound surface, three argv shapes and no more (§2n.3)
// ---------------------------------------------------------------------------------------

/// `curl` on PATH, or exit 4 `curl_missing` **before any network call** — the same shape §2i's
/// preflight gives for `monerod` (§2n.3).
fn require_curl() -> Result<(), Error> {
    if proc::which("curl").is_some() {
        return Ok(());
    }
    Err(
        Error::backend("curl is not installed. fmonero needs it to get the node list.")
            .with_reason("curl_missing")
            .with_hint("omarchy pkg add curl"),
    )
}

/// argv shape 1 (§2n.3), built here and nowhere else.
fn fetch_argv<'a>(url: &'a str, max_time: &'a str, proto: &'a str) -> Vec<&'a str> {
    vec![
        "curl",
        "-sS",
        "--fail",
        "--proto",
        proto,
        "--max-time",
        max_time,
        "--max-filesize",
        "8388608",
        "-H",
        "Accept: application/json",
        "--url",
        url,
    ]
}

/// argv shape 2 (§2n.3). `--fail` is deliberately absent: a restricted node answering a
/// JSON-RPC error with a non-200 is still an answer we want to read.
fn probe_argv<'a>(url: &'a str, max_time: &'a str) -> Vec<&'a str> {
    vec![
        "curl",
        "-sS",
        "-m",
        max_time,
        "--proto",
        "=http,https",
        "--max-filesize",
        "65536",
        "-H",
        "Content-Type: application/json",
        "-d",
        PROBE_BODY,
        "--url",
        url,
    ]
}

/// GET one monero.fail document. The only computed arguments are the deadline and the URL,
/// and the URL's host is this module's own constant (or the test seam's, §2n.7).
fn fetch_doc(name: &str, cancel: &CancelToken) -> Result<Vec<u8>, Error> {
    let url = format!("{}/{name}", source_base());
    let max_time = FETCH_MAX_TIME_SEC.to_string();
    let proto = if seam_in_force() {
        "=http,https"
    } else {
        "=https"
    };
    let argv = fetch_argv(&url, &max_time, proto);
    let out = proc::run_with_deadline(
        &argv,
        Duration::from_secs(FETCH_MAX_TIME_SEC + DEADLINE_SLACK_SEC),
        Some(cancel),
    )?;
    // **A fetch that did not happen is exit 4 `unreachable`, not exit 5 `upstream`.** The two
    // exits mean different things in §4a: 5 is "it answered and the answer was an error", 4 is
    // "the thing we needed is not usable from here". monero.fail being down, a dropped
    // connection and a 503 are all the second, and the cached list still works in every one of
    // them — which is what the hint says.
    let unreachable = |detail: String| {
        Error::backend(format!("Could not reach {url}: {detail}"))
            .with_reason("unreachable")
            .with_hint("fmonero node list   # the cached list still works")
    };
    if out.timed_out {
        return Err(unreachable("no answer before the deadline".to_string()));
    }
    if !out.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(unreachable(if stderr.is_empty() {
            format!("curl exited {:?}", out.code)
        } else {
            stderr
        }));
    }
    // The byte cap fmonero enforces itself, independent of the curl on PATH (§2n.3).
    if out.stdout.len() > LIST_MAX_BYTES {
        return Err(Error::upstream(format!(
            "{url} answered with more than {LIST_MAX_BYTES} bytes"
        ))
        .with_reason("rpc_error")
        .with_hint("fmonero node list   # the cached list still works"));
    }
    Ok(out.stdout)
}

/// Fetch and parse both documents and merge them into candidates (§2n.1). Separated from
/// [`list`] because it is the half the fixtures test and the half with no cache semantics.
pub fn fetch(ctx: &Context, cancel: &CancelToken) -> Result<NodeList, Error> {
    let _ = ctx;
    require_curl()?;
    let nodes_doc = fetch_doc("nodes.json", cancel)?;
    let parsed = parse_nodes_json(&nodes_doc)?;
    // health.json is the optional half: a failure there costs the health columns, not the
    // list, because a candidate with no entry is already a rendered outcome (§2n.1).
    let health = match fetch_doc("health.json", cancel) {
        Ok(bytes) => parse_health_json(&bytes).unwrap_or_default(),
        Err(e) if e.is_interrupted() => return Err(e),
        Err(_) => HealthMap::new(),
    };
    Ok(merge(parsed, &health))
}

/// One probe (§2n.4). Also the configured-node probe, called by the TUI and by nothing else
/// automatic — `status`, the widget and the doctor never call it (§2e).
pub fn probe_one(ctx: &Context, target: &ProbeTarget, cancel: &CancelToken) -> Probe {
    let _ = ctx;
    if cancel.is_cancelled() {
        return Probe::failed("cancelled", None);
    }
    // `curl_missing` is an exit-4 refusal raised **before** any network call by `list`, so by the
    // time a probe runs curl is known to exist. This is the belt for a caller that reached
    // `probe_one` directly (the TUI's configured-node probe does), and it answers with a reason
    // rather than an error because `probe_many` must return one result per input.
    if require_curl().is_err() {
        return Probe::failed("refused", None);
    }
    let url = target.json_rpc_url();
    let max_time = PROBE_MAX_TIME_SEC.to_string();
    let argv = probe_argv(&url, &max_time);
    let out = match proc::run_with_deadline(
        &argv,
        Duration::from_secs(PROBE_MAX_TIME_SEC + DEADLINE_SLACK_SEC),
        Some(cancel),
    ) {
        Ok(o) => o,
        Err(e) if e.is_interrupted() => return Probe::failed("cancelled", None),
        Err(_) => return Probe::failed("refused", None),
    };
    let latency = Some(out.duration.as_millis().min(u128::from(u64::MAX)) as u64);
    if out.timed_out {
        return Probe::failed("timed_out", latency);
    }
    if let Some(code) = out.code.filter(|c| *c != 0) {
        return Probe::failed(curl_reason(code), latency);
    }
    if out.stdout.len() > PROBE_MAX_BYTES {
        return Probe::failed("bad_json", latency);
    }
    interpret_get_info(&out.stdout, latency)
}

/// curl's own exit codes, mapped to §2n.4's reason set. Anything unclassified is `http`,
/// which is the honest answer for "it answered in a way we could not use".
fn curl_reason(code: i32) -> &'static str {
    match code {
        28 => "timed_out",
        6 | 7 => "refused",
        35 | 51 | 58 | 59 | 60 | 66 | 77 | 82 | 83 | 91 => "tls",
        _ => "http",
    }
}

/// Read a `get_info` answer, `status` first (§1b.3, §2n.4).
///
/// Accepts both the JSON-RPC envelope (`{"result":{…}}`) and a bare result object, because
/// `monerod` sends the former and the transcript in §2n.4 was recorded trimmed — a parser that
/// only accepted one of the two would call a working node `bad_json`.
fn interpret_get_info(stdout: &[u8], latency_ms: Option<u64>) -> Probe {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(stdout) else {
        return Probe::failed("bad_json", latency_ms);
    };
    if v.get("error").map(|e| !e.is_null()).unwrap_or(false) {
        // A JSON-RPC error is still an answer: the node is reachable and told us no.
        return Probe::answered(latency_ms);
    }
    let result = match v.get("result") {
        Some(r) if r.is_object() => r,
        _ if v.is_object() => &v,
        _ => return Probe::failed("bad_json", latency_ms),
    };
    match result.get("status").and_then(|s| s.as_str()) {
        Some("OK") => {}
        // `"BUSY"` is the answer for an entire initial sync and its payload is
        // value-initialised — every numeric field zero (§1b.3). Rendering those zeros is the
        // untruth §2j forbids, so the row is `reachable: true` with nulls.
        Some(_) => return Probe::answered(latency_ms),
        None => return Probe::failed("bad_json", latency_ms),
    }
    // A restricted node returns `version: ""`; that is the same null rule §5 applies, in one
    // more place, and not a second rule.
    let version = result
        .get("version")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Probe {
        reachable: true,
        height: result.get("height").and_then(|h| h.as_u64()),
        synchronized: result.get("synchronized").and_then(|s| s.as_bool()),
        restricted: result.get("restricted").and_then(|r| r.as_bool()),
        nettype: result
            .get("nettype")
            .and_then(|n| n.as_str())
            .map(str::to_string),
        version,
        latency_ms,
        reason: None,
        probed_at: crate::now_unix(),
    }
}

/// Probe up to [`PROBE_CONCURRENCY`] at a time and return the results **in the input's
/// order**. A fixed-size pool over a shared index, not one thread per candidate.
pub fn probe_many(ctx: &Context, targets: &[ProbeTarget], cancel: &CancelToken) -> Vec<Probe> {
    let n = targets.len();
    if n == 0 {
        return Vec::new();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Probe>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let workers = PROBE_CONCURRENCY.min(n);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= n {
                    break;
                }
                let probe = if cancel.is_cancelled() {
                    Probe::failed("cancelled", None)
                } else {
                    probe_one(ctx, &targets[i], cancel)
                };
                if let Ok(mut slot) = slots[i].lock() {
                    *slot = Some(probe);
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|m| {
            m.into_inner()
                .ok()
                .flatten()
                .unwrap_or_else(|| Probe::failed("cancelled", None))
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// The one flow `node list` and the TUI share
// ---------------------------------------------------------------------------------------

/// §2n's whole flow: [`cached`], then fetch when `opts.refresh` or the cache is absent or
/// older than [`CACHE_MAX_AGE_SEC`], then probe when `opts.probe`. Writes the cache on any
/// change, cancellable at every step, and `Error::Backend{reason:"curl_missing"}` when `curl`
/// is not on PATH — raised before any network call.
pub fn list(ctx: &Context, opts: ListOpts, cancel: &CancelToken) -> Result<NodeList, Error> {
    let existing = cached(ctx)?;
    let must_fetch = opts.refresh || existing.as_ref().map(NodeList::is_stale).unwrap_or(true);
    let mut doc = if must_fetch {
        let fetched = fetch(ctx, cancel)?;
        // Parse in full, THEN replace: a partially written list is the one outcome that would
        // make the picker show a node that is not on it (§2n.6). `probe` results and `custom`
        // rows survive the refresh, matched by address.
        let merged = carry_forward(fetched, existing.as_ref());
        save(ctx, &merged)?;
        merged
    } else {
        existing.expect("must_fetch is true when the cache is absent")
    };

    if opts.probe {
        let limit = effective_limit(&opts, doc.nodes.len());
        sort_for_display(&mut doc.nodes, configured_address(ctx).as_deref());
        let targets: Vec<ProbeTarget> = doc
            .nodes
            .iter()
            .take(limit)
            .map(ProbeTarget::from_node)
            .collect();
        let probes = probe_many(ctx, &targets, cancel);
        for (i, probe) in probes.into_iter().enumerate() {
            doc.nodes[i].probe = Some(probe);
        }
        save(ctx, &doc)?;
        // The configured node's own probe is cached in state.json too, so `status --json` and
        // the bar widget can render a remote height without spawning anything (§2n.4, §2e).
        if let Some(configured) = configured_address(ctx) {
            if let Some(row) = doc.nodes.iter().find(|n| n.address == configured) {
                if let Some(p) = &row.probe {
                    let _ = crate::state::note_remote_probe(ctx, &configured, p);
                }
            }
        }
    }
    doc.cache_age_sec = Some(crate::now_unix().saturating_sub(doc.fetched_at).max(0) as u64);
    Ok(doc)
}

/// How many rows this invocation prints and probes (§4.1): `--all` means every usable
/// candidate, otherwise `--limit` with [`DEFAULT_LIMIT`] as its default.
pub fn effective_limit(opts: &ListOpts, total: usize) -> usize {
    if opts.all {
        total
    } else if opts.limit == 0 {
        DEFAULT_LIMIT.min(total)
    } else {
        opts.limit.min(total)
    }
}

/// A refresh **merges** rather than replaces, in exactly two respects (§2n.6): a `probe`
/// survives, and a `custom` row is preserved whether or not monero.fail lists it. Without the
/// second, a hand-typed LAN node would vanish on the next `f`.
fn carry_forward(mut fresh: NodeList, old: Option<&NodeList>) -> NodeList {
    let Some(old) = old else { return fresh };
    fresh.notice_ack = old.notice_ack;
    let by_url: BTreeMap<&str, &PublicNode> =
        old.nodes.iter().map(|n| (n.url.as_str(), n)).collect();
    let fresh_addrs: std::collections::BTreeSet<String> =
        fresh.nodes.iter().map(|n| n.address.clone()).collect();
    for n in &mut fresh.nodes {
        if let Some(prev) = by_url.get(n.url.as_str()) {
            n.probe = prev.probe.clone();
            // A custom entry whose url now appears in nodes.json keeps `custom` and gains
            // this fetch's health columns, because it is then both.
            n.custom = n.custom || prev.custom;
        }
    }
    for prev in &old.nodes {
        if prev.custom && !fresh_addrs.contains(&prev.address) {
            fresh.nodes.push(prev.clone());
        }
    }
    fresh
}

/// The address in force, as `units::daemon_target` renders it — the string the picker marks
/// `●` and the one `state::note_remote_probe` stores beside a probe.
pub fn configured_address(ctx: &Context) -> Option<String> {
    match ctx.settings.node.mode {
        crate::settings::NodeMode::Remote => {
            let remote = ctx.settings.node.remote.trim();
            (!remote.is_empty()).then(|| split_scheme(remote).address)
        }
        crate::settings::NodeMode::Local => None,
    }
}

/// Write a batch of probe results into the cache, matched by `address` (§2n.6).
///
/// The TUI's `p` needs this because its probes run in an `ActionSub` and the cache is the only
/// place a measurement survives the screen being left. A probe of an address that is no longer
/// in the cache — a `custom` row the user removed by hand — is dropped rather than re-added: the
/// cache's candidate set is the list's, not the probe's.
pub fn record_probes(ctx: &Context, probes: &[(String, Probe)]) -> Result<(), Error> {
    let Some(mut doc) = cached(ctx)? else {
        return Ok(());
    };
    let mut touched = false;
    for (address, probe) in probes {
        if let Some(row) = doc.nodes.iter_mut().find(|n| &n.address == address) {
            row.probe = Some(probe.clone());
            touched = true;
        }
    }
    if touched {
        save(ctx, &doc)?;
    }
    Ok(())
}

/// The display order of §2n.6: the configured address first, then descending health, then
/// ascending latency among probed. Exposed because the CLI and the TUI both need it and two
/// sorts that must agree are two sorts that will not (§2a.3).
pub fn sort_for_display(nodes: &mut [PublicNode], configured: Option<&str>) {
    nodes.sort_by(|a, b| {
        let key = |n: &PublicNode| {
            let is_configured = configured.map(|c| c == n.address).unwrap_or(false);
            // Sorting keys, in order: configured first; then reachable-if-probed; then health
            // descending (as an integer permille so the comparison needs no float ordering);
            // then latency ascending; then the address, so the order is total and a refresh
            // cannot shuffle two equal rows.
            let reachable = n.probe.as_ref().map(|p| p.reachable);
            (
                !is_configured,
                match reachable {
                    Some(true) => 0u8,
                    None => 1,
                    Some(false) => 2,
                },
                std::cmp::Reverse((n.health_ratio.unwrap_or(0.0) * 1000.0) as i64),
                n.probe
                    .as_ref()
                    .and_then(|p| p.latency_ms)
                    .unwrap_or(u64::MAX),
            )
        };
        key(a).cmp(&key(b)).then_with(|| a.address.cmp(&b.address))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_shape_1_is_exactly_the_table() {
        let argv = fetch_argv("https://monero.fail/nodes.json", "20", "=https");
        assert_eq!(
            argv,
            vec![
                "curl",
                "-sS",
                "--fail",
                "--proto",
                "=https",
                "--max-time",
                "20",
                "--max-filesize",
                "8388608",
                "-H",
                "Accept: application/json",
                "--url",
                "https://monero.fail/nodes.json",
            ]
        );
    }

    #[test]
    fn argv_shape_2_is_exactly_the_table() {
        let target = ProbeTarget {
            scheme: "http".into(),
            address: "xmr-node.cakewallet.com:18081".into(),
        };
        let url = target.json_rpc_url();
        assert_eq!(url, "http://xmr-node.cakewallet.com:18081/json_rpc");
        let argv = probe_argv(&url, "5");
        assert_eq!(
            argv,
            vec![
                "curl",
                "-sS",
                "-m",
                "5",
                "--proto",
                "=http,https",
                "--max-filesize",
                "65536",
                "-H",
                "Content-Type: application/json",
                "-d",
                r#"{"jsonrpc":"2.0","id":"0","method":"get_info"}"#,
                "--url",
                "http://xmr-node.cakewallet.com:18081/json_rpc",
            ]
        );
        // The URL is the LAST argument and it follows `--url`, so a value beginning with `-`
        // could never be read as an option (§2n.3, revision 2).
        assert_eq!(argv[argv.len() - 2], "--url");
    }

    /// The revision-2 fixture: a hostile publisher's rows are dropped and counted, and the
    /// survivor's argv is built from the validated address.
    #[test]
    fn hostile_candidates_are_dropped_and_counted() {
        let doc = br#"{"monero":{"clear":[
            "-o/tmp/pwned",
            "--config/tmp/x",
            "http://user:pass@real.example:18081",
            "http://a.example:18081@b.example:18081",
            "http://good.example:18081/json_rpc",
            "http://good.example:18081?x=1",
            "http://good.example:18081#frag",
            "portless.example",
            "http://good.example:18081"
        ],"web_compatible":[]},"unknown":{"x":1}}"#;
        let parsed = parse_nodes_json(doc).expect("parses");
        assert_eq!(
            parsed.nodes.len(),
            1,
            "only the clean row survives: {:?}",
            parsed.nodes.iter().map(|n| &n.url).collect::<Vec<_>>()
        );
        assert_eq!(parsed.nodes[0].address, "good.example:18081");
        assert_eq!(parsed.skipped_no_port, 1, "the portless row");
        assert_eq!(
            parsed.skipped_invalid, 7,
            "everything else the grammar refuses"
        );
        let target = ProbeTarget::from_node(&parsed.nodes[0]);
        assert_eq!(
            target.json_rpc_url(),
            "http://good.example:18081/json_rpc",
            "the probe URL is reconstructed from the validated address"
        );
    }

    #[test]
    fn health_ratio_uses_the_lists_own_length() {
        let doc = br#"{"monero":{"clear":{
            "http://three.example:18081":{"available":true,"checks":[true,false,true],
              "datetime_checked":"Fri, 18 Sep 2026 05:12:02 GMT","last_height":100},
            "http://empty.example:18081":{"available":false,"checks":[]},
            "http://none.example:18081":{"available":false}
        }}}"#;
        let map = parse_health_json(doc).expect("parses");
        let three = &map["http://three.example:18081"];
        // 2 of 3, not 2 of any hard-coded denominator.
        assert!((three.ratio.unwrap() - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(three.checks, Some(3));
        assert_eq!(three.last_height, Some(100));
        assert!(three.checked_at.is_some(), "the HTTP-date parsed");
        // An empty or absent list is None, NEVER 0.0 (§2n.1).
        assert_eq!(map["http://empty.example:18081"].ratio, None);
        assert_eq!(map["http://none.example:18081"].ratio, None);
    }

    #[test]
    fn web_compatible_is_a_flag_and_not_a_second_list() {
        let doc = br#"{"monero":{"clear":["https://w.example:18089","http://p.example:18081"],
                                  "web_compatible":["https://w.example:18089"]}}"#;
        let parsed = parse_nodes_json(doc).expect("parses");
        assert_eq!(
            parsed.nodes.len(),
            2,
            "no duplicate row for the flagged one"
        );
        let w = parsed
            .nodes
            .iter()
            .find(|n| n.address == "w.example:18089")
            .unwrap();
        assert!(w.web_compatible);
        assert_eq!(w.scheme, "https");
        let p = parsed
            .nodes
            .iter()
            .find(|n| n.address == "p.example:18081")
            .unwrap();
        assert!(!p.web_compatible);
        assert_eq!(p.scheme, "http", "`clear` is not `plaintext` (§2n.1)");
    }

    #[test]
    fn busy_is_reachable_with_nulls_and_never_zeros() {
        let body = br#"{"id":"0","jsonrpc":"2.0","result":{"height":0,"target_height":0,
            "synchronized":false,"status":"BUSY","untrusted":false}}"#;
        let p = interpret_get_info(body, Some(12));
        assert!(p.reachable);
        assert_eq!(p.height, None, "a BUSY node's zeros are never rendered");
        assert_eq!(p.synchronized, None);
        assert_eq!(p.reason, None);
    }

    #[test]
    fn a_restricted_nodes_empty_version_is_null() {
        let body = br#"{"height":3765172,"synchronized":true,"restricted":true,"version":"",
            "target_height":0,"nettype":"mainnet","status":"OK"}"#;
        let p = interpret_get_info(body, Some(100));
        assert!(p.reachable);
        assert_eq!(p.height, Some(3_765_172));
        assert_eq!(p.synchronized, Some(true));
        assert_eq!(p.restricted, Some(true));
        assert_eq!(p.version, None, "\"\" is null, not an empty string (§1b.3)");
        assert_eq!(p.nettype.as_deref(), Some("mainnet"));
    }

    #[test]
    fn garbage_is_bad_json() {
        assert_eq!(
            interpret_get_info(b"<html>nope</html>", None).reason,
            Some("bad_json")
        );
    }

    #[test]
    fn sort_puts_the_configured_node_first() {
        let node = |addr: &str, ratio: Option<f64>, latency: Option<u64>| PublicNode {
            url: format!("http://{addr}"),
            address: addr.to_string(),
            scheme: "http".into(),
            web_compatible: false,
            custom: false,
            health_available: Some(true),
            health_ratio: ratio,
            health_checks: Some(10),
            health_last_height: None,
            health_checked_at_text: None,
            health_checked_at: None,
            probe: latency.map(|ms| Probe {
                reachable: true,
                height: Some(1),
                synchronized: Some(true),
                restricted: None,
                nettype: None,
                version: None,
                latency_ms: Some(ms),
                reason: None,
                probed_at: 0,
            }),
        };
        let mut rows = vec![
            node("healthy.example:18081", Some(0.99), None),
            node("mine.example:18081", Some(0.10), None),
            node("fast.example:18081", Some(0.50), Some(5)),
            node("slow.example:18081", Some(0.50), Some(500)),
        ];
        sort_for_display(&mut rows, Some("mine.example:18081"));
        assert_eq!(rows[0].address, "mine.example:18081");
        // Probed rows come before unprobed, fastest first.
        assert_eq!(rows[1].address, "fast.example:18081");
        assert_eq!(rows[2].address, "slow.example:18081");
        assert_eq!(rows[3].address, "healthy.example:18081");
    }

    #[test]
    fn a_probe_target_cannot_be_built_from_a_hostile_string() {
        assert!(ProbeTarget::parse("-o/tmp/pwned").is_err());
        assert!(ProbeTarget::parse("http://u:p@h.example:18081").is_err());
        assert!(ProbeTarget::parse("h.example").is_err(), "no port");
        let t = ProbeTarget::parse("https://h.example:18089").expect("valid");
        assert_eq!(t.scheme, "https");
        assert_eq!(t.address, "h.example:18089");
    }

    /// The captured fixtures (§2n.7) — the repo's only CAPTURED files, real documents from the
    /// 2026-09-18 fetch trimmed by selection only. They drive the two parsers with no process at
    /// all, which is what makes every claim in §2n.1 checkable offline.
    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/nodes")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn the_captured_nodes_json_parses_the_way_2n1_says_it_does() {
        let parsed = parse_nodes_json(&fixture("nodes.json")).expect("parses");
        // 10 candidates: 2 portless (dropped and counted) and 2 spellings of one host:port
        // (http:// and https://xmr-node.cakewallet.com:18081) that collapse to ONE row, because
        // `address` is the identity — the picker, the confirm modal and `node remote` all speak
        // host:port and two rows for one node would be two ways to choose the same thing.
        assert_eq!(parsed.skipped_no_port, 2, "the two portless entries");
        assert_eq!(parsed.skipped_invalid, 0, "nothing else was refused");
        assert_eq!(
            parsed.nodes.len(),
            7,
            "10 - 2 portless - 1 duplicate address"
        );
        let by = |addr: &str| {
            parsed
                .nodes
                .iter()
                .find(|n| n.address == addr)
                .unwrap_or_else(|| panic!("{addr} missing"))
        };
        // `clear` means "not onion and not i2p", NOT "plaintext": both schemes are present.
        assert_eq!(by("xmr-node.cakewallet.com:18081").scheme, "http");
        assert_eq!(by("node.sethforprivacy.com:443").scheme, "https");
        // `web_compatible` is a FLAG on a row and not a second list.
        assert!(by("monero-rpc.cheems.de.box.skhron.com.ua:18089").web_compatible);
        assert!(!by("nodes.hashvault.pro:18081").web_compatible);
        // The `_provenance` string and the empty onion/i2p/ipv6 buckets are ignored, not read.
        assert!(!parsed.nodes.iter().any(|n| n.url.contains("captured from")));
    }

    #[test]
    fn the_captured_health_json_is_an_exact_url_lookup() {
        let health = parse_health_json(&fixture("health.json")).expect("parses");
        // `checks` has no fixed length: 4 and 86 in this capture. A hard-coded 32 would report
        // 269 % for the long one (§2n.1).
        assert_eq!(health["https://node.xmr.pub:443"].checks, Some(4));
        assert_eq!(
            health["https://node.sethforprivacy.com:443"].checks,
            Some(86)
        );
        for (url, h) in &health {
            if let (Some(r), Some(_)) = (h.ratio, h.checks) {
                assert!((0.0..=1.0).contains(&r), "{url} ratio {r} out of range");
            }
        }
        // A real `available: false` row, read as a boolean and not as a presence test.
        assert_eq!(health["http://116.202.196.52:18081"].available, Some(false));
        // The HTTP-dates parse (they are RFC 2822, not RFC 3339).
        assert!(health["http://xmr-node.cakewallet.com:18081"]
            .checked_at
            .is_some());
    }

    #[test]
    fn a_candidate_absent_from_health_json_renders_nulls_and_is_not_an_error() {
        let parsed = parse_nodes_json(&fixture("nodes.json")).expect("parses");
        let health = parse_health_json(&fixture("health.json")).expect("parses");
        let list = merge(parsed, &health);
        let orphan = list
            .nodes
            .iter()
            .find(|n| n.address == "xmr-de-2.boldsuck.org:18081")
            .expect("the candidate whose health entry the fixture drops");
        assert_eq!(orphan.health_available, None);
        assert_eq!(orphan.health_ratio, None, "None, never 0.0 (§2n.1)");
        assert_eq!(orphan.health_checks, None);
        assert_eq!(orphan.health_last_height, None);
        assert_eq!(orphan.health_checked_at_text, None);
        assert_eq!(orphan.health_checked_at, None);
        // And a candidate that IS in health.json carries all six.
        let known = list
            .nodes
            .iter()
            .find(|n| n.address == "xmr-node.cakewallet.com:18081")
            .unwrap();
        assert_eq!(known.health_available, Some(true));
        assert!(known.health_ratio.is_some());
        assert!(known.health_checked_at_text.is_some());
    }

    #[test]
    fn a_custom_row_survives_a_refresh_and_a_probe_does_too() {
        let fresh = || {
            let parsed = parse_nodes_json(&fixture("nodes.json")).expect("parses");
            merge(parsed, &HealthMap::new())
        };
        let mut old = fresh();
        // A node the user typed by hand, which monero.fail has never heard of.
        old.nodes.push(PublicNode {
            url: "http://lan.example:18081".into(),
            address: "lan.example:18081".into(),
            scheme: "http".into(),
            web_compatible: false,
            custom: true,
            health_available: None,
            health_ratio: None,
            health_checks: None,
            health_last_height: None,
            health_checked_at_text: None,
            health_checked_at: None,
            probe: None,
        });
        // And a measurement of a listed one.
        old.nodes[0].probe = Some(Probe::answered(Some(42)));
        let carried = carry_forward(fresh(), Some(&old));
        assert!(
            carried
                .nodes
                .iter()
                .any(|n| n.address == "lan.example:18081" && n.custom),
            "the hand-typed node must not vanish on the next `f` (§2n.6)"
        );
        assert_eq!(
            carried.nodes[0].probe.as_ref().and_then(|p| p.latency_ms),
            Some(42),
            "re-probing 158 nodes because the list moved would make `f` cost what `p` costs"
        );
    }

    #[test]
    fn an_unparsable_cache_is_none_and_not_an_error() {
        let doc: Result<NodeList, _> = serde_json::from_slice(b"{\"schema\":1}");
        assert!(doc.is_err(), "a truncated document does not deserialise");
    }
}
