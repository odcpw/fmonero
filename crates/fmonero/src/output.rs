//! Output policy and rendering (DESIGN.md §4).
//!
//! * `--json`, or a non-TTY stdout, means one JSON object on stdout and nothing else. The
//!   three exceptions are named in §4 and enforced in `main.rs`: `robot-docs` and `--help`
//!   are always plain text, and `tui` exits 2 without a terminal.
//! * stdout is data; progress and diagnostics go to stderr.
//! * Colour is off when stdout is not a TTY, with `--no-color`, or when `NO_COLOR` or `CI`
//!   is set.
//! * **Core never prints**: every byte a user sees is written here (§2g).
//!
//! `main.rs` builds the §5c payload for every command and hands it to [`Out::emit`] with a
//! label; this module decides how a human sees it. That split is what lets Phase 1b enrich
//! the rendering without touching `main.rs`, which is frozen at the 1a gate (§2k).

use fmonero_core::Error;
use unicode_width::UnicodeWidthStr;

/// True when stdout is a terminal.
pub fn stdout_is_tty() -> bool {
    unsafe { libc::isatty(1) == 1 }
}

/// ANSI attributes, used only through [`Out::paint`].
#[derive(Clone, Copy)]
pub enum Paint {
    Dim,
    Green,
    Yellow,
    Red,
}

impl Paint {
    fn code(self) -> &'static str {
        match self {
            Paint::Dim => "\x1b[2m",
            Paint::Green => "\x1b[32m",
            Paint::Yellow => "\x1b[33m",
            Paint::Red => "\x1b[31m",
        }
    }
}

/// `-v` repetitions, recorded once in `main` so that `setup`, `doctor` and the TUI — which
/// build their own [`Out`] from a `Context` — honour the same verbosity.
static VERBOSE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// `--no-color`, recorded the same way.
static NO_COLOR_FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The resolved output mode for one invocation (§5b).
#[derive(Copy, Clone, Debug)]
pub struct Out {
    pub json: bool,
    pub color: bool,
}

impl Out {
    /// Apply the whole §4 policy once, at startup.
    pub fn new(json_flag: bool, no_color: bool, verbose: u8) -> Out {
        use std::sync::atomic::Ordering;
        if no_color {
            NO_COLOR_FLAG.store(true, Ordering::SeqCst);
        }
        VERBOSE.store(verbose, Ordering::SeqCst);
        let tty = stdout_is_tty();
        let env_no_color =
            std::env::var_os("NO_COLOR").is_some() || std::env::var_os("CI").is_some();
        Out {
            // A non-TTY stdout implies --json for every command that has a JSON payload (§4).
            json: json_flag || !tty,
            color: tty && !NO_COLOR_FLAG.load(Ordering::SeqCst) && !env_no_color,
        }
    }

    /// Force JSON for a command that is always machine output (`capabilities`, §4).
    #[must_use]
    pub fn forced_json(mut self) -> Out {
        self.json = true;
        self
    }

    pub fn paint(&self, text: &str, p: Paint) -> String {
        if self.color {
            format!("{}{text}\x1b[0m", p.code())
        } else {
            text.to_string()
        }
    }

    /// One JSON object, or the human rendering of the same payload. The only place either
    /// is printed. `label` is the command, so a renderer can special-case it.
    ///
    /// The **notes** are emitted in both modes, because they go to stderr and stderr is not
    /// the data stream (§4): a JSON consumer's payload stays exactly the §5c key set while a
    /// human still gets the footer §4.1 promises.
    pub fn emit(&self, label: &str, value: serde_json::Value) {
        if self.json {
            println!("{value}");
        } else {
            human(self, label, &value);
        }
        for note in footers(label, &value) {
            self.note(note);
        }
    }

    /// A line of data on stdout.
    pub fn line(&self, text: impl AsRef<str>) {
        println!("{}", text.as_ref());
    }

    /// A diagnostic on stderr. Never part of the data stream.
    pub fn note(&self, text: impl AsRef<str>) {
        eprintln!("{}", text.as_ref());
    }

