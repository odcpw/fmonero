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
        title: "monerod is installed and answers --version",
        detect: monerod_installed,
    },
    CheckDef {
        id: "wallet_rpc_installed",
        group: DoctorGroup::Binaries,
        class: Class::D,
        severity: Severity::Error,
        title: "monero-wallet-rpc is installed and answers --version",
        detect: wallet_rpc_installed,
    },
    CheckDef {
        id: "version_supported",
        group: DoctorGroup::Binaries,
        class: Class::D,
        severity: Severity::Warn,
        title: "monerod is a 0.18.x build",
        detect: version_supported,
    },
    // ---- node ---------------------------------------------------------------
    CheckDef {
        id: "node_layout",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Info,
        title: "which monerod fmonero is talking to (§2i)",
        detect: node_layout,
    },
    CheckDef {
        id: "node_reachable",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Error,
        title: "the node answers get_info",
        detect: node_reachable,
    },
    CheckDef {
        id: "remote_node_untrusted",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Warn,
        title: "a remote node is not marked trusted unless it is private (§2m.6)",
        detect: remote_node_untrusted,
    },
    CheckDef {
        id: "node_unrestricted",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Warn,
        title: "the node is not in restricted-RPC mode",
        detect: node_unrestricted,
    },
    CheckDef {
        id: "node_synced",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Warn,
        title: "the node is synchronized",
        detect: node_synced,
    },
    CheckDef {
        id: "node_peers",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Warn,
        title: "the node has outgoing peers",
        detect: node_peers,
    },
    CheckDef {
        id: "node_clock",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Error,
        title: "this machine's clock agrees with the daemon's",
        detect: node_clock,
    },
    CheckDef {
        id: "node_data_dir_single_writer",
        group: DoctorGroup::Node,
        class: Class::D,
        severity: Severity::Error,
        title: "at most one monerod holds the data directory",
        detect: node_data_dir_single_writer,
    },
    // ---- wallet -------------------------------------------------------------
    CheckDef {
        id: "wallet_rpc_reachable",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Warn,
        title: "monero-wallet-rpc answers get_version",
        detect: wallet_rpc_reachable,
    },
    CheckDef {
        id: "wallet_rpc_auth",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Error,
        title: "the wallet daemon's login file is owner-only and works",
        detect: wallet_rpc_auth,
    },
    CheckDef {
        id: "wallet_daemon_link",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Warn,
        title: "the open wallet is close to the node's height",
        detect: wallet_daemon_link,
    },
    CheckDef {
        id: "wallet_daemon_auth",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Error,
        title: "the wallet has the node's rpc-login credential",
        detect: wallet_daemon_auth,
    },
    CheckDef {
        id: "wallet_nettype_matches",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Warn,
        title: "the open wallet's network is the configured one",
        detect: wallet_nettype_matches,
    },
    CheckDef {
        id: "wallet_dir_exists",
        group: DoctorGroup::Wallet,
        class: Class::B,
        severity: Severity::Warn,
        title: "wallet.dir and every ancestor exist at 0700",
        detect: wallet_dir_exists,
    },
    CheckDef {
        id: "pending_send",
        group: DoctorGroup::Wallet,
        class: Class::D,
        severity: Severity::Error,
        title: "no send is recorded as possibly in flight",
        detect: pending_send,
    },
    // ---- units --------------------------------------------------------------
    CheckDef {
        id: "units_present",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Warn,
        title: "both unit files exist and match units::render_*",
        detect: units_present,
    },
    CheckDef {
        id: "unit_loopback",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Error,
        title: "both units bind 127.0.0.1 only",
        detect: unit_loopback,
    },
    CheckDef {
        id: "unit_no_password_in_argv",
        group: DoctorGroup::Units,
        class: Class::D,
        severity: Severity::Error,
        title: "no credential flag in either ExecStart, and --log-level 0",
        detect: unit_no_password_in_argv,
    },
    CheckDef {
        id: "unit_no_node_wants",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Error,
        title: "no unit Wants= a monerod around the single-writer check",
        detect: unit_no_node_wants,
    },
    CheckDef {
        id: "unit_enabled_state",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Info,
        title: "both units are disabled (they are started on demand)",
        detect: unit_enabled_state,
    },
    CheckDef {
        id: "wallet_dir_matches_unit",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Error,
        title: "the unit's --wallet-dir is settings.wallet.dir, byte for byte",
        detect: wallet_dir_matches_unit,
    },
    CheckDef {
        id: "wallet_daemon_matches_unit",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Error,
        title: "the unit's --daemon-address, trust flag and --daemon-ssl are the ones in force",
        detect: wallet_daemon_matches_unit,
    },
    CheckDef {
        id: "wallet_port_consistent",
        group: DoctorGroup::Units,
        class: Class::B,
        severity: Severity::Error,
        title: "wallet.port, wallet.url and the unit's --rpc-bind-port agree",
        detect: wallet_port_consistent,
    },
    // ---- storage ------------------------------------------------------------
    CheckDef {
        id: "disk_space",
        group: DoctorGroup::Storage,
        class: Class::D,
        severity: Severity::Warn,
        title: "the data directory's filesystem has room for a pruned chain",
        detect: disk_space,
    },
    CheckDef {
        id: "prune_configured",
        group: DoctorGroup::Storage,
        class: Class::D,
        severity: Severity::Info,
        title: "monerod.conf prunes when settings.node.prune says so",
        detect: prune_configured,
    },
    // ---- install ------------------------------------------------------------
    CheckDef {
        id: "binary_on_path",
        group: DoctorGroup::Install,
        class: Class::D,
        severity: Severity::Warn,
        title: "~/.local/bin/fmonero exists (the shell's PATH has no ~/.cargo/bin)",
        detect: binary_on_path,
    },
    CheckDef {
        id: "state_valid",
        group: DoctorGroup::Install,
        class: Class::A,
        severity: Severity::Warn,
        title: "state.json parses",
        detect: state_valid,
    },
    CheckDef {
        id: "settings_valid",
        group: DoctorGroup::Install,
        class: Class::A,
        severity: Severity::Warn,
        title: "config.toml parses and every key is in the §4 table",
        detect: settings_valid,
    },
    CheckDef {
        id: "history_cache_fresh",
        group: DoctorGroup::Install,
        class: Class::D,
        severity: Severity::Info,
        title: "the history snapshot's header matches this wallet and this build",
        detect: history_cache_fresh,
    },
    CheckDef {
        id: "public_node_cache",
        group: DoctorGroup::Install,
        class: Class::D,
        severity: Severity::Info,
        title: "the public-node list is present and not a day old",
        detect: public_node_cache,
    },
    CheckDef {
        id: "plugin_linked",
        group: DoctorGroup::Install,
        class: Class::B,
        severity: Severity::Warn,
        title: "the bar widget is linked into ~/.config/omarchy/plugins",
        detect: plugin_linked,
    },
    CheckDef {
        id: "plugin_enabled",
        group: DoctorGroup::Install,
        class: Class::B,
        severity: Severity::Warn,
        title: "the bar widget is enabled in shell.json",
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
        title: "~/.config/fmonero/monerod.conf is 0600 (it carries rpc-login)",
        detect: monerod_conf_mode,
    },
    CheckDef {
        id: "node_auth",
        group: DoctorGroup::Security,
        class: Class::D,
        severity: Severity::Warn,
        title: "the node required and accepted a credential",
        detect: node_auth,
    },
    CheckDef {
        id: "state_perms",
        group: DoctorGroup::Security,
        class: Class::A,
        severity: Severity::Error,
        title: "everything fmonero writes is 0600, every directory 0700",
        detect: state_perms,
    },
    CheckDef {
        id: "ports_owned",
        group: DoctorGroup::Security,
        class: Class::D,
        severity: Severity::Warn,
        title: "who listens on the node and wallet ports",
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
            "monerod is not installed, so there is nothing to ask",
        )
        .cmd("omarchy pkg add monero")
    })
}

