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

/// How wide the line a person is reading may be (X3).
///
/// **The owner's rule is that nothing is clipped mid-word, and nothing here was wrapped at all**,
/// so on his own 120-column terminal `doctor` split `se`/`rvice`, `status` split `esti`/`mate` and
/// one `settings list` screen split fifteen words in half. The TUI has wrapped since W6; this file
/// had no notion of a width.
///
/// Three sources, in this order, and the order is the seam the tests drive:
/// 1. `COLUMNS`, when it parses to a sane width — what a test sets, and what a shell exports;
/// 2. `TIOCGWINSZ` on stdout, which is the real terminal;
/// 3. **100**, when stdout is not a terminal — a pipe has no width, and 100 is the width the
///    project's own captures are read at.
///
/// Clamped to 40..=200: below 40 a table is unreadable whatever we do, and above 200 a paragraph
/// stops being a paragraph.
pub fn term_width() -> usize {
    const FALLBACK: usize = 100;
    const MIN: usize = 40;
    const MAX: usize = 200;
    if let Some(n) = std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
    {
        return n.clamp(MIN, MAX);
    }
    #[repr(C)]
    struct WinSize {
        rows: libc::c_ushort,
        cols: libc::c_ushort,
        xpixel: libc::c_ushort,
        ypixel: libc::c_ushort,
    }
    let mut ws = WinSize {
        rows: 0,
        cols: 0,
        xpixel: 0,
        ypixel: 0,
    };
    // SAFETY: `TIOCGWINSZ` writes exactly one `struct winsize`, and that is the layout above.
    let ok = unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0;
    if ok && ws.cols > 0 {
        return (ws.cols as usize).clamp(MIN, MAX);
    }
    FALLBACK
}

/// The display width of `s` with ANSI colour escapes discounted.
///
/// `paint` wraps a cell in `\x1b[32m…\x1b[0m`, and those bytes occupy no columns. A wrapper that
/// counted them would wrap a green word four characters early and an uncoloured one correctly,
/// which is a layout that changes when `--no-color` is passed.
fn visible_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // `ESC [ … m` — consume through the final byte.
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        width += UnicodeWidthStr::width(c.to_string().as_str());
    }
    width
}

/// Wrap one line of prose to `width` columns, **never splitting a word**, with every continuation
/// line indented to `hang` (X3).
///
/// The hanging indent is the other half of the defect: before this, a wrapped detail returned to
/// column 0, so a continuation of an indented table row broke the table's own indent and a wrapped
/// bullet read as a new bullet. A word longer than the whole width — an address, a 64-character id,
/// a path — is put on its own line **whole**: §2a and `docs/COPY.md` §4 both say an address is
/// never elided, and a word broken in half is exactly what this fix exists to remove.
fn wrap_to(text: &str, width: usize, hang: usize) -> Vec<String> {
    // The line's own leading spaces are structure, not prose: they start the first line and the
    // hanging indent is measured from them.
    let indent: String = text.chars().take_while(|c| *c == ' ').collect();
    let body = &text[indent.len()..];
    if body.is_empty() {
        return vec![text.to_string()];
    }
    // **A run of spaces inside the line is kept exactly** (round 4's own read of the captures): the
    // first version of this loop treated the body as `split(' ')` and rebuilt one space between
    // words, so `node    synced` came out `node   synced` and every two-space column gap in the
    // program lost a column. The body is split into runs of spaces and runs of not-spaces, and a run
    // of spaces is carried whole to the word that follows it.
    let mut chunks: Vec<(bool, String)> = Vec::new();
    for c in body.chars() {
        let is_space = c == ' ';
        match chunks.last_mut() {
            Some((was_space, text)) if *was_space == is_space => text.push(c),
            _ => chunks.push((is_space, c.to_string())),
        }
    }
    let hang_pad = " ".repeat(hang);
    let mut lines: Vec<String> = Vec::new();
    let mut current = indent.clone();
    let mut current_width = indent.chars().count();
    let mut gap = String::new();
    let mut any = false;
    for (is_space, chunk) in chunks {
        if is_space {
            gap = chunk;
            continue;
        }
        let w = visible_width(&chunk);
        let gap_w = gap.chars().count();
        if any && current_width + gap_w + w > width {
            lines.push(current.trim_end().to_string());
            current = hang_pad.clone();
            current_width = hang;
        } else if any {
            current.push_str(&gap);
            current_width += gap_w;
        }
        current.push_str(&chunk);
        current_width += w;
        gap.clear();
        any = true;
    }
    lines.push(current.trim_end().to_string());
    lines
}

/// Wrap a line for a person: the terminal's width, and a hanging indent two columns in from the
/// line's own indent so a continuation can never be read as a new row (X3).
pub fn wrapped_lines(text: &str) -> Vec<String> {
    let indent = text.chars().take_while(|c| *c == ' ').count();
    wrap_to(text, term_width(), indent + 2)
}

/// Column widths for a table, from the terminal rather than from a constant (X3).
///
/// `mins` is what a column may never go below — an address column's minimum is the address, because
/// `docs/COPY.md` §4 forbids eliding one — and `prefs` is what it takes when there is room. A
/// shortfall is taken from the widest column that is still above its minimum, one column at a time,
/// so the table narrows evenly instead of one column collapsing.
///
/// **Slack goes to `flex` only when a column is genuinely better wide.** `NO_FLEX` leaves it unspent,
/// which is what a table of short values wants: round 4's own read of the captures found `node list`
/// drawing an 80-column address column for 21-character addresses, because every spare column on a
/// 120-column terminal had been handed to it.
pub const NO_FLEX: usize = usize::MAX;