    /// A diagnostic shown only with `-v`.
    pub fn verbose_note(&self, text: impl AsRef<str>) {
        if VERBOSE.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            eprintln!("{}", text.as_ref());
        }
    }

    /// Print an error the way the caller asked for it and return its exit code (§4).
    pub fn report(&self, e: &Error) -> i32 {
        if self.json {
            println!("{}", e.json());
        } else {
            eprintln!(
                "{}: {}",
                self.paint(&format!("fmonero: error[{}]", e.code_name()), Paint::Red),
                e
            );
            if let Some(reason) = e.reason() {
                eprintln!("  reason: {reason}");
            }
            // v5.6, §2o: the technical text — a transport error, an RPC method name, a serde
            // message — is no longer in the sentence, and this is one of its two homes (the
            // other is `--json`'s `detail`). Behind `-v`, because the person who did not ask
            // for it is the person §2o is about.
            if let Some(detail) = e.detail() {
                self.verbose_note(format!("  detail: {detail}"));
            }
            // §2o rule 11: `hint:` labelled a field; `Try:` is an instruction. The command
            // itself is unchanged — the CLI's own hints are the point of §9.
            eprintln!("  Try: {}", e.hint());
        }
        e.exit_code()
    }
}

// ---------------------------------------------------------------- formatters

/// A block number as a person reads it: `3 765 346`, a space every three digits (§2o rule 9).
///
/// **The implementation is [`fmonero_core::grouped`]** and this is a re-export (W7). It lived
/// here while the callers were the TUI and the doctor; core's own warnings were the caller that
/// could not reach it, so `This wallet has 6122 blocks to go.` stood in the same panel as
/// `6 122 blocks left`. One formatter, in the crate every surface already depends on.
pub use fmonero_core::grouped;

/// `1.5 GB`, `842 kB` — decimal units, the way a disk meter reads.
///
/// Integer arithmetic, with one decimal place carried by hand. Bytes are not money, but
/// §2a.5's enforcement is a grep for float casts across `crates/*/src`, and a formatter that
/// makes that grep fire is a formatter somebody later exempts — at which point the grep no
/// longer protects the amount paths it exists for.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut unit = 0;
    let mut scale: u64 = 1;
    while unit + 1 < UNITS.len() && n / scale >= 1000 {
        scale *= 1000;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        let whole = n / scale;
        let tenths = (n % scale) * 10 / scale;
        format!("{whole}.{tenths} {}", UNITS[unit])
    }
}

/// `3d 04h`, `2h 07m`, `45s`.
pub fn human_uptime(secs: i64) -> String {
    let secs = secs.max(0) as u64;
    let (d, h, m, s) = (
        secs / 86400,
        (secs % 86400) / 3600,
        (secs % 3600) / 60,
        secs % 60,
    );
    if d > 0 {
        format!("{d}d {h:02}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Pad to `width` display columns (CJK- and emoji-safe).
pub fn pad(text: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(text);
    if w >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - w))
    }
}

/// One JSON value as a human cell. `null` is **`—`**, never `0` and never blank: §5's rule
/// that a zero may not stand in for unknown is a rendering rule too.
fn cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "—".to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(a) => a.iter().map(cell).collect::<Vec<_>>().join(", "),
        serde_json::Value::Object(o) => {
            let inner: Vec<String> = o.iter().map(|(k, v)| format!("{k}={}", cell(v))).collect();
            inner.join(" ")
        }
    }
}