fn need_wallet_rpc(p: &Probes) -> Option<Verdict> {
    p.which_wallet_rpc.is_none().then(|| {
        Verdict::skip(
            "wallet_rpc_not_installed",
            "monero-wallet-rpc is not installed, so there is nothing to ask",
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
            format!(
                "node.mode is remote ({}): there is no local monerod to ask, and no wallet-RPC \
                 method reports a remote daemon's own state (§1b.9 item 5)",
                p.daemon_address
            ),
        )
        .cmd("fmonero node local --yes")
    })
}

fn need_units(p: &Probes) -> Option<Verdict> {
    (!p.node_unit.exists || !p.wallet_unit.exists).then(|| {
        Verdict::skip(
            "unit_missing",
            format!(
                "{} is not written yet",
                if p.node_unit.exists {
                    &p.wallet_unit_name
                } else {
                    &p.node_unit_name
                }
            ),
        )
        .cmd("fmonero setup --units --yes")
    })
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
                "monerod is not on PATH, and node.mode is remote, so nothing needs it; \
                 `fmonero node local --yes` is what would",
            )
            .cmd("omarchy pkg add monero");
        }
        return Verdict::fail("not_installed", "monerod is not on PATH")
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
    Verdict::pass(format!("{path} — {}", p.monerod_version.first_line()))
}

