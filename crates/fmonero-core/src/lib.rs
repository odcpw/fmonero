//! fmonero's engine: everything the CLI, the TUI, the doctor and the bar widget share.
//!
//! **This crate is silent.** No function here writes to stdout or stderr; all output is
//! produced by the binary (`output.rs`), and a Phase 3 gate greps this crate for print
//! macros (DESIGN.md §2g).
//!
//! The public surface was frozen at the Phase 1a gate (§5b, §8): later phases fill bodies
//! in, they do not change signatures. Every function returns `Result<_, Error>` — never
//! `anyhow` — so the binary maps a failure to its §4a exit code without downcasting.
//!
//! What is **implemented** at the 1a gate: `error`, `context`, `paths`, `settings`, `proc`,
//! `amount`, `address`, `rpc::require_loopback`, `rpc::agent`, the two client constructors'
//! loopback gate, `status()`'s empty document and `agent::capabilities_json`. Everything
//! else is a signature-only module whose bodies return `Error::unimplemented` or empty data
//! — §2h's one sanctioned handover, listed in §8.

pub mod address;
pub mod agent;
pub mod amount;
pub mod context;
pub mod error;
pub mod fsinfo;
pub mod history;
pub mod log;
pub mod nodes;
pub mod paths;
pub mod proc;
pub mod rpc;
pub mod send;
pub mod session;
pub mod settings;
pub mod state;
pub mod status;
pub mod units;
pub mod wallet_dir;

pub use address::{AddrForm, AddrKind, Address, NetType, PREFIXES};
pub use amount::Piconero;
pub use context::{Context, Systemctl};
pub use error::{Error, EXIT_CODES, REASONS};
pub use history::{Destination, HistoryQuery, SubaddrIndex, Transfer, TransferKind};
pub use nodes::{ListOpts, NodeList, Probe, ProbeTarget, PublicNode};
pub use paths::Paths;
pub use proc::{CancelToken, Output};
pub use rpc::{Login, NodeClient, NodeMutators, WalletClient};
pub use send::{BuiltTx, SendRequest, SweepRequest, TransferRequest};
// `wallet_session::open` and `node_session::up` are spelled without `session::` in §5b.
pub use session::{node_mode, node_session, wallet_session};
pub use settings::{
    Network, NodeMode, Settings, SslSupport, TimeoutSettings, Timeouts, TuiSettings,
};
pub use state::{Daemon, PendingKind, PendingSend, PendingTx, StateDoc};
pub use status::{
    node_off_words, LastError, NetStatus, NodeStatus, PendingStatus, PendingTxStatus,
    RemoteProbeStatus, Severity, Staleness, Status, UnitsStatus, WalletStatus, Warning,
    WALLET_BEHIND_BLOCKS, WARNING_IDS,
};

/// The tool's name, as it appears in argv, hints and the plugin.
pub const NAME: &str = "fmonero";
/// The workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The Status/JSON schema version (§5). Bumped only by a breaking change to the document.
pub const SCHEMA_VERSION: u32 = 1;
/// The machine contract's version, independent of the tool version.
pub const CONTRACT_VERSION: &str = "1.0.0";

/// The one sentence fmonero uses when another program on this machine owns the node
/// (§2i's `system_unit` layout). **Core and the TUI share the literal so the two cannot
/// drift** (§2o): it is the refusal for `node up`, `node down` and the log-level key, and it
/// is what the Node screen's hero says.
pub const FOREIGN_NODE_READ_ONLY: &str =
    "Another program on this machine runs this node. fmonero can read it but not change it.";
/// The same fact stated as an observation rather than a refusal (the `foreign_node_running`
/// warning of §5, and the doctor's `node_layout` detail of §7).
pub const FOREIGN_NODE_INFO: &str =
    "Another program on this machine runs this node. fmonero reads it and never changes it.";

/// **Decision 7, in one sentence, for every surface** (§7a.5, §6.5, W10).
///
/// The wallet's scanning happens inside the wallet service, so closing the TUI window or the
/// bar's panel does not stop it. Three surfaces said that in three different wordings — the
/// TUI's two screens, the bar widget and this crate's own `wallet_behind` warning — and a user
/// who reads two of them cannot tell whether they describe one thing. The Rust callers quote
/// this constant; `plugin/oliver.monero/Panel.qml` carries the same literal and
/// `tests/plugin-smoke.sh` asserts the two are byte for byte the same sentence.
pub const SYNC_KEEPS_GOING: &str =
    "This keeps going in the background, even after you close this window.";

