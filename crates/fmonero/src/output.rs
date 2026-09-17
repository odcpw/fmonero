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
            eprintln!("  hint: {}", e.hint());
        }
        e.exit_code()
    }
}

// ---------------------------------------------------------------- formatters

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

/// `fmonero status` in human form, rendered from the §5 document's JSON so that the
/// document stays the single source of truth (§5).
pub fn status_human(out: &Out, v: &serde_json::Value) {
    let node = &v["node"];
    let wallet = &v["wallet"];
    let node_badge = if node["reachable"] == serde_json::Value::Bool(true) {
        if node["synchronized"] == serde_json::Value::Bool(true) {
            out.paint("synced", Paint::Green)
        } else {
            out.paint("syncing", Paint::Yellow)
        }
    } else {
        out.paint("down", Paint::Dim)
    };
    out.line(format!(
        "node    {}  {}  height {}  layout {}",
        node_badge,
        cell(&node["url"]),
        cell(&node["height"]),
        cell(&node["layout"])
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
        "        sync {}%  peers {} out / {} in  txpool {}  db {}  up {}",
        cell(&node["sync_percent"]),
        cell(&node["outgoing_connections_count"]),
        cell(&node["incoming_connections_count"]),
        cell(&node["tx_pool_size"]),
        size,
        up
    ));
    let wallet_badge = if wallet["open"] == serde_json::Value::Bool(true) {
        out.paint("open", Paint::Green)
    } else if wallet["reachable"] == serde_json::Value::Bool(true) {
        out.paint("closed", Paint::Yellow)
    } else {
        out.paint("down", Paint::Dim)
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
                "pending {} of {} relayed  {} XMR → {}",
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
        out.line(format!("  hint: {}", cell(&w["hint"])));
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
        "wallet history" => {
            if v["cached"] == serde_json::Value::Bool(true) {
                out.push("cached: confirmations unknown (no daemon call)".to_string());
            }
            if v["pool_unavailable"] == serde_json::Value::Bool(true) {
                out.push(
                    "the txpool could not be read (the wallet daemon could not reach the node); \
                     every other row is complete"
                        .to_string(),
                );
            }
        }
        "wallet list" => {
            if v["wallets"].as_array().is_some_and(|a| a.is_empty()) {
                let dir = v["dir"].as_str().unwrap_or("<wallet.dir>");
                out.push(format!("no wallets in {dir}"));
                out.push("hint: fmonero wallet create <name> --yes".to_string());
                // The one command that brings an existing seed back (§4.1): `wallet restore`
                // is cut from v0.1 precisely so no seed enters fmonero's process (§0, §2b.2).
                out.push(format!(
                    "hint: monero-wallet-cli --restore-deterministic-wallet --wallet-file {dir}/<name>   # then: fmonero wallet open <name>"
                ));
            }
        }
        "node log" => {
            // §4.1: a missing log file is exit 0 with "no log yet". In JSON mode the payload is
            // `lines: []` — the machine form — so the sentence goes to stderr, where it reaches
            // a human without ever entering the data stream (§4).
            if v["lines"].as_array().is_some_and(|a| a.is_empty()) {
                out.push(format!(
                    "no log yet at {}",
                    v["path"].as_str().unwrap_or("the configured log file")
                ));
            }
        }
        "wallet create" => {
            if let Some(hint) = v["seed_backup_hint"].as_str() {
                out.push(hint.to_string());
            }
        }
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
    out.line(format!(
        "Account   {}   Priority {}   Weight {}",
        cell(&v["account_index"]),
        cell(&v["priority"]),
        cell(&v["weight"])
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
                    out.paint("relayed", Paint::Green)
                } else {
                    out.paint("not relayed", Paint::Dim)
                }
            ));
        }
    }
    let relayed = v["relayed"] == serde_json::Value::Bool(true);
    let dry = v["dry_run"] == serde_json::Value::Bool(true);
    let verdict = if relayed {
        out.paint("broadcast", Paint::Green)
    } else if dry {
        out.paint("dry run: nothing was broadcast", Paint::Dim)
    } else {
        out.paint("nothing was broadcast", Paint::Yellow)
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
        out.line("no transfers");
        return;
    }
    out.line(format!(
        "{}  {}  {}  {}  {}  {}",
        pad("date", 16),
        pad("type", 7),
        pad("amount", 18),
        pad("fee", 12),
        pad("conf", 6),
        "locked (est.)"
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
    out.line(format!(
        "{} row(s)   high water {}   cache age {}s",
        rows.len(),
        cell(&v["high_water"]),
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
                "locked    for {} more block(s) (~{}s)",
                cell(&v["blocks_to_unlock"]),
                cell(&v["time_to_unlock_sec"])
            ),
            Paint::Yellow,
        ));
    }
    if let Some(subs) = v["subaddresses"].as_array() {
        // Only the FUNDED subaddresses are here (§5), so an absent row is not an absent
        // subaddress — `wallet address` lists them all.
        out.line("funded subaddresses:");
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
            out.line(format!("{} live connection(s)", rows.len()));
            for c in rows {
                out.line(format!(
                    "  {}  {}  {}  height {}  seed {}  {}s  ↓{} ↑{}",
                    pad(&cell(&c["host"]), 40),
                    pad(&cell(&c["port"]), 6),
                    pad(&cell(&c["state"]), 12),
                    cell(&c["height"]),
                    cell(&c["pruning_seed"]),
                    cell(&c["live_time"]),
                    human_bytes(c["avg_download"].as_u64().unwrap_or(0)),
                    human_bytes(c["avg_upload"].as_u64().unwrap_or(0)),
                ));
            }
        }
        None => out.line("connections   — (not available on a restricted node)"),
    }
    for (key, title) in [("white", "white list"), ("gray", "gray list")] {
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
            None => out.line(format!("{title}   — (not available on a restricted node)")),
        }
    }
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
    out.line(format!(
        "{}  {}  {} · ssl {}",
        out.paint(
            &pad(&mode, 7),
            if mode == "remote" {
                Paint::Yellow
            } else {
                Paint::Green
            }
        ),
        cell(&v["address"]),
        if v["trusted"] == serde_json::Value::Bool(true) {
            "trusted"
        } else {
            "untrusted"
        },
        cell(&v["ssl_support"])
    ));
    // The same `units::FileAction[]` shape and the same rendering `setup` uses (§5b).
    setup_human(out, v);
    if v["wallet_unit_restarted"] == serde_json::Value::Bool(true) {
        out.note("monero-wallet-rpc was restarted: it holds --daemon-address in its argv");
    }
    // §2m.5 step 10: the restart closed whatever was open, so say so rather than leave
    // `status` claiming a wallet that is gone — a stale fact about money.
    if let Some(name) = v["wallet_must_reopen"].as_str() {
        out.note(format!("the wallet {name} was closed by that restart"));
        out.note(format!("hint: fmonero wallet open {name}"));
    }
    if !dry && v["changed"] == serde_json::Value::Bool(false) {
        out.note("nothing changed: this machine was already in that mode");
    }
    if mode == "remote" {
        // §2m.6, in the same words README and robot-docs use.
        out.note(
            "this node learns your IP address and the timing of every request, supplies the \
             decoys your transactions are built from, and can lie about the chain tip and the \
             fee — bound a send with --max-fee, and never mark a public node --trusted",
        );
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