fn wallet_rpc_installed(p: &Probes) -> Verdict {
    let Some(path) = &p.which_wallet_rpc else {
        return Verdict::fail("not_installed", "monero-wallet-rpc is not on PATH")
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
    Verdict::pass(format!(
        "{path} — {}",
        p.wallet_rpc_version_cmd.first_line()
    ))
}

fn version_supported(p: &Probes) -> Verdict {
    if let Some(v) = need_monerod(p) {
        return v;
    }
    let line = p.monerod_version.first_line();
    if line.is_empty() {
        return Verdict::skip("no_version_line", "monerod --version printed nothing");
    }
    match version_of(&line) {
        Some(v) if v == SUPPORTED_MONERO => Verdict::pass(format!("{line} (0.18.x)")),
        Some(v) => Verdict::warn(
            "unsupported_version",
            format!("{line} is {v}.x; fmonero was built against {SUPPORTED_MONERO}.x (§1b)"),
        ),
        None => Verdict::warn(
            "unparsable_version",
            format!("cannot read a version out of '{line}'"),
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
            format!("cannot detect a layout: {e}"),
        );
    }
    match p.layout.as_str() {
        "user_unit" => Verdict::pass(format!(
            "user_unit — fmonero's own {} at {}",
            p.node_unit_name, p.node_url
        )),
        // §0: Arch's node is "detected, used read-only, never fought". That is a healthy
        // layout, not a defect — the node mutators are simply not ours to call (§2i).
        "system_unit" => Verdict::pass(
            "system_unit — Arch's monerod.service owns the node; fmonero uses it read-only, \
             never mutates it, and `node up`/`down`/`restart` exit 3 here (§2i)",
        )
        .cmd("sudo systemctl status monerod"),
        // v5.2, §2m.3: **the one `node` check that still runs in remote mode, and it passes**
        // — the group's only remaining source of truth, so that `doctor --only node` is never
        // an empty table. It was a `warn:non_loopback_url` while the layout was keyed on a
        // non-loopback `node.url`, which `settings set` refuses to store: a warning about a
        // state the tool could not enter. Now the mode is a supported configuration and the
        // detail names what is in force.
        "remote" => Verdict::pass(format!(
            "remote — monero-wallet-rpc talks to {} ({}, ssl {}); fmonero itself never does (§2c)",
            p.daemon_address,
            if p.daemon_trusted {
                "trusted"
            } else {
                "untrusted"
            },
            p.daemon_ssl
        ))
        .cmd("fmonero node local --yes"),
        _ => Verdict::warn(
            "no_layout",
            "none — no fmonero unit and no system monerod.service: run `fmonero setup --units`",
        )
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
            "{} answered get_info at height {}",
            p.node_url,
            p.node_height.unwrap_or_default()
        ));
    }
    Verdict::fail(
        "node_unreachable",
        format!(
            "{} did not answer: {}",
            p.node_url,
            p.node_error.as_deref().unwrap_or("no response")
        ),
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
        return Verdict::skip("node_unreachable", "the node did not answer get_info");
    }
    match p.node_restricted {
        Some(true) => Verdict::warn(
            "node_restricted",
            "restricted RPC: sync_info, get_connections, get_peer_list, get_bans, get_net_stats \
             and set_limit are not registered at all, so the peers, bans, spans and net-rate \
             panes have no data source (§1b.2)",
        )
        .cmd(if p.layout == "user_unit" {
            "fmonero setup --units --yes"
        } else {
            "sudo systemctl status monerod   # a restricted node is not ours to reconfigure"
        }),
        Some(false) => Verdict::pass("unrestricted: every node pane has a data source"),
        None => Verdict::skip("no_get_info", "get_info carried no `restricted` field"),
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
        return Verdict::skip("node_unreachable", "the node did not answer get_info");
    }
    let height = p.node_height.unwrap_or_default();
    let target = p.node_target_height.unwrap_or_default();
    match p.node_synchronized {
        Some(true) => Verdict::pass(format!("synchronized at height {height}")),
        Some(false) => {
            // target_height is 0 when synced, so the percentage special-cases it (§1b.3).
            let denom = target.max(height).max(1);
            let pct = (height as f64 / denom as f64) * 100.0;
            Verdict::warn(
                "node_syncing",
                format!("syncing: {height} of {denom} ({pct:.1}%)"),
            )
            .cmd("fmonero node up --wait --wait-timeout 86400")
        }
        None => Verdict::skip("no_get_info", "get_info carried no `synchronized` field"),
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
        return Verdict::skip("node_unreachable", "the node did not answer get_info");
    }
    // A restricted node reports 0 by construction (§1b.3), so a `warn` here would be a
    // false alarm on a perfectly healthy machine.
    if p.node_restricted == Some(true) {
        return Verdict::skip(
            "node_restricted",
            "a restricted node reports 0 connections by construction (§1b.3)",
        );
    }
    if p.node_offline == Some(true) {
        return Verdict::skip("node_offline", "monerod was started --offline");
    }
    match p.node_outgoing_connections {
        Some(0) => Verdict::warn("no_peers", "no outgoing connections")
            .cmd("fmonero node limit --out-peers 16"),
        Some(n) => Verdict::pass(format!("{n} outgoing connections")),
        None => Verdict::skip("no_get_info", "get_info carried no connection counts"),
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
        return Verdict::skip("node_unreachable", "the node did not answer get_info");
    };
    let skew = adjusted - p.now_unix;
    if skew.abs() <= CLOCK_SKEW_SEC {
        return Verdict::pass(format!(
            "clock within {skew}s of the daemon's adjusted time"
        ));
    }
    Verdict::fail(
        "clock_skew",
        format!(
            "this machine is {skew}s from the daemon's adjusted time (limit ±{CLOCK_SKEW_SEC}s); \
             a skewed clock breaks unlock times and peer handshakes"
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
        return Verdict::skip("probe_failed", format!("cannot probe {}: {e}", p.data_dir));
    }
    match p.data_dir_holder {
        None => Verdict::pass(format!("no monerod holds {}", p.data_dir)),
        // §7.1's fix column for this row IS "detail names the PID": there is no command.
        Some(pid) => Verdict::fail(
            "data_dir_in_use",
            format!(
                "monerod pid {pid} already has {} open; two monerod on one LMDB corrupt it (§1.2)",
                p.data_dir
            ),
        )
        .cmd("fmonero status --json   # a monerod is already serving this data dir"),
    }
}

// ------------------------------------------------------------------ wallet

