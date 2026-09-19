//! Every §7.1 check, as a **pure function of captured [`Probes`]**.
//!
//! Nothing in this file spawns a process, reads a file, issues an RPC or looks at the
//! clock: a detector sees only what `probes.rs` captured. That is what lets the seven
//! detectors that parse structured text be driven by `tests/fixtures/doctor/`, and the
//! one-line predicates by the table test at the bottom.
//!
//! A detector never applies anything. When a repair exists it names a [`PlanKind`];
//! `mutate.rs` turns that into the actual [`Action`](super::mutate::Action)s.
//!
//! **Every check whose precondition is missing reports `skip:<reason>`, never `pass`**
//! (§7). On this machine today that makes the whole `node`, `wallet` and `storage` groups
//! `skip:monerod_not_installed` / `skip:wallet_rpc_not_installed`, which is §4b's GREEN row
//! and the Phase 2 doctor gate.

use serde::{Deserialize, Serialize};

use super::probes::{
    Probes, CACHE_STALE_BLOCKS, CLOCK_SKEW_SEC, DISK_FLOOR_BYTES, SUPPORTED_MONERO,
    WALLET_BEHIND_BLOCKS,
};
use super::DoctorGroup;

/// The safety class of a check's repair (§7).
///
/// There is deliberately **no class C**: fproton's table carried one for "foreign but
/// reversible" with no members, and a class with no members is a `--fix` branch nothing
/// exercises and a question every fresh doctor agent has to ask (§7).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Class {
    /// fmonero-owned state; runs under `--fix`; reverts by file backup.
    A,
    /// fmonero-created integration; runs under `--fix`; reverts by a recorded inverse.
    B,
    /// Never auto-fixed.
    D,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::A => "A",
            Class::B => "B",
            Class::D => "D",
        }
    }
    pub fn describes(self) -> &'static str {
        match self {
            Class::A => "fmonero-owned state; reverts by file backup",
            Class::B => "fmonero-created integration; reverts by a recorded inverse",
            Class::D => "never auto-fixed; the detail names the command for a human",
        }
    }
}

/// How bad a non-pass is for this check (§7.1's `sev` column).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warn,
    Error,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Error => "error",
        }
    }
}

/// A check's verdict (§7 `status`).
///
/// **The wire spelling is part of the contract** (§5b rule 3): `pass` · `warn` · `fail` ·
/// `skip`, lower-case. A plain derive would emit `Pass`, which is a JSON document no
/// section of DESIGN.md describes — and this type lives here rather than in
/// `fmonero-core`, so its round trip is asserted by the Phase 2 doctor gate rather than by
/// Phase 1a's (§8).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl CheckStatus {
    pub fn name(self) -> &'static str {
        match self {
            CheckStatus::Pass => "pass",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
            CheckStatus::Skip => "skip",
        }
    }
}

/// The repairs `mutate.rs` knows how to plan. A detector only ever names one.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    WriteNodeUnit,
    WriteWalletUnit,
    WriteBothUnits,
    NormaliseWalletPort,
    CreateWalletDir,
    DisableUnits,
    ChmodMonerodConf,
    FixStatePerms,
    ResetStateJson,
    RepairSettings,
    LinkPlugin,
    EnablePlugin,
    MergeMenu,
}

impl PlanKind {
    pub fn name(self) -> &'static str {
        match self {
            PlanKind::WriteNodeUnit => "write_node_unit",
            PlanKind::WriteWalletUnit => "write_wallet_unit",
            PlanKind::WriteBothUnits => "write_both_units",
            PlanKind::NormaliseWalletPort => "normalise_wallet_port",
            PlanKind::CreateWalletDir => "create_wallet_dir",
            PlanKind::DisableUnits => "disable_units",
            PlanKind::ChmodMonerodConf => "chmod_monerod_conf",
            PlanKind::FixStatePerms => "fix_state_perms",
            PlanKind::ResetStateJson => "reset_state_json",
            PlanKind::RepairSettings => "repair_settings",
            PlanKind::LinkPlugin => "link_plugin",
            PlanKind::EnablePlugin => "enable_plugin",
            PlanKind::MergeMenu => "merge_menu",
        }
    }
}

/// What one detector answers.
#[derive(Clone, Debug)]
pub struct Verdict {
    pub status: CheckStatus,
    /// The short machine reason behind a non-pass (`not_installed`, `node_unreachable`).
    pub reason: Option<&'static str>,
    pub detail: String,
    /// A command for the human. Always set when a repair exists, fixable or not.
    pub fix_cmd: Option<String>,
    /// Set only when the doctor itself can repair this.
    pub plan: Option<PlanKind>,
}

impl Verdict {
    pub fn pass(detail: impl Into<String>) -> Verdict {
        Verdict {
            status: CheckStatus::Pass,
            reason: None,
            detail: detail.into(),
            fix_cmd: None,
            plan: None,
        }
    }
    pub fn warn(reason: &'static str, detail: impl Into<String>) -> Verdict {
        Verdict {
            status: CheckStatus::Warn,
            reason: Some(reason),
            detail: detail.into(),
            fix_cmd: None,
            plan: None,
        }
    }
    pub fn fail(reason: &'static str, detail: impl Into<String>) -> Verdict {
        Verdict {
            status: CheckStatus::Fail,
            reason: Some(reason),
            detail: detail.into(),
            fix_cmd: None,
            plan: None,
        }
    }
    pub fn skip(reason: &'static str, detail: impl Into<String>) -> Verdict {
        Verdict {
            status: CheckStatus::Skip,
            reason: Some(reason),
            detail: detail.into(),
            fix_cmd: None,
            plan: None,
        }
    }
    /// A command a human can run. Does **not** make the check fixable.
    #[must_use]
    pub fn cmd(mut self, cmd: impl Into<String>) -> Verdict {
        self.fix_cmd = Some(cmd.into());
        self
    }
    /// The doctor can repair this; the command shown is the fmonero invocation.
    #[must_use]
    pub fn fixable(mut self, plan: PlanKind, cmd: impl Into<String>) -> Verdict {
        self.plan = Some(plan);
        self.fix_cmd = Some(cmd.into());
        self
    }
}

/// One check, statically declared. `doctor capabilities` prints this table verbatim.
pub struct CheckDef {
    pub id: &'static str,
    pub group: DoctorGroup,
    pub class: Class,
    pub severity: Severity,
    pub title: &'static str,
    pub detect: fn(&Probes) -> Verdict,
}

