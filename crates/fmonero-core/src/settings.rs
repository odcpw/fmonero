//! fmonero's own settings (`~/.config/fmonero/config.toml`) — the §4 key table, its
//! validation, and the resolved deadlines of §2d.
//!
//! `KEYS` is the single table `settings list` prints and `settings set` validates
//! against. An unknown key is **redirected, never merely refused** (§4): twenty dotted
//! keys is exactly the namespace an agent half-remembers, and `KEYS` already holds the
//! data to suggest from.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::address::NetType;
use crate::error::Error;
use crate::paths::Paths;

/// The accepted `create_wallet` languages (§1b.4) — `(native, english)`, from
/// `src/mnemonics/electrum-words.cpp:437-452`. **Either** spelling is accepted by
/// upstream's `is_valid_language` (`:494-500`), the match is byte-exact, and `EnglishOld`
/// is deliberately absent: it is a `Language` class but not in `get_language_list()`, so
/// the daemon rejects it. Order as upstream.
pub const SEED_LANGUAGES: &[(&str, &str)] = &[
    ("Deutsch", "German"),
    ("English", "English"),
    ("Español", "Spanish"),
    ("Français", "French"),
    ("Italiano", "Italian"),
    ("Nederlands", "Dutch"),
    ("Português", "Portuguese"),
    ("русский язык", "Russian"),
    ("日本語", "Japanese"),
    ("简体中文 (中国)", "Chinese (simplified)"),
    ("Esperanto", "Esperanto"),
    ("Lojban", "Lojban"),
];

/// The default seed language, used when `--language` is omitted (§4.1).
pub const DEFAULT_SEED_LANGUAGE: &str = "English";

/// Validate a `create_wallet` language locally, before the daemon can answer a bare
/// `-1 UNKNOWN_ERROR` that the §1b.4 map would turn into exit 5 (§1b.4).
pub fn validate_seed_language(language: &str) -> Result<(), Error> {
    if SEED_LANGUAGES
        .iter()
        .any(|(native, english)| *native == language || *english == language)
    {
        return Ok(());
    }
    let english: Vec<&str> = SEED_LANGUAGES.iter().map(|(_, e)| *e).collect();
    Err(Error::usage(format!(
        "\"{language}\" is not one of the seed languages. Choose one of these: {}.",
        english.join(", ")
    ))
    .with_reason("bad_args")
    .with_hint(format!(
        "fmonero wallet create <name> --language {DEFAULT_SEED_LANGUAGE} --yes"
    )))
}

/// The resolved deadlines of §2d.
///
/// The first six come from `TimeoutSettings` (the §4 keys) and are clamped by
/// `--timeout`; `relay` (30 s) and `unit_probe` (30 s) are fixed constants with no
/// settings key, because neither is a knob a user has a reason to turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timeouts {
    pub node: Duration,
    pub wallet: Duration,
    pub refresh: Duration,
    pub transfer: Duration,
    pub open: Duration,
    pub systemctl: Duration,
    pub unit_probe: Duration,
    pub relay: Duration,
}

/// `node up`'s health probe, and `units::ensure_running`'s deadline (§2d).
pub const UNIT_PROBE_SEC: u64 = 30;
/// `relay_tx` (§2d).
pub const RELAY_SEC: u64 = 30;

impl Timeouts {
    /// The deadlines the settings ask for.
    pub fn from_settings(t: &TimeoutSettings) -> Timeouts {
        Timeouts {
            node: Duration::from_secs(t.node_sec),
            wallet: Duration::from_secs(t.wallet_sec),
            refresh: Duration::from_secs(t.refresh_sec),
            transfer: Duration::from_secs(t.transfer_sec),
            open: Duration::from_secs(t.open_sec),
            systemctl: Duration::from_secs(t.systemctl_sec),
            unit_probe: Duration::from_secs(UNIT_PROBE_SEC),
            relay: Duration::from_secs(RELAY_SEC),
        }
    }

    /// `--timeout <SECONDS>` **lowers** every deadline to `min(configured, SECONDS)` and
    /// never raises one (§4). It applies to the six settings-backed deadlines **and** to
    /// `unit_probe` and `relay`.
    pub fn clamp_to(&mut self, secs: u64) {
        let ceiling = Duration::from_secs(secs.max(1));
        for d in [
            &mut self.node,
            &mut self.wallet,
            &mut self.refresh,
            &mut self.transfer,
            &mut self.open,
            &mut self.systemctl,
            &mut self.unit_probe,
            &mut self.relay,
        ] {
            *d = (*d).min(ceiling);
        }
    }
}

/// Which network is configured. Exactly one at a time (§4).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Mainnet,
    Stagenet,
    Testnet,
}

impl Network {
    pub fn as_str(self) -> &'static str {
        match self {
            Network::Mainnet => "mainnet",
            Network::Stagenet => "stagenet",
            Network::Testnet => "testnet",
        }
    }
    /// The node's RPC port for this network (§1b.1).
    pub fn node_rpc_port(self) -> u16 {
        match self {
            Network::Mainnet => 18081,
            Network::Stagenet => 38081,
            Network::Testnet => 28081,
        }
    }
    /// The node's P2P port for this network (§1b.1).
    pub fn node_p2p_port(self) -> u16 {
        match self {
            Network::Mainnet => 18080,
            Network::Stagenet => 38080,
            Network::Testnet => 28080,
        }
    }
    /// The wallet RPC port for this network (§4): 18082 is monerod's ZMQ port, so the
    /// wallet sits at 18083 | 38083 | 28083.
    pub fn wallet_rpc_port(self) -> u16 {
        match self {
            Network::Mainnet => 18083,
            Network::Stagenet => 38083,
            Network::Testnet => 28083,
        }
    }
    /// The nettype an address of this network carries (§2a.6).
    pub fn net_type(self) -> NetType {
        match self {
            Network::Mainnet => NetType::Mainnet,
            Network::Stagenet => NetType::Stagenet,
            Network::Testnet => NetType::Testnet,
        }
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Network {
    type Err = Error;
    fn from_str(s: &str) -> Result<Network, Error> {
        match s {
            "mainnet" => Ok(Network::Mainnet),
            "stagenet" => Ok(Network::Stagenet),
            "testnet" => Ok(Network::Testnet),
            other => Err(Error::usage(format!(
                "\"{other}\" is not a Monero network. The three are mainnet, stagenet and \
                 testnet."
            ))
            .with_reason("bad_args")
            .with_hint("fmonero settings set node.network mainnet")),
        }
    }
}

/// Which daemon `monero-wallet-rpc` is pointed at (§2m.1, v5.2).
///
/// The **only** input to `units::detect_layout`'s `Remote` branch (§2i). It is a setting and
/// not a flag because every consumer — the wallet unit, `set_daemon`, `status`, the node
/// commands, the doctor, the TUI and the bar widget — has to change behaviour together, and a
/// flag on one command cannot coordinate that (§2m).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeMode {
    Local,
    Remote,
}

impl NodeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeMode::Local => "local",
            NodeMode::Remote => "remote",
        }
    }
    /// True when no local `monerod` is in play at all (§2m.3).
    pub fn is_remote(self) -> bool {
        matches!(self, NodeMode::Remote)
    }
}

