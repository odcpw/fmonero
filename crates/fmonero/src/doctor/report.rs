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
use unicode_width::UnicodeWidthStr;

/// One row of `report.json::checks` (§7).
#[derive(Clone, Debug, Serialize)]
pub struct CheckReport {
    pub id: String,
    /// The sentence a person reads down the left of the table (§7, W1). It is `CheckDef::title`,
    /// carried here because the human table is rendered from this document and printed the `id`
    /// instead — so all forty rewritten titles reached no reader, and fifteen identifiers did.
    pub title: String,
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
            title: def.title.to_string(),
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

/// The human table (§7, W1): **the title, a plain-English verdict, and the detail under it.**
///
/// What this printed until W1 was `check.id` where §7 says the title goes, and `:{reason}` after
/// the status word — so the forty titles `docs/COPY.md` §3.12 rewrote reached no human at all,
/// and what a person read instead was the identifier set: `unit_no_password_in_argv`,
/// `wallet_daemon_matches_unit`, `wallet_rpc_auth`, fifteen of the forty carrying a word §2o rule
/// 7 deletes outright. The ids are the agent contract and they stay in `--json`, where agents
/// already read them (§7); `pass` · `warn` · `fail` · `skip` stay there too, as `status`.
///
/// The shape: one leader line per check ending in its verdict, then the detail indented under it,
/// then any fix line and any mutation. The verdict starts at a fixed column so a reader can scan
/// forty of them, and `not checked` carries **why** in words, because one missing precondition
/// silences a whole group and the reason is the only thing that explains twelve rows at once.
pub fn human(out: &Out, report: &Report) {
    for line in human_lines(out, report) {
        out.line(line);
    }
}

/// Every line the table prints, in order — so a test can read the whole of it (W1).
fn human_lines(out: &Out, report: &Report) -> Vec<String> {
    /// The column the verdict starts at. The widest title is 66 characters and the widest
    /// `not checked: …` phrase is 47, so the longest line this can draw is 118 — inside an
    /// 80-column terminal's wrap and inside the 120 the captures are read at.
    const VERDICT_COL: usize = 70;
    let mut lines: Vec<String> = Vec::new();
    let mut group = String::new();
    // **X24: a sentence that is true of a whole group is said once, under the group's heading.**
    // One public-node run printed nine identical details — seven rows of `Your node` and two of
    // `Your wallet`, all reading `You are using the public node …, so there is nothing of your own
    // to check here.` The phrase beside each title (`not checked: …`) already makes the rows
    // scannable; the sentence itself belongs to the group.
    for check in &report.checks {
        if check.group != group {
            group = check.group.clone();
            // The heading is the group's plain name (W1). `[units]` was a word §2o rule 7
            // deletes, printed over seven rows; `--only units` and `--json` are unchanged.
            let heading = GROUPS
                .iter()
                .find(|g| g.name() == group)
                .map_or(group.clone(), |g| g.title().to_string());
            lines.push(out.paint(&heading, Paint::Dim));
            for note in group_notes(report, &group) {
                lines.push(out.paint(&format!("  {note}"), Paint::Dim));
            }
        }
        let (verdict, paint) = verdict_phrase(check);
        let head = format!("  {} ", check.title);
        let dots = VERDICT_COL.saturating_sub(UnicodeWidthStr::width(head.as_str()) + 1);
        lines.push(format!(
            "{head}{} {}",
            out.paint(&".".repeat(dots), Paint::Dim),
            out.paint(&verdict, paint)
        ));
        // X24: printed once above, as the group's own note.
        if !check.detail.is_empty() && !group_notes(report, &group).contains(&check.detail) {
            lines.push(format!("    {}", check.detail));
        }
        if let Some(cmd) = &check.fix_cmd {
            // Only for a finding. A `skip` already says why it could not be asked, and 25
            // repetitions of `Try: omarchy pkg add monero` bury the three rows that matter —
            // the run-level `Try:` lines carry the machine-wide move instead.
            if matches!(check.status.as_str(), "warn" | "fail") {
                // §2o rule 11, and the same word the footer and every CLI error use.
                lines.push(out.paint(&format!("    Try: {cmd}"), Paint::Dim));
            }
        }
        if let Some(outcome) = &check.fix_outcome {
            lines.push(out.paint(&format!("    {}", fix_words(outcome)), Paint::Dim));
        }
        for action in &check.actions {
            let tag = match action.outcome.as_str() {
                "applied" => out.paint("changed", Paint::Green),
                "already" => out.paint("already set", Paint::Dim),
                "planned" => out.paint("would change", Paint::Yellow),
                _ => out.paint("could not change", Paint::Red),
            };
            lines.push(format!("    {tag}: {}", action.action));
            if let Some(e) = &action.error {
                lines.push(format!("    {}", out.paint(e, Paint::Red)));
            }
        }
    }

    let s = &report.summary;
    lines.push(String::new());
    // The counts, in the same words the rows use. `4 warn, 0 fail, 12 skip` restated three
    // machine words a reader had just been shown as sentences (§2o rule 8).
    let needs = if s.warn == 1 { "needs" } else { "need" };
    lines.push(format!(
        "{} checks: {} OK, {} {needs} attention, {} not working, {} not checked",
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
            lines.push(out.paint(
                &format!(
                    "nothing was changed. It would write to: {}",
                    paths.join(", ")
                ),
                Paint::Dim,
            ));
        }
    }
    if let Some(dir) = &s.run_dir {
        lines.push(out.paint(&format!("run artifacts: {dir}"), Paint::Dim));
    }
    if !s.next.is_empty() {
        lines.push(String::new());
        for cmd in &s.next {
            // §2o rule 11: `Try:` is an instruction; `next:` labelled a field.
            lines.push(format!("  Try: {cmd}"));
        }
    }
    lines
}

/// The details **more than one check in this group reports word for word** (X24), in the order they
/// first appear.
///
/// The case it exists for is a precondition that silences a whole group: with a public node, seven
/// node checks and two wallet checks all report the same sentence, and nine copies of it is the
/// shape a reader stops reading. Two identical details are already a repetition, so the threshold is
/// two.
fn group_notes(report: &Report, group: &str) -> Vec<String> {
    let mut seen: Vec<(String, usize)> = Vec::new();
    for c in report.checks.iter().filter(|c| c.group == group) {
        if c.detail.is_empty() {
            continue;
        }
        match seen.iter_mut().find(|(d, _)| *d == c.detail) {
            Some((_, n)) => *n += 1,
            None => seen.push((c.detail.clone(), 1)),
        }
    }
    seen.into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(d, _)| d)
        .collect()
}