/// Every §7.1 check, in report order.
pub const CHECKS: &[CheckDef] = &[
    // ---- binaries -----------------------------------------------------------
    CheckDef {
        id: "monerod_installed",
        group: DoctorGroup::Binaries,
        class: Class::D,
        severity: Severity::Error,
        title: "Monero's node program is installed",
        detect: monerod_installed,
    },
    CheckDef {
        id: "wallet_rpc_installed",
        group: DoctorGroup::Binaries,
        class: Class::D,
        severity: Severity::Error,
        title: "Monero's wallet program is installed",
        detect: wallet_rpc_installed,
    },
    CheckDef {
        id: "version_supported",
        group: DoctorGroup::Binaries,
        class: Class::D,
        severity: Severity::Warn,
        title: "The installed Monero is version 0.18",
        detect: version_supported,
    },
    // ---- node ---------------------------------------------------------------
    CheckDef {
        id: "node_layout",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Info,
        title: "Which node fmonero uses",
        detect: node_layout,
    },
    CheckDef {
        id: "node_reachable",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Error,
        title: "Your node answers",
        detect: node_reachable,
    },
    CheckDef {
        id: "remote_node_untrusted",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Warn,
        title: "A public node is told nothing about your amounts",
        detect: remote_node_untrusted,
    },
    CheckDef {
        id: "node_unrestricted",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Warn,
        title: "Your node answers more than basic questions",
        detect: node_unrestricted,
    },
    CheckDef {
        id: "node_synced",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Warn,
        title: "Your node is synced",
        detect: node_synced,
    },
    CheckDef {
        id: "node_peers",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Warn,
        title: "Your node has peers",
        detect: node_peers,
    },
    CheckDef {
        id: "node_clock",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Error,
        title: "This machine's clock agrees with the node's",
        detect: node_clock,
    },
    CheckDef {
        id: "node_data_dir_single_writer",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Error,
        title: "Only one program is using the chain folder",
        detect: node_data_dir_single_writer,
    },
    // ---- wallet -------------------------------------------------------------
    CheckDef {
        id: "wallet_rpc_reachable",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Warn,
        title: "The wallet service answers",
        detect: wallet_rpc_reachable,
    },
    CheckDef {
        id: "wallet_rpc_auth",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Error,
        title: "The wallet service's password file is private and works",
        detect: wallet_rpc_auth,
    },
    CheckDef {
        id: "wallet_daemon_link",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Warn,
        title: "The open wallet is close to the node's block",
        detect: wallet_daemon_link,
    },
    CheckDef {
        id: "wallet_daemon_auth",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Error,
        title: "The wallet can sign in to your node",
        detect: wallet_daemon_auth,
    },
    CheckDef {
        id: "wallet_nettype_matches",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Warn,
        title: "The open wallet is on the network your settings name",
        detect: wallet_nettype_matches,
    },
    CheckDef {
        id: "wallet_dir_exists",
        group: DoctorGroup::Wallet,
        class: Class::B,
        severity: Severity::Warn,
        title: "Your wallet folder exists and is private",
        detect: wallet_dir_exists,
    },
    CheckDef {
        id: "pending_send",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Error,
        title: "No payment is left unaccounted for",
        detect: pending_send,
    },
    // ---- units --------------------------------------------------------------
    CheckDef {
        id: "units_present",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Warn,
        title: "fmonero's two background services are set up",
        detect: units_present,
    },
    CheckDef {
        id: "unit_loopback",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Error,
        title: "Both services listen only on this machine",
        detect: unit_loopback,
    },
    CheckDef {
        id: "unit_no_password_in_argv",
        group: DoctorGroup::Units,
        class: Class::D,
        severity: Severity::Error,
        title: "No password appears in either service's command line",
        detect: unit_no_password_in_argv,
    },
    CheckDef {
        id: "unit_no_node_wants",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Error,
        title: "Neither service starts a second node behind fmonero's back",
        detect: unit_no_node_wants,
    },
    CheckDef {
        id: "unit_enabled_state",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Info,
        title: "Neither service starts on its own",
        detect: unit_enabled_state,
    },
    CheckDef {
        id: "wallet_dir_matches_unit",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Error,
        title: "The wallet service uses your wallet folder",
        detect: wallet_dir_matches_unit,
    },
    CheckDef {
        id: "wallet_daemon_matches_unit",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Error,
        title: "The wallet service points at the node you chose",
        detect: wallet_daemon_matches_unit,
    },
    CheckDef {
        id: "wallet_port_consistent",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Error,
        title: "The wallet service's port matches your settings",
        detect: wallet_port_consistent,
    },
    // ---- storage ------------------------------------------------------------
    CheckDef {
        id: "disk_space",
        group: DoctorGroup::Storage,
        class: Class::D,
        severity: Severity::Warn,
        title: "The chain folder's disk has room",
        detect: disk_space,
    },
    CheckDef {
        id: "prune_configured",
        group: DoctorGroup::Storage,
        class: Class::D,
        severity: Severity::Info,
        // One vocabulary (§2o rule 8): every other surface says "a smaller copy of the
        // blockchain", including this check's own detail (self-read, round 3).
        title: "Your node keeps the size of blockchain you asked for",
        detect: prune_configured,
    },
    // ---- install ------------------------------------------------------------
    CheckDef {
        id: "binary_on_path",
        group: DoctorGroup::Install,
        class: Class::D,
        severity: Severity::Warn,
        // Self-read (round 3): the old title was `~/.local/bin/fmonero exists (the shell's PATH
        // has no ~/.cargo/bin)` — a path, a shell variable and a second path, as the sentence a
        // person reads. The detail names the path; the title says what the check is for.
        title: "fmonero is installed where your shell looks for it",
        detect: binary_on_path,
    },
    CheckDef {
        id: "state_valid",
        group: DoctorGroup::Install,
        class: Class::A,
        severity: Severity::Warn,
        title: "fmonero's own saved state is readable",
        detect: state_valid,
    },
    CheckDef {
        id: "settings_valid",
        group: DoctorGroup::Install,
        class: Class::A,
        severity: Severity::Warn,
        title: "Your settings file is readable and holds only known keys",
        detect: settings_valid,
    },
    CheckDef {
        id: "history_cache_fresh",
        group: DoctorGroup::Install,
        class: Class::D,
        severity: Severity::Info,
        title: "The saved copy of your payments belongs to this wallet",
        detect: history_cache_fresh,
    },
    CheckDef {
        id: "public_node_cache",
        group: DoctorGroup::Install,
        class: Class::D,
        severity: Severity::Info,
        title: "The list of public nodes is less than a day old",
        detect: public_node_cache,
    },
    CheckDef {
        id: "plugin_linked",
        group: DoctorGroup::Install,
        class: Class::B,
        severity: Severity::Warn,
        title: "The bar widget is installed",
        detect: plugin_linked,
    },
    CheckDef {
        id: "plugin_enabled",
        group: DoctorGroup::Install,
        class: Class::B,
        severity: Severity::Warn,
        title: "The bar widget is switched on",
        detect: plugin_enabled,
    },
    CheckDef {
        id: "menu_entries",
        group: DoctorGroup::Install,
        class: Class::B,
        severity: Severity::Info,
        title: "fmonero's Omarchy menu entries are present",
        detect: menu_entries,
    },
    // ---- security -----------------------------------------------------------
    CheckDef {
        id: "monerod_conf_mode",
        group: DoctorGroup::Security,
        class: Class::B,
        severity: Severity::Error,
        title: "Only you can read the password your node asks for",
        detect: monerod_conf_mode,
    },
    CheckDef {
        id: "node_auth",
        group: DoctorGroup::Security,
        class: Class::D,
        severity: Severity::Warn,
        title: "Your node asked fmonero for a password",
        detect: node_auth,
    },
    CheckDef {
        id: "state_perms",
        group: DoctorGroup::Security,
        class: Class::A,
        severity: Severity::Error,
        title: "Everything fmonero writes is private to you",
        detect: state_perms,
    },
    CheckDef {
        id: "ports_owned",
        group: DoctorGroup::Security,
        class: Class::D,
        severity: Severity::Warn,
        title: "Who is listening on the node and wallet ports",
        detect: ports_owned,
    },
];

// ------------------------------------------------------------------ preconditions

/// Every `node` and `storage` check needs `monerod`; every `wallet` check needs
/// `monero-wallet-rpc`. Without them the whole group is `skip:<reason>`, never `pass`
/// (§7) — which is exactly §4b's GREEN row for this machine.
fn need_monerod(p: &Probes) -> Option<Verdict> {
    p.which_monerod.is_none().then(|| {
        Verdict::skip(
            "monerod_not_installed",
            "Monero is not installed, so there is nothing to check.",
        )
        .cmd("omarchy pkg add monero")
    })
}

fn need_wallet_rpc(p: &Probes) -> Option<Verdict> {
    p.which_wallet_rpc.is_none().then(|| {
        Verdict::skip(
            "wallet_rpc_not_installed",
            "Monero is not installed, so there is nothing to check.",
        )
        .cmd("omarchy pkg add monero")
    })
}

/// A units check that reads a unit file has nothing to say when the file is absent;
/// `units_present` is the check that reports that, once.
/// **Remote mode is a precondition like any other, and `skip:remote_node` is how it is
/// reported** (v5.2, §2m.3, §7.1).
///
/// The full set that skips behind it: every `node` check except `node_layout` (which passes and
/// names the address) and `remote_node_untrusted` (which is only meaningful there);
/// `wallet_daemon_link` and `wallet_daemon_auth`; `node_auth`; `disk_space` and
/// `prune_configured`. Everything else runs unchanged, because remote mode moves the daemon,
/// not the machine — in particular `unit_loopback`, which asserts the two units' **own**
/// `--rpc-bind-ip` and has nothing to do with the address they dial (§2c).
fn need_local_node(p: &Probes) -> Option<Verdict> {
    (p.node_mode == "remote").then(|| {
        Verdict::skip(
            "remote_node",
            // **The single highest-volume string in the program**: it appears twelve times in
            // one public-node `doctor` run, and it was 28 words of architecture (§2o rule 2).
            format!(
                "You are using the public node {}, so there is nothing of your own to check here.",
                p.daemon_address
            ),
        )
        .cmd("fmonero node local --yes")
    })
}

fn need_units(p: &Probes, about: Which) -> Option<Verdict> {
    let node_missing = !p.node_unit.exists;
    let wallet_missing = !p.wallet_unit.exists;
    let missing = match about {
        Which::Wallet => wallet_missing,
        Which::Both => node_missing || wallet_missing,
    };
    missing.then(|| {
        // **W11: the row's own subject, named** — `wallet_dir_matches_unit`,
        // `wallet_daemon_matches_unit` and `wallet_port_consistent` are three checks of the
        // WALLET service, and all three read `fmonero has not set up the node service yet.`
        // because one word was chosen for all seven skips from whichever file happened to exist.
        let what = match (about, node_missing, wallet_missing) {
            (Which::Wallet, _, _) => "the wallet service",
            (Which::Both, true, true) => "its two background services",
            (Which::Both, true, false) => "your node",
            (Which::Both, _, _) => "the wallet service",
        };
        Verdict::skip(
            "unit_missing",
            format!("fmonero has not set up {what} yet."),
        )
        .cmd("fmonero setup --units --yes")
    })
}

/// Which of the two background services a `units` check is about (W11).
///
/// `Both` is every check that reads the two files together — the bind address, the credential
/// scan, the `Wants=` scan, the enabled state — and `Wallet` is the three that read only the
/// wallet unit. There is no node-only check today: `units_present` reports the two files itself
/// and does not skip behind this at all.
#[derive(Copy, Clone, Debug)]
enum Which {
    Wallet,
    Both,
}

// ------------------------------------------------------------------ binaries

fn monerod_installed(p: &Probes) -> Verdict {
    let Some(path) = &p.which_monerod else {
        // **In remote mode this is a `warn`, not a `fail`** (v5.2, §2m.3): remote mode uses no
        // local daemon, and a check that fails on a machine configured correctly is the check
        // an agent relaxes (§7). The detail names what would require it.
        if p.node_mode == "remote" {
            return Verdict::warn(
                "not_needed_in_remote_mode",
                "You are using a public node, so this machine needs no node program.",
            )
            .cmd("omarchy pkg add monero");
        }
        return Verdict::fail("not_installed", "Monero's node program is not installed.")
            .cmd("omarchy pkg add monero");
    };
    if !p.monerod_version.success() {
        return Verdict::fail(
            "version_probe_failed",
            format!(
                "{path} does not answer --version: {}",
                p.monerod_version.failure_detail()
            ),
        )
        .cmd("monerod --version");
    }
    Verdict::pass(format!("{path}: {}", p.monerod_version.first_line()))
}

fn wallet_rpc_installed(p: &Probes) -> Verdict {
    let Some(path) = &p.which_wallet_rpc else {
        return Verdict::fail("not_installed", "Monero's wallet program is not installed.")
            .cmd("omarchy pkg add monero");
    };
    if !p.wallet_rpc_version_cmd.success() {
        return Verdict::fail(
            "version_probe_failed",
            format!(
                "{path} does not answer --version: {}",
                p.wallet_rpc_version_cmd.failure_detail()
            ),
        )
        .cmd("monero-wallet-rpc --version");
    }
    Verdict::pass(format!("{path}: {}", p.wallet_rpc_version_cmd.first_line()))
}