impl std::fmt::Display for NodeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for NodeMode {
    type Err = Error;
    fn from_str(s: &str) -> Result<NodeMode, Error> {
        match s {
            "local" => Ok(NodeMode::Local),
            "remote" => Ok(NodeMode::Remote),
            other => Err(Error::usage(format!(
                "\"{other}\" is not one of the two. Use local for a node on this machine, or \
                 remote for a public one."
            ))
            .with_reason("bad_args")
            .with_hint("fmonero node local --yes")),
        }
    }
}

/// `set_daemon`'s `ssl_support`, and `--daemon-ssl`'s value (§1b.9 item 2, v5.2).
///
/// The wire spellings are epee's three and no others (`net_ssl.cpp:654-665`); they are also the
/// three values of `--daemon-ssl` (`wallet2.cpp:253`), so one type serves the unit render and the
/// `set_daemon` body. [`SslSupport::Enabled`] is never constructed by fmonero in v0.1: it throws
/// at process start without a CA file, a fingerprint list or `--daemon-ssl-allow-any-cert`
/// (`wallet2.cpp:416-432`), all three of which §0 cuts. It is a variant so that adding pinned TLS
/// in v0.2 is a settings key rather than a new type.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SslSupport {
    Disabled,
    Enabled,
    Autodetect,
}

impl SslSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            SslSupport::Disabled => "disabled",
            SslSupport::Enabled => "enabled",
            SslSupport::Autodetect => "autodetect",
        }
    }
}