fn wallet_rpc_reachable(p: &Probes) -> Verdict {
    if let Some(v) = need_wallet_rpc(p) {
        return v;
    }
    if p.wallet_reachable {
        return Verdict::pass(format!(
            "{} answered (RPC {}{})",
            p.wallet_url,
            p.wallet_rpc_version.as_deref().unwrap_or("?"),
            if p.wallet_open {
                ", a wallet is open"
            } else {
                ", no wallet open"
            }
        ));
    }
    Verdict::warn(
        "wallet_unreachable",
        format!(
            "{} did not answer: {}",
            p.wallet_url,
            p.wallet_error.as_deref().unwrap_or("no response")
        ),
    )
    .cmd("systemctl --user start fmonero-wallet-rpc.service")
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
                    "{} is absent while {} is active: fmonero looks for port {}, the unit binds {}",
                    p.wallet_login_file.path, p.wallet_unit_name, p.wallet_login_port, unit_port
                ),
            )
            .cmd("systemctl --user restart fmonero-wallet-rpc.service");
        }
        return Verdict::skip(
            "wallet_daemon_down",
            format!(
                "{} is absent, which is what it means for monero-wallet-rpc not to be running (§2c)",
                p.wallet_login_file.path
            ),
        )
        .cmd("systemctl --user start fmonero-wallet-rpc.service");
    }
    // Not a literal 0600: `private_file::create` settles the file at 0400, so §2c's rule is
    // "no group and no other bits" and stays right if upstream changes the dance.
    if !p.wallet_login_file.owner_only() {
        return Verdict::fail(
            "login_file_readable",
            format!(
                "{} is mode {:04o}: it must carry no group and no other permission bits (§2c)",
                p.wallet_login_file.path,
                p.wallet_login_file.mode.unwrap_or(0)
            ),
        )
        .cmd("systemctl --user restart fmonero-wallet-rpc.service");
    }
    match (p.wallet_reachable, p.wallet_authenticated) {
        (true, Some(true)) => Verdict::pass(format!(
            "{} is owner-only and get_version completed a Digest handshake",
            p.wallet_login_file.path
        )),
        (true, Some(false)) => Verdict::fail(
            "no_challenge",
            "the wallet daemon accepted an unauthenticated request: it was started with \
             --disable-rpc-login, which §2c forbids",
        )
        .cmd("fmonero setup --units --yes"),
        (true, None) => Verdict::skip("no_request_completed", "no request completed this run"),
        (false, _) => Verdict::skip(
            "wallet_unreachable",
            "the login file is owner-only, but the daemon did not answer",
        )
        .cmd("systemctl --user start fmonero-wallet-rpc.service"),
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
            "a wallet height and a node height are both needed to compare them",
        );
    };
    let behind = nh as i64 - wh as i64;
    if behind <= WALLET_BEHIND_BLOCKS {
        return Verdict::pass(format!("wallet at {wh}, node at {nh} ({behind} behind)"));
    }
    Verdict::warn(
        "wallet_behind",
        format!("wallet at {wh}, node at {nh}: {behind} blocks behind"),
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
            "our monerod.conf carries no rpc-login, so the wallet needs none",
        );
    }
    if !p.wallet_reachable || !p.node_reachable {
        return Verdict::skip(
            "daemon_unreachable",
            "both daemons must answer before this can mean anything",
        );
    }
    if p.node_synchronized != Some(true) {
        return Verdict::skip(
            "node_syncing",
            "the node is not synchronized, so a wallet behind it is expected",
        );
    }
    let (Some(wh), Some(nh)) = (p.wallet_height, p.node_height) else {
        return Verdict::skip(
            "no_heights",
            "no wallet is open, so there is no height to compare",
        );
    };
    if (wh as i64) + WALLET_BEHIND_BLOCKS < nh as i64 {
        return Verdict::fail(
            "wallet_not_syncing",
            format!(
                "the node is synchronized at {nh} and the wallet is at {wh}. A synced node with a \
                 wallet far behind is the signature of a missing daemon credential (§4.2) — but \
                 this check cannot distinguish it from a wallet still catching up on a large \
                 restore height"
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
    Verdict::pass(format!("wallet at {wh}, node synchronized at {nh}"))
}

fn wallet_nettype_matches(p: &Probes) -> Verdict {
    if let Some(v) = need_wallet_rpc(p) {
        return v;
    }
    let Some(net) = &p.last_open_nettype else {
        return Verdict::skip(
            "wallet_closed",
            "no wallet is open, so there is no nettype to read",
        );
    };
    if *net == p.network {
        return Verdict::pass(format!(
            "the open wallet {} is a {net} wallet, as node.network says",
            p.last_open_name.as_deref().unwrap_or("?")
        ));
    }
    // A warning, not a refusal: the send path follows the WALLET (§2a.6), so this is a
    // configuration fact — but it means wallet.dir, the ports and the node the wallet talks
    // to were all rendered for a different network.
    Verdict::warn(
        "nettype_mismatch",
        format!(
            "the open wallet is {net} while node.network is {}; wallet.dir, both ports and \
             monerod.conf were rendered for {}",
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
                "{} does not exist ({} missing: {})",
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
            format!("wallet directories must be 0700: {}", list.join(", ")),
        )
        .cmd(format!("chmod 700 {}", p.wallet_dir));
    }
    Verdict::pass(format!("{} exists at 0700", p.wallet_dir))
}

fn pending_send(p: &Probes) -> Verdict {
    if let Some(v) = need_wallet_rpc(p) {
        return v;
    }
    let Some(rec) = &p.pending else {
        return Verdict::pass("no send is recorded as unresolved");
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
                "{} transaction(s) to {} are all relayed; the next send resolves the record by \
                 itself (§2a.4)",
                rec.txs.len(),
                rec.dest
            ),
        )
        .cmd("fmonero wallet history --limit 5");
    }
    Verdict::fail(
        "send_may_be_in_flight",
        format!(
            "{} of {} transaction(s) to {} are NOT marked relayed: {}. Only a human can decide \
             whether they landed — the doctor never touches this record (§2a.4)",
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
    for (name, exists, matches) in [
        (&p.node_unit_name, p.node_unit.exists, p.node_unit_matches),
        (
            &p.wallet_unit_name,
            p.wallet_unit.exists,
            p.wallet_unit_matches,
        ),
    ] {
        if !exists {
            bad.push(format!("{name} is absent: run `fmonero setup --units`"));
        } else if !matches {
            bad.push(format!(
                "{name} has drifted from what `setup --units` renders"
            ));
        }
    }
    if bad.is_empty() {
        return Verdict::pass(format!(
            "{} and {} match units::render_* byte for byte",
            p.node_unit_name, p.wallet_unit_name
        ));
    }
    Verdict::warn("unit_drift", bad.join("; "))
        .fixable(PlanKind::WriteBothUnits, "fmonero doctor --fix")
}

fn unit_loopback(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p) {
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
                bad.push(format!("{name} binds {ip}"));
            }
        }
        if argv.iter().any(|a| a == "--confirm-external-bind") {
            bad.push(format!("{name} carries --confirm-external-bind"));
        }
    }
    if bad.is_empty() {
        return Verdict::pass("both units bind 127.0.0.1 and neither confirms an external bind");
    }
    Verdict::fail("non_loopback_bind", bad.join("; "))
        .fixable(PlanKind::WriteBothUnits, "fmonero doctor --fix")
}