/// The human rendering of a command's payload.
///
/// The generic form — one `key  value` line per scalar, one indented block per nested
/// object, one row per array entry — is honest for every §5c payload, and `status` gets its
/// own summary on top. Phase 1b enriches the branches here; `main.rs` never changes.
pub fn human(out: &Out, label: &str, value: &serde_json::Value) {
    match label {
        "status" => return status_human(out, value),
        "wallet send" | "wallet sweep" => return money_human(out, value),
        "wallet history" => return history_human(out, value),
        "wallet balance" => return balance_human(out, value),
        "wallet address" | "wallet new-address" => return address_human(out, value),
        "wallet list" => return list_human(out, value),
        "node peers" => return peers_human(out, value),
        "node list" => return nodes_human(out, value),
        "node log" => return log_human(out, value),
        "settings list" => return settings_human(out, value),
        "setup" => return setup_human(out, value),
        // v5.2: the two mode commands share one key set and therefore one renderer (§5c).
        "node remote" | "node local" => return mode_human(out, value),
        _ => {}
    }
    let Some(map) = value.as_object() else {
        out.line(cell(value));
        return;
    };
    let width = map
        .keys()
        .filter(|k| *k != "ok" && *k != "schema")
        .map(|k| UnicodeWidthStr::width(k.as_str()))
        .max()
        .unwrap_or(0)
        .min(24);
    for (key, v) in map {
        if key == "ok" || key == "schema" {
            continue; // envelope keys: the human form is the absence of an error
        }
        match v {
            serde_json::Value::Array(rows) if rows.iter().any(|r| r.is_object()) => {
                out.line(format!("{}:", key));
                for row in rows {
                    out.line(format!("  {}", cell(row)));
                }
            }
            serde_json::Value::Object(inner) => {
                out.line(format!("{}:", key));
                for (k2, v2) in inner {
                    out.line(format!("  {} {}", pad(k2, width), cell(v2)));
                }
            }
            _ => out.line(format!("{} {}", pad(key, width), cell(v))),
        }
    }
}

/// The node's two rows **with a public node** (W3).
///
/// §2m.3 makes `node.reachable` and `node.synchronized` **`null`** here, because fmonero never
/// asks a public node anything — the wallet service does. Reading `reachable != true` therefore
/// printed `not answering` about a node nobody had contacted, in the first word of the first line
/// of the first command, with six `—` under it. The measurement that does exist is the cached
/// check (§5's `node.remote_probe`), which is what the TUI's ` This node ` pane, the widget's
/// `Checked` row and the doctor's `node_layout` row all render from the same document.
fn public_node_rows(out: &Out, node: &serde_json::Value) {
    let probe = &node["remote_probe"];
    let reachable = probe["reachable"] == serde_json::Value::Bool(true);
    let badge = if probe.is_null() {
        out.paint("public node · not checked yet", Paint::Dim)
    } else if !reachable {
        out.paint("public node · no answer", Paint::Yellow)
    } else if probe["synchronized"] == serde_json::Value::Bool(true) {
        out.paint("public node · synced", Paint::Green)
    } else if probe["synchronized"] == serde_json::Value::Bool(false) {
        out.paint("public node · syncing", Paint::Yellow)
    } else {
        out.paint("public node · answered", Paint::Green)
    };
    out.line(format!("node    {badge}  {}", cell(&node["url"])));
    // The second row carries what the check measured, or nothing at all. Six `—` for fields that
    // have no source with a public node is the shape §6.5 deleted from the widget: a row that
    // carries no information for the state the reader is in.
    if probe.is_null() {
        return;
    }
    let age = probe["age_sec"]
        .as_i64()
        .map(|a| fmonero_core::age(a.max(0) as u64))
        .unwrap_or_else(|| "—".to_string());
    let mut fields = Vec::new();
    if reachable {
        if let Some(h) = probe["height"].as_u64() {
            fields.push(format!("block {}", grouped(h)));
        }
        if let Some(ms) = probe["latency_ms"].as_u64() {
            fields.push(format!("answered in {ms} ms"));
        }
    }
    fields.push(format!("checked {age}"));
    out.line(format!("        {}", fields.join("  ")));
}