fn version_supported(p: &Probes) -> Verdict {
    if let Some(v) = need_monerod(p) {
        return v;
    }
    let line = p.monerod_version.first_line();
    if line.is_empty() {
        return Verdict::skip(
            "no_version_line",
            "Monero's node program printed no version.",
        );
    }
    match version_of(&line) {
        Some(v) if v == SUPPORTED_MONERO => {
            Verdict::pass(format!("{line}. That is the {v} fmonero is written for."))
        }
        Some(v) => Verdict::warn(
            "unsupported_version",
            format!("This is Monero {v}. fmonero was written for {SUPPORTED_MONERO}."),
        ),
        None => Verdict::warn(
            "unparsable_version",
            format!("Could not read a version from \"{line}\"."),
        ),
    }
}

/// `Monero 'Fluorine Fermi' (v0.18.5.1-release)` → `0.18`.
fn version_of(line: &str) -> Option<String> {
    let start = line.find(|c: char| c.is_ascii_digit())?;
    let rest = &line[start..];
    let mut it = rest.split('.');
    let major = it.next()?;
    let minor = it.next()?;
    let minor: String = minor.chars().take_while(|c| c.is_ascii_digit()).collect();
    if major.is_empty() || minor.is_empty() {
        return None;
    }
    Some(format!("{major}.{minor}"))
}

// ------------------------------------------------------------------ node

fn node_layout(p: &Probes) -> Verdict {
    if let Some(v) = need_monerod(p) {
        return v;
    }
    if let Some(e) = &p.layout_error {
        return Verdict::warn(
            "layout_probe_failed",
            format!("Could not work out which node fmonero uses: {e}"),
        );
    }
    match p.layout.as_str() {
        "user_unit" => Verdict::pass(format!("fmonero runs your node at {}.", p.node_url)),
        // §0: Arch's node is "detected, used read-only, never fought". That is a healthy
        // layout, not a defect — the node mutators are simply not ours to call (§2i).
        //
        // **The one place `systemctl` survives a fix line, and V10's rule is why it may.** That
        // rule is about fmonero reaching around itself to start and stop its OWN services, which
        // it now never suggests. This node is not fmonero's: there is no fmonero command that can
        // tell you anything about it, `status` is read-only, and the alternative would be a fix
        // line with no fix in it.
        "system_unit" => {
            Verdict::pass(fmonero_core::FOREIGN_NODE_INFO).cmd("sudo systemctl status monerod")
        }
        // v5.2, §2m.3: **the one `node` check that still runs in remote mode, and it passes**
        // — the group's only remaining source of truth, so that `doctor --only node` is never
        // an empty table. It was a `warn:non_loopback_url` while the layout was keyed on a
        // non-loopback `node.url`, which `settings set` refuses to store: a warning about a
        // state the tool could not enter. Now the mode is a supported configuration and the
        // detail names what is in force.
        // **v5.6 drops the trust flag and the ssl mode from this detail** (§2o.2 deletes both
        // words everywhere): they are `--json` fields, `remote_node_untrusted` is the check that
        // exists to judge the first, and a detail repeating a value no reader can act on is what
        // made this row three clauses long.
        "remote" => Verdict::pass(format!(
            "You are using the public node {}.",
            p.daemon_address
        ))
        .cmd("fmonero node local --yes"),
        _ => Verdict::warn("no_layout", "No node is set up on this machine.")
            .cmd("fmonero setup --units --yes"),
    }
}

fn node_reachable(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_monerod(p) {
        return v;
    }
    if p.node_reachable {
        return Verdict::pass(format!(
            "{} answered, at block {}.",
            p.node_url,
            crate::output::grouped(p.node_height.unwrap_or_default())
        ));
    }
    Verdict::fail(
        "node_unreachable",
        // The message is already a sentence (§2o, V8), so the address goes after it rather than
        // in front of it: `http://127.0.0.1:18081 did not answer. The node is not answering.` was
        // the same fact twice, which is what happens when a detail concatenates two rewrites.
        match p.node_error.as_deref() {
            Some(e) => format!("{e} fmonero asked it on {}.", p.node_url),
            None => format!("Your node did not answer on {}.", p.node_url),
        },
    )
    .cmd("fmonero node up")
}

fn node_unrestricted(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_monerod(p) {
        return v;
    }
    if !p.node_reachable {
        return Verdict::skip("node_unreachable", "Your node did not answer.");
    }
    match p.node_restricted {
        Some(true) => Verdict::warn("node_restricted", "This node only answers basic questions.")
            .cmd(if p.layout == "user_unit" {
                "fmonero setup --units --yes"
            } else {
                // §2o: the trailing comment is part of what the user reads, and the whole
                // literal is allow-listed by the grep for beginning with a command — so the word
                // `restricted` was slipping through in the one place the allow-list cannot see.
                "sudo systemctl status monerod   # a node fmonero does not own is not ours to change"
            }),
        Some(false) => Verdict::pass("Your node answers everything fmonero asks."),
        None => Verdict::skip("no_get_info", "Your node did not say."),
    }
}

fn node_synced(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_monerod(p) {
        return v;
    }
    if !p.node_reachable {
        return Verdict::skip("node_unreachable", "Your node did not answer.");
    }
    let height = p.node_height.unwrap_or_default();
    let target = p.node_target_height.unwrap_or_default();
    match p.node_synchronized {
        Some(true) => Verdict::pass(format!(
            "Synced at block {}.",
            crate::output::grouped(height)
        )),
        Some(false) => {
            // target_height is 0 when synced, so the percentage special-cases it (§1b.3).
            let denom = target.max(height).max(1);
            let pct = (height as f64 / denom as f64) * 100.0;
            Verdict::warn(
                "node_syncing",
                format!(
                    "Syncing: block {} of {} ({}).",
                    crate::output::grouped(height),
                    crate::output::grouped(denom),
                    // X11: one rounding, from core.
                    fmonero_core::percent(pct)
                ),
            )
            .cmd("fmonero node up --wait --wait-timeout 86400")
        }
        None => Verdict::skip("no_get_info", "Your node did not say how far along it is."),
    }
}

fn node_peers(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_monerod(p) {
        return v;
    }
    if !p.node_reachable {
        return Verdict::skip("node_unreachable", "Your node did not answer.");
    }
    // A restricted node reports 0 by construction (§1b.3), so a `warn` here would be a
    // false alarm on a perfectly healthy machine.
    if p.node_restricted == Some(true) {
        return Verdict::skip(
            "node_restricted",
            "This node only answers basic questions, so it never says how many others it \
             is connected to.",
        );
    }
    if p.node_offline == Some(true) {
        return Verdict::skip("node_offline", "Your node was started with networking off.");
    }
    match p.node_outgoing_connections {
        Some(0) => Verdict::warn(
            "no_peers",
            "Your node is not connected to any other machine.",
        )
        .cmd("fmonero node limit --out-peers 16"),
        Some(n) => Verdict::pass(format!("Connected to {n} other machines.")),
        None => Verdict::skip(
            "no_get_info",
            "Your node did not say who it is connected to.",
        ),
    }
}

fn node_clock(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_monerod(p) {
        return v;
    }
    let Some(adjusted) = p.node_adjusted_time else {
        return Verdict::skip("node_unreachable", "Your node did not answer.");
    };
    let skew = adjusted - p.now_unix;
    // **X27: `-230030s` and `±60s` are not times a person reads**, and `fmonero_core::age` is the
    // formatter every other surface uses for exactly this. `age` ends in "ago", which is wrong for
    // a gap rather than an elapsed time, so the "ago" comes off and the direction is a word.
    let gap = |secs: i64| {
        fmonero_core::age(secs.unsigned_abs())
            .trim_end_matches(" ago")
            .to_string()
    };
    if skew.abs() <= CLOCK_SKEW_SEC {
        return Verdict::pass(format!(
            "This machine's clock is within {} of the Monero network's.",
            gap(skew)
        ));
    }
    Verdict::fail(
        "clock_skew",
        format!(
            "This machine's clock is {} {} the Monero network's, and {} is the most fmonero \
             accepts. A clock this far out gets the times your money unlocks at wrong and stops \
             other machines talking to yours.",
            gap(skew),
            if skew < 0 { "behind" } else { "ahead of" },
            gap(CLOCK_SKEW_SEC)
        ),
    )
    .cmd("timedatectl set-ntp true")
}

fn node_data_dir_single_writer(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_monerod(p) {
        return v;
    }
    if let Some(e) = &p.data_dir_probe_error {
        return Verdict::skip(
            "probe_failed",
            format!("Could not check {}. {e}", p.data_dir),
        );
    }
    match p.data_dir_holder {
        None => Verdict::pass(format!("Nothing else is using {}.", p.data_dir)),
        // §7.1's fix column for this row IS "detail names the PID": there is no command.
        Some(pid) => Verdict::fail(
            "data_dir_in_use",
            format!(
                "Another Monero node is already using {} (process {pid}). Two of them on one \
                 copy of the blockchain would damage it, so fmonero will not start a second.",
                p.data_dir
            ),
        )
        .cmd("fmonero status --json   # the node that is already running is serving this data dir"),
    }
}

// ------------------------------------------------------------------ wallet

fn wallet_rpc_reachable(p: &Probes) -> Verdict {
    if let Some(v) = need_wallet_rpc(p) {
        return v;
    }
    if p.wallet_reachable {
        return Verdict::pass(format!(
            "{} answered, version {}{}.",
            p.wallet_url,
            p.wallet_rpc_version.as_deref().unwrap_or("unknown"),
            if p.wallet_open {
                ", and a wallet is open"
            } else {
                ", with no wallet open"
            }
        ));
    }
    Verdict::warn(
        "wallet_unreachable",
        match p.wallet_error.as_deref() {
            Some(e) => format!("{e} fmonero asked it on {}.", p.wallet_url),
            None => format!("The wallet service did not answer on {}.", p.wallet_url),
        },
    )
    // V10: never `systemctl`. Opening a wallet starts the service on the way in, which is the
    // one route fmonero has and the one the TUI's `o` takes.
    .cmd("fmonero wallet open <name>   # starts the wallet service, then opens")
}

