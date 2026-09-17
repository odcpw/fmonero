//! The doctor's two outputs: the §7 `report.json` document (also what `--json` prints) and
//! the human table. `doctor capabilities` lives here too, because it describes exactly the
//! table [`CHECKS`](super::checks::CHECKS) declares — generated from that registry, never
//! hand-maintained, so the two cannot drift.
//!
//! **The top-level key set is §5c's, exactly.** §5c's `doctor` row is "the §7 object", and
//! §7 spells that `{ok, run_id, checks:[…], summary}`; `doctor capabilities` is `checks`,
//! `groups`, `classes`. Everything else a reader needs is nested inside those keys — the
//! per-check `actions` array carries `writes_to`, and `summary` carries the run metadata —
//! because §5c's gate asserts the top level and nothing below it.

use fmonero_core::{Context, Error};
use serde::Serialize;

use super::checks::{CheckDef, Class, PlanKind, Verdict, CHECKS};
use super::mutate::{write_scopes, PlannedAction, KEEP_RUNS};
use super::{DoctorGroup, GROUPS};
use crate::output::{Out, Paint};

/// One row of `report.json::checks` (§7).
#[derive(Clone, Debug, Serialize)]
pub struct CheckReport {
    pub id: String,
    pub group: String,
    pub severity: String,
    pub status: String,
    /// The short machine reason behind a non-pass; `null` on a pass.
    pub reason: Option<String>,
    /// `fail:not_installed` — status and reason joined, the way the table prints it.
    pub verdict: String,
    pub detail: String,
    pub fixable: bool,
    pub class: String,
    pub fix_cmd: Option<String>,
    /// What `--fix` did with this check, when it ran.
    pub fix_outcome: Option<String>,
    pub fix_error: Option<String>,
    /// The mutations this check planned or applied, each with the paths it writes to — the
    /// blast radius §7 wants disclosed in `--dry-run`.
    pub actions: Vec<PlannedAction>,
}

impl CheckReport {
    pub fn new(def: &CheckDef, v: &Verdict) -> CheckReport {
        CheckReport {
            id: def.id.to_string(),
            group: def.group.name().to_string(),
            severity: def.severity.name().to_string(),
            status: v.status.name().to_string(),
            reason: v.reason.map(str::to_string),
            verdict: match v.reason {
                Some(r) => format!("{}:{r}", v.status.name()),
                None => v.status.name().to_string(),
            },
            detail: v.detail.clone(),
            fixable: v.plan.is_some(),
            class: def.class.name().to_string(),
            fix_cmd: v.fix_cmd.clone(),
            fix_outcome: None,
            fix_error: None,
            actions: Vec::new(),
        }
    }

    /// Re-state the row from a fresh verdict, after a fixer changed the world (§7).
    pub fn restate(&mut self, v: &Verdict) {
        self.status = v.status.name().to_string();
        self.reason = v.reason.map(str::to_string);
        self.verdict = match v.reason {
            Some(r) => format!("{}:{r}", v.status.name()),
            None => v.status.name().to_string(),
        };
        self.detail = v.detail.clone();
        self.fixable = v.plan.is_some();
        self.fix_cmd = v.fix_cmd.clone();
    }
}

/// `report.json` in full — and exactly §5c's top-level key set for `doctor`.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub ok: bool,
    pub schema: u32,
    pub run_id: Option<String>,
    pub checks: Vec<CheckReport>,
    pub summary: Summary,
}

/// The run's counts **and** its metadata. §5c gives `doctor` four keys plus the envelope, so
/// "which run was this, and what did it do" lives here rather than at the top level.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    pub total: usize,
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
    pub skip: usize,
    pub fixable: usize,
    pub fixed: usize,
    pub fix_failed: usize,
    pub planned: usize,
    pub generated_at: String,
    pub fix: bool,
    pub dry_run: bool,
    pub only: Option<String>,
    pub run_dir: Option<String>,
    /// The ids that are `fail`, so a reader of `report.json` alone knows what this machine
    /// looked like without re-deriving it from every row.
    pub failing: Vec<String>,
    pub exit_code: i32,
    /// The next moves, as runnable commands (Axiom 0's second question).
    pub next: Vec<String>,
}