/// The node's two rows **with your own node** — every field here has a live source (§5).
fn own_node_rows(out: &Out, node: &serde_json::Value) {
    let badge = if node["reachable"] == serde_json::Value::Bool(true) {
        if node["synchronized"] == serde_json::Value::Bool(true) {
            out.paint("synced", Paint::Green)
        } else {
            out.paint("syncing", Paint::Yellow)
        }
    } else {
        out.paint("not answering", Paint::Dim)
    };
    // `layout` is the JSON key and its values are internal words (§2o rule 5): the human row
    // says which node it is instead.
    let whose = match node["layout"].as_str() {
        Some("none") => "no node is set up yet",
        _ => "your own node",
    };
    out.line(format!(
        "node    {badge}  {}  block {}  {whose}",
        cell(&node["url"]),
        node["height"]
            .as_u64()
            .map(grouped)
            .unwrap_or_else(|| "—".to_string())
    ));
    let size = node["database_size"]
        .as_u64()
        .map(human_bytes)
        .unwrap_or_else(|| "—".to_string());
    let up = node["uptime_sec"]
        .as_i64()
        .map(human_uptime)
        .unwrap_or_else(|| "—".to_string());
    out.line(format!(
        "        synced {}%  peers {} out / {} in  waiting {}  disk {}  running {}",
        cell(&node["sync_percent"]),
        cell(&node["outgoing_connections_count"]),
        cell(&node["incoming_connections_count"]),
        cell(&node["tx_pool_size"]),
        size,
        up
    ));
}

/// `fmonero status` in human form, rendered from the §5 document's JSON so that the
/// document stays the single source of truth (§5).
pub fn status_human(out: &Out, v: &serde_json::Value) {
    let node = &v["node"];
    let wallet = &v["wallet"];
    if node["layout"].as_str() == Some("remote") {
        public_node_rows(out, node);
    } else {
        own_node_rows(out, node);
    }
    let wallet_badge = if wallet["open"] == serde_json::Value::Bool(true) {
        out.paint("open", Paint::Green)
    } else if wallet["reachable"] == serde_json::Value::Bool(true) {
        out.paint("closed", Paint::Yellow)
    } else {
        out.paint("not running", Paint::Dim)
    };
    out.line(format!(
        "wallet  {}  {}  {}  balance {} XMR",
        wallet_badge,
        cell(&wallet["url"]),
        cell(&wallet["name"]),
        cell(&wallet["balance_xmr"])
    ));
    if let Some(p) = v["pending"].as_object() {
        out.line(out.paint(
            &format!(
                "sending {} of {} sent  {} XMR to {}",
                cell(&p["relayed_count"]),
                cell(&p["total_count"]),
                cell(&p["total_amount_xmr"]),
                cell(&p["dest"])
            ),
            Paint::Red,
        ));
    }
    for w in v["warnings"].as_array().into_iter().flatten() {
        let severity = w["severity"].as_str().unwrap_or("info");
        let paint = match severity {
            "error" => Paint::Red,
            "warn" => Paint::Yellow,
            _ => Paint::Dim,
        };
        out.line(format!(
            "{} {}",
            out.paint(&format!("[{severity}]"), paint),
            cell(&w["text"])
        ));
        out.line(format!("  Try: {}", cell(&w["hint"])));
    }
}

// ---------------------------------------------------------------- per-command renderings
//
// `main.rs` builds every §5c payload and is frozen (§2k); these render the same JSON a
// machine gets, so the two can never describe different numbers. Each one is driven by the
// payload's own keys, never by a second call into core.