fn wallet_rpc_auth(p: &Probes) -> Verdict {
    if let Some(v) = need_wallet_rpc(p) {
        return v;
    }
    if !p.wallet_login_file.exists {
        // §4.2's failure mode: a network switch that left wallet.port behind has the daemon
        // writing one file while the client reads another, and the symptom ("the wallet
        // daemon is not running") points away from the cause. So name BOTH ports.
        let unit_port = p
            .unit_wallet_port
            .map(|x| x.to_string())
            .unwrap_or_else(|| "unwritten".to_string());
        if p.wallet_unit_state.active {
            return Verdict::fail(
                "login_file_missing",
                format!(
                    "The wallet service is running but its password file ({}) is not there. \
                     fmonero looks for it under port {}, and {} was set up to answer on {}.",
                    p.wallet_login_file.path,
                    p.wallet_login_port,
                    p.wallet_unit_name,
                    unit_port
                ),
            )
            // V10: never `systemctl`. `W` on the Wallet screen stops the service; opening a
            // wallet starts it again, and the new one writes the file under the right port.
            .cmd("fmonero tui   # on the Wallet screen: W stops the wallet service, o starts it again");
        }
        return Verdict::skip(
            "wallet_daemon_down",
            format!(
                "The wallet service is not running, so its password file ({}) is absent.",
                p.wallet_login_file.path
            ),
        )
        .cmd("fmonero wallet open <name>   # starts the wallet service, then opens");
    }
    // Not a literal 0600: `private_file::create` settles the file at 0400, so §2c's rule is
    // "no group and no other bits" and stays right if upstream changes the dance.
    if !p.wallet_login_file.owner_only() {
        return Verdict::fail(
            "login_file_readable",
            format!(
                "{} is {:04o}, so other people who use this machine can read it. Only you \
                 may.",
                p.wallet_login_file.path,
                p.wallet_login_file.mode.unwrap_or(0)
            ),
        )
        .cmd(
            "fmonero tui   # on the Wallet screen: W stops the wallet service, o starts it again",
        );
    }
    match (p.wallet_reachable, p.wallet_authenticated) {
        (true, Some(true)) => Verdict::pass(format!(
            "Only you can read {}, and the wallet service asked fmonero for it.",
            p.wallet_login_file.path
        )),
        (true, Some(false)) => Verdict::fail(
            "no_challenge",
            "The wallet service answered without asking fmonero for a password. It was started \
             with its password turned off, which fmonero never does.",
        )
        .cmd("fmonero setup --units --yes"),
        (true, None) => Verdict::skip("no_request_completed", "Nothing was asked of it this run."),
        (false, _) => Verdict::skip(
            "wallet_unreachable",
            "Only you can read the password file, but the wallet service did not answer.",
        )
        .cmd("fmonero wallet open <name>   # starts the wallet service, then opens"),
    }
}

fn wallet_daemon_link(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_wallet_rpc(p) {
        return v;
    }
    let (Some(wh), Some(nh)) = (p.wallet_height, p.node_height) else {
        return Verdict::skip(
            "no_heights",
            "This needs a block number from the wallet and one from the node, and one of them \
             is missing.",
        );
    };
    let behind = nh as i64 - wh as i64;
    if behind <= WALLET_BEHIND_BLOCKS {
        return Verdict::pass(format!(
            "The wallet is at block {}, the node at {}: {behind} to go.",
            crate::output::grouped(wh),
            crate::output::grouped(nh)
        ));
    }
    Verdict::warn(
        "wallet_behind",
        format!(
            "The wallet is at block {}, the node at {}: {behind} blocks to go.",
            crate::output::grouped(wh),
            crate::output::grouped(nh)
        ),
    )
    .cmd("fmonero wallet refresh")
}

fn wallet_daemon_auth(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_wallet_rpc(p) {
        return v;
    }
    // One-shot and honest about what it cannot distinguish (§7.1): "stuck while the node
    // advances" is a two-sample-over-time observation, and this file's whole contract is
    // that a detector is a pure function of ONE run's probes.
    if !p.monerod_conf_has_login {
        return Verdict::skip(
            "no_node_credential",
            "Your node asks for no password, so the wallet needs none.",
        );
    }
    if !p.wallet_reachable || !p.node_reachable {
        return Verdict::skip(
            "daemon_unreachable",
            "Your node and the wallet service both have to answer before this means anything.",
        );
    }
    if p.node_synchronized != Some(true) {
        return Verdict::skip(
            "node_syncing",
            "Your node is still catching up, so a wallet behind it is expected.",
        );
    }
    let (Some(wh), Some(nh)) = (p.wallet_height, p.node_height) else {
        return Verdict::skip(
            "no_heights",
            "No wallet is open, so there is no block number to compare.",
        );
    };
    if (wh as i64) + WALLET_BEHIND_BLOCKS < nh as i64 {
        return Verdict::fail(
            "wallet_not_syncing",
            format!(
                "Your node is fully synced at block {} and the wallet is still at {}. Usually \
                 that means the wallet has not been given the password your node asks for. It \
                 can also mean the wallet is simply scanning a long way back, and this check \
                 cannot tell the two apart.",
                crate::output::grouped(nh),
                crate::output::grouped(wh)
            ),
        )
        // Built from the CONFIGURED node, never a literal: §4's network table puts the node at
        // 38081 on stagenet and 28081 on testnet, and §4b's whole RED rehearsal runs on
        // stagenet — so a hard-coded 18081 repointed the wallet at a port nothing is listening
        // on, from the command this very check supplied.
        .cmd(format!(
            "fmonero wallet set-daemon {} --trusted --yes",
            fmonero_core::units::host_port(&p.node_url)
        ));
    }
    Verdict::pass(format!(
        "The wallet is at block {} and the node is synced at {}.",
        crate::output::grouped(wh),
        crate::output::grouped(nh)
    ))
}

fn wallet_nettype_matches(p: &Probes) -> Verdict {
    if let Some(v) = need_wallet_rpc(p) {
        return v;
    }
    let Some(net) = &p.last_open_nettype else {
        // **Two rows of one screen may not disagree** (the 2026-09-19 verification's item 19):
        // `wallet_rpc_reachable` said `and a wallet is open` two rows above this one saying
        // `No wallet is open`, and both were right — one reads the live service, the other reads
        // what fmonero recorded when it opened a wallet. When the service says one is open, this
        // row says what it is actually missing.
        if p.wallet_open {
            return Verdict::skip(
                "no_recorded_network",
                "A wallet is open, and fmonero has no record of which network it belongs to. It \
                 writes that down when it opens one itself.",
            )
            .cmd("fmonero wallet open <name>");
        }
        return Verdict::skip(
            "wallet_closed",
            "No wallet is open, so there is no network to read.",
        );
    };
    if *net == p.network {
        return Verdict::pass(format!(
            "The open wallet {} belongs to the {net} network, which is the one node.network names.",
            p.last_open_name.as_deref().unwrap_or("this one")
        ));
    }
    // A warning, not a refusal: the send path follows the WALLET (§2a.6), so this is a
    // configuration fact — but it means wallet.dir, the ports and the node the wallet talks
    // to were all rendered for a different network.
    Verdict::warn(
        "nettype_mismatch",
        format!(
            "The open wallet belongs to the {net} network while node.network says {}. Every \
             path and port fmonero set up is the {} one.",
            p.network, p.network
        ),
    )
    .cmd(format!(
        "fmonero settings set node.network {net} && fmonero setup --units --yes"
    ))
}

fn wallet_dir_exists(p: &Probes) -> Verdict {
    if let Some(v) = need_wallet_rpc(p) {
        return v;
    }
    if !p.wallet_dir_missing.is_empty() {
        // A missing ANCESTOR is what stops the unit from starting: monero-wallet-rpc
        // creates only the leaf, with one mkdir(dir, 0700), and treats any failure but
        // EEXIST as fatal (§4.2).
        return Verdict::warn(
            "missing_ancestor",
            format!(
                "{} is not there yet. {} directories have to be made: {}.",
                p.wallet_dir,
                p.wallet_dir_missing.len(),
                p.wallet_dir_missing.join(", ")
            ),
        )
        .fixable(PlanKind::CreateWalletDir, "fmonero doctor --fix");
    }
    if !p.wallet_dir_bad_modes.is_empty() {
        let list: Vec<String> = p
            .wallet_dir_bad_modes
            .iter()
            .map(|(path, mode)| format!("{path} is {mode:04o}"))
            .collect();
        return Verdict::warn(
            "bad_mode",
            format!(
                "Other people who use this machine can look inside your wallet directories: {}. \
                 Only you may.",
                list.join(", ")
            ),
        )
        .cmd(format!("chmod 700 {}", p.wallet_dir));
    }
    Verdict::pass(format!(
        "{} is there, and only you can read it.",
        p.wallet_dir
    ))
}

fn pending_send(p: &Probes) -> Verdict {
    if let Some(v) = need_wallet_rpc(p) {
        return v;
    }
    let Some(rec) = &p.pending else {
        return Verdict::pass("No payment is waiting to be accounted for.");
    };
    let unrelayed: Vec<&str> = rec
        .txs
        .iter()
        .filter(|t| !t.relayed)
        .map(|t| t.txid.as_str())
        .collect();
    if unrelayed.is_empty() {
        // Fully relayed: the next `send` resolves it by itself (§2a.4). Reporting `fail`
        // here would teach the user to run `clear-pending` out of habit, which is the one
        // thing the interlock exists to prevent.
        return Verdict::warn(
            "resolvable_record",
            format!(
                "{} payment(s) to {} have all gone out. fmonero clears this note by itself the \
                 next time you send.",
                rec.txs.len(),
                rec.dest
            ),
        )
        .cmd("fmonero wallet history --limit 5");
    }
    Verdict::fail(
        "send_may_be_in_flight",
        format!(
            "{} of {} payment(s) to {} are not marked as sent: {}. Only you can decide whether \
             they arrived, so fmonero never clears this note on its own.",
            unrelayed.len(),
            rec.txs.len(),
            rec.dest,
            unrelayed.join(", ")
        ),
    )
    .cmd("fmonero wallet history --limit 5   # then: fmonero wallet clear-pending --yes")
}

// ------------------------------------------------------------------ units

