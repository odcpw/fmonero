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
        "'{language}' is not a Monero seed language; known: {}",
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
                "'{other}' is not a network; known: mainnet, stagenet, testnet"
            ))
            .with_reason("bad_args")
            .with_hint("fmonero settings set node.network mainnet")),
        }
    }
}

/// The unit names, which never change with the network (§4).
pub const NODE_UNIT: &str = "fmonero-monerod.service";
pub const WALLET_UNIT: &str = "fmonero-wallet-rpc.service";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NodeSettings {
    pub network: Network,
    pub url: String,
    pub unit: String,
    pub data_dir: PathBuf,
    pub log_file: PathBuf,
    pub prune: bool,
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
            url: "http://127.0.0.1:18081".to_string(),
            unit: NODE_UNIT.to_string(),
            data_dir: PathBuf::from("~/.bitmonero"),
            log_file: PathBuf::from("~/.bitmonero/bitmonero.log"),
            prune: true,
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
        }
    }
}

/// `(key, default, one-line doc)` — the §4 table, and the thing `settings list` prints.
/// The defaults are written as the user sees them: a leading `~` is expanded once, when
/// the settings load (§4.2).
const KEY_TABLE: &[(&str, &str, &str)] = &[
    (
        "node.network",
        "mainnet",
        "mainnet | stagenet | testnet — one at a time; changing it needs `setup --units` again",
    ),
    (
        "node.url",
        "http://127.0.0.1:18081",
        "the node's JSON-RPC URL; loopback http only (§2c)",
    ),
    (
        "node.unit",
        NODE_UNIT,
        "the systemd user unit that runs monerod",
    ),
    (
        "node.data_dir",
        "~/.bitmonero",
        "monerod's LMDB data directory",
    ),
    (
        "node.log_file",
        "~/.bitmonero/bitmonero.log",
        "the log `node log` tails",
    ),
    (
        "node.prune",
        "true",
        "write --prune-blockchain into monerod.conf (a full chain does not fit, §1.3)",
    ),
    (
        "wallet.url",
        "http://127.0.0.1:18083",
        "monero-wallet-rpc's URL; loopback http only (§2c)",
    ),
    (
        "wallet.unit",
        WALLET_UNIT,
        "the systemd user unit that runs monero-wallet-rpc",
    ),
    (
        "wallet.dir",
        "~/.local/share/fmonero/wallets/<network>",
        "the --wallet-dir the daemon resolves wallet names against",
    ),
    (
        "wallet.port",
        "18083",
        "the wallet RPC bind port; it names the daemon's login file (§2c)",
    ),
    ("wallet.account", "0", "the default account index"),
    (
        "wallet.priority",
        "2",
        "default fee priority 0-4 (2 = normal; 0 is an alias of the lowest tier, §1b.4)",
    ),
    ("timeouts.node_sec", "3", "node RPC probe deadline"),
    ("timeouts.wallet_sec", "5", "wallet RPC probe deadline"),
    ("timeouts.refresh_sec", "120", "`wallet refresh` deadline"),
    (
        "timeouts.transfer_sec",
        "120",
        "transfer/sweep build deadline",
    ),
    ("timeouts.open_sec", "300", "`open_wallet` deadline"),
    ("timeouts.systemctl_sec", "10", "any systemctl call"),
    ("tui.poll_ms", "1000", "the TUI's status poll interval"),
    ("tui.tick_ms", "120", "the TUI's spinner frame interval"),
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
            Error::usage(format!("'{other}' is not true/false for {key}"))
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
            "'{value}' is not a number in {lo}..={hi} for {key}"
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
        return Err(
            Error::usage(format!("'{value}' is not a systemd unit name for {key}"))
                .with_reason("bad_args")
                .with_hint(format!("fmonero settings set {key} {NODE_UNIT}")),
        );
    }
    Ok(value.to_string())
}

fn parse_dir(key: &str, value: &str) -> Result<PathBuf, Error> {
    let p = PathBuf::from(value);
    if value.is_empty() || !(p.is_absolute() || value.starts_with('~')) {
        return Err(Error::usage(format!(
            "'{value}' is not an absolute path for {key} (a leading ~ is allowed)"
        ))
        .with_reason("bad_args")
        .with_hint(format!("fmonero settings set {key} $HOME/.bitmonero")));
    }
    Ok(p)
}

