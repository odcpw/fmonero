//! The doctor: detect, report, and apply a small set of reversible repairs (DESIGN.md §7).
//!
//! The shape §7 fixes, and the doctor kernel's axioms 1–3 and 7:
//!
//! * [`probes`] is the only place a process is spawned, a file read or an RPC issued;
//! * [`checks`] holds every detector as a **pure function** of the captured probes;
//! * [`mutate`] is the single chokepoint every `--fix` mutation flows through, with verbatim
//!   backups, sha256 before and after, an append-only `actions.jsonl` and a recorded inverse
//!   for `doctor undo`;
//! * [`report`] renders `report.json`, the human table and `doctor capabilities`.
//!
//! Read-only by default: a run without `--fix` writes nothing unless `--save`. **The doctor
//! never stops a daemon it did not start, never touches a wallet file, never touches the
//! `pending.json` money record, never removes a file, and never runs sudo** (§7, unnarrowed).
//!
//! `DoctorArgs` and its subcommands were frozen at the Phase 1a gate because `main.rs`'s
//! clap tree references them and `main.rs` may not be edited afterwards (§2k, §5b); later
//! phases may **add** fields.

pub mod checks;
pub mod mutate;
pub mod probes;
pub mod report;

#[cfg(test)]
mod fixture_tests;

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use fmonero_core::{Context, Error};

use crate::output::Out;
use checks::{CheckStatus, Class, PlanKind, CHECKS};
use report::{CheckReport, Report, Summary};

/// The check groups `--only` accepts (§7).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DoctorGroup {
    Binaries,
    Node,
    Wallet,
    Units,
    Storage,
    Install,
    Security,
}

/// Every group, in report order.
pub const GROUPS: &[DoctorGroup] = &[
    DoctorGroup::Binaries,
    DoctorGroup::Node,
    DoctorGroup::Wallet,
    DoctorGroup::Units,
    DoctorGroup::Storage,
    DoctorGroup::Install,
    DoctorGroup::Security,
];