fn units_present(p: &Probes) -> Verdict {
    let mut bad: Vec<String> = Vec::new();
    // `matches` is the comparison made on the UNREDACTED bytes at capture time; "absent"
    // and "drifted" share one severity and one fixer (§7.1) but must read differently,
    // because the user-facing meaning differs completely: absent means "run setup",
    // drifted means "something changed these".
    for (name, subject, exists, matches) in [
        (
            &p.node_unit_name,
            "Your node",
            p.node_unit.exists,
            p.node_unit_matches,
        ),
        (
            &p.wallet_unit_name,
            "The wallet service",
            p.wallet_unit.exists,
            p.wallet_unit_matches,
        ),
    ] {
        let _ = name;
        if !exists {
            // **X37: both unit files were named as prose** — `fmonero-monerod.service has not been
            // written yet. fmonero-wallet-rpc.service has not been written yet.` §2o.2 says a
            // systemd user unit is never named and is part of *your node* / *the wallet service*,
            // and every other row in this group already reads that way. The file names are in
            // `doctor --json`, and in `settings list` under the keys that hold them.
            bad.push(format!("{subject} has not been set up yet."));
        } else if !matches {
            bad.push(format!(
                "{subject} does not start the way fmonero writes it, so something has changed it."
            ));
        }
    }
    if bad.is_empty() {
        return Verdict::pass(
            "Your node and the wallet service both start exactly the way fmonero sets them up."
                .to_string(),
        );
    }
    Verdict::warn("unit_drift", bad.join(" "))
        .fixable(PlanKind::WriteBothUnits, "fmonero doctor --fix")
}

fn unit_loopback(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p, Which::Both) {
        return v;
    }
    let mut bad = Vec::new();
    for (name, probe) in [
        (&p.node_unit_name, &p.node_unit),
        (&p.wallet_unit_name, &p.wallet_unit),
    ] {
        let Some(text) = &probe.text else { continue };
        let argv = super::probes::exec_start_argv(text);
        if let Some(ip) = super::probes::flag_value(&argv, "--rpc-bind-ip") {
            if ip != "127.0.0.1" {
                bad.push(format!("{name} opens {ip}, which is not just this machine"));
            }
        }
        if argv.iter().any(|a| a == "--confirm-external-bind") {
            bad.push(format!("{name} allows other machines to connect"));
        }
    }
    if bad.is_empty() {
        return Verdict::pass(
            "Both services answer on this machine only, and neither allows anything else.",
        );
    }
    Verdict::fail("non_loopback_bind", bad.join("; "))
        .fixable(PlanKind::WriteBothUnits, "fmonero doctor --fix")
}

fn unit_no_password_in_argv(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p, Which::Both) {
        return v;
    }
    let mut bad = Vec::new();
    for (name, probe) in [
        (&p.node_unit_name, &p.node_unit),
        (&p.wallet_unit_name, &p.wallet_unit),
    ] {
        let Some(text) = &probe.text else { continue };
        // On ExecStart's argv only: /proc/<pid>/cmdline is world-readable, while the node
        // unit's own `rpc-login=` INSIDE its --config-file is correct and must not be
        // reported (§2c, §4.2).
        let argv = super::probes::exec_start_argv(text);
        for flag in [
            "--password",
            "--password-file",
            "--rpc-login",
            "--daemon-login",
        ] {
            if argv
                .iter()
                .any(|a| a == flag || a.starts_with(&format!("{flag}=")))
            {
                bad.push(format!(
                    "{name} passes {flag} where anyone on this machine can read it"
                ));
            }
        }
        // The other half of §7.1's condition, and the half a detector forgets. An ABSENT
        // --log-level satisfies it: the node unit takes its level from the config file.
        if let Some(level) = super::probes::flag_value(&argv, "--log-level") {
            if level != "0" {
                bad.push(format!(
                    "{name} sets --log-level {level}. Above 0, Monero writes the contents of its \
                     own requests to a log file on this machine"
                ));
            }
        }
    }
    if bad.is_empty() {
        return Verdict::pass(
            "Neither service is started with a password on its command line, and both log the \
             least they can.",
        );
    }
    Verdict::fail("credential_in_argv", bad.join("; ")).cmd(
        "fmonero setup --units --yes   # writes both service files again from fmonero's own \
             template",
    )
}

fn unit_no_node_wants(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p, Which::Both) {
        return v;
    }
    let mut bad = Vec::new();
    for (name, probe) in [
        (&p.node_unit_name, &p.node_unit),
        (&p.wallet_unit_name, &p.wallet_unit),
    ] {
        let Some(text) = &probe.text else { continue };
        for line in text.lines() {
            let t = line.trim();
            let Some(rest) = t
                .strip_prefix("Wants=")
                .or_else(|| t.strip_prefix("Requires="))
            else {
                continue;
            };
            if rest.split_whitespace().any(|u| u.contains("monerod")) {
                bad.push(format!("{name} would pull your node up with it ({t})"));
            }
        }
    }
    if bad.is_empty() {
        return Verdict::pass(
            "Neither service can start your node behind fmonero's back, so every start goes \
             through the check that stops two nodes using one copy of the blockchain.",
        );
    }
    Verdict::fail(
        "node_wants",
        format!(
            "{}. That would start a node without the check that stops two of them using one \
             copy of the blockchain, which damages it.",
            bad.join("; ")
        ),
    )
    .fixable(PlanKind::WriteBothUnits, "fmonero doctor --fix")
}

fn unit_enabled_state(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p, Which::Both) {
        return v;
    }
    let enabled: Vec<&str> = [&p.node_unit_state, &p.wallet_unit_state]
        .iter()
        .filter(|s| s.enabled)
        .map(|s| s.name.as_str())
        .collect();
    if enabled.is_empty() {
        return Verdict::pass(
            "Neither service starts by itself; fmonero starts them when you \
             need them.",
        );
    }
    Verdict::warn(
        "unit_enabled",
        format!(
            "{} would start every time you log in. fmonero starts these only when you need them.",
            enabled.join(", ")
        ),
    )
    .fixable(PlanKind::DisableUnits, "fmonero doctor --fix")
}

/// **v5.2, §7.1.** The installed wallet unit's `--daemon-address`, trust flag and
/// `--daemon-ssl` are the ones `units::daemon_target` says are in force, byte for byte.
///
/// Its own check rather than a clause of `units_present` because it is the one skew with a
/// **silent** symptom: a `monero-wallet-rpc` started before any `wallet open` auto-refreshes
/// against whatever its argv said (§1b.9 item 3), so a unit naming the old node keeps a wallet
/// syncing from a node `status` does not name — and `units_present`'s byte comparison, which
/// would also catch it, reports the generic `unit_drift` and buries the reason.
///
/// It is also the third argument for this check, and the hardest: in remote mode
/// `on_create_wallet` reads `/getheight` over the address in the **unit's** argv and sets the
/// new wallet's restore height from it, before fmonero's own `set_daemon` ever runs
/// (`wallet_rpc_server.cpp:3596`, `:3606-3609`, §2m.3).
fn wallet_daemon_matches_unit(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p, Which::Wallet) {
        return v;
    }
    let Some(address) = &p.unit_daemon_address else {
        return Verdict::fail(
            "no_daemon_address_flag",
            format!(
                "{} does not say which node the wallet should use.",
                p.wallet_unit_name
            ),
        )
        .fixable(PlanKind::WriteWalletUnit, "fmonero doctor --fix");
    };
    if *address != p.daemon_address {
        return Verdict::fail(
            "daemon_address_mismatch",
            format!(
                "The wallet service was set up to use {address} while your settings say {}. The \
                 service starts with the address it was given, so your wallet would scan against \
                 a node fmonero does not show you.",
                p.daemon_address
            ),
        )
        .fixable(
            PlanKind::WriteWalletUnit,
            "fmonero node remote <addr> --yes",
        );
    }
    // Neither flag is NOT "untrusted": `make_basic` falls back to
    // `is_local_address(daemon_address)` (`wallet2.cpp:451-462`), so the absence means
    // "whatever the address parse decided" — the implicit configuration §2j objects to on the
    // one setting that decides whether the wallet's own amounts travel inside a request.
    if p.unit_daemon_trusted != Some(p.daemon_trusted) {
        return Verdict::fail(
            "daemon_trust_mismatch",
            format!(
                "The wallet service was set up to {} while your settings say it should {}.",
                match p.unit_daemon_trusted {
                    Some(true) => "tell the node what your wallet holds",
                    Some(false) => "keep what your wallet holds to itself",
                    None => "decide that from the address, which is neither answer",
                },
                if p.daemon_trusted {
                    "tell it"
                } else {
                    "keep it to itself"
                }
            ),
        )
        .fixable(PlanKind::WriteWalletUnit, "fmonero doctor --fix");
    }
    if p.unit_daemon_ssl.as_deref() != Some(p.daemon_ssl.as_str()) {
        return Verdict::fail(
            "daemon_ssl_mismatch",
            format!(
                "The wallet service was set up for {} connections while the node's address asks \
                 for {}. Mismatched, it retries an encrypted handshake on every reconnection.",
                match p.unit_daemon_ssl.as_deref() {
                    Some("enabled") => "encrypted",
                    Some("disabled") => "plain",
                    Some(other) => other,
                    None => "unstated",
                },
                if p.daemon_ssl == "enabled" {
                    "encrypted"
                } else {
                    "plain"
                }
            ),
        )
        .fixable(PlanKind::WriteWalletUnit, "fmonero doctor --fix");
    }
    Verdict::pass(format!(
        "The wallet service uses {address}, {}, over a {} connection.",
        if p.daemon_trusted {
            "and tells it what your wallet holds"
        } else {
            "and keeps what your wallet holds to itself"
        },
        if p.daemon_ssl == "enabled" {
            "encrypted"
        } else {
            "plain"
        }
    ))
}