impl Summary {
    pub fn of(checks: &[CheckReport], actions: &[&PlannedAction]) -> Summary {
        let count = |s: &str| checks.iter().filter(|c| c.status == s).count();
        Summary {
            total: checks.len(),
            pass: count("pass"),
            warn: count("warn"),
            fail: count("fail"),
            skip: count("skip"),
            fixable: checks.iter().filter(|c| c.fixable).count(),
            fixed: checks
                .iter()
                .filter(|c| c.fix_outcome.as_deref() == Some("fixed"))
                .count(),
            fix_failed: actions.iter().filter(|a| a.outcome == "failed").count(),
            planned: actions.iter().filter(|a| a.outcome == "planned").count(),
            failing: checks
                .iter()
                .filter(|c| c.status == "fail")
                .map(|c| c.id.clone())
                .collect(),
            ..Summary::default()
        }
    }
}

/// The §4a exit code this run implies (§7).
///
/// A run **without** `--fix` exits **11** if any check is `fail`, else 0. A run **with**
/// `--fix` exits **12** if any attempted fixer failed or fixed only part; **11** if every
/// fixer succeeded but `fail` checks remain that are not fixable; **0** only when no `fail`
/// remains. Warnings alone never fail a run.
pub fn exit_code(summary: &Summary) -> i32 {
    if summary.fix && summary.fix_failed > 0 {
        return 12;
    }
    if summary.fail > 0 {
        return 11;
    }
    0
}

/// The human table (§7). One line per check, then the counts and the next moves.
pub fn human(out: &Out, report: &Report) {
    let mut group = String::new();
    let width = report
        .checks
        .iter()
        .map(|c| c.id.len())
        .max()
        .unwrap_or(28)
        .max(28);
    for check in &report.checks {
        if check.group != group {
            group = check.group.clone();
            out.line(out.paint(&format!("[{group}]"), Paint::Dim));
        }
        let tag = match check.status.as_str() {
            "pass" => out.paint("pass", Paint::Green),
            "warn" => out.paint("warn", Paint::Yellow),
            "fail" => out.paint("fail", Paint::Red),
            _ => out.paint("skip", Paint::Dim),
        };
        let reason = match &check.reason {
            Some(r) if check.status != "pass" => format!(":{r}"),
            _ => String::new(),
        };
        out.line(format!(
            "  {tag}{reason:<26} {:<width$}  {}",
            check.id, check.detail
        ));
        if let Some(cmd) = &check.fix_cmd {
            if check.status != "pass" {
                out.line(format!(
                    "{:33}{:<width$}  {}",
                    "",
                    "",
                    out.paint(&format!("fix: {cmd}"), Paint::Dim)
                ));
            }
        }
        if let Some(outcome) = &check.fix_outcome {
            out.line(format!(
                "{:33}{:<width$}  {}",
                "",
                "",
                out.paint(&format!("--fix: {outcome}"), Paint::Dim)
            ));
        }
        for action in &check.actions {
            let tag = match action.outcome.as_str() {
                "applied" => out.paint("applied", Paint::Green),
                "already" => out.paint("already", Paint::Dim),
                "planned" => out.paint("would  ", Paint::Yellow),
                _ => out.paint("failed ", Paint::Red),
            };
            out.line(format!(
                "{:33}{:<width$}  {tag} [{}] {}",
                "", "", action.class, action.action
            ));
            if let Some(e) = &action.error {
                out.line(format!(
                    "{:33}{:<width$}  {}",
                    "",
                    "",
                    out.paint(e, Paint::Red)
                ));
            }
        }
    }

    let s = &report.summary;
    out.line("");
    out.line(format!(
        "{} checks: {} pass, {} warn, {} fail, {} skip",
        s.total, s.pass, s.warn, s.fail, s.skip
    ));
    if s.dry_run && s.planned > 0 {
        let paths: Vec<String> = report
            .checks
            .iter()
            .flat_map(|c| c.actions.iter())
            .flat_map(|a| a.writes_to.iter())
            .cloned()
            .collect();
        if !paths.is_empty() {
            out.line(out.paint(&format!("blast radius: {}", paths.join(", ")), Paint::Dim));
        }
    }
    if let Some(dir) = &s.run_dir {
        out.line(out.paint(&format!("run artifacts: {dir}"), Paint::Dim));
    }
    if !s.next.is_empty() {
        out.line("");
        for cmd in &s.next {
            out.line(format!("  next: {cmd}"));
        }
    }
}