impl std::fmt::Display for SslSupport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The unit names, which never change with the network (§4).
pub const NODE_UNIT: &str = "fmonero-monerod.service";
pub const WALLET_UNIT: &str = "fmonero-wallet-rpc.service";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NodeSettings {
    pub network: Network,
    /// §2m.1. `local` (the default) or `remote`.
    pub mode: NodeMode,
    pub url: String,
    /// The remote daemon address, in §2m.2's grammar; empty iff `mode == Local`.
    ///
    /// **NEVER handed to `NodeClient`/`WalletClient`** — it is rendered into the wallet unit's
    /// `--daemon-address` and sent as `set_daemon`'s `address`, both consumed by
    /// `monero-wallet-rpc` (§2c). fmonero's own HTTP client stays loopback-plaintext-only.
    pub remote: String,
    /// Sent as `set_daemon`'s `trusted` and rendered as `--trusted-daemon`/`--untrusted-daemon`.
    /// **A public node is never trusted**; this key exists only for a remote node the owner
    /// runs (§2m.6).
    pub remote_trusted: bool,
    pub unit: String,
    pub data_dir: PathBuf,
    pub log_file: PathBuf,
    pub prune: bool,
    /// v5.5, §2n.4: how often the TUI probes **the configured node** while the Node screen is
    /// showing. It is a TUI cadence and nothing else reads it — `status`, the widget, the
    /// doctor and `node info` spawn nothing, ever (§2e). `0` turns the automatic probe off and
    /// leaves `r` and the picker's `p`, which are user actions.
    pub remote_probe_sec: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WalletSettings {
    pub url: String,
    pub unit: String,
    pub dir: PathBuf,
    pub port: u16,
    pub account: u32,
    pub priority: u32,
    /// v5.5, §7a.5: when true, `fmonero tui` starts the wallet daemon on launch. Read in
    /// exactly one place, `tui::run`; `status` never honours it (§2e) and neither does the
    /// widget or the doctor.
    pub autostart: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimeoutSettings {
    pub node_sec: u64,
    pub wallet_sec: u64,
    pub refresh_sec: u64,
    pub transfer_sec: u64,
    pub open_sec: u64,
    pub systemctl_sec: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TuiSettings {
    pub poll_ms: u64,
    pub tick_ms: u64,
    /// v5.5, §7a.2: mouse capture. `false` starts the program with
    /// `MouseCapturePolicy::Off`, which is the escape hatch for a terminal whose own selection
    /// and scrollback the user needs back. Default **true**: the owner has no usable mouse for
    /// a keyboard-only app, which is what §7a.2 exists to fix.
    pub mouse: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub node: NodeSettings,
    pub wallet: WalletSettings,
    pub timeouts: TimeoutSettings,
    pub tui: TuiSettings,
}

impl Default for NodeSettings {
    fn default() -> Self {
        NodeSettings {
            network: Network::Mainnet,
            mode: NodeMode::Local,
            remote: String::new(),
            remote_trusted: false,
            url: "http://127.0.0.1:18081".to_string(),
            unit: NODE_UNIT.to_string(),
            data_dir: PathBuf::from("~/.bitmonero"),
            log_file: PathBuf::from("~/.bitmonero/bitmonero.log"),
            prune: true,
            remote_probe_sec: 60,
        }
    }
}

impl Default for WalletSettings {
    fn default() -> Self {
        WalletSettings {
            url: "http://127.0.0.1:18083".to_string(),
            unit: WALLET_UNIT.to_string(),
            dir: PathBuf::from("~/.local/share/fmonero/wallets/mainnet"),
            port: 18083,
            account: 0,
            // 2 = normal. NOT 0: since HF 15 priority 0 behaves exactly as priority 1,
            // the cheapest level, and is not a "wallet default" (§1b.4).
            priority: 2,
            autostart: false,
        }
    }
}

impl Default for TimeoutSettings {
    fn default() -> Self {
        TimeoutSettings {
            node_sec: 3,
            wallet_sec: 5,
            refresh_sec: 120,
            transfer_sec: 120,
            open_sec: 300,
            systemctl_sec: 10,
        }
    }
}

impl Default for TuiSettings {
    fn default() -> Self {
        TuiSettings {
            poll_ms: 1000,
            tick_ms: 120,
            mouse: true,
        }
    }
}

/// `(key, default, one-line doc)` — the §4 table, and the thing `settings list` prints.
/// The defaults are written as the user sees them: a leading `~` is expanded once, when
/// the settings load (§4.2).
///
/// **Every description was rewritten in v5.6** (§2o, DECISIONS V9). `settings list` is a surface
/// a person reads — the owner's own `fmonero settings list` is one command from the TUI — and the
/// column read like the header comments of the code it came from: seven rows carried a `§`
/// number, and the set carried daemon, unit, systemd, RPC, loopback, probe, LMDB and four
/// command-line flags. Each row now says **what the setting does for the user and which values
/// are valid**, in that order, and the key name is left to say the rest: a settings key is
/// something the user types, which rule 5 exempts, and the sentence beside it is not.
const KEY_TABLE: &[(&str, &str, &str)] = &[
    (
        "node.network",
        "mainnet",
        "Which Monero network to use: mainnet, stagenet or testnet. mainnet is real money. The \
         other two are for practice. Set fmonero up again after a change.",
    ),
    (
        "node.mode",
        "local",
        "Where your wallet gets its blocks: local, a node on this machine, or remote, \
         a public node someone else runs.",
    ),
    (
        "node.remote",
        "",
        "The public node to use while node.mode is remote, written host:port. An http:// or \
         https:// prefix is allowed. A path or a password in the address is not.",
    ),
    (
        "node.remote_trusted",
        "false",
        "true tells a public node more about what your wallet is asking for, which is only \
         ever safe for a node you run yourself. Leave it false. true or false.",
    ),
    (
        "node.remote_probe_sec",
        "60",
        "How often, in seconds, fmonero checks the public node it is using. 0 turns the \
         checks off. Anything under 10 is treated as 10.",
    ),
    (
        "node.url",
        "http://127.0.0.1:18081",
        "The address fmonero talks to your own node on. The host has to be this machine. \
         Not used while you are on a public node.",
    ),
    (
        "node.unit",
        NODE_UNIT,
        "The name of the background service that runs your own node. Change it only if you \
         have a second one.",
    ),
    (
        "node.data_dir",
        "~/.bitmonero",
        "The folder your own node keeps its copy of the blockchain in. It grows to tens \
         of gigabytes.",
    ),
    (
        "node.log_file",
        "~/.bitmonero/bitmonero.log",
        "The file your own node writes its log to, and the file the Log screen shows.",
    ),
    (
        "node.prune",
        "true",
        "true keeps a smaller copy of the blockchain, which is what fits on this machine. \
         false keeps all of it. true or false.",
    ),
    (
        "wallet.url",
        "http://127.0.0.1:18083",
        "The address fmonero talks to the wallet service on. The host has to be this machine.",
    ),
    (
        "wallet.unit",
        WALLET_UNIT,
        "The name of the background service that holds your wallet open.",
    ),
    (
        "wallet.dir",
        "~/.local/share/fmonero/wallets/<network>",
        "The folder your wallet files are kept in. fmonero wallet list shows what is there.",
    ),
    (
        "wallet.port",
        "18083",
        "The port on this machine the wallet service answers on. Change it only if something \
         else is already using it.",
    ),
    (
        "wallet.account",
        "0",
        "Which account inside the wallet to use unless you say otherwise. 0 is the first one.",
    ),
    (
        "wallet.priority",
        "2",
        // M20: `fee level` is what the Send screen's chips and the send confirm call these four,
        // and this description was the one surface that named them without the phrase.
        "The fee level to use unless you say otherwise: 0 or 1 cheapest, 2 normal, 3 fast, \
         4 fastest.",
    ),
    (
        "wallet.autostart",
        "false",
        "true starts the wallet service as soon as the fmonero tui window opens. false waits \
         until you open or create a wallet. Nothing but the tui reads it.",
    ),
    (
        "timeouts.node_sec",
        "3",
        "How many seconds to wait for your own node to answer before giving up.",
    ),
    (
        "timeouts.wallet_sec",
        "5",
        "How many seconds to wait for the wallet service to answer before giving up.",
    ),
    (
        "timeouts.refresh_sec",
        "120",
        "How many seconds to allow while fmonero checks for payments.",
    ),
    (
        "timeouts.transfer_sec",
        "120",
        "How many seconds to allow while a payment is prepared.",
    ),
    (
        "timeouts.open_sec",
        "300",
        "How many seconds to allow while a wallet is being opened.",
    ),
    (
        "timeouts.systemctl_sec",
        "10",
        "How many seconds to allow when a background service is started or stopped.",
    ),
    (
        "tui.poll_ms",
        "1000",
        "How often the fmonero tui window refreshes what it shows, in milliseconds.",
    ),
    (
        "tui.tick_ms",
        "120",
        "How often the spinner in the fmonero tui window moves, in milliseconds.",
    ),
    (
        "tui.mouse",
        "true",
        "true lets you click tabs, rows, fields and buttons. false gives the terminal its own \
         text selection back.",
    ),
];

/// Case-insensitive Levenshtein distance, for the §4 "did you mean" redirect.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_ascii_lowercase().chars().collect();
    let b: Vec<char> = b.to_ascii_lowercase().chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn expand_tilde(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

fn parse_bool(key: &str, value: &str) -> Result<bool, Error> {
    match value {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        other => Err(
            Error::usage(format!("{key} takes true or false, not \"{other}\"."))
                .with_reason("bad_args")
                .with_hint(format!("fmonero settings set {key} true")),
        ),
    }
}

fn parse_num<T>(key: &str, value: &str, lo: T, hi: T) -> Result<T, Error>
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy,
{
    let bad = || {
        // The hint offers the key's DOCUMENTED DEFAULT from `KEY_TABLE`, not the range's low
        // bound: for `wallet.priority` the low bound is 0, which §1b.4 spends a paragraph
        // explaining is the cheapest tier and an alias of 1 — so hinting it nudged the user
        // away from the documented default of 2 (normal) every time they mistyped a priority.
        // Phase 3's ergonomics pass; the fallback stays `lo` for a key not in the table.
        let suggested = KEY_TABLE
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, default, _)| (*default).to_string())
            .unwrap_or_else(|| lo.to_string());
        Error::usage(format!(
            "{key} takes a number from {lo} to {hi}, not \"{value}\"."
        ))
        .with_reason("bad_args")
        .with_hint(format!("fmonero settings set {key} {suggested}"))
    };
    let n: T = value.parse().map_err(|_| bad())?;
    if n < lo || n > hi {
        return Err(bad());
    }
    Ok(n)
}

fn parse_unit(key: &str, value: &str) -> Result<String, Error> {
    if value.is_empty() || value.contains('/') || !value.ends_with(".service") {
        return Err(Error::usage(format!(
            "{key} takes a service file name ending in .service, not \"{value}\"."
        ))
        .with_reason("bad_args")
        .with_hint(format!("fmonero settings set {key} {NODE_UNIT}")));
    }
    Ok(value.to_string())
}

/// The same refusal with **`detail: "no_port"`** attached (v5.6).
///
/// `nodes::parse` counts the portless rows of monero.fail's list separately, because §2n.1 quotes
/// the figure (37 of 195) and a reader comparing 158 to 195 needs it — and until v5.6 it told them
/// apart by `e.to_string().contains("the port is required")`, i.e. by the prose, which §4 forbids an
/// agent from doing and which the §2o rewrite of that very sentence duly broke. `detail` is the
/// machine half of an error (§2o), so it is what the counter now branches on: the words are free to
/// change and the count cannot drift again.
fn no_port(msg: &str) -> Error {
    bad_remote(msg).with_detail("no_port")
}

/// Every §2m.2 refusal carries this hint, verbatim.
const REMOTE_HINT: &str =
    "fmonero node remote <host>:18089 --yes   # host:port, no path, no credential";

fn bad_remote(msg: &str) -> Error {
    Error::usage(msg.to_string())
        .with_reason("bad_remote_address")
        .with_hint(REMOTE_HINT)
}

/// One DNS label: 1..63 of `[A-Za-z0-9-]`, not `-`-edged (§2m.2).
fn is_dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

/// The host half of a §2m.2 address, with its brackets if it had any.
///
/// `"http://node:18089"` → `"node"`, `"[::1]:18089"` → `"[::1]"`. Used by
/// [`crate::units::ssl_for_address`] and the doctor's `remote_node_untrusted`, which both ask a
/// question about the host and nothing else.
pub fn remote_host(address: &str) -> &str {
    let rest = match address.find("://") {
        Some(i) => &address[i + 3..],
        None => address,
    };
    if rest.starts_with('[') {
        return match rest.find(']') {
            Some(close) => &rest[..=close],
            None => rest,
        };
    }
    match rest.rsplit_once(':') {
        Some((host, _)) => host,
        None => rest,
    }
}

/// `host:port` of a §2m.2 address — the form `--daemon-address` and `set_daemon` want when the
/// scheme is dropped. Kept verbatim otherwise.
pub fn remote_host_port(address: &str) -> &str {
    match address.find("://") {
        Some(i) => &address[i + 3..],
        None => address,
    }
}

/// Whether a §2m.2 host is a **literal** loopback address — `127.0.0.0/8` or `::1` (§2c's own
/// rule, applied to an address fmonero never dials itself). A DNS name is never loopback here:
/// deciding that means resolving it, and [`crate::units::ssl_for_address`] is called while
/// rendering a unit file.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.strip_prefix('[').unwrap_or(host);
    let host = host.strip_suffix(']').unwrap_or(host);
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_loopback(),
        Ok(std::net::IpAddr::V6(v6)) => v6.is_loopback(),
        Err(_) => false,
    }
}