/// **v5.2, §2m.6, §7.1.** A remote node marked `trusted` that is not a private literal.
///
/// The only check in this document that is meaningful **exclusively** in remote mode, which is
/// why it is the second exception to §2m.3's `skip:remote_node` rule: skipping it with the rest
/// of its group would disable the trusted-public-node warning in the only mode it can fire.
fn remote_node_untrusted(p: &Probes) -> Verdict {
    if p.node_mode != "remote" {
        return Verdict::skip(
            "local_mode",
            "You are using your own node, which fmonero trusts because it is yours.",
        );
    }
    let host = fmonero_core::settings::remote_host(&p.daemon_address);
    if !p.daemon_trusted {
        return Verdict::pass(format!(
            "fmonero does not tell {} which amounts your wallet holds.",
            host
        ));
    }
    if fmonero_core::settings::is_private_host(host) {
        return Verdict::pass(format!(
            "{host} is an address on your own network, which is the one case for trusting a node \
             you did not start yourself."
        ));
    }
    // A DNS name is never treated as private: classifying one means resolving it, a network
    // call inside a detector §7 requires to be pure, whose answer a name server can change
    // between runs. So a private node reached by name warns, and the detail says so.
    Verdict::warn(
        "trusted_public_node",
        format!(
            "{host} is trusted, so your wallet puts the amounts it holds inside the questions it \
             asks that node. If the node is yours, give it by its address on your own network \
             instead of by name: fmonero never treats a name as private, because working that \
             out means asking the network."
        ),
    )
    .cmd("fmonero node remote <addr> --yes   # without --trusted")
}

fn wallet_dir_matches_unit(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p, Which::Wallet) {
        return v;
    }
    let Some(service_dir) = &p.unit_wallet_dir else {
        return Verdict::fail(
            "no_wallet_dir_flag",
            format!(
                "{} does not say which directory your wallets are in.",
                p.wallet_unit_name
            ),
        )
        .fixable(PlanKind::WriteWalletUnit, "fmonero doctor --fix");
    };
    if *service_dir == p.wallet_dir {
        return Verdict::pass(format!("The wallet service uses {service_dir}."));
    }
    // Unbridgeable at call time: the daemon resolves <wallet-dir>/<filename> and a name may
    // not contain '/', so a one-level mismatch has `wallet create` writing into one
    // directory while `wallet list` reads the other (§4.2).
    Verdict::fail(
        "wallet_dir_mismatch",
        format!(
            "The wallet service looks for wallets in {service_dir} while wallet.dir says {}. A \
             wallet name cannot contain a '/', so nothing you type can bridge the two.",
            p.wallet_dir
        ),
    )
    .fixable(PlanKind::WriteWalletUnit, "fmonero doctor --fix")
}

fn wallet_port_consistent(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p, Which::Wallet) {
        return v;
    }
    let unit = p.unit_wallet_port;
    let url = p.settings_wallet_url_port;
    let setting = p.settings_wallet_port;
    let want = p.expected_wallet_port;
    if unit == Some(want) && url == Some(want) && setting == want {
        return Verdict::pass(format!(
            "Everything agrees that the wallet service answers on port {want}, which is the {} \
             one.",
            p.network
        ));
    }
    let observed =
        format!(
        "wallet.port says {setting}, wallet.url says {}, and the wallet service was set up for {}",
        url.map(|x| x.to_string()).unwrap_or_else(|| "nothing".into()),
        unit.map(|x| x.to_string()).unwrap_or_else(|| "nothing".into())
    );
    // Rebinding a live daemon's port from under it is how a running wallet becomes
    // unreachable mid-send, so the fixer is refused while the unit is active (§7.1).
    if p.wallet_unit_state.active {
        return Verdict::skip(
            "wallet_rpc_active",
            format!(
                "{observed}. On the {} network it has to be {want}. The wallet service is \
                 running, so fmonero changes nothing under it.",
                p.network
            ),
        )
        // V10: never `systemctl`. `W` stops the service from inside the app, and `fmonero doctor
        // --fix` then puts the port right.
        .cmd("fmonero tui   # on the Wallet screen, W stops the wallet service");
    }
    // node.network is the authority, not either observed value: a three-way disagreement
    // with no stated authority is a coin flip inside a class-B fixer (§7.1).
    Verdict::fail(
        "port_mismatch",
        format!(
            "{observed}. On the {} network {want} is the only right answer, and with the wrong \
             one fmonero looks for a password file the wallet service never wrote.",
            p.network
        ),
    )
    .fixable(PlanKind::NormaliseWalletPort, "fmonero doctor --fix")
}

// ------------------------------------------------------------------ storage

fn disk_space(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_monerod(p) {
        return v;
    }
    let Some(free) = p.free_bytes else {
        return Verdict::skip(
            "cannot_measure",
            p.free_bytes_error
                .clone()
                .unwrap_or_else(|| "Could not measure the free space on this disk.".into()),
        );
    };
    let gb = free as f64 / 1e9;
    let floor_gb = DISK_FLOOR_BYTES as f64 / 1e9;
    if free >= DISK_FLOOR_BYTES {
        return Verdict::pass(format!(
            "{gb:.0} GB free where {} lives. fmonero wants at least {floor_gb:.0} GB, which is an \
             estimate rather than a measurement.",
            p.data_dir
        ));
    }
    Verdict::warn(
        "low_disk",
        format!(
            "Only {gb:.0} GB free where {} lives. A smaller copy of the blockchain and room to \
             work needs about {floor_gb:.0} GB, which is an estimate rather than a measurement.",
            p.data_dir
        ),
    )
    .cmd("df -h ~")
}

fn prune_configured(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_monerod(p) {
        return v;
    }
    if !p.prune_setting {
        return Verdict::skip(
            "prune_off",
            "node.prune is false, so your node is meant to keep the whole blockchain.",
        );
    }
    match p.monerod_conf_prune {
        Some(true) => {
            Verdict::pass("Your node is set up to keep a smaller copy of the blockchain.")
        }
        Some(false) => Verdict::warn(
            "not_pruned",
            "node.prune is true, but your node was not set up to keep a smaller copy of the \
             blockchain. The whole chain does not fit on this machine.",
        )
        .cmd("fmonero setup --units --yes"),
        None => Verdict::skip(
            "no_conf",
            "Your node has not been set up on this machine yet.",
        )
        .cmd("fmonero setup --units --yes"),
    }
}

// ------------------------------------------------------------------ install

fn binary_on_path(p: &Probes) -> Verdict {
    if p.local_bin_exists {
        return Verdict::pass(format!("{} is there.", p.local_bin_fmonero));
    }
    Verdict::warn(
        "not_installed",
        format!(
            "{} is not there. The bar widget looks for fmonero in ~/.local/bin and nowhere \
             else, so it cannot find one installed anywhere different.",
            p.local_bin_fmonero
        ),
    )
    .cmd("scripts/install.sh --yes")
}

fn state_valid(p: &Probes) -> Verdict {
    if !p.state_file.exists {
        return Verdict::pass("fmonero has nothing remembered yet, which is fine.");
    }
    match &p.state_parse_error {
        None => Verdict::pass(format!("{} is readable.", p.state_file.path)),
        Some(e) => Verdict::warn(
            "unparsable",
            format!(
                "fmonero could not read what it remembered in {}. {e}",
                p.state_file.path
            ),
        )
        .fixable(PlanKind::ResetStateJson, "fmonero doctor --fix"),
    }
}

fn settings_valid(p: &Probes) -> Verdict {
    if !p.settings_file.exists {
        return Verdict::pass("You have changed no settings, so every one is at its default.");
    }
    if let Some(e) = &p.settings_parse_error {
        // The half that decides whether the one command that repairs config.toml can run at
        // all on a machine whose config.toml is broken.
        return Verdict::warn(
            "unparsable",
            format!(
                "fmonero could not read your settings file {}. {e}",
                p.settings_file.path
            ),
        )
        .fixable(PlanKind::RepairSettings, "fmonero doctor --fix");
    }
    let mut problems: Vec<String> = p
        .settings_problems
        .iter()
        .map(|(k, why)| format!("{k}: {why}"))
        .collect();
    for key in &p.settings_unknown_keys {
        problems.push(format!("{key} is not one of fmonero's settings"));
    }
    if problems.is_empty() {
        return Verdict::pass(format!(
            "{} is readable and every setting in it is one fmonero knows.",
            p.settings_file.path
        ));
    }
    Verdict::warn("invalid_keys", problems.join("; "))
        .fixable(PlanKind::RepairSettings, "fmonero doctor --fix")
}

/// §7.1, v5.5. **Never a `fail` in any state, and never fixable.** A stale list is served with
/// its age (§2n.6), a corrupt one is treated as STALE and refetched, so the code already recovers
/// from every condition a fixer would fix — the identical argument §7 gives for
/// `history_cache_fresh` being class D. And `skip:no_cache` rather than `warn` when the file is
/// absent: a machine that has never run `node list` is not faulty, and a warning here would fire
/// on every fresh install, which is the alarm §7 keeps saying people learn to ignore.
fn public_node_cache(p: &Probes) -> Verdict {
    let Some((age, count)) = p.public_nodes else {
        return Verdict::skip(
            "no_cache",
            "You have never fetched a list of public nodes, and nothing here needs one.",
        )
        .cmd(
            "fmonero node list --refresh   # fetches the list again, which tells monero.fail \
             your IP address",
        );
    };
    if age > fmonero_core::nodes::CACHE_MAX_AGE_SEC {
        return Verdict::pass(format!(
            "{count} public nodes, listed {}. fmonero shows the age with the list and \
             fetches a fresh one by itself after a day.",
            fmonero_core::age(age)
        ))
        .cmd(
            "fmonero node list --refresh   # fetches the list again, which tells monero.fail \
             your IP address",
        );
    }
    // W12: the shared formatter, so `5h 42m ago` here and on the screen — this printed
    // `listed 342 min ago` five and a half hours in.
    Verdict::pass(format!(
        "{count} public nodes, listed {}.",
        fmonero_core::age(age)
    ))
}