impl DoctorGroup {
    pub fn name(self) -> &'static str {
        match self {
            DoctorGroup::Binaries => "binaries",
            DoctorGroup::Node => "node",
            DoctorGroup::Wallet => "wallet",
            DoctorGroup::Units => "units",
            DoctorGroup::Storage => "storage",
            DoctorGroup::Install => "install",
            DoctorGroup::Security => "security",
        }
    }

    /// The heading a person reads above the group's rows (W1).
    ///
    /// `name()` is what `--only` takes and what `--json` carries, and one of the seven is
    /// `units` — a word §2o rule 7 deletes outright, printed as a heading over seven rows. The
    /// machine name stays exactly as it is; the heading says what the group is about.
    pub fn title(self) -> &'static str {
        match self {
            DoctorGroup::Binaries => "Monero's own programs",
            DoctorGroup::Node => "Your node",
            DoctorGroup::Wallet => "Your wallet",
            DoctorGroup::Units => "The two background services",
            DoctorGroup::Storage => "Room on the disk",
            DoctorGroup::Install => "fmonero's own files",
            DoctorGroup::Security => "Privacy on this machine",
        }
    }

    /// `--only GROUP`. `DoctorArgs::only` is a frozen `Option<String>` (§5b), so the parse
    /// happens here — and an unknown group names every legal one rather than saying "no".
    pub fn parse(name: &str) -> Result<DoctorGroup, Error> {
        GROUPS
            .iter()
            .copied()
            .find(|g| g.name() == name)
            .ok_or_else(|| {
                Error::usage(format!(
                    "'{name}' is not a check group; known: {}",
                    GROUPS
                        .iter()
                        .map(|g| g.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .with_reason("bad_args")
                .with_hint("fmonero doctor capabilities --json")
            })
    }
}

/// `plugin/oliver.monero` inside **this repo's checkout** — what `plugin_linked` validates
/// the resolved symlink against (§7.1).
///
/// `CARGO_MANIFEST_DIR` is `crates/fmonero`, so the repository root is two levels up. It
/// shares the plugin gate's blind spot, named in §7.1: the installed link resolves to the
/// main checkout, never to a Phase 2 worktree — which is why the plugin agent works in the
/// main checkout (§8).
pub fn repo_plugin_dir() -> Option<PathBuf> {
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("plugin").join(probes::PLUGIN_ID));
    if let Some(dir) = built_from {
        if dir.join("manifest.json").is_file() {
            return Some(dir);
        }
    }
    // A release binary is run from ~/.local/bin, where the build directory may be gone.
    let mut dir = std::env::current_dir().ok();
    while let Some(d) = dir {
        let candidate = d.join("plugin").join(probes::PLUGIN_ID);
        if candidate.join("manifest.json").is_file() {
            return Some(candidate);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
}

/// `fmonero doctor [--fix] [--only GROUP] [--save]`, plus `doctor undo` and
/// `doctor capabilities`.
#[derive(Args, Clone, Debug, Default)]
pub struct DoctorArgs {
    /// Apply the fixable findings (each one reversible with `doctor undo latest`)
    #[arg(long)]
    pub fix: bool,
    /// Only run one group: binaries | node | wallet | units | storage | install | security
    #[arg(long, value_name = "GROUP")]
    pub only: Option<String>,
    /// Write the run artifacts even without --fix
    #[arg(long)]
    pub save: bool,
    #[command(subcommand)]
    pub cmd: Option<DoctorCmd>,
    /// Set from the global --yes (not a flag of its own); accepted and ignored by --fix (§7).
    #[clap(skip)]
    pub yes: bool,
    /// Set from the global --dry-run (not a flag of its own).
    #[clap(skip)]
    pub dry_run: bool,
}

#[derive(Subcommand, Clone, Debug)]
pub enum DoctorCmd {
    /// Replay a `--fix` run in reverse (exit 13 if a backup is missing or altered)
    Undo {
        /// A run id, or `latest`
        #[arg(default_value = "latest")]
        run: String,
    },
    /// List the checks, groups and safety classes
    Capabilities,
}

/// Run the doctor. Exit **11** if any check is `fail` and no `--fix` was asked for; **12**
/// if a fixer failed or fixed only part; **13** on a refusal or an undo integrity failure;
/// **0** only when no `fail` remains (§7).
pub fn run(args: DoctorArgs, ctx: Context) -> anyhow::Result<i32> {
    let out = Out::new(ctx.json, false, ctx.verbose);
    match &args.cmd {
        Some(DoctorCmd::Capabilities) => {
            // Always machine output, whatever the flags say — it is a contract document (§4).
            let out = out.forced_json();
            out.emit("doctor capabilities", report::capabilities(&ctx));
            Ok(0)
        }
        Some(DoctorCmd::Undo { run }) => undo(run, &ctx, &out),
        None => diagnose(args, ctx, out),
    }
}

// ------------------------------------------------------------------ undo

fn undo(run: &str, ctx: &Context, out: &Out) -> anyhow::Result<i32> {
    let dir = mutate::resolve_run(ctx, run)?;
    let run_id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.rsplit_once("__").map(|(_, id)| id.to_string()));
    match mutate::undo(ctx, &dir) {
        Ok(steps) => {
            let failed = steps.iter().filter(|s| !s.ok).count();
            // §5c: run_id · undone[{seq, action, path, ok}] · refused · reason.
            let payload = serde_json::json!({
                "ok": failed == 0,
                "schema": fmonero_core::SCHEMA_VERSION,
                "run_id": run_id,
                "undone": steps,
                "refused": false,
                "reason": serde_json::Value::Null,
            });
            if out.json {
                out.line(payload.to_string());
            } else {
                if steps.is_empty() {
                    out.line("nothing to undo: that run changed nothing");
                }
                for step in &steps {
                    let tag = if step.ok {
                        out.paint("undone ", crate::output::Paint::Green)
                    } else {
                        out.paint("failed ", crate::output::Paint::Red)
                    };
                    out.line(format!(
                        "{tag} [{}] {} {} — {}",
                        step.seq,
                        step.check,
                        step.action,
                        step.note.as_deref().unwrap_or("")
                    ));
                }
                out.note(format!("run: {}", dir.display()));
            }
            Ok(if failed > 0 { 12 } else { 0 })
        }
        // A refusal is the doctor working correctly, and §5c gives this command its own
        // `refused`/`reason` keys — so it is reported in the command's own payload rather
        // than as a bare error envelope, and exits 13.
        Err(e) if e.exit_code() == 13 => {
            let payload = serde_json::json!({
                "ok": false,
                "schema": fmonero_core::SCHEMA_VERSION,
                "run_id": run_id,
                "undone": Vec::<serde_json::Value>::new(),
                "refused": true,
                "reason": e.to_string(),
            });
            if out.json {
                out.line(payload.to_string());
            } else {
                out.note(format!("refused: {e}"));
                out.note(format!("hint: {}", e.hint()));
            }
            Ok(13)
        }
        Err(e) => Err(e.into()),
    }
}

// ------------------------------------------------------------------ diagnose

fn diagnose(args: DoctorArgs, ctx: Context, out: Out) -> anyhow::Result<i32> {
    let only = match &args.only {
        Some(name) => Some(DoctorGroup::parse(name)?),
        None => None,
    };
    let selected: Vec<&checks::CheckDef> = CHECKS
        .iter()
        .filter(|c| only.is_none() || only == Some(c.group))
        .collect();

    let mut probes = probes::Probes::capture(&ctx, only);
    let mut rows: Vec<CheckReport> = Vec::new();
    let mut plans: Vec<(&checks::CheckDef, PlanKind)> = Vec::new();
    for def in &selected {
        let verdict = (def.detect)(&probes);
        if let Some(plan) = verdict.plan {
            plans.push((def, plan));
        }
        rows.push(CheckReport::new(def, &verdict));
    }

    // §7: `--fix` alone performs every fixer in v0.1, and `--yes` is accepted and ignored,
    // so `fmonero doctor --fix --yes` is always safe to script. `--dry-run` plans only.
    let planning_only = args.dry_run;
    let mut mutator = None;
    let mut refused = false;
    if args.fix {
        let run = if planning_only {
            None
        } else {
            Some(mutate::RunDir::create(&ctx)?)
        };
        let mut m = mutate::Mutator::new(&ctx, run, planning_only);
        for (def, kind) in &plans {
            if def.class == Class::D {
                // Belt and braces: `fixable_plan` already refuses to name a plan for a
                // class-D row, and no class-D detector may return one (a unit test asserts
                // both). This is the third statement of the same rule, on the path that
                // would actually mutate.
                set_outcome(
                    &mut rows,
                    def.id,
                    "skipped: class D is never auto-fixed",
                    None,
                );
                continue;
            }
            let before = m.log.len();
            let outcome = apply_one(&mut m, &ctx, &probes, def, *kind, planning_only);
            let actions: Vec<mutate::PlannedAction> = m.log[before..].to_vec();
            if let Some(row) = rows.iter_mut().find(|r| r.id == def.id) {
                row.actions = actions;
            }
            match outcome {
                Ok(text) => set_outcome(&mut rows, def.id, &text, None),
                Err(e) => {
                    set_outcome(&mut rows, def.id, "refused", Some(e.to_string()));
                    if e.exit_code() == 13 {
                        refused = true;
                    }
                }
            }
        }
        mutator = Some(m);
    }

    // Anything actually applied changes the world: re-probe and re-run just the checks that
    // were fixed, so `fixed` is a measurement rather than a claim (§7).
    let applied = mutator.as_ref().map(|m| m.applied()).unwrap_or(0);
    if applied > 0 {
        probes = probes::Probes::capture(&ctx, only);
        for def in &selected {
            let Some(row) = rows.iter_mut().find(|r| r.id == def.id) else {
                continue;
            };
            if row.fix_outcome.as_deref() != Some("applied") {
                continue;
            }
            let after = (def.detect)(&probes);
            let cleared = matches!(after.status, CheckStatus::Pass | CheckStatus::Skip);
            row.restate(&after);
            row.fix_outcome = Some(
                if cleared {
                    "fixed"
                } else {
                    "applied, still failing"
                }
                .to_string(),
            );
        }
    }

    let all_actions: Vec<&mutate::PlannedAction> =
        rows.iter().flat_map(|r| r.actions.iter()).collect();
    let mut summary = Summary::of(&rows, &all_actions);
    summary.generated_at = fmonero_core::now_rfc3339();
    summary.fix = args.fix;
    summary.dry_run = planning_only;
    summary.only = only.map(|g| g.name().to_string());
    summary.run_dir = mutator
        .as_ref()
        .and_then(|m| m.run_dir())
        .map(|d| d.display().to_string());
    let mut code = report::exit_code(&summary);
    if refused {
        code = 13;
    }
    summary.exit_code = code;
    summary.next = next_commands(&summary, &args);

    let report = Report {
        ok: summary.fail == 0 && summary.fix_failed == 0 && !refused,
        schema: fmonero_core::SCHEMA_VERSION,
        run_id: mutator
            .as_ref()
            .and_then(|m| m.run_id().map(str::to_string)),
        checks: rows,
        summary,
    };

    // A run without `--fix` writes nothing unless `--save` is passed (§7).
    let mut saved_dir: Option<PathBuf> = None;
    if let Some(dir) = mutator.as_ref().and_then(|m| m.run_dir()) {
        write_artifacts(dir, &report, &probes)?;
    } else if args.save {
        let run = mutate::RunDir::create(&ctx)?;
        write_artifacts(&run.dir, &report, &probes)?;
        saved_dir = Some(run.dir.clone());
    }

    if out.json {
        out.line(
            serde_json::to_string(&report)
                .map_err(|e| Error::failure(format!("cannot serialise the report: {e}")))?,
        );
    } else {
        report::human(&out, &report);
    }
    if let Some(dir) = saved_dir {
        out.note(format!("run artifacts: {}", dir.display()));
    }
    Ok(code)
}

fn set_outcome(rows: &mut [CheckReport], id: &str, outcome: &str, error: Option<String>) {
    if let Some(row) = rows.iter_mut().find(|r| r.id == id) {
        row.fix_outcome = Some(outcome.to_string());
        row.fix_error = error;
    }
}

fn apply_one(
    m: &mut mutate::Mutator,
    ctx: &Context,
    probes: &probes::Probes,
    def: &checks::CheckDef,
    kind: PlanKind,
    planning_only: bool,
) -> Result<String, Error> {
    let actions = mutate::plan(kind, ctx, probes)?;
    if actions.is_empty() {
        return Ok("nothing to do".to_string());
    }
    // A sandboxed run declines what it may not touch, and says so, rather than letting the
    // chokepoint refuse it and counting that as a failed fixer. **A dry run is exempt**: it
    // mutates nothing by construction, and the whole point of `--dry-run` is to disclose the
    // blast radius — including the part a sandbox would refuse to apply.
    if !planning_only {
        if let Some(reason) = mutate::refused_here(ctx, &actions) {
            return Ok(reason);
        }
    }
    let mut applied = 0;
    let mut already = 0;
    for action in actions {
        match m.mutate(def.id, def.class, action)? {
            mutate::Outcome::Applied => applied += 1,
            mutate::Outcome::AlreadyDone => already += 1,
            mutate::Outcome::Planned => {}
        }
    }
    Ok(if applied > 0 {
        "applied".to_string()
    } else if already > 0 {
        "already done".to_string()
    } else {
        "planned".to_string()
    })
}

/// The next moves, as runnable commands — Axiom 0's second question.
fn next_commands(summary: &Summary, args: &DoctorArgs) -> Vec<String> {
    let mut cmds = Vec::new();
    if summary.fixable > 0 && !args.fix {
        cmds.push("fmonero doctor --fix --dry-run".to_string());
    }
    if summary.fail > 0 || summary.warn > 0 {
        cmds.push(
            "fmonero doctor --json | jq '.checks[] | select(.status != \"pass\")'".to_string(),
        );
    }
    if args.fix && !summary.dry_run && summary.fixed > 0 {
        cmds.push("fmonero doctor undo latest".to_string());
    }
    cmds
}

fn write_artifacts(
    dir: &Path,
    report: &Report,
    probes: &probes::Probes,
) -> Result<(), fmonero_core::Error> {
    report::save(dir, report)?;
    let probe_dir = dir.join("probes");
    if !probe_dir.is_dir() {
        std::fs::create_dir_all(&probe_dir)
            .map_err(|e| Error::failure(format!("cannot create {}: {e}", probe_dir.display())))?;
    }
    for (name, probe) in probes::named_commands(probes) {
        // Each artifact is truncated at 64 KiB with a trailer, redacted at capture and
        // written 0600 (§7, §2b.8).
        let _ = fmonero_core::write_private(
            &probe_dir.join(format!("{name}.txt")),
            probes::artifact_text(probe).as_bytes(),
        );
    }
    // The captured facts, as JSON. `Probes` never carries `monerod.conf`'s or the login
    // file's bytes and every captured line is redacted, so this file cannot hold a
    // credential (§2b.8).
    if let Ok(text) = serde_json::to_string_pretty(probes) {
        let _ = fmonero_core::write_private(
            &probe_dir.join("facts.json"),
            probes::truncate(text).as_bytes(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_group_has_a_name_and_parses_back() {
        assert_eq!(GROUPS.len(), 7);
        for g in GROUPS {
            assert!(!g.name().is_empty());
            assert_eq!(DoctorGroup::parse(g.name()).expect("round trip"), *g);
        }
        let err = DoctorGroup::parse("keyring").expect_err("unknown group");
        assert_eq!(err.exit_code(), 2);
        // The error names every legal group rather than merely refusing (Axiom 10).
        for g in GROUPS {
            assert!(err.to_string().contains(g.name()), "{err}");
        }
    }

    #[test]
    fn the_repo_plugin_dir_points_at_this_checkout_when_there_is_one() {
        // A worktree still has plugin/ only if the branch carries it; the check's own
        // verdict handles the absence, so this only asserts the shape.
        if let Some(dir) = repo_plugin_dir() {
            assert!(dir.ends_with("plugin/oliver.monero"), "{}", dir.display());
        }
    }
}