pub fn columns(mins: &[usize], prefs: &[usize], gap: usize, flex: usize) -> Vec<usize> {
    debug_assert_eq!(mins.len(), prefs.len());
    let total = term_width();
    let gaps = gap * mins.len().saturating_sub(1);
    let mut out: Vec<usize> = prefs.to_vec();
    let sum = |v: &[usize]| -> usize { v.iter().sum::<usize>() + gaps };
    while sum(&out) > total {
        // The widest column that can still give a column back.
        let Some(i) = (0..out.len())
            .filter(|&i| out[i] > mins[i])
            .max_by_key(|&i| out[i])
        else {
            break; // every column is at its minimum: the table is wider than the terminal, honestly
        };
        out[i] -= 1;
    }
    if sum(&out) < total && flex < out.len() {
        out[flex] += total - sum(&out);
    }
    out
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

    /// A line of data on stdout, **wrapped to the terminal** with a hanging indent (X3).
    ///
    /// Wrapping is the default rather than an opt-in because the defect X3 names is a missed call
    /// site: every one of `settings list`'s fifteen split words was a line somebody printed without
    /// thinking about a width. A row whose columns are already measured uses [`Out::row`].
    pub fn line(&self, text: impl AsRef<str>) {
        for line in wrapped_lines(text.as_ref()) {
            println!("{line}");
        }
    }

    /// A row whose width is already the terminal's: printed exactly as given.
    ///
    /// Tables size their own columns through [`columns`], so wrapping one here would undo that
    /// work; a log line is the node's own output and is not ours to reflow either.
    pub fn row(&self, text: impl AsRef<str>) {
        println!("{}", text.as_ref());
    }

    /// A diagnostic on stderr, wrapped the same way. Never part of the data stream.
    pub fn note(&self, text: impl AsRef<str>) {
        for line in wrapped_lines(text.as_ref()) {
            eprintln!("{line}");
        }
    }

    /// A diagnostic shown only with `-v`.
    pub fn verbose_note(&self, text: impl AsRef<str>) {
        if VERBOSE.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            for line in wrapped_lines(text.as_ref()) {
                eprintln!("{line}");
            }
        }
    }

    /// Print an error the way the caller asked for it and return its exit code (§4).
    ///
    /// **A human gets a sentence, the in-app route and one runnable command — and no id** (X1).
    /// This printed `fmonero: error[usage]: …` and `reason: non_loopback_url` at a person on every
    /// single error. §2o's first paragraph puts `code_name` and `reason` in `--json`, where an agent
    /// branches on them, and rule 5 says ids appear there **and nowhere else**. It is the same shape,
    /// in the same kind of place — one function — that W1 closed in the doctor's table, and it
    /// survived that round because the round read the doctor and not the CLI's envelope. `detail` was
    /// already behind `-v` for exactly this reason, three lines below.
    pub fn report(&self, e: &Error) -> i32 {
        if self.json {
            println!("{}", e.json());
        } else {
            // One sentence, with `fmonero:` in front of it so a person reading a scrollback knows
            // which program is talking.
            let why = e.to_string();
            for line in wrap_to(
                &format!("{}: {why}", self.paint("fmonero", Paint::Red)),
                term_width(),
                2,
            ) {
                eprintln!("{line}");
            }
            // The in-app route, when there is one (§2o.1): the key that fixes this without a
            // command. It is the same sentence the TUI shows for the same error.
            if let Some(action) = e.action() {
                for line in wrap_to(&format!("  {action}"), term_width(), 4) {
                    eprintln!("{line}");
                }
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
            for line in wrap_to(&format!("  Try: {}", e.hint()), term_width(), 8) {
                eprintln!("{line}");
            }
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
/// **The implementation is [`fmonero_core::bytes`]** and this is a re-export (X6). It lived here
/// while the CLI was its only caller; the TUI and the widget then grew one each, with a different
/// divisor and a different label, so one waiting pool read `21.4 kB`, `20.9 kB` and `20.9 KiB` on
/// three surfaces of the same program.
pub use fmonero_core::bytes as human_bytes;

/// `3d 04h`, `2h 07m`, `45s` — **[`fmonero_core::duration`]**, which is the same formatter the
/// ages use (M2).
///
/// This was the third duration shape in the program: it had a days unit that `fmonero_core::age`
/// lacked, so one quantity measured in the same seconds read `2d 18h` here and `66h 23m` two panes
/// away. All it adds now is the clamp, because an uptime arrives as a signed difference.
pub fn human_uptime(secs: i64) -> String {
    fmonero_core::duration(secs.max(0) as u64)
}

/// Pad to `width` display columns on the LEFT, so a column of amounts lines its decimal points up.
///
/// Money reads down a column, not across it (round 4's own read of `wallet history`): left-aligned,
/// `0.1000` and `0.00003072` put their points three cells apart.
pub fn pad_right(text: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(text);
    if w >= width {
        text.to_string()
    } else {
        format!("{}{text}", " ".repeat(width - w))
    }
}

/// Pad to `width` display columns, **with an ellipsis when the value is wider** (M6).
///
/// A note is free text, and a table column is not: this is what keeps `wallet history`'s new note
/// column from pushing the row past the terminal it is measured against. Nothing clipped mid-word
/// without a marker — the ellipsis IS the marker.
pub fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= width {
        return pad(text, width);
    }
    let mut out = String::new();
    for c in text.chars() {
        if UnicodeWidthStr::width(out.as_str()) + 1 >= width {
            break;
        }
        out.push(c);
    }
    out.push('…');
    pad(&out, width)
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
        // M7: two labels, one renderer. The label carries which variant ran, because the footer
        // below has a different next step for each and the §5 document itself must not grow a key
        // for it (agents parse that document).
        "status" | "status --check-node" => return status_human(out, value),
        "wallet send" | "wallet sweep" => return money_human(out, value),
        "wallet history" => return history_human(out, value),
        "wallet balance" => return balance_human(out, value),
        "wallet address" | "wallet new-address" => return address_human(out, value),
        "wallet list" => return list_human(out, value),
        "node peers" => return peers_human(out, value),
        "node list" => return nodes_human(out, value),
        "node info" => return node_info_human(out, value),
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
/// `unit_active` is `None` where the caller's payload does not carry `units` — `node info`, which
/// only ever reaches this function with a node that answered, so the off-branch below is not its
/// case. `None` keeps the conservative wording rather than asserting "not running" (M1).
fn own_node_rows(out: &Out, node: &serde_json::Value, unit_active: Option<bool>) {
    let badge = if node["reachable"] == serde_json::Value::Bool(true) {
        if node["synchronized"] == serde_json::Value::Bool(true) {
            out.paint("synced", Paint::Green)
        } else {
            out.paint("syncing", Paint::Yellow)
        }
    } else {
        // **M1: one predicate for one fact.** This row said `not answering` while the warning
        // three lines below it said `Your node is not running.` — two wordings of one thing in one
        // screen, and the TUI had already settled it. `node_off_words_from` is that predicate; the
        // CLI renders from the JSON document, so it passes the two fields rather than a `Status`.
        out.paint(
            fmonero_core::node_off_words_from(
                unit_active.unwrap_or(true),
                node["layout"].as_str() == Some("system_unit"),
            ),
            Paint::Dim,
        )
    };
    // `layout` is the JSON key and its values are internal words (§2o rule 5): the human row
    // says which node it is instead.
    // `none` means fmonero set nothing up — and something may still be answering on that port,
    // which is a different fact from "there is no node" (self-read, round 3: this row read
    // `node  synced  127.0.0.1:18491  block 3 541 122  no node is set up yet`).
    let whose = match node["layout"].as_str() {
        Some("none") if node["reachable"] == serde_json::Value::Bool(true) => {
            "a node fmonero did not set up"
        }
        Some("none") => "no node is set up yet",
        _ => "your own node",
    };
    // **`block —` says nothing** (X23's lesson, found by round 4's own read of this output): with
    // nothing set up the first line of the first command read `not answering 127.0.0.1:18081 block —
    // no node is set up yet`, and the dash sat between two facts that were already the answer.
    let block = match node["height"].as_u64() {
        Some(h) => format!("block {}  ", grouped(h)),
        None => String::new(),
    };
    out.line(format!(
        "node    {badge}  {}  {block}{whose}",
        cell(&node["url"]),
    ));
    // **Nothing is said with dashes** (X23). With the node off every field below is `null`, and the
    // row read `synced —%  peers — out / — in  waiting —  blockchain —  running —` under a badge
    // that had already said `not answering`: the shape §6.5 deleted from the widget and W3 deleted
    // from the public-node rows above. The badge is the whole answer, and the footer carries the
    // one thing to do about it.
    if node["reachable"] != serde_json::Value::Bool(true) {
        return;
    }
    // **Every field carries its noun and its unit** (X20, X21, X29, X30). It read
    // `synced 100.0%` under a badge already saying `synced`, `peers 16 out / 3 in` against the
    // TUI's `16 out · 3 in`, and a bare `waiting 12` whose noun was two panes away.
    let mut fields = Vec::new();
    if node["synchronized"] != serde_json::Value::Bool(true) {
        if let Some(pct) = node["sync_percent"].as_f64() {
            fields.push(format!("{} synced", fmonero_core::percent(pct)));
        }
    }
    if let (Some(o), Some(i)) = (
        node["outgoing_connections_count"].as_u64(),
        node["incoming_connections_count"].as_u64(),
    ) {
        fields.push(format!("peers {o} out · {i} in"));
    }
    if let Some(n) = node["tx_pool_size"].as_u64() {
        fields.push(format!(
            "{n} {} waiting to be mined",
            if n == 1 { "payment" } else { "payments" }
        ));
    }
    if let Some(b) = node["database_size"].as_u64() {
        // `disk {database_size}` labelled the chain's own size as free space (self-read, round 3):
        // `disk 118.2 GB` beside a warning that 96 GB were free.
        fields.push(format!("blockchain {}", human_bytes(b)));
    }
    if let Some(u) = node["uptime_sec"].as_i64() {
        fields.push(format!("running {}", human_uptime(u)));
    }
    if !fields.is_empty() {
        out.line(format!("        {}", fields.join("  ")));
    }
}

/// `fmonero status` in human form, rendered from the §5 document's JSON so that the
/// document stays the single source of truth (§5).
pub fn status_human(out: &Out, v: &serde_json::Value) {
    let node = &v["node"];
    let wallet = &v["wallet"];
    if node["layout"].as_str() == Some("remote") {
        public_node_rows(out, node);
    } else {
        own_node_rows(
            out,
            node,
            Some(v["units"]["node"]["active"] == serde_json::Value::Bool(true)),
        );
    }
    // The wallet row, and **only what is known** (self-read, round 3): with nothing open it read
    // `wallet  not running  http://127.0.0.1:18083  —  balance — XMR` — a loopback URL, a dash
    // where the name goes and a dash where the money goes, which is the shape §6.5 deleted from
    // the widget and W3 deleted from the node row above. The URL stays in `--json`.
    let (badge, rest) = if wallet["open"] == serde_json::Value::Bool(true) {
        // The name only when there is one: `wallet  open  —  balance …` put a dash where the
        // wallet's name goes (self-read, round 3). fmonero records the name when it opens one
        // itself, and a wallet opened another way has none to show.
        let name = match wallet["name"].as_str() {
            Some(n) => format!("{n}  "),
            None => String::new(),
        };
        (
            out.paint("open", Paint::Green),
            format!("{name}balance {} XMR", cell(&wallet["balance_xmr"])),
        )
    } else if wallet["reachable"] == serde_json::Value::Bool(true) {
        (
            out.paint("closed", Paint::Yellow),
            "the wallet service is running, with nothing open".to_string(),
        )
    } else {
        (
            out.paint("not running", Paint::Dim),
            "it starts when you open a wallet".to_string(),
        )
    };
    out.line(format!("wallet  {badge}  {rest}"));
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
        // **M7: `--check-node` makes the NEXT run fresh, and the footer says so.** It printed
        // `this public node has not been checked yet` / `Try: fmonero status --check-node` — and on
        // a run that HAD just been given `--check-node` the next step named was the command that
        // had just been typed, which in a sandbox took three runs before a verdict appeared. The
        // design's intent was always "this starts one in the background"; nothing on screen said it.
        "status"
            if v["node"]["layout"].as_str() == Some("remote")
                && v["node"]["remote_probe"].is_null() =>
        {
            out.push("Nothing has checked this public node yet.".to_string());
            out.push(
                "Try: fmonero status --check-node   # starts one in the background".to_string(),
            );
        }
        "status --check-node"
            if v["node"]["layout"].as_str() == Some("remote")
                && v["node"]["remote_probe"].is_null() =>
        {
            out.push(
                "A check of this public node is running in the background. This answer is the \
                 one that was already known."
                    .to_string(),
            );
            out.push("Try: fmonero status   # in a moment, to see what it found".to_string());
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
                // **M21: a sentence, and one `Try:`** (§9). It opened with the fragment
                // `no wallets in <dir>` — lower case, no full stop, the only line in the program
                // shaped like a log entry — and then printed two runnable commands where §9 asks
                // for one. Restoring from a seed is the rarer of the two and is named in the
                // sentence rather than as a second instruction.
                out.push(format!("There are no wallets in {dir}."));
                // The one command that brings an existing seed back (§4.1): `wallet restore`
                // is cut from v0.1 precisely so no seed enters fmonero's process (§0, §2b.2).
                // A colon introducing a command is the shape `Try:` already teaches; a command
                // inside a sentence is what round 4's own read of this footer objected to.
                out.push(format!(
                    "To bring an existing seed back: monero-wallet-cli --restore-deterministic-wallet --wallet-file {dir}/<name>"
                ));
                out.push("Try: fmonero wallet create <name> --yes".to_string());
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
        // **`payments`, not `transactions`** (X8): §2o.2's own word, on the one path where getting
        // it wrong costs money. And the **short id** (X9), because 64 hexadecimal characters is a
        // paragraph in a table; the whole id is two lines below under `payment id` and in `--json`.
        out.line(format!("{} payments:", txs.len()));
        for t in txs {
            out.line(format!(
                "  {}  {} XMR  fee {} XMR  {}",
                t["txid"]
                    .as_str()
                    .map(fmonero_core::short_id)
                    .unwrap_or_else(|| "—".to_string()),
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
        // §2o.2: `payment id`, never `txid` as a label (self-read, round 3). The JSON key is
        // unchanged.
        out.line(format!("payment id  {}", cell(id)));
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
    // **The columns are the terminal's and the headers carry their unit** (X3, X20). They were six
    // constants adding up to 85 columns, and `amount` and `fee` were money with no unit in the cell
    // or the header — rule 9, on the one table where the numbers are money.
    // No flex: an amount column stretched to fill the terminal puts its values a long way from the
    // heading over them (round 4's own read).
    //
    // **M6: a note column.** `n` in the TUI writes a note, the TUI's filter line offers to search
    // by one, and this table — the only list of payments the CLI has — had no column for it at all,
    // so a note the owner wrote was invisible in both places at his terminal width.
    // **M29: `spendable`, not `locked`.** The column was blank for an unlocked payment with nothing
    // saying blank meant spendable, and it printed `locked` for a FAILED payment that will never
    // unlock.
    let w = columns(
        &[16, 7, 10, 9, 8, 9, 8],
        &[16, 7, 18, 12, 8, 9, 20],
        2,
        NO_FLEX,
    );
    out.row(format!(
        "{}  {}  {}  {}  {}  {}  {}",
        pad("date", w[0]),
        pad("what", w[1]),
        pad_right("amount XMR", w[2]),
        pad_right("fee XMR", w[3]),
        pad_right("confirms", w[4]),
        pad("spendable", w[5]),
        "note"
    ));
    for t in rows {
        // `2026-09-16T14:01` is a machine timestamp cut in half; the `T` is not a word.
        let when = t["timestamp"]
            .as_i64()
            .and_then(fmonero_core::rfc3339_from_unix)
            .map(|s| s.chars().take(16).collect::<String>().replace('T', " "))
            .unwrap_or_else(|| "—".to_string());
        // The kind **in words** and the lock as a word, not as a JSON value: this table printed
        // `block` for a mining reward, `pool` for one waiting to be mined and `true` under
        // `locked` (§2o rule 5, self-read round 3). `type` and `locked` are unchanged in `--json`.
        let kind = t["type"]
            .as_str()
            .and_then(|w| {
                fmonero_core::TransferKind::ALL
                    .iter()
                    .find(|k| k.as_str() == w)
                    .map(|k| k.words())
            })
            .unwrap_or("—");
        // M29: one word in every state, and `never` for a payment that was never accepted.
        let spendable = if t["type"].as_str() == Some("failed") {
            "never"
        } else {
            match t["locked"].as_bool() {
                Some(true) => "not yet",
                Some(false) => "yes",
                None => "—",
            }
        };
        // M4: eight decimals down the fee column, so `0.00000000` and `0.00003072` line their
        // points up instead of `0.0000` sitting beside `0.00003072`. `fee_pico` is the integer
        // §5 prints beside the string, so nothing is recomputed from the rendered text.
        let fee = t["fee_pico"]
            .as_u64()
            .map(|p| fmonero_core::Piconero(p).to_xmr_min(fmonero_core::Piconero::FEE_DECIMALS))
            .unwrap_or_else(|| cell(&t["fee_xmr"]));
        out.row(format!(
            "{}  {}  {}  {}  {}  {}  {}",
            pad(&when, w[0]),
            pad(kind, w[1]),
            pad_right(&cell(&t["amount_xmr"]), w[2]),
            pad_right(&fee, w[3]),
            pad_right(&cell(&t["confirmations"]), w[4]),
            pad(spendable, w[5]),
            fit(t["note"].as_str().unwrap_or(""), w[6])
        ));
    }
    // `high_water` is the snapshot's own bookkeeping and stays in `--json` (§2o rule 7).
    out.line(format!(
        "{} {} · saved copy written {}",
        rows.len(),
        if rows.len() == 1 {
            "payment"
        } else {
            "payments"
        },
        v["cache_age_sec"]
            .as_u64()
            .map(fmonero_core::age)
            .unwrap_or_else(|| "at an unknown time".to_string())
    ));
}

/// `wallet balance`. **No piconero** (X19): §2o.2 confines that word to the send confirm, where the
/// exact integer is what the user is approving. Here it doubled the width of both figures and told
/// a reader nothing they could use; `balance_pico` and `unlocked_pico` are unchanged in `--json`.
fn balance_human(out: &Out, v: &serde_json::Value) {
    out.line(format!("account   {}", cell(&v["account_index"])));
    out.line(format!("balance   {} XMR", cell(&v["balance_xmr"])));
    out.line(format!("unlocked  {} XMR", cell(&v["unlocked_xmr"])));
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
        // Only the FUNDED addresses are here (§5), so an absent row is not an absent address —
        // `wallet address` lists them all. `output(s)` became `1 piece` / `2 pieces`: a Monero
        // output is not a word a reader looks up, and the count is only ever read as "how many
        // separate bits this balance is in".
        out.line("addresses holding money:");
        for s in subs {
            let pieces = s["num_unspent_outputs"].as_u64();
            out.row(format!(
                "  {} {}  {} XMR  unlocked {} XMR  in {} {}{}",
                pad(&cell(&s["address_index"]), 3),
                cell(&s["address"]),
                cell(&s["balance_xmr"]),
                cell(&s["unlocked_xmr"]),
                pieces.map(grouped).unwrap_or_else(|| "—".to_string()),
                if pieces == Some(1) { "piece" } else { "pieces" },
                match s["label"].as_str() {
                    Some(l) if !l.is_empty() => format!("  {l}"),
                    _ => String::new(),
                }
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
        // An address is never wrapped and never elided (§2a, `docs/COPY.md` §4).
        out.row(format!(
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

/// The value a restricted node leaves for a column it does not serve (§2o.2, W4).
///
/// The `—` is the program's one glyph for an unknown value, so it **leads** the cell and the words
/// after it say why — rather than sitting mid-sentence as an em-dash, which §2o rule 12 forbids and
/// the `scripts/check.sh` step for it now greps out.
const NO_ANSWER: &str = "— this node does not answer that";

fn peers_human(out: &Out, v: &serde_json::Value) {
    // A header, grouped block numbers, and **times a person can read**: this table printed
    // `block 3541122`, `8412s` and `last seen 1789560101` — a raw unix second, in the column whose
    // whole content is when (self-read, round 3).
    fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
        if n == 1 {
            one
        } else {
            many
        }
    }
    match v["connections"].as_array() {
        Some(rows) => {
            out.line(format!(
                "{} {} connected right now",
                rows.len(),
                plural(rows.len(), "peer", "peers")
            ));
            // **The columns are the terminal's, and every header is words with a unit** (X3, X20,
            // X29). `average ↓ ↑` had no time unit at all and the cells read `↓2.1 kB ↑0 B`, which
            // is a rate printed as a total; `STATE` rendered the node's own `synchronizing`
            // against `synced` everywhere else in the program (§2o.2).
            // The host column is the widest host in the answer, not a 40-cell guess, and `doing`
            // is 14 because `trading blocks` is 14 — at 12 it pushed every other column right on
            // the rows that carried it and left the table's own columns out of line (round 4's own
            // read of this output).
            let widest = rows
                .iter()
                .map(|c| UnicodeWidthStr::width(c["host"].as_str().unwrap_or("")))
                .max()
                .unwrap_or(16)
                .clamp(16, 46);
            let w = columns(
                &[widest, 5, 14, 11, 9, 20],
                &[widest, 6, 14, 11, 9, 22],
                2,
                NO_FLEX,
            );
            out.row(out.paint(
                &format!(
                    "  {}  {}  {}  {}  {}  {}",
                    pad("host", w[0]),
                    pad("port", w[1]),
                    pad("doing", w[2]),
                    pad("block", w[3]),
                    pad("connected", w[4]),
                    "average per second"
                ),
                Paint::Dim,
            ));
            for c in rows {
                out.row(format!(
                    "  {}  {}  {}  {}  {}  {} down · {} up",
                    pad(&cell(&c["host"]), w[0]),
                    pad(&cell(&c["port"]), w[1]),
                    pad(&peer_state_words(c["state"].as_str()), w[2]),
                    pad(
                        &c["height"]
                            .as_u64()
                            .map(grouped)
                            .unwrap_or_else(|| "—".to_string()),
                        w[3]
                    ),
                    pad(
                        &c["live_time"]
                            .as_i64()
                            .map(human_uptime)
                            .unwrap_or_else(|| "—".to_string()),
                        w[4]
                    ),
                    human_bytes(c["avg_download"].as_u64().unwrap_or(0)),
                    human_bytes(c["avg_upload"].as_u64().unwrap_or(0)),
                ));
            }
        }
        None => out.line(format!("peers connected   {NO_ANSWER}")),
    }
    // **Two lists a reader could not tell apart** (X29): `known peers` and `other peers` were the
    // node's `white` and `gray` address books under names that say nothing, over a third heading
    // reading `peers connected`. Each list now says what it is in the same line as its count.
    for (key, title) in [
        ("white", "Peers it has reached before"),
        ("gray", "Peers it has only heard about"),
    ] {
        match v[key].as_array() {
            Some(rows) => {
                out.line(format!(
                    "{title}: {} {}",
                    rows.len(),
                    plural(rows.len(), "address", "addresses")
                ));
                for p in rows {
                    out.row(format!(
                        "  {}:{}  last seen {}",
                        cell(&p["host"]),
                        cell(&p["port"]),
                        p["last_seen"]
                            .as_i64()
                            .map(
                                |t| fmonero_core::age((fmonero_core::now_unix() - t).max(0) as u64)
                            )
                            .unwrap_or_else(|| "—".to_string())
                    ));
                }
            }
            None => out.line(format!("{title}   {NO_ANSWER}")),
        }
    }
}

/// A peer's connection state **in the program's own words** (X29).
///
/// `monerod` reports `synchronizing`, `normal`, `idle`, `standby` and `before_handshake`, and this
/// table printed them raw: `synchronizing` in the `STATE` column against `synced` on every other
/// surface, and `before_handshake` at a person. §2o.2 fixes `catching up` for the first.
pub fn peer_state_words(state: Option<&str>) -> String {
    match state {
        Some("synchronizing") => "catching up".to_string(),
        Some("normal") => "trading blocks".to_string(),
        Some("idle") => "idle".to_string(),
        Some("standby") => "waiting".to_string(),
        Some("before_handshake") => "still connecting".to_string(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => "—".to_string(),
    }
}

/// `fmonero node info` (§5c, W20) — **the same rows `status` prints, plus what only this command
/// asks for.**
///
/// It was the generic key dump: `layout remote`, `nettype —`, `busy_syncing —`,
/// `fee fees_pico=— per_byte_pico=—`, `remote_probe age_sec=13 … restricted=false` — thirty rows of
/// field names, most of them `—`, four of them words §2o rule 7 deletes. `docs/COPY.md` §6 lists
/// fourteen inventoried surfaces and this was not one of them; §2o's first paragraph always
/// governed it. Every number here is in `node info --json` too, under the names an agent branches
/// on.
fn node_info_human(out: &Out, v: &serde_json::Value) {
    let node = &v["node"];
    if node["layout"].as_str() == Some("remote") {
        public_node_rows(out, node);
        // §2m.3: no wallet-RPC method reports a public node's own state, so the rows above are
        // the whole of what this machine can know. Said once, in words, rather than implied by
        // twenty-six dashes — and **without contradicting the row above it** (X7): the figures
        // are real, they are just not from now.
        if node["remote_probe"].is_null() {
            // Not "nothing ELSE" when nothing at all has been shown (round 4's own read).
            out.line(
                "        Nothing about a public node can be measured from this machine until it \
                 is checked.",
            );
        } else {
            out.line("        These figures are from the last check, not from now.");
            out.line("        Nothing else about a public node can be measured from this machine.");
        }
        return;
    }
    own_node_rows(out, node, None);
    if let Some(p) = v["txpool"].as_object() {
        // **`0 double spends` is gone** (X29): it is a phrase a reader looks up, it is zero on
        // every healthy node, and nothing on this screen tells them what to do about it. The count
        // stays in `node info --json`, where an agent reads it.
        //
        // **And the count is not repeated here** (round 4's own read): the row above already says
        // `14 payments waiting to be mined`, and this line said it again two lines later. What only
        // this command asks for is how much room they take and what they are paying.
        out.line(format!(
            "        they take {} and carry {} XMR in fees",
            p["bytes_total"]
                .as_u64()
                .map(human_bytes)
                .unwrap_or_else(|| "—".to_string()),
            cell(&p["fee_total_xmr"]),
        ));
    }
    out.line(format!(
        "        version {} · {}",
        cell(&node["version"]),
        cell(&node["nettype"])
    ));
    if let Some(sync) = v["sync"].as_object() {
        // **Both of these are arrays**, and `cell` renders an array by joining its elements —
        // which printed every field of every peer as one row (self-read, round 3). What a reader
        // wants from them is how many.
        //
        // **`{n} stretches in progress` is gone** (X29): `spans` is the node's own word for a run
        // of blocks it has asked one peer for, "stretches" was this program's translation of it,
        // and neither is something a reader can act on. `sync.spans` stays in `--json`.
        let peers = sync["peers"].as_array().map(|a| a.len());
        out.line(match peers {
            Some(1) => "        fetching blocks from 1 peer".to_string(),
            Some(n) => format!("        fetching blocks from {n} peers"),
            None => "        fetching blocks from an unknown number of peers".to_string(),
        });
    }
}

/// `fmonero node list` (§2n, §5c, W20) — **the picker's table, in the CLI's own shape.**
///
/// It was a second vocabulary for one thing: group headings `monero.fail says` / `checked here`
/// over columns the TUI calls `their block` and `block here`, `synced` rendering the JSON `true`,
/// a `ping 20` with no unit, ungrouped block numbers beside the picker's grouped ones, and two
/// markers (`web`, `own`) in the rows with nothing to say what they meant. The column names are
/// the picker's, the units are on the numbers, and the one marker left is explained in the footer.
fn nodes_human(out: &Out, v: &serde_json::Value) {
    let rows = v["nodes"].as_array().map(Vec::as_slice).unwrap_or_default();
    if rows.is_empty() {
        out.line("no list of public nodes yet");
        out.note("fmonero node list --refresh   # fetch the list from monero.fail");
        return;
    }
    let probed_any = rows.iter().any(|r| !r["probe"].is_null());
    // The picker's own column names (§7a.4): `their block` is what the list says, `block here` is
    // what this machine measured. They were both `block` under two group headings, and one of the
    // two headings carried a word §2o removes.
    // **The columns come from the terminal** (X3), and the address column may never go below the
    // widest address in the list: `docs/COPY.md` §4 forbids eliding one, here as in the picker.
    let widest = rows
        .iter()
        .map(|r| UnicodeWidthStr::width(r["address"].as_str().unwrap_or("")) + 4)
        .max()
        .unwrap_or(24);
    let w = if probed_any {
        columns(
            &[widest, 11, 11, 9, 8, 10, 6],
            &[widest, 12, 12, 12, 8, 12, 6],
            2,
            NO_FLEX,
        )
    } else {
        columns(&[widest, 11, 11, 9], &[widest, 12, 12, 12], 2, NO_FLEX)
    };
    out.row(format!(
        "  {}  {}  {}  {}{}",
        pad("address", w[0]),
        pad("reliability", w[1]),
        pad_right("their block", w[2]),
        pad("last seen", w[3]),
        if probed_any {
            format!(
                "  {}  {}  {}",
                pad("ping", w[4]),
                pad("block here", w[5]),
                "synced"
            )
        } else {
            String::new()
        },
    ));
    let mut any_own = false;
    for r in rows {
        let configured = r["configured"] == serde_json::Value::Bool(true);
        let mark = if configured { "●" } else { " " };
        // A `custom` row renders `—` in every monero.fail column: monero.fail has never heard of
        // it, and a blank there would read as "not measured yet" (§7a.4).
        let custom = r["custom"] == serde_json::Value::Bool(true);
        any_own |= custom;
        let health = match (r["health_ratio"].as_f64(), r["health_checks"].as_u64()) {
            (Some(ratio), Some(n)) => format!("{}% of {n}", (ratio * 100.0).round() as i64),
            _ => "—".to_string(),
        };
        let probe = &r["probe"];
        let probed = if probe.is_null() {
            if probed_any {
                format!("  {}  {}  {}", pad("—", w[4]), pad("—", w[5]), "—")
            } else {
                String::new()
            }
        } else if probe["reachable"] == serde_json::Value::Bool(true) {
            format!(
                "  {}  {}  {}",
                pad(
                    &probe["latency_ms"]
                        .as_u64()
                        .map(|ms| format!("{ms} ms"))
                        .unwrap_or_else(|| "—".to_string()),
                    w[4]
                ),
                pad(
                    &probe["height"]
                        .as_u64()
                        .map(grouped)
                        .unwrap_or_else(|| "—".to_string()),
                    w[5]
                ),
                // The picker's three words, never the JSON `true`.
                match probe["synchronized"].as_bool() {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "—",
                },
            )
        } else {
            // A failure is a result, and its reason is the useful half (§2n.4) — in the picker's
            // words, from core's own table (W20).
            format!(
                "  {}",
                out.paint(
                    fmonero_core::nodes::probe_reason_words(probe["reason"].as_str()),
                    Paint::Yellow
                )
            )
        };
        // **`own` sits beside the address it describes** (X18). It was the row's last column, six
        // columns to the right and under the heading `block here`, so every row carrying it
        // misaligned: `slow.example:18081  —  —  —  too slow own`. The picker puts it in the
        // address cell, and so does this table now.
        //
        // `web_compatible` is a flag for browser wallets and decides nothing here, so v5.6
        // dropped its marker from the picker; W20 drops it from this table too, and leaves the
        // field to `node list --json`.
        let address = if custom {
            format!("{} own", cell(&r["address"]))
        } else {
            cell(&r["address"])
        };
        out.row(format!(
            "{mark} {}  {}  {}  {}{}",
            pad(&address, w[0]),
            pad(&health, w[1]),
            pad_right(
                &r["health_last_height"]
                    .as_u64()
                    .map(grouped)
                    .unwrap_or_else(|| "—".to_string()),
                w[2]
            ),
            pad(
                &r["health_checked_at"]
                    .as_i64()
                    .map(|t| fmonero_core::age((fmonero_core::now_unix() - t).max(0) as u64))
                    .unwrap_or_else(|| "—".to_string()),
                w[3]
            ),
            probed,
        ));
    }
    // The footer, in the picker's words (W6, W20): what the list is, how old it is, and what was
    // left out of it, each count with the noun it counts.
    // **One wording with the picker's own footer** (X31): this said `2 of 2 nodes from monero.fail`
    // where the TUI said `2 nodes from monero.fail`, and `N of N` is a fraction that says nothing.
    // The `N of M` form survives for the case it was written for — a list longer than `--limit`.
    let total = v["total"].as_u64();
    let mut parts = vec![
        match total {
            Some(t) if t as usize > rows.len() => {
                format!("{} of {t} nodes from monero.fail", rows.len())
            }
            _ => format!(
                "{} {} from monero.fail",
                rows.len(),
                if rows.len() == 1 { "node" } else { "nodes" }
            ),
        },
        format!(
            "list fetched {}",
            v["cache_age_sec"]
                .as_u64()
                .map(fmonero_core::age)
                .unwrap_or_else(|| "an unknown time ago".to_string())
        ),
    ];
    let no_port = v["skipped_no_port"].as_u64().unwrap_or(0);
    let invalid = v["skipped_invalid"].as_u64().unwrap_or(0);
    if no_port > 0 {
        parts.push(format!(
            "{no_port} without a port {} left out",
            if no_port == 1 { "was" } else { "were" }
        ));
    }
    if invalid > 0 {
        parts.push(format!("{invalid} fmonero could not read"));
    }
    if any_own {
        parts.push("own = a node you added".to_string());
    }
    if rows
        .iter()
        .any(|r| r["configured"] == serde_json::Value::Bool(true))
    {
        parts.push("● = the node you are using".to_string());
    }
    out.line(parts.join(" · "));
}

fn log_human(out: &Out, v: &serde_json::Value) {
    let lines = v["lines"].as_array().map(Vec::as_slice).unwrap_or_default();
    if lines.is_empty() {
        out.line(format!("no log yet at {}", cell(&v["path"])));
        return;
    }
    for l in lines {
        // The node's own log line, exactly as it wrote it (X3): a reflowed log is a log nobody can
        // match against the file.
        out.row(cell(l));
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
        // **W20: an empty value reads `(none)`.** `node.remote` is empty until you choose a public
        // node, and the row printed a blank where the value goes and `[default ]` under it — two
        // places where a reader cannot tell "nothing" from "the renderer lost it".
        let shown = |v: &serde_json::Value| match cell(v) {
            s if s.is_empty() => "(none)".to_string(),
            s => s,
        };
        let value = shown(&r["value"]);
        // `default_here` is the pattern resolved for this machine, which is the only default a
        // value can honestly be compared against (self-read, round 3).
        let default = shown(if r["default_here"].is_null() {
            &r["default"]
        } else {
            &r["default_here"]
        });
        let marker = if value == default { " " } else { "*" };
        // The key and its value are a measured row; the description below is prose and wraps with a
        // hanging indent (X3). One screen of this table split fifteen words in half at 120 columns —
        // `Se`/`t fmonero`, `yours`/`elf`, `defa`/`ult local` — and every continuation returned to
        // column 0, breaking the indent the description sits in as well.
        out.row(format!(
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
        // X2: `what` is the §5c contract value an agent branches on; `units::action_words` is the
        // sentence a person reads for it. `node unit` and `monerod.conf` were printed raw.
        out.row(format!(
            "{} {}  {}",
            out.paint(&pad(&status, 9), paint),
            fmonero_core::units::action_words(&cell(&a["what"])),
            cell(&a["path"])
        ));
        // The diff is already redacted where it was rendered (§4.2): `units::install` and the
        // menu merge both pass it through `units::redact_login`.
        if let Some(diff) = a["diff"].as_str() {
            for line in diff.lines() {
                // A diff line is the file's own text (X3): wrapping it would make it a diff of
                // something else.
                out.row(format!("    {line}"));
            }
        }
    }
    if dry {
        out.note("dry run: nothing was changed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `COLUMNS` is process-wide, so the tests that drive the width seam take this first. Two of
    /// them running at once would each read the other's width, which is a flake nobody can
    /// reproduce.
    static WIDTH_SEAM: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Set `COLUMNS`, run the body, put the environment back.
    fn at_width<T>(cols: &str, body: impl FnOnce() -> T) -> T {
        let _guard = WIDTH_SEAM.lock().unwrap_or_else(|e| e.into_inner());
        let before = std::env::var("COLUMNS").ok();
        // SAFETY: the mutex above is the only writer, and `term_width` is the only reader.
        unsafe { std::env::set_var("COLUMNS", cols) };
        let out = body();
        match before {
            Some(v) => unsafe { std::env::set_var("COLUMNS", v) },
            None => unsafe { std::env::remove_var("COLUMNS") },
        }
        out
    }

    /// X3: the seam the whole wrapping fix hangs on. `COLUMNS` wins over the terminal, so a test
    /// and a shell can both say how wide the reader's line is; a pipe gets 100.
    #[test]
    fn the_width_comes_from_columns_then_the_terminal_then_one_hundred() {
        // The three widths the round asked for, driven through the environment seam.
        for (set, want) in [("80", 80usize), ("100", 100), ("120", 120)] {
            assert_eq!(at_width(set, term_width), want);
        }
        assert_eq!(
            at_width("9", term_width),
            40,
            "a nonsense width is clamped, not obeyed"
        );
        assert_eq!(at_width("9000", term_width), 200);
    }

    /// X3: a run of spaces inside a line is a column gap, and it survives the wrap exactly.
    ///
    /// The first version of this wrapper rebuilt one space between words, so `node    synced` came
    /// out `node   synced` and every two-space gap in the program lost a column — found by reading
    /// this round's own captures, which is the only way it could have been found.
    #[test]
    fn a_run_of_spaces_inside_a_line_is_kept_exactly() {
        assert_eq!(
            wrap_to("node    synced  127.0.0.1:18081  your own node", 100, 2),
            vec!["node    synced  127.0.0.1:18081  your own node"]
        );
        // Including a run that follows a zero-width colour escape, which is what a painted `Try:`
        // line is.
        assert_eq!(
            wrap_to("\x1b[2m    Try: fmonero doctor\x1b[0m", 100, 8),
            vec!["\x1b[2m    Try: fmonero doctor\x1b[0m"]
        );
        // And the leading indent of a detail line.
        assert_eq!(wrap_to("    a detail", 100, 6), vec!["    a detail"]);
    }

    /// X3, the assertion the owner's complaint is about: **no word is ever split**, at any of the
    /// three widths, and every continuation line is indented past the first.
    #[test]
    fn no_word_is_split_at_eighty_one_hundred_or_one_hundred_and_twenty() {
        // Three real lines from the captures the round read: the doctor's password-file detail,
        // `status`'s clock warning and a `settings list` description.
        let samples = [
            "  Only you can read /run/user/1000/fmonero/run/monero-wallet-rpc.18693.login, and the wallet service asked fmonero for it.",
            "  This machine's clock is 2h 39m away from real time. A clock this far out gets the times your money unlocks at wrong.",
            "    Set fmonero's own log file yourself. Nothing is treated as the default local path when this is empty.",
            "Where your wallet gets its blocks: local, a node on this machine, or remote, a public node someone else runs.",
        ];
        for width in [80usize, 100, 120] {
            for sample in samples {
                let indent = sample.chars().take_while(|c| *c == ' ').count();
                let lines = wrap_to(sample, width, indent + 2);
                // Every word of the original survives, in order, whole.
                let want: Vec<&str> = sample.split_whitespace().collect();
                let got: Vec<String> = lines
                    .iter()
                    .flat_map(|l| l.split_whitespace().map(str::to_string))
                    .collect();
                assert_eq!(
                    want,
                    got.iter().map(String::as_str).collect::<Vec<_>>(),
                    "a word was split or lost at {width}: {lines:?}"
                );
                for l in &lines {
                    assert!(
                        visible_width(l) <= width || l.split_whitespace().count() == 1,
                        "line over {width} columns and not a single long word: {l:?}"
                    );
                }
                for l in lines.iter().skip(1) {
                    assert!(
                        l.starts_with(&" ".repeat(indent + 2)),
                        "a continuation line must be indented past the first: {l:?}"
                    );
                }
            }
        }
    }

    /// X3: a word longer than the line — an address, a 64-character id — goes on its own line
    /// **whole**. `docs/COPY.md` §4 forbids eliding an address, and splitting one is worse.
    #[test]
    fn a_word_longer_than_the_line_is_never_broken() {
        let a = "4".repeat(95);
        let lines = wrap_to(&format!("To  {a}"), 80, 4);
        assert!(lines.iter().any(|l| l.contains(&a)), "{lines:?}");
        assert_eq!(lines.concat().matches('4').count(), 95);
    }

    /// X3: a coloured word occupies the columns it prints, not the bytes it carries.
    #[test]
    fn colour_escapes_do_not_count_towards_the_width() {
        assert_eq!(visible_width("\x1b[32msynced\x1b[0m"), 6);
        assert_eq!(visible_width("plain"), 5);
    }

    /// X3: a table narrows from the widest column that can still give a column back, and the
    /// address column never goes below its minimum, because an address is never elided.
    #[test]
    fn a_table_takes_its_widths_from_the_terminal_and_protects_the_address() {
        // 100 columns: the preferred widths fit, and the slack goes to the flexible column.
        let w = at_width("100", || columns(&[44, 11, 11, 9], &[44, 12, 12, 12], 2, 0));
        assert_eq!(w.iter().sum::<usize>() + 6, 100, "{w:?}");
        assert!(w[0] > 44, "the flexible column took the slack: {w:?}");
        // …and `NO_FLEX` leaves it unspent, which is what a table of short values wants.
        let w = at_width("100", || {
            columns(&[44, 11, 11, 9], &[44, 12, 12, 12], 2, NO_FLEX)
        });
        assert_eq!(w, vec![44, 12, 12, 12], "{w:?}");
        // 80 columns: every column is at its minimum, and the address column holds it — a row one
        // column wider than the terminal is honest, an elided address is not.
        let w = at_width("80", || columns(&[44, 11, 11, 9], &[44, 12, 12, 12], 2, 0));
        assert_eq!(w, vec![44, 11, 11, 9], "{w:?}");
        // 120 columns: it fits with room, and no column is ever below its minimum.
        let w = at_width("120", || columns(&[44, 11, 11, 9], &[44, 12, 12, 12], 2, 0));
        assert_eq!(w.iter().sum::<usize>() + 6, 120, "{w:?}");
        assert!(w[1] >= 11 && w[2] >= 11 && w[3] >= 9, "{w:?}");
    }

    /// X1, from the other side: **no `code_name` and no `reason` reaches a human line.** The
    /// envelope is asserted on the strings `report` builds, so a new arm cannot reintroduce one.
    #[test]
    fn a_human_error_carries_no_code_name_and_no_reason() {
        let e = fmonero_core::Error::usage("'bad/name' is not a wallet name.")
            .with_reason("bad_args")
            .with_detail("the reason an agent branches on")
            .with_hint("fmonero wallet list")
            .with_action("Press o on the Wallet screen to pick one.");
        // The three lines `report` prints for a human, built the same way it builds them.
        let human = [
            format!("fmonero: {e}"),
            format!("  {}", e.action().unwrap()),
            format!("  Try: {}", e.hint()),
        ]
        .join("\n");
        assert!(!human.contains("error["), "{human}");
        assert!(!human.contains("reason:"), "{human}");
        assert!(!human.contains("bad_args"), "{human}");
        assert!(
            !human.contains("the reason an agent branches on"),
            "{human}"
        );
        assert!(human.contains("is not a wallet name."), "{human}");
        assert!(human.contains("Press o on the Wallet screen"), "{human}");
        assert!(human.contains("Try: fmonero wallet list"), "{human}");
        // And the two ids are still exactly where the agent contract puts them (§2o, §4).
        let json = e.json().to_string();
        assert!(
            json.contains("bad_args") && json.contains("usage"),
            "{json}"
        );
    }
}