/// Notes that go to **stderr in both modes** — the footers §4.1 promises and the one hint
/// §4.1 attaches to an empty `wallet list`. stdout stays the data stream (§4).
fn footers(label: &str, v: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    match label {
        // W3: with a public node the node rows are the cached check, and when there is no check
        // yet the one line that changes that is the useful thing to say.
        "status"
            if v["node"]["layout"].as_str() == Some("remote")
                && v["node"]["remote_probe"].is_null() =>
        {
            out.push("this public node has not been checked yet".to_string());
            out.push("Try: fmonero status --check-node".to_string());
        }
        "wallet history" => {
            if v["cached"] == serde_json::Value::Bool(true) {
                out.push("from the saved copy: confirmation counts are not known".to_string());
            }
            if v["pool_unavailable"] == serde_json::Value::Bool(true) {
                out.push(
                    "what is waiting to be mined could not be read; every other row is complete"
                        .to_string(),
                );
            }
        }
        "wallet list" => {
            if v["wallets"].as_array().is_some_and(|a| a.is_empty()) {
                let dir = v["dir"].as_str().unwrap_or("<wallet.dir>");
                out.push(format!("no wallets in {dir}"));
                out.push("Try: fmonero wallet create <name> --yes".to_string());
                // The one command that brings an existing seed back (§4.1): `wallet restore`
                // is cut from v0.1 precisely so no seed enters fmonero's process (§0, §2b.2).
                out.push(format!(
                    "Try: monero-wallet-cli --restore-deterministic-wallet --wallet-file {dir}/<name>   # then: fmonero wallet open <name>"
                ));
            }
        }
        // §4.1: a missing log file is exit 0 with "no log yet". In JSON mode the payload is
        // `lines: []` — the machine form — so the sentence goes to stderr, where it reaches a
        // human without ever entering the data stream (§4).
        "node log" if v["lines"].as_array().is_some_and(|a| a.is_empty()) => {
            out.push(format!(
                "no log yet at {}",
                v["path"].as_str().unwrap_or("the configured log file")
            ));
        }
        // **No `wallet create` footer.** `main.rs` notes the `seed_backup_hint` the moment
        // `create_wallet` returns — before the daemon step, which is the step that can fail
        // with the wallet already on disk (v5.3, §4.1). A second copy here would print it
        // twice on the path that succeeds and would still be the only copy on the path that
        // does not.
        _ => {}
    }
    out
}

/// `wallet send` / `wallet sweep`: the confirmation block, identical for a dry run, for the
/// no-`--yes` refusal and for a relayed success, so a human reads one shape too (§5c).
fn money_human(out: &Out, v: &serde_json::Value) {
    let pair =
        |pico: &str, xmr: &str| format!("{} XMR ({} piconero)", cell(&v[xmr]), cell(&v[pico]));
    out.line(format!("To        {}", cell(&v["dest"])));
    out.line(format!("Amount    {}", pair("amount_pico", "amount_xmr")));
    out.line(format!("Fee       {}", pair("fee_pico", "fee_xmr")));
    out.line(format!("Total     {}", pair("total_pico", "total_xmr")));
    // The transaction's weight is an internal number and stays in `--json` (§2o).
    let level = v["priority"]
        .as_u64()
        .map(|p| crate::tui::fee_level_name(p as u32))
        .unwrap_or("—");
    out.line(format!(
        "Account   {}   Fee level {level}",
        cell(&v["account_index"])
    ));
    let txs = v["txs"].as_array().map(Vec::as_slice).unwrap_or_default();
    if txs.len() > 1 {
        out.line(format!("{} transactions:", txs.len()));
        for t in txs {
            out.line(format!(
                "  {}  {} XMR  fee {} XMR  {}",
                cell(&t["txid"]),
                cell(&t["amount_xmr"]),
                cell(&t["fee_xmr"]),
                if t["relayed"] == serde_json::Value::Bool(true) {
                    out.paint("sent", Paint::Green)
                } else {
                    out.paint("not sent", Paint::Dim)
                }
            ));
        }
    }
    let relayed = v["relayed"] == serde_json::Value::Bool(true);
    let dry = v["dry_run"] == serde_json::Value::Bool(true);
    let verdict = if relayed {
        out.paint("sent", Paint::Green)
    } else if dry {
        out.paint("dry run: nothing was sent", Paint::Dim)
    } else {
        out.paint("nothing was sent", Paint::Yellow)
    };
    out.line(verdict);
    for id in v["txids"].as_array().map(Vec::as_slice).unwrap_or_default() {
        out.line(format!("txid      {}", cell(id)));
    }
}