fn unit_no_password_in_argv(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p) {
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
                bad.push(format!("{name} carries {flag}"));
            }
        }
        // The other half of §7.1's condition, and the half a detector forgets. An ABSENT
        // --log-level satisfies it: the node unit takes its level from the config file.
        if let Some(level) = super::probes::flag_value(&argv, "--log-level") {
            if level != "0" {
                bad.push(format!(
                    "{name} carries --log-level {level}: above 0 epee logs HTTP headers and \
                     wallet2 internals into the state dir (§4.2)"
                ));
            }
        }
    }
    if bad.is_empty() {
        return Verdict::pass("no credential flag in either ExecStart, and --log-level is 0");
    }
    Verdict::fail("credential_in_argv", bad.join("; "))
        .cmd("fmonero setup --units --yes   # rewrites both units from the template")
}

fn unit_no_node_wants(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p) {
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
                bad.push(format!("{name} has {t}"));
            }
        }
    }
    if bad.is_empty() {
        return Verdict::pass(
            "neither unit Wants= or Requires= a monerod: every node start goes through \
             `node up`, which runs the §2i single-writer check first",
        );
    }
    Verdict::fail(
        "node_wants",
        format!(
            "{} — Wants= would start a monerod around the single-writer check, and two \
             monerod on one LMDB corrupt it (§1.2, §4.2)",
            bad.join("; ")
        ),
    )
    .fixable(PlanKind::WriteBothUnits, "fmonero doctor --fix")
}

fn unit_enabled_state(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p) {
        return v;
    }
    let enabled: Vec<&str> = [&p.node_unit_state, &p.wallet_unit_state]
        .iter()
        .filter(|s| s.enabled)
        .map(|s| s.name.as_str())
        .collect();
    if enabled.is_empty() {
        return Verdict::pass("both units are disabled and are started on demand (§4.2)");
    }
    Verdict::warn(
        "unit_enabled",
        format!(
            "{} would start at login; fmonero's units are on-demand (§4.2)",
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
    if let Some(v) = need_units(p) {
        return v;
    }
    let Some(address) = &p.unit_daemon_address else {
        return Verdict::fail(
            "no_daemon_address_flag",
            format!(
                "{}'s ExecStart carries no --daemon-address",
                p.wallet_unit_name
            ),
        )
        .fixable(PlanKind::WriteWalletUnit, "fmonero doctor --fix");
    };
    if *address != p.daemon_address {
        return Verdict::fail(
            "daemon_address_mismatch",
            format!(
                "the unit says --daemon-address {address} while the settings say \
                 {}: a monero-wallet-rpc started before any `wallet open` refreshes against \
                 its argv, so the wallet would sync from a node `status` does not name (§2m.3)",
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
                "the unit's trust flag is {} while the settings say {}",
                match p.unit_daemon_trusted {
                    Some(true) => "--trusted-daemon",
                    Some(false) => "--untrusted-daemon",
                    None => "absent, which means neither and is not 'untrusted'",
                },
                if p.daemon_trusted {
                    "--trusted-daemon"
                } else {
                    "--untrusted-daemon"
                }
            ),
        )
        .fixable(PlanKind::WriteWalletUnit, "fmonero doctor --fix");
    }
    if p.unit_daemon_ssl.as_deref() != Some(p.daemon_ssl.as_str()) {
        return Verdict::fail(
            "daemon_ssl_mismatch",
            format!(
                "the unit says --daemon-ssl {} while the address asks for {}: an omitted or \
                 wrong value means a TLS handshake attempt on every reconnect (§1b.9 item 2)",
                p.unit_daemon_ssl
                    .clone()
                    .unwrap_or_else(|| "nothing".into()),
                p.daemon_ssl
            ),
        )
        .fixable(PlanKind::WriteWalletUnit, "fmonero doctor --fix");
    }
    Verdict::pass(format!(
        "--daemon-address {address} · {} · --daemon-ssl {}",
        if p.daemon_trusted {
            "--trusted-daemon"
        } else {
            "--untrusted-daemon"
        },
        p.daemon_ssl
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
            "node.mode is local: the trust flag is fmonero's own node's, and it is trusted",
        );
    }
    let host = fmonero_core::settings::remote_host(&p.daemon_address);
    if !p.daemon_trusted {
        return Verdict::pass(format!("{} is untrusted, which is the rule (§2m.6)", host));
    }
    if fmonero_core::settings::is_private_host(host) {
        return Verdict::pass(format!(
            "{host} is a private literal, which is the case --trusted exists for"
        ));
    }
    // A DNS name is never treated as private: classifying one means resolving it, a network
    // call inside a detector §7 requires to be pure, whose answer a name server can change
    // between runs. So a private node reached by name warns, and the detail says so.
    Verdict::warn(
        "trusted_public_node",
        format!(
            "{host} is marked trusted, so `select_available_outputs_from_histogram` puts this \
             wallet's own unspent amounts INSIDE its requests (`wallet2.cpp:12281-12282`). If \
             this node is yours, name it by its private address rather than by a DNS name — a \
             name is never treated as private, because resolving one is a network call"
        ),
    )
    .cmd("fmonero node remote <addr> --yes   # without --trusted")
}

fn wallet_dir_matches_unit(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p) {
        return v;
    }
    let Some(unit_dir) = &p.unit_wallet_dir else {
        return Verdict::fail(
            "no_wallet_dir_flag",
            format!("{}'s ExecStart carries no --wallet-dir", p.wallet_unit_name),
        )
        .fixable(PlanKind::WriteWalletUnit, "fmonero doctor --fix");
    };
    if *unit_dir == p.wallet_dir {
        return Verdict::pass(format!("--wallet-dir is {unit_dir}"));
    }
    // Unbridgeable at call time: the daemon resolves <wallet-dir>/<filename> and a name may
    // not contain '/', so a one-level mismatch has `wallet create` writing into one
    // directory while `wallet list` reads the other (§4.2).
    Verdict::fail(
        "wallet_dir_mismatch",
        format!(
            "the unit says {unit_dir} while settings.wallet.dir says {}: the daemon resolves \
             <wallet-dir>/<name> and a name may not contain '/', so the two can never meet (§4.2)",
            p.wallet_dir
        ),
    )
    .fixable(PlanKind::WriteWalletUnit, "fmonero doctor --fix")
}

