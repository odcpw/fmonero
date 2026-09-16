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
    pub fn emit(&self, label: &str, value: serde_json::Value) {
        if self.json {
            println!("{value}");
        } else {
            human(self, label, &value);
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
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
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
    if label == "status" {
        status_human(out, value);
        return;
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