/// A block number as a person reads it: `3 765 346`, a space every three digits (§2o rule 9).
///
/// **One implementation for the whole program** (W7). It lived in `crates/fmonero/src/output.rs`,
/// which this crate cannot reach, so core's own warnings printed `6122 blocks to go` in the same
/// panel as `6 122 blocks left`, and the picker, the status bar and the hero each made their own
/// choice. The bar widget's `Model.grouped` is the JavaScript half of the same formatter, and
/// `tests/plugin-smoke.sh` holds it to the same output.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// How long ago something happened, in the words every surface uses: `5h 42m ago`, `3m 07s ago`,
/// `12s ago` (§2o.2's "checked 2m ago").
///
/// **One implementation** (W12): the TUI had this and the doctor divided by 60 itself, so a
/// five-and-a-half-hour-old list read `342 min ago` in the doctor and `5h 42m ago` on the screen.
pub fn age(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}h {m:02}m ago")
    } else if m > 0 {
        format!("{m}m {s:02}s ago")
    } else {
        format!("{s}s ago")
    }
}

/// A size on disk or on the wire: `1.5 GB`, `842 kB`, `0 B` (X6).
///
/// **One implementation for the whole program.** There were three, and they disagreed about both
/// halves: `output.rs` divided by 1000 and said `kB`, the TUI divided by 1024 and said `kB` (wrong
/// for its own divisor), and the widget divided by 1024 and said `KiB` — so one waiting pool read
/// `21.4 kB` in `node info`, `20.9 kB` in the TUI and `20.9 KiB` on the bar. The divisor is 1000
/// and the labels are the decimal ones, which is what a disk meter and a download both show a
/// person.
///
/// Integer arithmetic, with the one decimal place carried by hand: §2a.5's enforcement is a grep
/// for a float cast across `crates/*/src`, and a formatter that makes that grep fire is a formatter
/// somebody later exempts — at which point the grep no longer protects the amount paths it exists
/// for. Bytes are not money, but the grep cannot tell.
pub fn bytes(n: u64) -> String {
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

/// A size per second: `842 kB/s` (X29). The same formatter, so a rate and a total agree.
pub fn bytes_per_sec(n: u64) -> String {
    format!("{}/s", bytes(n))
}

/// How far along something is, in the one wording every surface uses: `34%`, `100%` (X11, X21).
///
/// **Rounded down, always.** There were three roundings — `{:.0}` in core's warning (which rounds
/// 33.5 up to 34), `Math.floor` in the widget and the raw JSON number in the CLI (`100.0%`) — so
/// one machine showed `Your node syncing 33%` over `Your node is 34% synced.` and `synced 100.0%`
/// under a badge already reading `synced`. Rounding down is the half that cannot lie: a node at
/// 99.97% reads `99%`, and only a node that is actually there reads `100%`.
/// `plugin/oliver.monero/Model.js`'s `percent` is the JavaScript half of this function and
/// `tests/plugin-smoke.sh` holds the two to the same output, the way `grouped` is held.
pub fn percent(p: f64) -> String {
    let clamped = if p.is_nan() { 0.0 } else { p.clamp(0.0, 100.0) };
    format!("{}%", clamped.floor() as i64)
}

/// Enough of a payment id to recognise it: the first eight characters, then the last six (W8).
///
/// A Monero id is 64 hexadecimal characters. Printed whole it is a paragraph in a panel 380
/// pixels wide and it wraps any terminal, which is what `send_may_be_in_flight` — the most
/// serious warning the program has — did on the bar. The whole id stays in `--json`, where the
/// agent that needs it reads it, and in the payment row `wallet history` prints.
pub fn short_id(id: &str) -> String {
    if id.chars().count() <= 16 {
        return id.to_string();
    }
    let chars: Vec<char> = id.chars().collect();
    let head: String = chars[..8].iter().collect();
    let tail: String = chars[chars.len() - 6..].iter().collect();
    format!("{head}…{tail}")
}

/// The whole of §5, without starting anything (see [`status::status`]).
pub fn status(ctx: &Context) -> Result<Status, Error> {
    status::status(ctx)
}

/// §5's `--check-node` (v5.6): the same document, plus a detached check of the configured
/// public node when the cached one is stale (see [`status::status_checking_node`]).
pub fn status_checking_node(ctx: &Context) -> Result<Status, Error> {
    status::status_checking_node(ctx)
}

/// Now, as RFC 3339 with the **local** offset and second precision (§5).
pub fn now_rfc3339() -> String {
    let zoned = jiff::Zoned::now();
    let ts = zoned.timestamp();
    let ts = ts.round(jiff::Unit::Second).unwrap_or(ts);
    ts.display_with_offset(zoned.offset()).to_string()
}

/// A Unix epoch second as RFC 3339 with the local offset (§5).
pub fn rfc3339_from_unix(seconds: i64) -> Option<String> {
    let ts = jiff::Timestamp::from_second(seconds).ok()?;
    let offset = jiff::tz::TimeZone::system().to_offset(ts);
    Some(ts.display_with_offset(offset).to_string())
}

/// The inverse of [`rfc3339_from_unix`]: a timestamp §5 renders as a string, back to a second
/// (v5.6).
///
/// §5 carries `wallet.last_refresh_at` as an RFC3339 string because that is what a JSON consumer
/// wants. The TUI wants an **age** — `refreshed 2s ago`, the same phrasing the node pane beside it
/// uses for its own measurement (DECISIONS V5) — and printing the timestamp instead was one of the
/// two things that row got wrong. Rather than add a second field to §5's schema for a value it
/// already carries, the one caller converts.
pub fn unix_from_rfc3339(s: &str) -> Option<i64> {
    s.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

/// Now, as a Unix epoch second.
pub fn now_unix() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// Now, as a Unix epoch **millisecond**. Used by exactly one thing (§5's `--check-node`),
/// which measures how long a detached check took by subtracting this from the answer file's
/// modification time; a second's resolution there would render every check as `0 ms`.
pub fn now_unix_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// Write `bytes` to `path` **at mode 0600**, through `.tmp.<pid>` + `rename(2)` (§2b.7).
///
/// The target mode is set at creation (`O_EXCL` on the temporary file with `mode(0o600)`),
/// never `chmod`ed afterwards, which would leave a window in which a file recording who was
/// paid what is readable by every local user. Every fmonero-owned file goes through here.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let dir = path
        .parent()
        .ok_or_else(|| Error::failure(format!("{} has no parent directory", path.display())))?;
    let tmp = dir.join(format!(
        ".{}.tmp.{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "fmonero".to_string()),
        std::process::id()
    ));
    // A leftover temporary from a killed process must not make the write fail.
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| Error::failure(format!("cannot create {}: {e}", tmp.display())))?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| Error::failure(format!("cannot write {}: {e}", tmp.display())))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::failure(format!("cannot replace {}: {e}", path.display()))
    })
}