fn wallet_port_consistent(p: &Probes) -> Verdict {
    if let Some(v) = need_units(p) {
        return v;
    }
    let unit = p.unit_wallet_port;
    let url = p.settings_wallet_url_port;
    let setting = p.settings_wallet_port;
    let want = p.expected_wallet_port;
    if unit == Some(want) && url == Some(want) && setting == want {
        return Verdict::pass(format!(
            "wallet.port, wallet.url and --rpc-bind-port all say {want} ({} )",
            p.network
        ));
    }
    let observed = format!(
        "settings.wallet.port {setting}, wallet.url {}, unit --rpc-bind-port {}",
        url.map(|x| x.to_string()).unwrap_or_else(|| "none".into()),
        unit.map(|x| x.to_string()).unwrap_or_else(|| "none".into())
    );
    // Rebinding a live daemon's port from under it is how a running wallet becomes
    // unreachable mid-send, so the fixer is refused while the unit is active (§7.1).
    if p.wallet_unit_state.active {
        return Verdict::skip(
            "wallet_rpc_active",
            format!(
                "{observed}; node.network {} wants {want}. The wallet daemon is running, so \
                 nothing is rewritten",
                p.network
            ),
        )
        .cmd("systemctl --user stop fmonero-wallet-rpc.service");
    }
    // node.network is the authority, not either observed value: a three-way disagreement
    // with no stated authority is a coin flip inside a class-B fixer (§7.1).
    Verdict::fail(
        "port_mismatch",
        format!(
            "{observed}; node.network {} makes {want} the only right answer, and \
             Paths::wallet_login_file(port) looks for a file the daemon never wrote otherwise",
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
                .unwrap_or_else(|| "statvfs failed".into()),
        );
    };
    let gb = free as f64 / 1e9;
    let floor_gb = DISK_FLOOR_BYTES as f64 / 1e9;
    if free >= DISK_FLOOR_BYTES {
        return Verdict::pass(format!(
            "{gb:.0} GB free on {}'s filesystem (floor {floor_gb:.0} GB — an ESTIMATE, AMBER: \
             §1.3 measured no chain on this machine)",
            p.data_dir
        ));
    }
    Verdict::warn(
        "low_disk",
        format!(
            "{gb:.0} GB free on {}'s filesystem, under the {floor_gb:.0} GB floor for a pruned \
             chain plus headroom. The floor is an ESTIMATE (AMBER, §1.3), not a measurement",
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
            "settings.node.prune is false, so monerod.conf is expected to carry no prune line",
        );
    }
    match p.monerod_conf_prune {
        Some(true) => Verdict::pass("monerod.conf carries prune-blockchain=1"),
        Some(false) => Verdict::warn(
            "not_pruned",
            "settings.node.prune is true but monerod.conf has no prune-blockchain=1: a full \
             chain does not fit this disk (§1.3)",
        )
        .cmd("fmonero setup --units --yes"),
        None => Verdict::skip(
            "no_conf",
            "~/.config/fmonero/monerod.conf does not exist yet",
        )
        .cmd("fmonero setup --units --yes"),
    }
}

// ------------------------------------------------------------------ install

fn binary_on_path(p: &Probes) -> Verdict {
    if p.local_bin_exists {
        return Verdict::pass(format!("{} exists", p.local_bin_fmonero));
    }
    Verdict::warn(
        "not_installed",
        format!(
            "{} does not exist. The Omarchy shell's PATH includes ~/.local/bin but NOT \
             ~/.cargo/bin (§1.5), so the bar widget cannot find a cargo-installed binary",
            p.local_bin_fmonero
        ),
    )
    .cmd("scripts/install.sh --yes")
}

fn state_valid(p: &Probes) -> Verdict {
    if !p.state_file.exists {
        return Verdict::pass("state.json does not exist yet, which is a valid empty state");
    }
    match &p.state_parse_error {
        None => Verdict::pass(format!("{} parses", p.state_file.path)),
        Some(e) => Verdict::warn(
            "unparsable",
            format!("{} does not parse: {e}", p.state_file.path),
        )
        .fixable(PlanKind::ResetStateJson, "fmonero doctor --fix"),
    }
}