fn history_cache_fresh(p: &Probes) -> Verdict {
    if !p.cache_present {
        return Verdict::pass(
            "No saved copy of your payments yet. One is written the first time you look.",
        );
    }
    let Some(c) = &p.cache else {
        return Verdict::warn(
            "unreadable",
            "There is a saved copy of your payments, but fmonero could not read the start of it.",
        )
        .cmd("fmonero wallet history --limit 1   # writes the saved copy again");
    };
    // Which part did not match matters to a user even though §5b treats all three the same:
    // "corrupt or foreign file", "upgraded fmonero" and "this is another wallet's cache".
    if !c.magic_ok {
        return Verdict::warn(
            "bad_magic",
            "The saved copy of your payments does not begin the way fmonero's own files do: it \
             is damaged, or it belongs to another program. Nothing is lost: fmonero ignores it \
             and asks your wallet again.",
        )
        .cmd("fmonero wallet history --limit 1   # writes the saved copy again");
    }
    if c.format_version != c.expected_format_version {
        return Verdict::warn(
            "format_version",
            format!(
                "The saved copy of your payments was written by fmonero version {} and this one \
                 writes version {}. The old file cannot be read, and fmonero asks your wallet \
                 again instead.",
                c.format_version, c.expected_format_version
            ),
        )
        .cmd("fmonero wallet history --limit 1   # writes the saved copy again");
    }
    if c.fingerprint_matches == Some(false) {
        return Verdict::warn(
            "wallet_fingerprint",
            format!(
                "The saved copy of your payments ({}) belongs to a different wallet. fmonero \
                 checks that, so one wallet is never shown another's addresses, amounts and notes.",
                &c.wallet_fp[..8.min(c.wallet_fp.len())]
            ),
        )
        .cmd("fmonero wallet history --limit 1   # writes the saved copy again");
    }
    let Some(wh) = p.wallet_height else {
        return Verdict::skip(
            "no_wallet_height",
            // Not "no wallet is open": with `--only install` the wallet is never asked, so this
            // row cannot know whether one is (self-read, round 3 — the same shape as the two rows
            // that disagreed about it).
            format!(
                "The saved copy of your payments reaches block {}, and there is no wallet block \
                 number to compare it with.",
                crate::output::grouped(c.high_water)
            ),
        );
    };
    // **A high water of 0 means the snapshot holds no transfers**, not that it is `wh` blocks
    // stale: `high_water` is the greatest height any recorded transfer sits at, so a wallet
    // that has never received or sent anything has 0 there. "3 764 638 behind the wallet's
    // 3 764 638" is true subtraction and a false sentence, and the sentence is what a reader
    // acts on (§2j). The verdict is unchanged in both branches; only the words are.
    let empty = c.high_water == 0;
    let behind = wh.saturating_sub(c.high_water);
    if behind <= CACHE_STALE_BLOCKS {
        return Verdict::pass(if empty {
            format!(
                "The saved copy of your payments is empty and the wallet is at block {}: there \
                 is nothing to be behind.",
                crate::output::grouped(wh)
            )
        } else {
            format!(
                "The saved copy of your payments reaches block {} and the wallet is at {}: {} \
                 to go.",
                crate::output::grouped(c.high_water),
                crate::output::grouped(wh),
                crate::output::grouped(behind)
            )
        });
    }
    Verdict::warn(
        "stale",
        if empty {
            format!(
                "The saved copy of your payments holds none at all, while the wallet is at block \
                 {}. Looking at your payments fills it in by itself.",
                crate::output::grouped(wh)
            )
        } else {
            format!(
                "The saved copy of your payments reaches block {}, {} behind the wallet's {}. \
                 Looking at your payments tops it up by itself.",
                crate::output::grouped(c.high_water),
                crate::output::grouped(behind),
                crate::output::grouped(wh)
            )
        },
    )
    .cmd("fmonero wallet history --limit 1   # writes the saved copy again")
}

fn plugin_linked(p: &Probes) -> Verdict {
    let link = &p.plugin_link;
    let Some(want) = &link.want_target else {
        return Verdict::skip(
            "no_checkout",
            "fmonero cannot find its own copy of the bar widget, so there is nothing to check \
             the link against.",
        )
        .cmd("cd ~/Projects/odcpw/fmonero && fmonero setup --plugin --yes");
    };
    match (&link.resolved, link.exists) {
        (Some(resolved), _) if resolved == want => {
            Verdict::pass(format!("{} → {resolved}", link.path))
        }
        (Some(resolved), _) => Verdict::warn(
            "wrong_target",
            format!("{} points at {resolved} instead of {want}.", link.path),
        )
        .cmd("fmonero setup --plugin --yes"),
        (None, true) => Verdict::warn(
            "dangling",
            format!("{} points at something that is not there.", link.path),
        )
        .cmd("fmonero setup --plugin --yes"),
        (None, false) => Verdict::warn(
            "not_linked",
            format!(
                "{} is not there, so the bar widget is not installed.",
                link.path
            ),
        )
        .fixable(PlanKind::LinkPlugin, "fmonero doctor --fix"),
    }
}

fn plugin_enabled(p: &Probes) -> Verdict {
    if !p.omarchy_plugin_list.taken {
        return Verdict::skip("no_omarchy", "Omarchy is not installed on this machine.");
    }
    if !p.omarchy_plugin_list.success() {
        return Verdict::skip(
            "list_failed",
            format!(
                "Omarchy could not list its widgets. {}",
                p.omarchy_plugin_list.failure_detail()
            ),
        );
    }
    if p.omarchy_plugin_list
        .stdout
        .contains(super::probes::PLUGIN_ID)
    {
        return Verdict::pass(format!(
            "The bar widget is switched on ({}).",
            super::probes::PLUGIN_ID
        ));
    }
    Verdict::warn(
        "not_enabled",
        format!(
            "{} is not switched on in Omarchy, so the bar widget will not appear.",
            super::probes::PLUGIN_ID
        ),
    )
    .fixable(PlanKind::EnablePlugin, "fmonero doctor --fix")
}

fn menu_entries(p: &Probes) -> Verdict {
    if p.menu_missing_ids.is_empty() {
        return Verdict::pass("Every fmonero menu entry is present.");
    }
    Verdict::warn(
        "missing_ids",
        format!(
            "These fmonero entries are not in the Omarchy menu: {}.",
            p.menu_missing_ids.join(", ")
        ),
    )
    .fixable(PlanKind::MergeMenu, "fmonero doctor --fix")
}

// ------------------------------------------------------------------ security

fn monerod_conf_mode(p: &Probes) -> Verdict {
    if !p.monerod_conf.exists {
        return Verdict::skip(
            "no_conf",
            format!("{} has not been written yet.", p.monerod_conf.path),
        )
        .cmd("fmonero setup --units --yes");
    }
    match p.monerod_conf.mode {
        Some(0o600) => Verdict::pass(format!("Only you can read {}.", p.monerod_conf.path)),
        Some(m) => Verdict::fail(
            "bad_mode",
            format!(
                "{} is {m:04o}, so other people who use this machine can read it. It holds the \
                 password your node asks for, so only you may.",
                p.monerod_conf.path
            ),
        )
        .fixable(PlanKind::ChmodMonerodConf, "fmonero doctor --fix"),
        None => Verdict::skip("no_mode", "Who can read the file could not be worked out."),
    }
}

fn node_auth(p: &Probes) -> Verdict {
    // v5.2, §2m.3: no local daemon, no local chain, no local credential.
    if let Some(v) = need_local_node(p) {
        return v;
    }
    if let Some(v) = need_monerod(p) {
        return v;
    }
    if !p.node_reachable {
        return Verdict::skip(
            "node_unreachable",
            "Your node did not answer, so nothing was asked of it.",
        );
    }
    // What the WIRE did, never what a config file says: inferring this from
    // `Login::from_monerod_conf` returning Some would report `authenticated: true` against
    // a node that never challenged us, which is the false claim §2j forbids.
    match p.node_authenticated {
        Some(true) => Verdict::pass("Your node asked fmonero for its password and accepted it."),
        Some(false) if p.layout == "system_unit" => Verdict::skip(
            "foreign_node",
            "The node on this machine is not fmonero's, and whether it asks for a password is \
             not fmonero's to change.",
        ),
        Some(false) => Verdict::warn(
            "no_credential",
            "Your node answered fmonero without asking for a password. Any web page you visit \
             could ask it things too.",
        )
        .cmd("fmonero setup --units --yes"),
        None => Verdict::skip("no_request_completed", "Nothing was asked of it this run."),
    }
}

fn state_perms(p: &Probes) -> Verdict {
    let bad: Vec<&super::probes::PermProbe> = p.perms.iter().filter(|x| !x.ok()).collect();
    if bad.is_empty() {
        return Verdict::pass(format!(
            "All {} of fmonero's own files and directories can be read by you alone.",
            p.perms.len()
        ));
    }
    let daemon: Vec<String> = bad
        .iter()
        .filter(|x| x.daemon_owned)
        .map(|x| format!("{} can be read by others ({:04o})", x.path, x.mode))
        .collect();
    let ours: Vec<String> = bad
        .iter()
        .filter(|x| !x.daemon_owned)
        .map(|x| {
            format!(
                "{} can be read by others ({:04o}). It should be {:04o}.",
                x.path, x.mode, x.want
            )
        })
        .collect();
    if ours.is_empty() {
        // Never chmod'ed: the daemon recreates them on every start and
        // `StateDirectoryMode=0700` is the real fix (§7.1, class D).
        return Verdict::fail(
            "daemon_file_readable",
            format!(
                "{}. The wallet service writes these again every time it starts, so stopping and \
                 starting it is the fix rather than changing the files.",
                daemon.join("; ")
            ),
        )
        // V10: never `systemctl`. `W` stops the wallet service from inside the app and opening a
        // wallet starts it again.
        .cmd(
            "fmonero tui   # on the Wallet screen: W stops the wallet service, o starts it again",
        );
    }
    let mut detail = ours.join("; ");
    if !daemon.is_empty() {
        detail.push_str(&format!("; and {}", daemon.join("; ")));
    }
    Verdict::fail("bad_mode", detail).fixable(PlanKind::FixStatePerms, "fmonero doctor --fix")
}