/// `fmonero doctor capabilities --json` (§7, §5c): every check, group and class.
pub fn capabilities(ctx: &Context) -> serde_json::Value {
    let checks: Vec<serde_json::Value> = CHECKS
        .iter()
        .map(|c| {
            let plan = fixable_plan(c);
            serde_json::json!({
                "id": c.id,
                "group": c.group.name(),
                "class": c.class.name(),
                "severity": c.severity.name(),
                "title": c.title,
                "auto_fixable": plan.is_some(),
                "plan": plan.map(PlanKind::name),
                "statuses": ["pass", "warn", "fail", "skip"],
            })
        })
        .collect();
    serde_json::json!({
        "ok": true,
        "schema": fmonero_core::SCHEMA_VERSION,
        "checks": checks,
        "groups": GROUPS.iter().map(|g| serde_json::json!({
            "group": g.name(),
            "checks": CHECKS.iter().filter(|c| c.group == *g).count(),
            // Every `node`, `wallet` and `storage` check reports `skip:<reason>` when its
            // daemon's binary is absent — §4b's GREEN row for a machine with no Monero.
            "needs": match g {
                DoctorGroup::Node | DoctorGroup::Storage => "monerod",
                DoctorGroup::Wallet => "monero-wallet-rpc",
                _ => "nothing",
            },
        })).collect::<Vec<_>>(),
        "classes": ([Class::A, Class::B, Class::D]).iter().map(|c| serde_json::json!({
            "class": c.name(),
            "meaning": c.describes(),
            "runs_under": match c {
                Class::D => serde_json::Value::Null,
                _ => serde_json::json!("--fix"),
            },
            "members": CHECKS.iter().filter(|x| x.class == *c)
                .map(|x| x.id).collect::<Vec<_>>(),
            "write_scopes": match c {
                Class::D => Vec::new(),
                _ => write_scopes(ctx).iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            },
            "never_does": match c {
                Class::D => vec![
                    "anything: a class D finding is reported and the detail names the command",
                ],
                _ => vec![
                    "remove a file: a file a run created is moved to the run's quarantine/ on undo",
                    "stop a daemon it did not start",
                    "touch a wallet file, /etc/monerod.conf or the system monerod.service",
                    "touch the pending.json money record (§2a.4)",
                    "run sudo, or a shell string: every command is argv (§2f)",
                    "write outside the scopes above",
                ],
            },
            "run_artifacts": match c {
                Class::D => serde_json::Value::Null,
                _ => serde_json::json!({
                    "dir": ctx.paths.doctor_runs_dir().display().to_string(),
                    "latest": ctx.paths.doctor_latest().display().to_string(),
                    "written_when": "--fix (always) or --save",
                    "keep": KEEP_RUNS,
                    "files": ["report.json", "actions.jsonl", "probes/", "backups/", "quarantine/"],
                    "undo": "fmonero doctor undo latest",
                    "health_check": "fmonero doctor --json >/dev/null; echo $?",
                    "exit_codes": fmonero_core::EXIT_CODES.iter()
                        .filter(|(c, _, _)| matches!(c, 0 | 11 | 12 | 13))
                        .map(|(code, name, meaning)| serde_json::json!({
                            "code": code, "code_name": name, "meaning": meaning}))
                        .collect::<Vec<_>>(),
                }),
            },
        })).collect::<Vec<_>>(),
    })
}