fn settings_valid(p: &Probes) -> Verdict {
    if !p.settings_file.exists {
        return Verdict::pass("config.toml does not exist yet; every key is at its default");
    }
    if let Some(e) = &p.settings_parse_error {
        // The half that decides whether the one command that repairs config.toml can run at
        // all on a machine whose config.toml is broken.
        return Verdict::warn(
            "unparsable",
            format!("{} is not valid TOML: {e}", p.settings_file.path),
        )
        .fixable(PlanKind::RepairSettings, "fmonero doctor --fix");
    }
    let mut problems: Vec<String> = p
        .settings_problems
        .iter()
        .map(|(k, why)| format!("{k}: {why}"))
        .collect();
    for key in &p.settings_unknown_keys {
        problems.push(format!("{key} is not a settings key (§4)"));
    }
    if problems.is_empty() {
        return Verdict::pass(format!(
            "{} parses and every key is in the §4 table",
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
            "no public-node list has been fetched; nothing needs one",
        )
        .cmd(
            "fmonero node list --refresh   # rebuilds the cache (this contacts monero.fail, §2n.5)",
        );
    };
    if age > fmonero_core::nodes::CACHE_MAX_AGE_SEC {
        return Verdict::pass(format!(
            "{count} nodes, {}h old — served with its age, and `node list` refetches on its own              past 24 h (§2n.6)",
            age / 3600
        ))
        .cmd("fmonero node list --refresh   # rebuilds the cache (this contacts monero.fail, §2n.5)");
    }
    Verdict::pass(format!("{count} nodes, {} min old", age / 60))
}

fn history_cache_fresh(p: &Probes) -> Verdict {
    if !p.cache_present {
        return Verdict::pass("no history snapshot yet; `wallet history` builds one");
    }
    let Some(c) = &p.cache else {
        return Verdict::warn(
            "unreadable",
            "history.bin exists but its header could not be read",
        )
        .cmd("fmonero wallet history --limit 1   # rebuilds the snapshot");
    };
    // Which part did not match matters to a user even though §5b treats all three the same:
    // "corrupt or foreign file", "upgraded fmonero" and "this is another wallet's cache".
    if !c.magic_ok {
        return Verdict::warn(
            "bad_magic",
            "the snapshot's magic is not FMHX: the file is corrupt or belongs to another tool. \
             It is STALE, not an error — `wallet history` ignores it and refetches (§5b)",
        )
        .cmd("fmonero wallet history --limit 1   # rebuilds the snapshot");
    }
    if c.format_version != c.expected_format_version {
        return Verdict::warn(
            "format_version",
            format!(
                "the snapshot is format {} and this build writes {}: postcard is not \
                 self-describing, so the body's layout changed and the old bytes cannot be decoded",
                c.format_version, c.expected_format_version
            ),
        )
        .cmd("fmonero wallet history --limit 1   # rebuilds the snapshot");
    }
    if c.fingerprint_matches == Some(false) {
        return Verdict::warn(
            "wallet_fingerprint",
            format!(
                "the snapshot's fingerprint {} is another wallet's; without it, opening wallet B \
                 would serve wallet A's addresses, amounts and notes (§5b)",
                &c.wallet_fp[..8.min(c.wallet_fp.len())]
            ),
        )
        .cmd("fmonero wallet history --limit 1   # rebuilds the snapshot");
    }
    let Some(wh) = p.wallet_height else {
        return Verdict::skip(
            "no_wallet_height",
            format!(
                "the header is valid (high water {}), but no wallet height is available to \
                 measure staleness against",
                c.high_water
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
            format!("the snapshot is empty and the wallet is at {wh}: nothing to be behind")
        } else {
            format!(
                "high water {} against a wallet at {wh} ({behind} behind)",
                c.high_water
            )
        });
    }
    Verdict::warn(
        "stale",
        if empty {
            format!(
                "the snapshot holds no transfers (high water 0) while the wallet is at {wh}. \
                 The next `wallet history` without --cached rebuilds it by itself (§5b)"
            )
        } else {
            format!(
                "high water {}, {behind} behind the wallet's {wh}. The next `wallet history` \
                 without --cached refetches from high_water - 720 by itself (§5b)",
                c.high_water
            )
        },
    )
    .cmd("fmonero wallet history --limit 1   # rebuilds the snapshot")
}

fn plugin_linked(p: &Probes) -> Verdict {
    let link = &p.plugin_link;
    let Some(want) = &link.want_target else {
        return Verdict::skip(
            "no_checkout",
            "cannot find this repository's plugin/oliver.monero, so there is nothing to \
             validate the link against",
        )
        .cmd("cd ~/Projects/odcpw/fmonero && fmonero setup --plugin --yes");
    };
    match (&link.resolved, link.exists) {
        (Some(resolved), _) if resolved == want => {
            Verdict::pass(format!("{} → {resolved}", link.path))
        }
        (Some(resolved), _) => Verdict::warn(
            "wrong_target",
            format!("{} resolves to {resolved}, not to {want}", link.path),
        )
        .cmd("fmonero setup --plugin --yes"),
        (None, true) => Verdict::warn(
            "dangling",
            format!("{} exists but does not resolve", link.path),
        )
        .cmd("fmonero setup --plugin --yes"),
        (None, false) => Verdict::warn(
            "not_linked",
            format!(
                "{} does not exist; the bar widget is not installed",
                link.path
            ),
        )
        .fixable(PlanKind::LinkPlugin, "fmonero doctor --fix"),
    }
}

fn plugin_enabled(p: &Probes) -> Verdict {
    if !p.omarchy_plugin_list.taken {
        return Verdict::skip("no_omarchy", "omarchy is not on PATH");
    }
    if !p.omarchy_plugin_list.success() {
        return Verdict::skip(
            "list_failed",
            format!(
                "`omarchy plugin list` failed: {}",
                p.omarchy_plugin_list.failure_detail()
            ),
        );
    }
    if p.omarchy_plugin_list
        .stdout
        .contains(super::probes::PLUGIN_ID)
    {
        return Verdict::pass(format!(
            "{} is listed by `omarchy plugin list`",
            super::probes::PLUGIN_ID
        ));
    }
    Verdict::warn(
        "not_enabled",
        format!(
            "{} is not in shell.json: the bar widget will not load",
            super::probes::PLUGIN_ID
        ),
    )
    .fixable(PlanKind::EnablePlugin, "fmonero doctor --fix")
}

fn menu_entries(p: &Probes) -> Verdict {
    if p.menu_missing_ids.is_empty() {
        return Verdict::pass("every fmonero menu id is present");
    }
    Verdict::warn(
        "missing_ids",
        format!(
            "missing from the Omarchy menu: {}",
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
            format!("{} does not exist yet", p.monerod_conf.path),
        )
        .cmd("fmonero setup --units --yes");
    }
    match p.monerod_conf.mode {
        Some(0o600) => Verdict::pass(format!("{} is 0600", p.monerod_conf.path)),
        Some(m) => Verdict::fail(
            "bad_mode",
            format!(
                "{} is {m:04o}: it carries rpc-login and must be 0600 (§2c)",
                p.monerod_conf.path
            ),
        )
        .fixable(PlanKind::ChmodMonerodConf, "fmonero doctor --fix"),
        None => Verdict::skip("no_mode", "the file's mode could not be read"),
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
            "the node did not answer, so no handshake happened",
        );
    }
    // What the WIRE did, never what a config file says: inferring this from
    // `Login::from_monerod_conf` returning Some would report `authenticated: true` against
    // a node that never challenged us, which is the false claim §2j forbids.
    match p.node_authenticated {
        Some(true) => Verdict::pass("the node challenged us and accepted the credential"),
        Some(false) if p.layout == "system_unit" => Verdict::skip(
            "foreign_node",
            "Arch's monerod.service has no rpc-login, which is not ours to change (§2c)",
        ),
        Some(false) => Verdict::warn(
            "no_credential",
            "the node accepted us with no credential at all: any web page the user visits can \
             POST to this port (§2c)",
        )
        .cmd("fmonero setup --units --yes"),
        None => Verdict::skip("no_request_completed", "no request completed this run"),
    }
}