/// One row's verdict, as a phrase a person reads, and the colour it is painted (W1).
///
/// A `skip` carries its reason in words. It is the one status whose reason explains something the
/// detail cannot: `You are using the public node …, so there is nothing of your own to check
/// here.` is the same sentence twelve times in one run, and the phrase beside the title is what
/// makes those twelve scannable as one fact.
fn verdict_phrase(check: &CheckReport) -> (String, Paint) {
    match check.status.as_str() {
        "pass" => ("OK".to_string(), Paint::Green),
        "warn" => ("needs attention".to_string(), Paint::Yellow),
        "fail" => ("not working".to_string(), Paint::Red),
        _ => (
            match check.reason.as_deref().and_then(not_checked_because) {
                Some(words) => format!("not checked: {words}"),
                None => "not checked".to_string(),
            },
            Paint::Dim,
        ),
    }
}

/// `--fix`'s own three outcomes, in the register of the table around them.
fn fix_words(outcome: &str) -> &str {
    match outcome {
        "fixed" => "fmonero fixed this.",
        "applied" => "fmonero changed something here.",
        "applied, still failing" => "fmonero changed something, and this still needs attention.",
        "failed" => "fmonero could not fix this.",
        other => other,
    }
}

/// Why a check was not run, in words (W1).
///
/// **Every `Verdict::skip` reason in `checks.rs` has an entry here**, and the test below reads
/// that file to prove it — a reason added without a phrase would otherwise print `not checked`
/// with no reason, silently, on a whole group of rows.
fn not_checked_because(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "monerod_not_installed" | "wallet_rpc_not_installed" => "Monero is not installed",
        "remote_node" => "you are using a public node",
        "local_mode" => "you are using your own node",
        "unit_missing" => "the services are not set up yet",
        "no_version_line" => "your node printed no version",
        "node_unreachable" | "no_get_info" => "your node did not answer",
        "node_restricted" => "it only answers basic questions",
        "node_syncing" => "your node is still syncing",
        "node_offline" => "your node is set to stay offline",
        "probe_failed" => "the chain folder could not be read",
        "wallet_unreachable" => "the wallet service did not answer",
        "wallet_daemon_down" => "the wallet service is not running",
        "no_request_completed" => "nothing was asked of it this run",
        "no_heights" => "a block number is missing",
        "no_node_credential" => "your node asks for no password",
        "daemon_unreachable" => "one of the two did not answer",
        "wallet_closed" => "no wallet is open",
        "no_wallet_height" => "no block number from a wallet",
        "no_recorded_network" => "the network was not recorded",
        "wallet_rpc_active" => "the wallet service is running",
        "cannot_measure" => "the free space is not measurable",
        "prune_off" => "you asked for the whole blockchain",
        "no_conf" => "your node is not set up yet",
        "no_mode" => "who can read the file is unknown",
        "no_cache" => "there is no list of nodes yet",
        "no_checkout" => "fmonero's own copy is missing",
        "no_omarchy" => "Omarchy is not installed",
        "list_failed" => "Omarchy could not list its widgets",
        "foreign_node" => "another program runs this node",
        "not_taken" => "this group was not asked for",
        _ => return None,
    })
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
        // v5.2, §7.1: both rewrite the wallet unit from `units::render_wallet_unit` and then
        // `systemctl --user daemon-reload` — a unit rewritten without a reload is a fixer that
        // reports success and changes nothing.
        "wallet_dir_matches_unit" | "wallet_daemon_matches_unit" => PlanKind::WriteWalletUnit,
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

    /// The literals `checks.rs` can produce, read out of the file itself: every `Verdict::skip`
    /// reason, and every reason of any kind. A table of 82 reasons maintained by hand beside a
    /// file of 2 400 lines drifts; reading the source does not.
    fn reasons_in_checks_rs(kind: &str) -> Vec<String> {
        let src = include_str!("checks.rs");
        let needle = format!("Verdict::{kind}(");
        let mut out = Vec::new();
        for (i, _) in src.match_indices(&needle) {
            let rest = &src[i + needle.len()..];
            let Some(open) = rest.find('"') else { continue };
            // Only a literal that follows the paren immediately (whitespace aside) is the
            // reason: `Verdict::warn(reason_variable, …)` would not be one.
            if rest[..open].chars().any(|c| !c.is_whitespace()) {
                continue;
            }
            let Some(close) = rest[open + 1..].find('"') else {
                continue;
            };
            let lit = &rest[open + 1..open + 1 + close];
            if lit
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                out.push(lit.to_string());
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// **W1.** The human table prints the **title** §7 says it prints, a verdict in words, and
    /// no identifier of any kind. The run that produced the finding read:
    ///
    /// ```text
    ///   warn:wallet_unreachable        wallet_rpc_reachable     The wallet service is not …
    ///   skip:wallet_daemon_down        wallet_rpc_auth          The wallet service is not …
    /// ```
    ///
    /// — three words §2o rule 7 deletes (`daemon`, `rpc`, `unit`) on the screen of the command
    /// that had just had 54 of its sentences rewritten. Every row is rendered here, in all four
    /// statuses, with every reason the file can produce.
    #[test]
    fn the_human_table_prints_titles_and_never_an_id_or_a_reason() {
        let plain = Out {
            json: false,
            color: false,
        };
        let reasons = reasons_in_checks_rs("skip");
        assert!(
            reasons.len() > 20,
            "the reason scan found only {reasons:?} — it is not reading checks.rs"
        );
        // One row per check per status, with the skip rows cycling through every skip reason, so
        // no reason can reach the table unrendered by this test.
        let mut rows: Vec<CheckReport> = Vec::new();
        for def in CHECKS {
            rows.push(CheckReport::new(def, &Verdict::pass("A sentence.")));
            rows.push(CheckReport::new(
                def,
                &Verdict::warn("unit_drift", "A sentence."),
            ));
            rows.push(CheckReport::new(
                def,
                &Verdict::fail("not_installed", "A sentence."),
            ));
            for reason in &reasons {
                let leaked: &'static str = Box::leak(reason.clone().into_boxed_str());
                rows.push(CheckReport::new(def, &Verdict::skip(leaked, "A sentence.")));
            }
        }
        let report = Report {
            ok: false,
            schema: fmonero_core::SCHEMA_VERSION,
            run_id: None,
            checks: rows,
            summary: Summary::default(),
        };
        let lines = human_lines(&plain, &report);
        let text = lines.join("\n");
        for def in CHECKS {
            assert!(
                text.contains(def.title),
                "the table must print the title: {}",
                def.title
            );
            assert!(
                !text.contains(def.id),
                "an id reached the human table: {}",
                def.id
            );
        }
        for reason in &reasons {
            assert!(
                !text.contains(reason.as_str()),
                "a reason reached the human table: {reason}"
            );
        }
        // And no machine status word either: `pass`, `warn`, `fail` and `skip` are `--json`'s.
        // As whole words, because `password` and `appears` are English and `skipped` is not what
        // this is about.
        let words: Vec<String> = lines
            .iter()
            .flat_map(|l| l.split(|c: char| !c.is_alphanumeric() && c != '_'))
            .map(str::to_ascii_lowercase)
            .collect();
        for machine in ["pass", "warn", "fail", "skip", "verdict"] {
            assert!(
                !words.iter().any(|w| w == machine),
                "{machine:?} is a machine word and reached the human table"
            );
        }
        // Rule 5 in general, over the whole rendering: nothing shaped like an identifier.
        for line in &lines {
            for token in line.split_whitespace() {
                let t = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                assert!(
                    !(t.contains('_') && !t.contains('/') && !t.contains('.')),
                    "an identifier-shaped token reached the table: {token:?} in {line:?}"
                );
            }
        }
    }

    /// **W1.** Every `Verdict::skip` reason has words, so `not checked` is never bare — one
    /// missing precondition silences a whole group, and the reason is what explains twelve rows
    /// at once.
    #[test]
    fn every_skip_reason_has_a_sentence_for_the_human_table() {
        for reason in reasons_in_checks_rs("skip") {
            let words = not_checked_because(&reason);
            assert!(
                words.is_some(),
                "skip:{reason} would print a bare `not checked` — add it to not_checked_because"
            );
            let words = words.unwrap_or_default();
            assert!(
                words.len() <= 34,
                "`not checked: {words}` is {} characters and the column budgets 47",
                words.len() + 13
            );
            assert!(
                !words.contains('_'),
                "the words for skip:{reason} are still an id: {words}"
            );
        }
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