/// Whether a §2m.2 host is a **literal** address inside one of the private ranges §2m.6 lists:
/// `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `100.64.0.0/10` (CGNAT —
/// where Tailscale lives), `169.254.0.0/16`, `::1`, `fc00::/7` or `fe80::/10`.
///
/// **A DNS name is never treated as private**, because classifying one means resolving it — a
/// network call inside the doctor's pure `fn detect(&Probes) -> Verdict` (§7's axiom), whose
/// answer a name server can change between runs. So a private node reached by name warns, and
/// the warning says to use its address if that is wrong (§7.1's `remote_node_untrusted`).
pub fn is_private_host(host: &str) -> bool {
    let host = host.strip_prefix('[').unwrap_or(host);
    let host = host.strip_suffix(']').unwrap_or(host);
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                // 100.64.0.0/10, CGNAT: `Ipv4Addr::is_shared` is unstable, so it is spelled out.
                || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            let seg = v6.segments();
            v6.is_loopback()
                // fc00::/7, unique local.
                || (seg[0] & 0xfe00) == 0xfc00
                // fe80::/10, link local.
                || (seg[0] & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

fn parse_dir(key: &str, value: &str) -> Result<PathBuf, Error> {
    let p = PathBuf::from(value);
    if value.is_empty() || !(p.is_absolute() || value.starts_with('~')) {
        return Err(Error::usage(format!(
            "\"{value}\" is not a full path. {key} needs one that starts at the top of the \
             filesystem, or with a ~ for your home folder."
        ))
        .with_reason("bad_args")
        .with_hint(format!("fmonero settings set {key} $HOME/.bitmonero")));
    }
    Ok(p)
}

/// The defaults that are computed from this machine's paths rather than written in
/// [`Settings::KEYS`]: the wallet directory (§4's per-network default) and, since W21, the chain
/// folder and the node log, which follow `Paths::bitmonero_dir` and therefore the sandbox when
/// `FMONERO_STATE_DIR` is set.
///
/// **Only the defaults move.** A value written in `config.toml` is the authority, here as
/// everywhere: each branch fires only while the field still holds the table's own default.
/// Computing `wallet.dir` from `$HOME` instead of from `paths.wallet_dir` was a second,
/// independent statement of one default — the two agree only while `XDG_DATA_HOME` is
/// `~/.local/share`, and `setup --units` derives its own `want_dir` from `paths.wallet_dir`
/// (§4.2), so anywhere else the two disagreed and `setup --units` planned a settings change on a
/// machine that needed none.
fn derive_machine_defaults(s: &mut Settings, paths: &Paths) {
    if s.wallet.dir == WalletSettings::default().dir {
        s.wallet.dir = paths.wallet_dir.join(s.node.network.as_str());
    }
    if s.node.data_dir == NodeSettings::default().data_dir {
        s.node.data_dir = paths.bitmonero_dir.clone();
    }
    if s.node.log_file == NodeSettings::default().log_file {
        s.node.log_file = paths.bitmonero_dir.join("bitmonero.log");
    }
}

impl Settings {
    /// `(key, default, one-line doc)` — the §4 table (§5b).
    pub const KEYS: &'static [(&'static str, &'static str, &'static str)] = KEY_TABLE;

    /// §2m.2's grammar, in **one** place: `[http[s]://]host:port`, port mandatory, no path and
    /// no userinfo. Returns the normalised address — scheme lower-cased and kept, host and port
    /// unchanged — or `Error::Usage{reason:"bad_remote_address"}` with the row's exact message
    /// and the one hint §2m.2 prescribes.
    ///
    /// Called by `Settings::set("node.remote", …)`, by `node_mode::set_remote` and by
    /// `wallet set-daemon`, so the three cannot accept different strings (§5b).
    ///
    /// Why each refusal, from `set_server`'s own behaviour (§1b.9 item 1):
    /// * **the port is mandatory** — a missing one is filled with 80 or 443, never 18081
    ///   (`abstract_http_client.cpp:139-144`), so guessing here would make fmonero's rendering
    ///   disagree with what `monero-wallet-rpc` does with the identical string;
    /// * **a path is refused** — `parse_url` puts it in `content.uri` and `set_server` never
    ///   reads that field, so accepting one would silently do nothing (§2j);
    /// * **userinfo is refused** — §0 cuts a remote-node credential;
    /// * **a foreign scheme is refused** — with the port mandatory the scheme has no effect at
    ///   all, so `ftp://` would be silently discarded rather than honoured.
    pub fn parse_remote_address(s: &str) -> Result<String, Error> {
        let s = s.trim();
        if s.is_empty() {
            return Err(bad_remote(
                "Type the node's address, like node.example.com:18089.",
            ));
        }
        let (scheme, rest) = match s.find("://") {
            Some(i) => {
                let scheme = s[..i].to_ascii_lowercase();
                if scheme != "http" && scheme != "https" {
                    return Err(bad_remote("Only http:// and https:// are understood."));
                }
                (Some(scheme), &s[i + 3..])
            }
            None => (None, s),
        };
        if rest.contains('@') {
            return Err(bad_remote(
                "An address cannot carry a name or a password. Use host:port, like \
                 node.example.com:18089.",
            ));
        }
        if rest.contains(['/', '?', '#']) {
            // §2o, and V11 is why it matters twice over: this message is what the TUI's
            // `a` modal shows under the field as the user types, so it has to read as advice
            // rather than as an account of what the wallet service does with a path.
            return Err(bad_remote(
                "An address is a host and a port, with nothing after it. Like \
                 node.example.com:18089.",
            ));
        }
        let ipv6 = || bad_remote("An IPv6 address needs brackets, like [2001:db8::1]:18089.");
        let (host, port) = if let Some(open) = rest.strip_prefix('[') {
            let close = open.find(']').ok_or_else(ipv6)?;
            let literal = &open[..close];
            if literal.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(ipv6());
            }
            let after = &open[close + 1..];
            let port = after
                .strip_prefix(':')
                .ok_or_else(|| no_port("The port is required, like [2001:db8::1]:18089."))?;
            (format!("[{literal}]"), port)
        } else {
            let (host, port) = rest.rsplit_once(':').ok_or_else(|| {
                // Never guessed: §1b.9 item 1 — the wallet service fills a missing port with 80
                // or 443 and never 18081, so a guess here would disagree with what it does with
                // the identical string. The user sees advice; the reason lives in this comment.
                no_port("The port is required, like node.example.com:18089.")
            })?;
            if host.contains(':') {
                return Err(ipv6());
            }
            if host.is_empty() || !host.split('.').all(is_dns_label) {
                return Err(bad_remote(
                    "The host has to be an address like 185.220.101.63, a bracketed one like \
                     [2001:db8::1], or a name like node.example.com.",
                ));
            }
            (host.to_string(), port)
        };
        let bad_port = || bad_remote("The port has to be a number from 1 to 65535.");
        if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad_port());
        }
        let n: u32 = port.parse().map_err(|_| bad_port())?;
        if n == 0 || n > u32::from(u16::MAX) {
            return Err(bad_port());
        }
        Ok(match scheme {
            Some(scheme) => format!("{scheme}://{host}:{port}"),
            None => format!("{host}:{port}"),
        })
    }

    /// Read `config.toml`, or the defaults when it does not exist, then expand a leading
    /// `~` in every path **once** (§4.2) and derive the network's wallet directory when
    /// the file did not name one.
    pub fn load(paths: &Paths) -> Result<Settings, Error> {
        let path = paths.config_file();
        let mut s: Settings = match std::fs::read_to_string(&path) {
            // S6: the toml crate's own message named a line and a column, in the slot a
            // person reads. The sentence says which file and what to do; `detail` keeps the rest.
            Ok(text) => toml::from_str(&text).map_err(|e| {
                Error::usage(format!(
                    "fmonero could not make sense of your settings file {}.",
                    path.display()
                ))
                .with_detail(e.to_string())
                .with_hint("fmonero settings list   # shows the effective values")
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            // S6.
            Err(e) => {
                return Err(Error::io("read", "your settings file", &path, &e)
                    .with_hint("fmonero doctor --only storage"))
            }
        };
        // The three defaults this machine computes rather than the table stating them: the
        // per-network wallet directory (§4, §5b's "default root of the per-network wallet
        // directories"), the chain folder and the node log. One function, shared with
        // `default_here`, so `settings list` can tell a value the user changed from a default
        // that is a pattern rather than a value.
        derive_machine_defaults(&mut s, paths);
        s.node.data_dir = expand_tilde(&s.node.data_dir, &paths.home);
        s.node.log_file = expand_tilde(&s.node.log_file, &paths.home);
        s.wallet.dir = expand_tilde(&s.wallet.dir, &paths.home);
        Ok(s)
    }

    /// One key, in `KEYS` spelling.
    pub fn get(&self, key: &str) -> Result<String, Error> {
        Ok(match key {
            "node.network" => self.node.network.to_string(),
            "node.mode" => self.node.mode.to_string(),
            "node.remote" => self.node.remote.clone(),
            "node.remote_trusted" => self.node.remote_trusted.to_string(),
            "node.url" => self.node.url.clone(),
            "node.unit" => self.node.unit.clone(),
            "node.data_dir" => self.node.data_dir.display().to_string(),
            "node.log_file" => self.node.log_file.display().to_string(),
            "node.prune" => self.node.prune.to_string(),
            "node.remote_probe_sec" => self.node.remote_probe_sec.to_string(),
            "wallet.url" => self.wallet.url.clone(),
            "wallet.unit" => self.wallet.unit.clone(),
            "wallet.dir" => self.wallet.dir.display().to_string(),
            "wallet.port" => self.wallet.port.to_string(),
            "wallet.account" => self.wallet.account.to_string(),
            "wallet.priority" => self.wallet.priority.to_string(),
            "wallet.autostart" => self.wallet.autostart.to_string(),
            "timeouts.node_sec" => self.timeouts.node_sec.to_string(),
            "timeouts.wallet_sec" => self.timeouts.wallet_sec.to_string(),
            "timeouts.refresh_sec" => self.timeouts.refresh_sec.to_string(),
            "timeouts.transfer_sec" => self.timeouts.transfer_sec.to_string(),
            "timeouts.open_sec" => self.timeouts.open_sec.to_string(),
            "timeouts.systemctl_sec" => self.timeouts.systemctl_sec.to_string(),
            "tui.poll_ms" => self.tui.poll_ms.to_string(),
            "tui.tick_ms" => self.tui.tick_ms.to_string(),
            "tui.mouse" => self.tui.mouse.to_string(),
            other => return Err(unknown_key(other, false)),
        })
    }

    /// The documented default of one key, as `settings list` prints it.
    /// The default a key resolves to **on this machine** (`settings list`).
    ///
    /// Three of the defaults in [`Settings::KEYS`] are patterns rather than values —
    /// `~/.bitmonero`, `~/.bitmonero/bitmonero.log` and
    /// `~/.local/share/fmonero/wallets/<network>` — so comparing a loaded value against the table
    /// marked all three as "changed from the default" on a machine where nobody had changed
    /// anything (self-read, round 3: `settings list` printed `*` against `node.data_dir`). This
    /// resolves them through [`derive_machine_defaults`], the same function `load` uses.
    pub fn default_here(key: &str, paths: &Paths, network: Network) -> Option<String> {
        let mut s = Settings {
            node: NodeSettings {
                network,
                ..NodeSettings::default()
            },
            ..Settings::default()
        };
        derive_machine_defaults(&mut s, paths);
        s.node.data_dir = expand_tilde(&s.node.data_dir, &paths.home);
        s.node.log_file = expand_tilde(&s.node.log_file, &paths.home);
        s.wallet.dir = expand_tilde(&s.wallet.dir, &paths.home);
        s.get(key).ok()
    }

    pub fn default_of(key: &str) -> Option<&'static str> {
        Self::KEYS
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, d, _)| *d)
    }

    /// Set one key from its string form, validating the value. An unknown key names the
    /// nearest key from `KEYS` and makes its hint that exact runnable command (§4).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), Error> {
        match key {
            "node.network" => self.node.network = value.parse()?,
            // §2m.1's invariant, enforced in both directions so that no consumer needs an
            // "empty address in remote mode" branch — and the honest behaviour of that branch
            // (refuse everything) would be a fourth layout nobody specified.
            "node.mode" => {
                let mode: NodeMode = value.parse()?;
                if mode.is_remote() && self.node.remote.trim().is_empty() {
                    return Err(Error::usage(
                        "Set node.remote to an address before you switch node.mode to remote.",
                    )
                    .with_reason("remote_unset")
                    .with_hint("fmonero node remote <host>:18089 --yes"));
                }
                self.node.mode = mode;
            }
            "node.remote" => {
                if value.trim().is_empty() {
                    // `&& !is_empty()` so that clearing an ALREADY-empty address is a no-op
                    // rather than a refusal: on a hand-edited file carrying `mode = "remote"`
                    // with no address the pair is already broken, `invalid_keys` reports it
                    // under `node.mode` — whose default is the repair — and a second row for a
                    // key whose "fix" is the value it already holds would be a fixer that
                    // cannot succeed (§7.1).
                    if self.node.mode.is_remote() && !self.node.remote.is_empty() {
                        return Err(Error::usage(
                            "node.remote cannot be cleared while node.mode is remote.",
                        )
                        .with_reason("remote_unset")
                        .with_hint("fmonero node local --yes   # then clear node.remote"));
                    }
                    self.node.remote = String::new();
                } else {
                    self.node.remote = Settings::parse_remote_address(value)?;
                }
            }
            "node.remote_trusted" => self.node.remote_trusted = parse_bool(key, value)?,
            "node.url" => {
                crate::rpc::require_loopback(value)?;
                self.node.url = value.to_string();
            }
            "node.unit" => self.node.unit = parse_unit(key, value)?,
            "node.data_dir" => self.node.data_dir = parse_dir(key, value)?,
            "node.log_file" => self.node.log_file = parse_dir(key, value)?,
            "node.prune" => self.node.prune = parse_bool(key, value)?,
            // Values 1..9 are refused rather than clamped: a five-second probe of a stranger's
            // node is a request pattern the stranger can see, and §2n.5's rule is that fmonero
            // contacts a node when the user is doing something, not on a heartbeat.
            "node.remote_probe_sec" => {
                let n: u32 = parse_num(key, value, 0u32, 86_400u32)?;
                if n > 0 && n < 10 {
                    return Err(Error::usage(format!(
                        "{key} is a number of seconds. Use 0 to turn the checks off, or 10 or \
                         more."
                    ))
                    .with_reason("bad_args")
                    .with_hint(format!("fmonero settings set {key} 60")));
                }
                self.node.remote_probe_sec = n;
            }
            "wallet.url" => {
                crate::rpc::require_loopback(value)?;
                self.wallet.url = value.to_string();
            }
            "wallet.unit" => self.wallet.unit = parse_unit(key, value)?,
            "wallet.dir" => self.wallet.dir = parse_dir(key, value)?,
            "wallet.port" => self.wallet.port = parse_num(key, value, 1u16, u16::MAX)?,
            "wallet.account" => self.wallet.account = parse_num(key, value, 0u32, 1_000_000u32)?,
            "wallet.priority" => self.wallet.priority = parse_num(key, value, 0u32, 4u32)?,
            "wallet.autostart" => self.wallet.autostart = parse_bool(key, value)?,
            "timeouts.node_sec" => self.timeouts.node_sec = parse_num(key, value, 1u64, 3600u64)?,
            "timeouts.wallet_sec" => {
                self.timeouts.wallet_sec = parse_num(key, value, 1u64, 3600u64)?
            }
            "timeouts.refresh_sec" => {
                self.timeouts.refresh_sec = parse_num(key, value, 1u64, 86400u64)?
            }
            "timeouts.transfer_sec" => {
                self.timeouts.transfer_sec = parse_num(key, value, 1u64, 86400u64)?
            }
            "timeouts.open_sec" => self.timeouts.open_sec = parse_num(key, value, 1u64, 86400u64)?,
            "timeouts.systemctl_sec" => {
                self.timeouts.systemctl_sec = parse_num(key, value, 1u64, 600u64)?
            }
            "tui.poll_ms" => self.tui.poll_ms = parse_num(key, value, 100u64, 60_000u64)?,
            "tui.tick_ms" => self.tui.tick_ms = parse_num(key, value, 20u64, 5_000u64)?,
            "tui.mouse" => self.tui.mouse = parse_bool(key, value)?,
            other => return Err(unknown_key(other, true)),
        }
        Ok(())
    }

    /// `config.toml` as bytes. Exposed because no doctor fixer may call
    /// [`Settings::save`] — a fixer that writes the file itself bypasses the backup, the
    /// sha256 pair and `actions.jsonl`, and therefore `doctor undo` (§7.1).
    pub fn to_toml_bytes(&self) -> Result<Vec<u8>, Error> {
        toml::to_string_pretty(self)
            .map(String::into_bytes)
            .map_err(|e| {
                Error::failure("fmonero could not write your settings out.")
                    .with_detail(e.to_string())
            })
    }

    /// Write `config.toml` at mode 0600 through tmp + `rename(2)` (§2b.7).
    pub fn save(&self, ctx: &crate::context::Context) -> Result<(), Error> {
        ctx.paths.ensure_dirs()?;
        crate::write_private(&ctx.paths.config_file(), &self.to_toml_bytes()?)
    }

    /// `(key, why)` for every key that is present but invalid — the doctor's
    /// `settings_valid` (§7.1). `Ok` means nothing is wrong.
    pub fn invalid_keys(&self) -> Vec<(String, String)> {
        let mut bad = Vec::new();
        let mut probe = self.clone();
        for (key, _, _) in Self::KEYS {
            let Ok(value) = self.get(key) else { continue };
            if let Err(e) = probe.set(key, &value) {
                bad.push(((*key).to_string(), e.to_string()));
            }
        }
        // §2m.1's invariant needs no clause of its own here: the loop above re-`set`s every key
        // it read, so a hand-edited `mode = "remote"` with an empty `node.remote` is refused by
        // `set` and lands in `bad` under `node.mode` — whose documented default, `local`, is
        // also the repair.
        if self.wallet.port != 0 {
            let expected = self.node.network.wallet_rpc_port();
            if self.wallet.port != expected {
                bad.push((
                    "wallet.port".to_string(),
                    format!(
                        // §2o: the doctor prints this one, so it says which value is wrong,
                        // which one is right, and that its own fixer repairs it — never a flag.
                        // M20: backticks print literally in a terminal.
                        "{} is not the port {} wallets use ({expected}). \
                         fmonero doctor --fix puts it back.",
                        self.wallet.port, self.node.network
                    ),
                ));
            }
        }
        bad
    }
}