fn state_perms(p: &Probes) -> Verdict {
    let bad: Vec<&super::probes::PermProbe> = p.perms.iter().filter(|x| !x.ok()).collect();
    if bad.is_empty() {
        return Verdict::pass(format!(
            "{} files and directories under the state, config and cache directories are \
             owner-only",
            p.perms.len()
        ));
    }
    let daemon: Vec<String> = bad
        .iter()
        .filter(|x| x.daemon_owned)
        .map(|x| format!("{} is {:04o}", x.path, x.mode))
        .collect();
    let ours: Vec<String> = bad
        .iter()
        .filter(|x| !x.daemon_owned)
        .map(|x| format!("{} is {:04o}, want {:04o}", x.path, x.mode, x.want))
        .collect();
    if ours.is_empty() {
        // Never chmod'ed: the daemon recreates them on every start and
        // `StateDirectoryMode=0700` is the real fix (§7.1, class D).
        return Verdict::fail(
            "daemon_file_readable",
            format!(
                "{} — the wallet daemon recreates these on every start, so restarting the unit \
                 is the fix, not chmod (§4.2)",
                daemon.join("; ")
            ),
        )
        .cmd("systemctl --user restart fmonero-wallet-rpc.service");
    }
    let mut detail = ours.join("; ");
    if !daemon.is_empty() {
        detail.push_str(&format!("; and {}", daemon.join("; ")));
    }
    Verdict::fail("bad_mode", detail).fixable(PlanKind::FixStatePerms, "fmonero doctor --fix")
}

fn ports_owned(p: &Probes) -> Verdict {
    if !p.ports_taken {
        return Verdict::skip("not_taken", "the security group was not selected");
    }
    if p.ports.is_empty() {
        // Name what was looked at, so this is distinguishable from a run against an empty
        // /proc — which §7.1's "our unit's PID **or nothing**" also makes a `pass`.
        return Verdict::pass(format!(
            "nothing listens on {}",
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
                "port {} is bound to {} by pid {}",
                x.port,
                x.address,
                x.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into())
            )
        })
        .collect();
    if !external.is_empty() {
        return Verdict::fail(
            "external_bind",
            format!(
                "{} — fmonero's daemons bind 127.0.0.1 only (§2c)",
                external.join("; ")
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
                "port {} on {} is held by pid {}{}",
                x.port,
                x.address,
                x.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
                x.comm
                    .as_deref()
                    .map(|c| format!(" ({c})"))
                    .unwrap_or_default()
            )
        })
        .collect();
    if !foreign.is_empty() {
        return Verdict::warn(
            "foreign_listener",
            format!("{} — not one of fmonero's own units", foreign.join("; ")),
        )
        .cmd("fmonero status --json");
    }
    let ours: Vec<String> = p
        .ports
        .iter()
        .map(|x| {
            format!(
                "port {} on {} is our pid {}",
                x.port,
                x.address,
                x.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into())
            )
        })
        .collect();
    Verdict::pass(ours.join("; "))
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
        assert!(v.detail.contains("ESTIMATE"), "{}", v.detail);
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