/// §5b rule 3 in one place: **every enum that appears in JSON carries its wire spelling**,
/// asserted against the literal string §5/§5c/§7 promise, in both directions.
///
/// A plain `#[derive(Serialize)]` would emit `"UserUnit"`, `"Warn"` and `"In"` — a JSON
/// document no section of the design describes — and every one of these types is frozen at
/// the 1a gate and compiled against by Phase 2, so the wrong spelling would ship into the
/// plugin's parser, the doctor's output and the e2e assertions before anyone noticed. The
/// doctor's `Verdict` status is deliberately **not** here: it lives in
/// `crates/fmonero/src/doctor/checks.rs`, which this phase does not own, and the Phase 2
/// doctor gate asserts it instead (§8).
#[cfg(test)]
mod wire_spelling {
    use super::*;

    /// Assert one variant's wire spelling, and that it deserializes back.
    fn both_ways<T>(value: T, wire: &str)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug + Copy,
    {
        let json = serde_json::to_value(value).expect("serialize");
        assert_eq!(json, serde_json::json!(wire), "{value:?} must be {wire:?}");
        let back: T = serde_json::from_value(serde_json::json!(wire)).expect("deserialize");
        assert_eq!(back, value, "{wire:?} must read back as {value:?}");
    }

    #[test]
    fn every_enum_that_appears_in_json_spells_itself_as_the_design_promises() {
        // units::Layout — §5's `node.layout`, snake_case.
        both_ways(units::Layout::UserUnit, "user_unit");
        both_ways(units::Layout::SystemUnit, "system_unit");
        both_ways(units::Layout::Remote, "remote");
        both_ways(units::Layout::None, "none");
        // Severity — §5's `warnings[].severity`.
        both_ways(Severity::Info, "info");
        both_ways(Severity::Warn, "warn");
        both_ways(Severity::Error, "error");
        // PendingKind — §5's `pending.kind` and the on-disk record's.
        both_ways(PendingKind::Send, "send");
        both_ways(PendingKind::Sweep, "sweep");
        // TransferKind — §5c's `transfers[].type`, `block` included (§1b.4).
        both_ways(TransferKind::In, "in");
        both_ways(TransferKind::Out, "out");
        both_ways(TransferKind::Pending, "pending");
        both_ways(TransferKind::Failed, "failed");
        both_ways(TransferKind::Pool, "pool");
        both_ways(TransferKind::Block, "block");
        // Network — §4's `node.network`.
        both_ways(Network::Mainnet, "mainnet");
        both_ways(Network::Stagenet, "stagenet");
        both_ways(Network::Testnet, "testnet");
        // NetType — the wallet's own nettype (§2a.6), and `wallet validate`'s answer.
        both_ways(NetType::Mainnet, "mainnet");
        both_ways(NetType::Testnet, "testnet");
        both_ways(NetType::Stagenet, "stagenet");
        // AddrKind — `wallet validate`'s `integrated`/`subaddress` verdict.
        both_ways(AddrKind::Standard, "standard");
        both_ways(AddrKind::Integrated, "integrated");
        both_ways(AddrKind::Subaddress, "subaddress");
        // units::FileStatus — §5c's `setup.actions[].status`.
        both_ways(units::FileStatus::Written, "written");
        both_ways(units::FileStatus::Unchanged, "unchanged");
        both_ways(units::FileStatus::Skipped, "skipped");
        both_ways(units::FileStatus::Planned, "planned");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339_with_a_local_offset_and_no_zulu() {
        let now = now_rfc3339();
        assert!(now.contains('T'), "{now}");
        assert!(
            !now.ends_with('Z'),
            "§5 wants a local offset, not Zulu: {now}"
        );
        assert_eq!(
            now.len(),
            25,
            "expected 2026-09-16T14:02:11+02:00, got {now}"
        );
        let then = rfc3339_from_unix(1_700_000_000).expect("a real instant");
        assert!(then.starts_with("2023-11-1"), "{then}");
        assert!(now_unix() > 1_700_000_000);
    }

    /// W7, W12, W8: the three formatters every surface shares, so a second implementation is a
    /// test failure rather than a screen a reader has to reconcile.
    #[test]
    fn the_shared_formatters_render_the_way_every_surface_promises() {
        assert_eq!(grouped(3_541_122), "3 541 122");
        assert_eq!(grouped(6_122), "6 122");
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(age(20_520), "5h 42m ago");
        assert_eq!(age(187), "3m 07s ago");
        assert_eq!(age(0), "0s ago");
        let id = "bbbb1111222233334444555566667777888899990000aaaabbbbccccddddeeee";
        assert_eq!(short_id(id), "bbbb1111…ddeeee");
        assert_eq!(short_id("short"), "short");
        assert!(
            !short_id(id).contains(&id[20..40]),
            "the middle of the id is what the shortening removes"
        );
    }

    /// X6 and X11: the two formatters round 4 collapsed, with the three answers the three old
    /// implementations gave for one fact pinned as the thing that must not come back.
    #[test]
    fn one_byte_formatter_and_one_percent_for_every_surface() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        // The pool that read three ways: 21 400 bytes.
        assert_eq!(bytes(21_400), "21.4 kB");
        assert_eq!(bytes(2048), "2.0 kB");
        assert_eq!(bytes(1_500_000_000), "1.5 GB");
        assert_eq!(bytes(118_200_000_000), "118.2 GB");
        assert!(
            !bytes(2048).contains("KiB") && !bytes(2048).contains("2.0 kB/s"),
            "the binary label and the rate suffix are not this function's"
        );
        assert_eq!(bytes_per_sec(0), "0 B/s");
        assert_eq!(bytes_per_sec(8192), "8.1 kB/s");
        // Rounded down: 33.5 is 33, and 99.97 is never "100".
        assert_eq!(percent(33.5), "33%");
        assert_eq!(percent(99.97), "99%");
        assert_eq!(percent(100.0), "100%");
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(-1.0), "0%");
        assert_eq!(percent(101.0), "100%");
        assert_eq!(percent(f64::NAN), "0%");
    }

    #[test]
    fn a_private_write_lands_at_0600_and_replaces_atomically() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("fmonero-wp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        write_private(&path, b"{}\n").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "§2b.7: owner-only, at the target mode");
        write_private(&path, b"{\"a\":1}\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":1}\n");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // No temporary left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