impl Settings {
    /// `(key, default, one-line doc)` — the §4 table (§5b).
    pub const KEYS: &'static [(&'static str, &'static str, &'static str)] = KEY_TABLE;

    /// Read `config.toml`, or the defaults when it does not exist, then expand a leading
    /// `~` in every path **once** (§4.2) and derive the network's wallet directory when
    /// the file did not name one.
    pub fn load(paths: &Paths) -> Result<Settings, Error> {
        let path = paths.config_file();
        let mut s: Settings = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| {
                Error::usage(format!("{}: {e}", path.display()))
                    .with_hint("fmonero settings list   # shows the effective values")
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => {
                return Err(Error::failure(format!(
                    "cannot read {}: {e}",
                    path.display()
                )))
            }
        };
        // The default wallet directory carries the network segment (§4).
        if s.wallet.dir == WalletSettings::default().dir {
            s.wallet.dir =
                PathBuf::from(format!("~/.local/share/fmonero/wallets/{}", s.node.network));
        }
        s.node.data_dir = expand_tilde(&s.node.data_dir, &paths.home);
        s.node.log_file = expand_tilde(&s.node.log_file, &paths.home);
        s.wallet.dir = expand_tilde(&s.wallet.dir, &paths.home);
        Ok(s)
    }

    /// One key, in `KEYS` spelling.
    pub fn get(&self, key: &str) -> Result<String, Error> {
        Ok(match key {
            "node.network" => self.node.network.to_string(),
            "node.url" => self.node.url.clone(),
            "node.unit" => self.node.unit.clone(),
            "node.data_dir" => self.node.data_dir.display().to_string(),
            "node.log_file" => self.node.log_file.display().to_string(),
            "node.prune" => self.node.prune.to_string(),
            "wallet.url" => self.wallet.url.clone(),
            "wallet.unit" => self.wallet.unit.clone(),
            "wallet.dir" => self.wallet.dir.display().to_string(),
            "wallet.port" => self.wallet.port.to_string(),
            "wallet.account" => self.wallet.account.to_string(),
            "wallet.priority" => self.wallet.priority.to_string(),
            "timeouts.node_sec" => self.timeouts.node_sec.to_string(),
            "timeouts.wallet_sec" => self.timeouts.wallet_sec.to_string(),
            "timeouts.refresh_sec" => self.timeouts.refresh_sec.to_string(),
            "timeouts.transfer_sec" => self.timeouts.transfer_sec.to_string(),
            "timeouts.open_sec" => self.timeouts.open_sec.to_string(),
            "timeouts.systemctl_sec" => self.timeouts.systemctl_sec.to_string(),
            "tui.poll_ms" => self.tui.poll_ms.to_string(),
            "tui.tick_ms" => self.tui.tick_ms.to_string(),
            other => return Err(unknown_key(other, false)),
        })
    }

    /// The documented default of one key, as `settings list` prints it.
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
            "node.url" => {
                crate::rpc::require_loopback(value)?;
                self.node.url = value.to_string();
            }
            "node.unit" => self.node.unit = parse_unit(key, value)?,
            "node.data_dir" => self.node.data_dir = parse_dir(key, value)?,
            "node.log_file" => self.node.log_file = parse_dir(key, value)?,
            "node.prune" => self.node.prune = parse_bool(key, value)?,
            "wallet.url" => {
                crate::rpc::require_loopback(value)?;
                self.wallet.url = value.to_string();
            }
            "wallet.unit" => self.wallet.unit = parse_unit(key, value)?,
            "wallet.dir" => self.wallet.dir = parse_dir(key, value)?,
            "wallet.port" => self.wallet.port = parse_num(key, value, 1u16, u16::MAX)?,
            "wallet.account" => self.wallet.account = parse_num(key, value, 0u32, 1_000_000u32)?,
            "wallet.priority" => self.wallet.priority = parse_num(key, value, 0u32, 4u32)?,
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
            .map_err(|e| Error::failure(format!("cannot serialise settings: {e}")))
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
        if self.wallet.port != 0 {
            let expected = self.node.network.wallet_rpc_port();
            if self.wallet.port != expected {
                bad.push((
                    "wallet.port".to_string(),
                    format!(
                        "{} is not the {} wallet port ({expected}); `setup --units` rewrites it",
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
    fn the_key_table_is_the_twenty_keys_of_the_design() {
        assert_eq!(Settings::KEYS.len(), 20);
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

    #[test]
    fn the_defaults_are_the_ones_the_design_names() {
        let s = Settings::default();
        assert_eq!(s.node.network, Network::Mainnet);
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