/// The plan a check can be fixed with — derived from the one detector that names it, so a
/// class-D row can never be reported fixable.
pub fn fixable_plan(def: &CheckDef) -> Option<PlanKind> {
    if def.class == Class::D {
        return None;
    }
    // The §7.1 "Every fixable row names the Action it emits" table, as code.
    Some(match def.id {
        "units_present" | "unit_loopback" | "unit_no_node_wants" => PlanKind::WriteBothUnits,
        "wallet_dir_matches_unit" => PlanKind::WriteWalletUnit,
        "wallet_port_consistent" => PlanKind::NormaliseWalletPort,
        "wallet_dir_exists" => PlanKind::CreateWalletDir,
        "unit_enabled_state" => PlanKind::DisableUnits,
        "monerod_conf_mode" => PlanKind::ChmodMonerodConf,
        "state_perms" => PlanKind::FixStatePerms,
        "state_valid" => PlanKind::ResetStateJson,
        "settings_valid" => PlanKind::RepairSettings,
        "plugin_linked" => PlanKind::LinkPlugin,
        "plugin_enabled" => PlanKind::EnablePlugin,
        "menu_entries" => PlanKind::MergeMenu,
        _ => return None,
    })
}

/// Write `report.json` into the run directory, at 0600 (§2b.7).
pub fn save(dir: &std::path::Path, report: &Report) -> Result<(), Error> {
    let mut text = serde_json::to_string_pretty(report)
        .map_err(|e| Error::failure(format!("cannot serialise the report: {e}")))?;
    text.push('\n');
    fmonero_core::write_private(&dir.join("report.json"), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capability_document_names_every_check_and_no_class_d_is_fixable() {
        let dir = std::env::temp_dir().join(format!("fmonero-doctor-caps-{}", std::process::id()));
        let ctx = Context::for_test(&dir).expect("context");
        let caps = capabilities(&ctx);

        // §5c: the top-level key set is exactly `ok`, `schema`, `checks`, `groups`, `classes`.
        let mut keys: Vec<&str> = caps
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["checks", "classes", "groups", "ok", "schema"]);

        assert_eq!(caps["checks"].as_array().unwrap().len(), CHECKS.len());
        for check in caps["checks"].as_array().unwrap() {
            if check["auto_fixable"] == serde_json::json!(true) {
                assert_ne!(check["class"], serde_json::json!("D"), "{check}");
            }
        }
        for def in CHECKS {
            if def.class == Class::D {
                assert!(fixable_plan(def).is_none(), "{}", def.id);
            } else {
                assert!(
                    fixable_plan(def).is_some(),
                    "{} is class {} but names no plan",
                    def.id,
                    def.class.name()
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exit_codes_follow_the_section_seven_rule() {
        let mut s = Summary::default();
        assert_eq!(exit_code(&s), 0);
        s.warn = 3;
        assert_eq!(exit_code(&s), 0, "warnings alone are never a failure");
        s.fail = 1;
        assert_eq!(exit_code(&s), 11);
        // 12 only under --fix: a detect-only run cannot have a fixer fail.
        s.fix_failed = 1;
        assert_eq!(exit_code(&s), 11);
        s.fix = true;
        assert_eq!(exit_code(&s), 12);
        s.fix_failed = 0;
        assert_eq!(
            exit_code(&s),
            11,
            "unfixable failures remain 11 under --fix"
        );
        s.fail = 0;
        assert_eq!(exit_code(&s), 0);
    }
}