fn ports_owned(p: &Probes) -> Verdict {
    if !p.ports_taken {
        return Verdict::skip("not_taken", "This group of checks was not asked for.");
    }
    if p.ports.is_empty() {
        // Name what was looked at, so this is distinguishable from a run against an empty
        // /proc — which §7.1's "our unit's PID **or nothing**" also makes a `pass`.
        return Verdict::pass(format!(
            "Nothing is answering on {}.",
            p.ports_watched
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
    let external: Vec<String> = p
        .ports
        .iter()
        .filter(|x| !x.loopback)
        .map(|x| {
            format!(
                "Port {} is open to {} by process {}",
                x.port,
                x.address,
                x.pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "unknown".into())
            )
        })
        .collect();
    if !external.is_empty() {
        return Verdict::fail(
            "external_bind",
            format!(
                "{}. fmonero's own services answer on this machine only.",
                external.join(". ")
            ),
        )
        .cmd("fmonero setup --units --yes");
    }
    let foreign: Vec<String> = p
        .ports
        .iter()
        .filter(|x| !x.ours)
        .map(|x| {
            format!(
                "Port {} on {} is held by process {}{}",
                x.port,
                x.address,
                x.pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                x.comm
                    .as_deref()
                    .map(|c| format!(" ({c})"))
                    .unwrap_or_default()
            )
        })
        .collect();
    if !foreign.is_empty() {
        // One sentence per port, and the qualifier as its own (self-read, round 3): joined with
        // `; ` it read as one sentence in which `which is not one of fmonero's own` attached to
        // the last port only, and §2o rule 1 has no semicolons in it.
        return Verdict::warn(
            "foreign_listener",
            format!(
                "{}. {}",
                foreign.join(". "),
                if foreign.len() == 1 {
                    "That is not one of fmonero's own."
                } else {
                    "None of those is one of fmonero's own."
                }
            ),
        )
        .cmd("fmonero status --json");
    }
    let ours: Vec<String> = p
        .ports
        .iter()
        .map(|x| {
            format!(
                "Port {} on {} is fmonero's own, process {}",
                x.port,
                x.address,
                x.pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "unknown".into())
            )
        })
        .collect();
    Verdict::pass(format!("{}.", ours.join(". ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5b rule 3, moved to this gate because `checks.rs` did not exist at the 1a gate (§8):
    /// the wire spelling of `Verdict`'s status is part of the contract.
    #[test]
    fn the_verdict_status_round_trips_through_its_documented_spelling() {
        for (value, wire) in [
            (CheckStatus::Pass, "pass"),
            (CheckStatus::Warn, "warn"),
            (CheckStatus::Fail, "fail"),
            (CheckStatus::Skip, "skip"),
        ] {
            let json = serde_json::to_string(&value).expect("serialize");
            assert_eq!(json, format!("\"{wire}\""));
            assert_eq!(value.name(), wire);
            let back: CheckStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, value);
        }
        for (value, wire) in [
            (Severity::Info, "info"),
            (Severity::Warn, "warn"),
            (Severity::Error, "error"),
        ] {
            assert_eq!(
                serde_json::to_string(&value).unwrap(),
                format!("\"{wire}\"")
            );
            assert_eq!(value.name(), wire);
        }
    }

    #[test]
    fn every_check_id_is_unique_and_every_group_has_members() {
        let mut ids: Vec<&str> = CHECKS.iter().map(|c| c.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "duplicate check id");
        for g in super::super::GROUPS {
            assert!(
                CHECKS.iter().any(|c| c.group == *g),
                "group {} has no checks",
                g.name()
            );
        }
    }

    /// The class table is the contract: a class-D check can never carry a plan (§7).
    #[test]
    fn no_class_d_check_can_name_a_plan() {
        let p = empty_probes();
        for def in CHECKS {
            if def.class != Class::D {
                continue;
            }
            let v = (def.detect)(&p);
            assert!(v.plan.is_none(), "{} named a plan", def.id);
        }
    }

    /// The table test §7.1 asks for, for the detectors that are one-line predicates. The
    /// seven that parse structured text are driven by `tests/fixtures/doctor/` instead.
    #[test]
    fn the_one_line_predicates_answer_as_the_table_says() {
        let mut p = empty_probes();

        // binaries: absent → fail:not_installed. The probe fields are cleared **explicitly**
        // rather than taken from the machine: Monero was installed here on 2026-09-17 (§1.1),
        // and a test that asserts a predicate by capturing the host asserts the host. This
        // failed on the pre-change tree for exactly that reason, and the fix is the assertion
        // this test always meant to make.
        p.which_monerod = None;
        p.which_wallet_rpc = None;
        let v = monerod_installed(&p);
        assert_eq!(v.status, CheckStatus::Fail);
        assert_eq!(v.reason, Some("not_installed"));
        assert_eq!(wallet_rpc_installed(&p).reason, Some("not_installed"));
        // …and present → pass, which is what this machine now answers (§1.1).
        p.which_monerod = Some("/usr/bin/monerod".into());
        assert_eq!(
            monerod_installed(&p).status,
            if p.monerod_version.success() {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            }
        );
        p.which_monerod = None;
        p.which_wallet_rpc = None;

        // …and every node/wallet/storage check skips behind them, never passes.
        for def in CHECKS.iter().filter(|c| {
            matches!(
                c.group,
                DoctorGroup::Node | DoctorGroup::Wallet | DoctorGroup::Storage
            )
        }) {
            let v = (def.detect)(&p);
            assert_eq!(v.status, CheckStatus::Skip, "{} did not skip", def.id);
        }

        // version_supported reads the first line of `monerod --version`.
        p.which_monerod = Some("/usr/bin/monerod".into());
        p.monerod_version.taken = true;
        p.monerod_version.code = Some(0);
        p.monerod_version.stdout = "Monero 'Fluorine Fermi' (v0.18.5.1-release)\n".into();
        assert_eq!(version_supported(&p).status, CheckStatus::Pass);
        p.monerod_version.stdout = "Monero 'Nitrogen Nebula' (v0.17.3.2-release)\n".into();
        assert_eq!(version_supported(&p).reason, Some("unsupported_version"));

        // node_clock is a pure comparison against the captured `now`.
        p.node_reachable = true;
        p.node_adjusted_time = Some(p.now_unix + 5);
        assert_eq!(node_clock(&p).status, CheckStatus::Pass);
        p.node_adjusted_time = Some(p.now_unix + CLOCK_SKEW_SEC + 1);
        assert_eq!(node_clock(&p).reason, Some("clock_skew"));

        // node_peers must SKIP rather than warn on a restricted node: it reports 0 by
        // construction, and a warn there is a false alarm on a healthy machine (§1b.3).
        p.node_outgoing_connections = Some(0);
        p.node_restricted = Some(true);
        assert_eq!(node_peers(&p).reason, Some("node_restricted"));
        p.node_restricted = Some(false);
        assert_eq!(node_peers(&p).reason, Some("no_peers"));
        p.node_outgoing_connections = Some(12);
        assert_eq!(node_peers(&p).status, CheckStatus::Pass);

        // disk_space measures locally and says the floor is an estimate.
        p.free_bytes = Some(DISK_FLOOR_BYTES + 1);
        let v = disk_space(&p);
        assert_eq!(v.status, CheckStatus::Pass);
        // v5.6, §2o: the sentence is plain, and "an estimate rather than a measurement" is the
        // same honesty §1.3 asked for in capitals.
        assert!(v.detail.contains("estimate"), "{}", v.detail);
        p.free_bytes = Some(DISK_FLOOR_BYTES - 1);
        assert_eq!(disk_space(&p).reason, Some("low_disk"));

        // binary_on_path.
        assert_eq!(binary_on_path(&p).reason, Some("not_installed"));
        p.local_bin_exists = true;
        assert_eq!(binary_on_path(&p).status, CheckStatus::Pass);

        // monerod_conf_mode: absent skips, 0600 passes, anything else fails and is fixable.
        assert_eq!(monerod_conf_mode(&p).reason, Some("no_conf"));
        p.monerod_conf.exists = true;
        p.monerod_conf.mode = Some(0o600);
        assert_eq!(monerod_conf_mode(&p).status, CheckStatus::Pass);
        p.monerod_conf.mode = Some(0o644);
        let v = monerod_conf_mode(&p);
        assert_eq!(v.status, CheckStatus::Fail);
        assert_eq!(v.plan, Some(PlanKind::ChmodMonerodConf));
    }

    /// `pending_send` is class D in **every** state, a fully-relayed record included: of all
    /// the doctor's checks it is the only one whose fixer could lose money (§7).
    #[test]
    fn a_pending_record_is_reported_and_never_fixed() {
        use fmonero_core::{PendingKind, PendingSend, PendingTx, Piconero};
        let mut p = empty_probes();
        p.which_wallet_rpc = Some("/usr/bin/monero-wallet-rpc".into());
        let tx = |relayed: bool| PendingTx {
            txid: "a".repeat(64),
            amount: Piconero(1),
            fee: Piconero(1),
            relayed,
            relayed_at: None,
        };
        p.pending = Some(PendingSend {
            kind: PendingKind::Send,
            account_index: 0,
            dest: "4addr".into(),
            built_at: 0,
            txs: vec![tx(false)],
        });
        let v = pending_send(&p);
        assert_eq!(v.status, CheckStatus::Fail);
        assert_eq!(v.reason, Some("send_may_be_in_flight"));
        assert!(v.plan.is_none(), "class D must never be fixable");

        p.pending.as_mut().unwrap().txs = vec![tx(true)];
        let v = pending_send(&p);
        assert_eq!(
            v.status,
            CheckStatus::Warn,
            "a relayed record resolves itself"
        );
        assert!(v.plan.is_none());
    }

    /// A `Probes` with nothing found: the state of this machine today (§1.1).
    pub(crate) fn empty_probes() -> Probes {
        let dir = std::env::temp_dir().join(format!("fmonero-doctor-empty-{}", std::process::id()));
        let ctx = fmonero_core::Context::for_test(&dir).expect("test context");
        let p = Probes::capture(&ctx, Some(DoctorGroup::Binaries));
        let _ = std::fs::remove_dir_all(&dir);
        p
    }
}