/// `wallet history`. `confirmations` and `locked` come from the payload — they are computed in
/// core and never here (§5b) — and a `null` renders `—`, never `0` (§5).
fn history_human(out: &Out, v: &serde_json::Value) {
    let rows = v["transfers"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if rows.is_empty() {
        out.line("no payments");
        return;
    }
    out.line(format!(
        "{}  {}  {}  {}  {}  {}",
        pad("date", 16),
        pad("what", 7),
        pad("amount", 18),
        pad("fee", 12),
        pad("confirms", 6),
        "locked"
    ));
    for t in rows {
        let when = t["timestamp"]
            .as_i64()
            .and_then(fmonero_core::rfc3339_from_unix)
            .map(|s| s.chars().take(16).collect::<String>())
            .unwrap_or_else(|| "—".to_string());
        out.line(format!(
            "{}  {}  {}  {}  {}  {}",
            pad(&when, 16),
            pad(&cell(&t["type"]), 7),
            pad(&cell(&t["amount_xmr"]), 18),
            pad(&cell(&t["fee_xmr"]), 12),
            pad(&cell(&t["confirmations"]), 6),
            cell(&t["locked"])
        ));
    }
    // `high_water` is the snapshot's own bookkeeping and stays in `--json` (§2o rule 7).
    out.line(format!(
        "{} payments · saved copy {}s old",
        rows.len(),
        cell(&v["cache_age_sec"])
    ));
}

fn balance_human(out: &Out, v: &serde_json::Value) {
    out.line(format!("account   {}", cell(&v["account_index"])));
    out.line(format!(
        "balance   {} XMR ({} piconero)",
        cell(&v["balance_xmr"]),
        cell(&v["balance_pico"])
    ));
    out.line(format!(
        "unlocked  {} XMR ({} piconero)",
        cell(&v["unlocked_xmr"]),
        cell(&v["unlocked_pico"])
    ));
    if v["blocks_to_unlock"].as_u64().unwrap_or(0) > 0 {
        out.line(out.paint(
            &format!(
                "locked    about {} min ({} blocks)",
                v["time_to_unlock_sec"]
                    .as_u64()
                    .map(|s| s.div_ceil(60).to_string())
                    .unwrap_or_else(|| "—".to_string()),
                cell(&v["blocks_to_unlock"])
            ),
            Paint::Yellow,
        ));
    }
    if let Some(subs) = v["subaddresses"].as_array() {
        // Only the FUNDED subaddresses are here (§5), so an absent row is not an absent
        // subaddress — `wallet address` lists them all.
        out.line("addresses holding money:");
        for s in subs {
            out.line(format!(
                "  {} {}  {} XMR  unlocked {} XMR  {} output(s)  {}",
                pad(&cell(&s["address_index"]), 3),
                cell(&s["address"]),
                cell(&s["balance_xmr"]),
                cell(&s["unlocked_xmr"]),
                cell(&s["num_unspent_outputs"]),
                cell(&s["label"])
            ));
        }
    }
}

fn address_human(out: &Out, v: &serde_json::Value) {
    out.line(format!("primary   {}", cell(&v["primary"])));
    out.line(format!("selected  {}", cell(&v["selected"])));
    for a in v["addresses"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        out.line(format!(
            "  {} {}  {}{}",
            pad(&cell(&a["address_index"]), 3),
            cell(&a["address"]),
            cell(&a["label"]),
            if a["used"] == serde_json::Value::Bool(true) {
                "  (used)"
            } else {
                ""
            }
        ));
    }
}

fn list_human(out: &Out, v: &serde_json::Value) {
    for name in v["wallets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        out.line(cell(name));
    }
}

fn peers_human(out: &Out, v: &serde_json::Value) {
    match v["connections"].as_array() {
        Some(rows) => {
            out.line(format!("{} peers connected", rows.len()));
            for c in rows {
                out.line(format!(
                    "  {}  {}  {}  block {}  {}s  ↓{} ↑{}",
                    pad(&cell(&c["host"]), 40),
                    pad(&cell(&c["port"]), 6),
                    pad(&cell(&c["state"]), 12),
                    cell(&c["height"]),
                    cell(&c["live_time"]),
                    human_bytes(c["avg_download"].as_u64().unwrap_or(0)),
                    human_bytes(c["avg_upload"].as_u64().unwrap_or(0)),
                ));
            }
        }
        None => out.line("connections   — (this node does not answer that)"),
    }
    for (key, title) in [("white", "known peers"), ("gray", "other peers")] {
        match v[key].as_array() {
            Some(rows) => {
                out.line(format!("{title}: {} peer(s)", rows.len()));
                for p in rows {
                    out.line(format!(
                        "  {}:{}  last seen {}",
                        cell(&p["host"]),
                        cell(&p["port"]),
                        cell(&p["last_seen"])
                    ));
                }
            }
            None => out.line(format!("{title}   — (this node does not answer that)")),
        }
    }
}

/// `fmonero node list` (§2n, §5c). Two column groups, headed so the table itself states
/// §2n.2's distinction: what **monero.fail says** and what was **probed here**. They are never
/// combined into a score — a single number mixing a measurement with a third party's claim is a
/// number nobody can check.
fn nodes_human(out: &Out, v: &serde_json::Value) {
    let rows = v["nodes"].as_array().map(Vec::as_slice).unwrap_or_default();
    if rows.is_empty() {
        out.line("no list of public nodes yet");
        out.note("fmonero node list --refresh   # fetch the list from monero.fail");
        return;
    }
    let probed_any = rows.iter().any(|r| !r["probe"].is_null());
    if probed_any {
        out.line(format!(
            "{}  {}   {}",
            pad("", 47),
            out.paint("monero.fail says", Paint::Dim),
            out.paint("checked here", Paint::Dim),
        ));
    }
    out.line(format!(
        "  {}  {}  {}  {}{}",
        pad("address", 44),
        pad("reliability", 12),
        pad("block", 10),
        pad("last seen", 10),
        if probed_any {
            format!("  {}  {}  {}", pad("ping", 6), pad("block", 10), "synced")
        } else {
            String::new()
        },
    ));
    for r in rows {
        let configured = r["configured"] == serde_json::Value::Bool(true);
        let mark = if configured { "●" } else { " " };
        // A `custom` row renders `—` in every monero.fail column: monero.fail has never heard of
        // it, and a blank there would read as "not measured yet" (§7a.4).
        let custom = r["custom"] == serde_json::Value::Bool(true);
        let health = match (r["health_ratio"].as_f64(), r["health_checks"].as_u64()) {
            (Some(ratio), Some(n)) => format!("{}% of {n}", (ratio * 100.0).round() as i64),
            _ => "—".to_string(),
        };
        let probe = &r["probe"];
        let probed = if probe.is_null() {
            if probed_any {
                format!("  {}  {}  {}", pad("—", 6), pad("—", 10), "—")
            } else {
                String::new()
            }
        } else if probe["reachable"] == serde_json::Value::Bool(true) {
            format!(
                "  {}  {}  {}",
                pad(&cell(&probe["latency_ms"]), 6),
                pad(&cell(&probe["height"]), 10),
                cell(&probe["synchronized"]),
            )
        } else {
            // A failure is a result, and its reason is the useful half (§2n.4).
            format!("  {}", out.paint(&cell(&probe["reason"]), Paint::Yellow))
        };
        out.line(format!(
            "{mark} {}  {}  {}  {}{}{}",
            pad(&cell(&r["address"]), 44),
            pad(&health, 12),
            pad(&cell(&r["health_last_height"]), 10),
            pad(
                &r["health_checked_at"]
                    .as_i64()
                    .map(|t| human_uptime(fmonero_core::now_unix() - t))
                    .unwrap_or_else(|| "—".to_string()),
                10
            ),
            probed,
            if custom {
                out.paint(" own", Paint::Dim)
            } else if r["web_compatible"] == serde_json::Value::Bool(true) {
                out.paint(" web", Paint::Dim)
            } else {
                String::new()
            },
        ));
    }
    let age = v["cache_age_sec"]
        .as_u64()
        .map(|s| human_uptime(s as i64))
        .unwrap_or_else(|| "—".to_string());
    out.line(format!(
        "{} of {} nodes · list {age} old · {} skipped · from monero.fail",
        rows.len(),
        cell(&v["total"]),
        v["skipped_no_port"].as_u64().unwrap_or(0) + v["skipped_invalid"].as_u64().unwrap_or(0),
    ));
}

fn log_human(out: &Out, v: &serde_json::Value) {
    let lines = v["lines"].as_array().map(Vec::as_slice).unwrap_or_default();
    if lines.is_empty() {
        out.line(format!("no log yet at {}", cell(&v["path"])));
        return;
    }
    for l in lines {
        out.line(cell(l));
    }
    if v["truncated"] == serde_json::Value::Bool(true) {
        out.note("(earlier lines were not read)");
    }
}

fn settings_human(out: &Out, v: &serde_json::Value) {
    let rows = v["settings"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let width = rows
        .iter()
        .map(|r| UnicodeWidthStr::width(r["key"].as_str().unwrap_or("")))
        .max()
        .unwrap_or(0);
    for r in rows {
        let value = cell(&r["value"]);
        let default = cell(&r["default"]);
        let marker = if value == default { " " } else { "*" };
        out.line(format!(
            "{marker} {}  {}",
            pad(&cell(&r["key"]), width),
            value
        ));
        out.line(format!(
            "    {}",
            out.paint(
                &format!("{}  [default {default}]", cell(&r["doc"])),
                Paint::Dim
            )
        ));
    }
    out.note("* = changed from the default");
}

/// `node remote` / `node local` (§5c, §2m.5). The two share one key set, so an agent parses
/// one shape and reads `mode`; a human gets the same facts in the same order.
fn mode_human(out: &Out, v: &serde_json::Value) {
    let mode = cell(&v["mode"]);
    let dry = v["dry_run"] == serde_json::Value::Bool(true);
    // §2o.2 deletes the trust flag and the ssl mode from every human surface; both stay in the
    // payload, where an agent reads them.
    out.line(format!(
        "{}  {}",
        out.paint(
            if mode == "remote" {
                "now using a public node"
            } else {
                "now using your own node"
            },
            if mode == "remote" {
                Paint::Yellow
            } else {
                Paint::Green
            }
        ),
        cell(&v["address"]),
    ));
    // The same `units::FileAction[]` shape and the same rendering `setup` uses (§5b).
    setup_human(out, v);
    if v["wallet_unit_restarted"] == serde_json::Value::Bool(true) {
        out.note("the wallet service was restarted so it picks up the new node");
    }
    // §2m.5 step 10: the restart closed whatever was open, so say so rather than leave
    // `status` claiming a wallet that is gone — a stale fact about money.
    if let Some(name) = v["wallet_must_reopen"].as_str() {
        out.note(format!("the wallet {name} was closed by that restart"));
        out.note(format!("Try: fmonero wallet open {name}"));
    }
    if !dry && v["changed"] == serde_json::Value::Bool(false) {
        out.note("nothing changed: this machine was already set that way");
    }
    if mode == "remote" {
        // §2m.6, in the same words README and robot-docs use.
        out.note("this node sees your IP address and when you ask it anything");
        out.note("it also supplies the fee estimate, so bound a send with --max-fee");
    }
}

fn setup_human(out: &Out, v: &serde_json::Value) {
    let dry = v["dry_run"] == serde_json::Value::Bool(true);
    for a in v["actions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let status = cell(&a["status"]);
        let paint = match status.as_str() {
            "written" => Paint::Green,
            "planned" => Paint::Yellow,
            "skipped" => Paint::Red,
            _ => Paint::Dim,
        };
        out.line(format!(
            "{} {}  {}",
            out.paint(&pad(&status, 9), paint),
            cell(&a["what"]),
            cell(&a["path"])
        ));
        // The diff is already redacted where it was rendered (§4.2): `units::install` and the
        // menu merge both pass it through `units::redact_login`.
        if let Some(diff) = a["diff"].as_str() {
            for line in diff.lines() {
                out.line(format!("    {line}"));
            }
        }
    }
    if dry {
        out.note("dry run: nothing was changed");
    }
}