/// The §4 redirect: name the nearest key and make the hint that exact runnable command.
///
/// Nearest is Levenshtein distance ≤ 3, **or** a key that the typed key is a prefix of
/// (case-insensitively). The prefix rule is not in §4's sentence but is what makes §4's own
/// three examples work: `wallet.priority` is one edit from `wallet.prioriti`, but
/// `timeouts.node` → `timeouts.node_sec` is **four** edits and `node.log` → `node.log_file`
/// is **five**, so a pure ≤ 3 rule would answer "did you mean?" to one of the three keys §4
/// names as the ones an agent half-remembers. The deviation is recorded in DECISIONS.md.
fn unknown_key(key: &str, settable: bool) -> Error {
    let typed = key.to_ascii_lowercase();
    let nearest = Settings::KEYS
        .iter()
        .map(|(k, d, _)| {
            let prefix = !typed.is_empty() && k.to_ascii_lowercase().starts_with(&typed);
            // A key the typed one is a prefix of wins over a merely similar one: `node.log`
            // is three edits from `node.url` and five from `node.log_file`, and the second is
            // obviously what was meant.
            (usize::from(!prefix), distance(key, k), *k, *d)
        })
        .filter(|(prefix, d, _, _)| *prefix == 0 || *d <= 3)
        .min_by_key(|(prefix, d, _, _)| (*prefix, *d))
        .map(|(_, d, k, default)| (d, k, default));
    let hint = match (nearest, settable) {
        (Some((_, k, default)), true) => {
            format!("fmonero settings set {k} {default}   # did you mean this?")
        }
        (Some((_, k, _)), false) => format!("fmonero settings get {k}   # did you mean this?"),
        (None, _) => "fmonero settings list".to_string(),
    };
    let msg = format!("unknown settings key '{key}'");
    // `settings set` on an unknown key is a usage error; `settings get` is a lookup
    // failure, which §5b spells `NotFound`.
    let base = if settable {
        Error::usage(msg)
    } else {
        Error::not_found(msg)
    };
    base.with_reason("unknown_key").with_hint(hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_table_is_the_twenty_six_keys_of_the_design() {
        // Twenty before v5.2; §2m.1's three remote-mode keys made it twenty-three; v5.5 adds
        // node.remote_probe_sec, wallet.autostart and tui.mouse (§4, §5b.4 row 5).
        assert_eq!(Settings::KEYS.len(), 26);
        let s = Settings::default();
        for (key, _, doc) in Settings::KEYS {
            assert!(s.get(key).is_ok(), "{key} must be readable");
            assert!(!doc.is_empty(), "{key} needs a one-line doc");
        }
        // Every readable key is also settable, with its own value.
        let mut s2 = s.clone();
        for (key, _, _) in Settings::KEYS {
            let value = s.get(key).unwrap();
            s2.set(key, &value)
                .unwrap_or_else(|e| panic!("{key} = '{value}' must round trip: {e}"));
        }
        assert_eq!(s2, s);
    }

    /// §2m.2's grammar, one case per row of the refusal table, plus what it accepts.
    #[test]
    fn the_remote_address_grammar_is_the_seven_refusals_of_the_design() {
        for (input, want) in [
            ("node.example.com:18089", "node.example.com:18089"),
            (
                "http://node.example.com:18089",
                "http://node.example.com:18089",
            ),
            // The scheme is stored lower-cased and kept; host and port are untouched (§2m.2).
            (
                "HTTPS://Node.Example.COM:18089",
                "https://Node.Example.COM:18089",
            ),
            ("127.0.0.1:18081", "127.0.0.1:18081"),
            ("[2001:db8::1]:18089", "[2001:db8::1]:18089"),
            ("[::1]:18081", "[::1]:18081"),
            ("  node:18089  ", "node:18089"),
        ] {
            assert_eq!(
                Settings::parse_remote_address(input).unwrap(),
                want,
                "{input}"
            );
        }
        // v5.6, §2o: every refusal was rewritten as advice (rule 10: say what a good one looks
        // like), because the TUI's `a` modal shows these under the field as the user types
        // (§7a.4, DECISIONS V11). The needles follow the new words; the exit code, the reason and
        // the hint are unchanged, which is the half an agent branches on.
        for (input, needle) in [
            ("", "Type the node's address"),
            ("node.example.com", "The port is required"),
            ("http://node.example.com:18089/json_rpc", "nothing after it"),
            ("user:pass@node:18089", "cannot carry a name or a password"),
            ("ftp://node:18089", "Only http:// and https://"),
            ("tcp://node:18089", "Only http:// and https://"),
            ("node.example.com:0", "a number from 1 to 65535"),
            ("node.example.com:70000", "a number from 1 to 65535"),
            ("[::1:18089", "needs brackets"),
            ("::1:18089", "needs brackets"),
            ("-bad-.example.com:18089", "The host has to be"),
        ] {
            let e = Settings::parse_remote_address(input).unwrap_err();
            assert_eq!(e.exit_code(), 2, "{input}");
            assert_eq!(e.reason(), Some("bad_remote_address"), "{input}");
            assert!(e.to_string().contains(needle), "{input}: {e}");
            assert_eq!(e.hint(), REMOTE_HINT, "{input}");
        }
    }

    /// §2m.1's invariant, in both directions. Before v5.2 neither key existed, so the mode was
    /// unreachable through the CLI at all (§2i).
    #[test]
    fn the_mode_and_the_address_hold_each_other_up() {
        let mut s = Settings::default();
        let e = s.set("node.mode", "remote").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert_eq!(e.reason(), Some("remote_unset"));
        assert!(
            e.to_string().contains("Set node.remote to an address"),
            "{e}"
        );
        assert_eq!(e.hint(), "fmonero node remote <host>:18089 --yes");
        assert_eq!(s.node.mode, NodeMode::Local, "nothing was written");

        s.set("node.remote", "node.example.com:18089").unwrap();
        s.set("node.mode", "remote").unwrap();
        assert_eq!(s.node.mode, NodeMode::Remote);

        let e = s.set("node.remote", "").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert_eq!(e.reason(), Some("remote_unset"));
        assert!(
            e.to_string().contains("cannot be cleared while node.mode"),
            "{e}"
        );
        assert_eq!(
            e.hint(),
            "fmonero node local --yes   # then clear node.remote"
        );
        assert_eq!(s.node.remote, "node.example.com:18089");

        // Back to local, and only then may the address be cleared.
        s.set("node.mode", "local").unwrap();
        s.set("node.remote", "").unwrap();
        assert_eq!(s.node.remote, "");

        // A hand-edited file that breaks the invariant is what `invalid_keys` is for (§7.1).
        let mut broken = Settings::default();
        broken.node.mode = NodeMode::Remote;
        let bad = broken.invalid_keys();
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert_eq!(bad[0].0, "node.mode");
        assert_eq!(Settings::default_of("node.mode"), Some("local"));

        // `node.remote_trusted` is accepted for any host: a node the owner runs on a LAN is a
        // legitimate case no local check can tell from a public one (§4, §7.1 warns instead).
        let mut s = Settings::default();
        s.set("node.remote", "node.example.com:18089").unwrap();
        s.set("node.remote_trusted", "true").unwrap();
        assert!(s.node.remote_trusted);
        assert_eq!(
            s.set("node.remote_trusted", "maybe")
                .unwrap_err()
                .exit_code(),
            2
        );
    }

    #[test]
    fn a_remote_address_is_split_into_its_host_and_its_host_port() {
        for (addr, host, host_port) in [
            (
                "node.example.com:18089",
                "node.example.com",
                "node.example.com:18089",
            ),
            ("http://node:18089", "node", "node:18089"),
            ("https://1.2.3.4:18089", "1.2.3.4", "1.2.3.4:18089"),
            (
                "[2001:db8::1]:18089",
                "[2001:db8::1]",
                "[2001:db8::1]:18089",
            ),
            ("http://[::1]:18081", "[::1]", "[::1]:18081"),
        ] {
            assert_eq!(remote_host(addr), host, "{addr}");
            assert_eq!(remote_host_port(addr), host_port, "{addr}");
        }
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.9.9.9"));
        assert!(is_loopback_host("[::1]"));
        assert!(!is_loopback_host("node.example.com"));
        assert!(!is_loopback_host("10.0.0.5"));
        // A DNS name is never loopback here: deciding that means resolving it (§2m.6).
        assert!(!is_loopback_host("localhost"));
    }

    #[test]
    fn the_defaults_are_the_ones_the_design_names() {
        let s = Settings::default();
        assert_eq!(s.node.network, Network::Mainnet);
        assert_eq!(s.node.mode, NodeMode::Local, "local by default (§2m.1)");
        assert_eq!(s.node.remote, "");
        assert!(!s.node.remote_trusted, "a public node is never trusted");
        assert_eq!(s.node.url, "http://127.0.0.1:18081");
        assert_eq!(s.wallet.url, "http://127.0.0.1:18083");
        assert_eq!(s.wallet.port, 18083);
        assert_eq!(s.wallet.priority, 2, "2 = normal, never 0 (§1b.4)");
        assert_eq!(s.wallet.account, 0);
        assert!(s.node.prune);
        assert_eq!(
            (
                s.timeouts.node_sec,
                s.timeouts.wallet_sec,
                s.timeouts.refresh_sec,
                s.timeouts.transfer_sec,
                s.timeouts.open_sec,
                s.timeouts.systemctl_sec
            ),
            (3, 5, 120, 120, 300, 10)
        );
        assert_eq!((s.tui.poll_ms, s.tui.tick_ms), (1000, 120));
        assert_eq!(s.node.unit, "fmonero-monerod.service");
        assert_eq!(s.wallet.unit, "fmonero-wallet-rpc.service");
    }

    #[test]
    fn an_unknown_key_is_redirected_with_a_runnable_hint() {
        let mut s = Settings::default();
        let e = s.set("wallet.prioriti", "2").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert_eq!(e.reason(), Some("unknown_key"));
        assert_eq!(
            e.hint(),
            "fmonero settings set wallet.priority 2   # did you mean this?"
        );

        // §4's own examples: these are two and five edits away, so the prefix rule is what
        // makes them land on a real key instead of on `settings list`.
        let e = s.set("timeouts.node", "3").unwrap_err();
        assert_eq!(e.reason(), Some("unknown_key"));
        assert!(e.hint().contains("timeouts.node_sec"), "{}", e.hint());
        let e = s.set("node.log", "/tmp/x.log").unwrap_err();
        assert!(e.hint().contains("node.log_file"), "{}", e.hint());

        // Nothing near: fall back to the list.
        let e = s.set("nope", "1").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert_eq!(e.reason(), Some("unknown_key"));
        assert_eq!(e.hint(), "fmonero settings list");

        // `get` on an unknown key is a lookup failure (§5b).
        let e = s.get("nope").unwrap_err();
        assert_eq!(e.exit_code(), 6);
        assert_eq!(e.reason(), Some("unknown_key"));
    }

    /// Phase 3's ergonomics pass: an out-of-range number is hinted with the key's DOCUMENTED
    /// DEFAULT, never with the range's low bound. For `wallet.priority` the low bound is 0 —
    /// the cheapest tier and an alias of 1 (§1b.4) — so the old hint taught the opposite of
    /// §4's `wallet.priority` default of 2 every time somebody mistyped a priority.
    #[test]
    fn an_out_of_range_number_is_hinted_with_the_documented_default() {
        let mut s = Settings::default();
        for (key, bad, want) in [
            (
                "wallet.priority",
                "9",
                "fmonero settings set wallet.priority 2",
            ),
            (
                "timeouts.node_sec",
                "0",
                "fmonero settings set timeouts.node_sec 3",
            ),
            ("tui.poll_ms", "5", "fmonero settings set tui.poll_ms 1000"),
            (
                "wallet.port",
                "99999999",
                "fmonero settings set wallet.port 18083",
            ),
        ] {
            let e = s.set(key, bad).unwrap_err();
            assert_eq!(e.exit_code(), 2, "{key}");
            assert_eq!(e.hint(), want, "{key}");
            // And the hint is itself accepted, which is what makes it runnable (§4).
            let suggested = want.rsplit(' ').next().unwrap();
            assert!(s.set(key, suggested).is_ok(), "{key} {suggested}");
        }
    }

    #[test]
    fn bad_values_are_refused_and_urls_must_be_loopback() {
        let mut s = Settings::default();
        assert_eq!(s.set("wallet.priority", "5").unwrap_err().exit_code(), 2);
        assert_eq!(s.set("node.network", "regtest").unwrap_err().exit_code(), 2);
        assert_eq!(s.set("timeouts.node_sec", "0").unwrap_err().exit_code(), 2);
        assert_eq!(s.set("node.prune", "maybe").unwrap_err().exit_code(), 2);
        assert_eq!(s.set("node.unit", "monerod").unwrap_err().exit_code(), 2);
        assert_eq!(s.set("wallet.dir", "wallets").unwrap_err().exit_code(), 2);
        for (key, url) in [
            ("node.url", "https://127.0.0.1:18081"),
            ("node.url", "http://10.0.0.5:18081"),
            ("wallet.url", "http://10.0.0.5:18083"),
            ("wallet.url", "http://example.com:18083"),
        ] {
            let e = s.set(key, url).unwrap_err();
            assert_eq!(e.exit_code(), 2, "{key} {url}");
            assert_eq!(e.reason(), Some("non_loopback_url"), "{key} {url}");
        }
        assert!(s.set("node.url", "http://127.0.0.1:38081").is_ok());
        assert!(s.set("wallet.url", "http://localhost:18083").is_ok());
    }

    #[test]
    fn settings_round_trip_through_toml_and_reject_unknown_fields() {
        let mut s = Settings::default();
        s.set("node.network", "stagenet").unwrap();
        s.set("timeouts.wallet_sec", "9").unwrap();
        let bytes = s.to_toml_bytes().unwrap();
        let back: Settings = toml::from_str(std::str::from_utf8(&bytes).unwrap()).unwrap();
        assert_eq!(back, s);
        let e = toml::from_str::<Settings>("[node]\nnetwrk = \"mainnet\"\n");
        assert!(e.is_err(), "deny_unknown_fields keeps typos visible");
    }

    #[test]
    fn the_timeout_flag_lowers_and_never_raises() {
        let mut t = Timeouts::from_settings(&TimeoutSettings::default());
        assert_eq!(t.open, Duration::from_secs(300));
        assert_eq!(t.unit_probe, Duration::from_secs(30));
        assert_eq!(t.relay, Duration::from_secs(30));
        t.clamp_to(3);
        for d in [
            t.node,
            t.wallet,
            t.refresh,
            t.transfer,
            t.open,
            t.systemctl,
            t.unit_probe,
            t.relay,
        ] {
            assert!(d <= Duration::from_secs(3), "{d:?}");
        }
        let mut t = Timeouts::from_settings(&TimeoutSettings::default());
        t.clamp_to(9999);
        assert_eq!(t.node, Duration::from_secs(3), "a ceiling never raises");
    }

    #[test]
    fn invalid_keys_names_a_port_that_does_not_match_the_network() {
        let mut s = Settings::default();
        assert!(s.invalid_keys().is_empty());
        s.node.network = Network::Stagenet;
        let bad = s.invalid_keys();
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert_eq!(bad[0].0, "wallet.port");
        s.wallet.port = 38083;
        assert!(s.invalid_keys().is_empty());
    }

    #[test]
    fn the_seed_language_list_is_upstreams_twelve_and_takes_either_spelling() {
        assert_eq!(SEED_LANGUAGES.len(), 12);
        assert!(validate_seed_language("English").is_ok());
        assert!(validate_seed_language("Deutsch").is_ok());
        assert!(validate_seed_language("German").is_ok());
        assert!(validate_seed_language("日本語").is_ok());
        assert!(validate_seed_language("Japanese").is_ok());
        // Not in `get_language_list()`, so the daemon rejects it (§1b.4).
        let e = validate_seed_language("EnglishOld").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.to_string().contains("Esperanto"), "{e}");
        assert_eq!(
            validate_seed_language("english").unwrap_err().exit_code(),
            2
        );
    }

    #[test]
    fn the_network_table_is_the_ports_of_the_design() {
        for (net, p2p, rpc, wallet) in [
            (Network::Mainnet, 18080, 18081, 18083),
            (Network::Stagenet, 38080, 38081, 38083),
            (Network::Testnet, 28080, 28081, 28083),
        ] {
            assert_eq!(net.node_p2p_port(), p2p);
            assert_eq!(net.node_rpc_port(), rpc);
            assert_eq!(net.wallet_rpc_port(), wallet);
        }
        assert_eq!(serde_json::to_value(Network::Stagenet).unwrap(), "stagenet");
    }
}
