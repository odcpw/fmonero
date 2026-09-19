//! The **one** chokepoint every `--fix` mutation flows through (DESIGN.md §7).
//!
//! Nothing else in the doctor writes to disk, creates a directory or a symlink, or runs a
//! mutating command. Every call here
//!
//! 1. refuses a path outside the declared write scopes — a bounded blast radius;
//! 2. takes a **verbatim** backup and records `sha256` before *and* after;
//! 3. appends one line to `runs/<id>/actions.jsonl` and `fsync`s it **before** the
//!    mutation, so a crash between the journal entry and the write is visible to `undo`
//!    rather than silent;
//! 4. records the inverse `doctor undo` will replay.
//!
//! **The doctor never removes a file** (§7, unnarrowed). A file a run *created* is moved
//! into that run's `quarantine/` on undo, never unlinked. The one removal §7 does sanction
//! is `CreateDir`'s inverse — `rmdir`, and only while the directory is still empty.
//!
//! No fixer calls `Settings::save`, `state::with_state` or `units::install`: all three write
//! the file themselves, which bypasses the backup, the sha256 pair and `actions.jsonl` — and
//! therefore `undo` (§7.1). That is why §5b exposes `Settings::to_toml_bytes`,
//! `StateDoc::to_bytes` and `state::load`.

use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt as _, PermissionsExt};
use std::path::{Path, PathBuf};

use fmonero_core::{proc, settings::Settings, units, Context, Error};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::checks::{Class, PlanKind};
use super::probes::{self, Probes, PLUGIN_ID};

/// Newest runs kept; every write prunes the rest (§7).
pub const KEEP_RUNS: usize = 10;

/// One mutation. The enum is the complete vocabulary of everything `--fix` can do (§7).
#[derive(Clone, Debug)]
pub enum Action {
    /// Replace a file's bytes (creating it when absent). Reverts from the backup, or by
    /// quarantining the file this run created.
    WriteFile {
        path: PathBuf,
        bytes: Vec<u8>,
        mode: u32,
    },
    /// Create a directory. Inverse: `rmdir`, and only while it is still empty.
    ///
    /// It exists because `wallet_dir_exists` creates a *directory*, which `WriteFile`
    /// cannot do, and a fixer that cannot be routed through the one chokepoint is a fixer
    /// that gets written around it as `RunArgv ["mkdir", …]` with no recorded inverse (§7).
    CreateDir { path: PathBuf, mode: u32 },
    /// Create a symlink. Inverse: remove the link **this run created**.
    CreateSymlink { link: PathBuf, target: PathBuf },
    /// Run a command; `inverse_argv` is what undoes it (empty = nothing to undo).
    RunArgv {
        argv: Vec<String>,
        inverse_argv: Vec<String>,
    },
}

impl Action {
    /// The one-line form printed by `--dry-run` and stored in `actions.jsonl`.
    pub fn describe(&self) -> String {
        match self {
            Action::WriteFile { path, bytes, mode } => format!(
                "write {} ({} bytes, mode {mode:04o})",
                path.display(),
                bytes.len()
            ),
            Action::CreateDir { path, mode } => {
                format!("mkdir {} (mode {mode:04o})", path.display())
            }
            Action::CreateSymlink { link, target } => {
                format!("symlink {} -> {}", link.display(), target.display())
            }
            Action::RunArgv { argv, inverse_argv } => {
                let mut s = format!("run {}", argv.join(" "));
                if !inverse_argv.is_empty() {
                    s.push_str(&format!("   (undo: {})", inverse_argv.join(" ")));
                }
                s
            }
        }
    }

    /// Every path this action writes to, for `--dry-run`'s blast-radius listing (§7).
    pub fn writes_to(&self) -> Vec<String> {
        match self {
            Action::WriteFile { path, .. } | Action::CreateDir { path, .. } => {
                vec![path.display().to_string()]
            }
            Action::CreateSymlink { link, .. } => vec![link.display().to_string()],
            Action::RunArgv { .. } => Vec::new(),
        }
    }
}

/// What one [`Mutator::mutate`] call did.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The mutation ran.
    Applied,
    /// Nothing needed doing — run twice = run once.
    AlreadyDone,
    /// `--dry-run`: printed, not run.
    Planned,
}

fn sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// The complete set of places `--fix` may write. Anything else is refused with exit 13.
///
/// `systemd_user_dir` is **not** a scope: the two unit *files* are, by name, because the
/// directory holds every other user unit on the machine.
pub fn write_scopes(ctx: &Context) -> Vec<PathBuf> {
    let mut scopes = vec![
        ctx.paths.state_dir.clone(),
        ctx.paths.config_dir.clone(),
        ctx.paths.cache_dir.clone(),
    ];
    // **Under a test state directory the scopes stop there.** §4.2 makes `units::install`
    // refuse to run at all when `FMONERO_STATE_DIR` is set, and a doctor that wrote the two
    // unit files itself would walk straight around that guard: the Phase 2 gate's own
    // `--fix` run would rewrite the OWNER's `~/.config/systemd/user`, run `systemctl --user
    // disable` against the owner's systemd and merge the owner's Omarchy menu — from a
    // sandbox that exists precisely so it cannot. `FMONERO_STATE_DIR`,
    // `FMONERO_CONFIG_DIR` and `FMONERO_CACHE_DIR` are the three locations a test redirects
    // (§4.2, §5b), so they are exactly the three a test may write. DESIGN.md states the rule
    // for `units::install` and not for the doctor; the extension is recorded in
    // `decisions/phase2-doctor.md`.
    if fmonero_core::paths::under_test_state_dir() {
        return scopes;
    }
    let unit_dir = probes::systemd_user_dir(ctx);
    scopes.push(ctx.paths.omarchy_config_dir.join("plugins"));
    scopes.push(ctx.paths.omarchy_config_dir.join("extensions"));
    // The two unit FILES, by name — not their directory, which holds every other user unit
    // on the machine.
    scopes.push(unit_dir.join(&ctx.settings.node.unit));
    scopes.push(unit_dir.join(&ctx.settings.wallet.unit));
    // `wallet_dir_exists` creates fmonero's own wallet subtree — `~/.local/share/fmonero`
    // and below — and nothing above it.
    if let Some(parent) = ctx.paths.wallet_dir.parent() {
        scopes.push(parent.to_path_buf());
    }
    scopes
}

/// The argv a `RunArgv` fixer may still run under a test state directory: `chmod`, whose
/// every path argument [`require_in_scope`] then checks.
///
/// Every other fixer argv reaches outside the three redirected directories by construction —
/// `systemctl --user disable` touches the owner's systemd, `omarchy plugin enable` rewrites
/// the owner's `shell.json`, `fmonero setup --menu` merges the owner's menu file — and a path
/// check cannot see it, because the path is not in the argv. So the allowlist is a name list.
const TEST_SAFE_ARGV0: &[&str] = &["chmod"];

/// True when `path` is inside one of [`write_scopes`].
///
/// `Path::starts_with` compares *components* and does not resolve `..`, so
/// `<state_dir>/../../../etc/x` would otherwise pass this guard. A `..` in a path the
/// doctor is about to write is never legitimate — every caller builds its path from
/// `ctx.paths` — so it is refused outright rather than canonicalised (the target of a write
/// may not exist yet, which `canonicalize` cannot handle).
fn in_scope(ctx: &Context, path: &Path) -> bool {
    if has_parent_dir(path) {
        return false;
    }
    write_scopes(ctx)
        .iter()
        .any(|scope| path == scope || path.starts_with(scope))
}

fn has_parent_dir(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// The same guard, as an error, for the undo side — `doctor undo` takes its targets straight
/// out of `actions.jsonl`, which is a file on disk like any other.
fn require_in_scope(ctx: &Context, path: &Path) -> Result<(), Error> {
    if in_scope(ctx, path) {
        return Ok(());
    }
    Err(Error::doctor_refused(format!(
        "{} is outside the doctor's write scopes; nothing was changed",
        path.display()
    ))
    .with_hint("fmonero doctor capabilities --json"))
}

/// Refuse, under a test state directory, an argv that would reach outside the three
/// redirected directories — see [`TEST_SAFE_ARGV0`]. Applied on both the apply and the undo
/// side, because an inverse argv runs the same commands the forward one did.
fn require_test_safe_argv(ctx: &Context, argv: &[&str]) -> Result<(), Error> {
    if !fmonero_core::paths::under_test_state_dir() {
        return Ok(());
    }
    let program = std::path::Path::new(argv.first().copied().unwrap_or_default())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !TEST_SAFE_ARGV0.contains(&program.as_str()) {
        return Err(Error::doctor_refused(format!(
            "`{}` would change this machine outside FMONERO_STATE_DIR; the doctor refuses \
             every fixer but chmod under a test state directory, the same guard §4.2 puts on \
             units::install",
            argv.join(" ")
        ))
        .with_hint("fmonero doctor --fix --dry-run   # to see the plan without a test state dir"));
    }
    for arg in argv.iter().skip(1) {
        if arg.starts_with('/') {
            require_in_scope(ctx, std::path::Path::new(arg))?;
        }
    }
    Ok(())
}

/// Map an absolute path to the relative one used under `backups/` and `quarantine/`,
/// refusing anything with a `..` component so the mapping cannot escape the run directory.
fn run_relative(path: &Path) -> Result<PathBuf, Error> {
    if has_parent_dir(path) {
        return Err(Error::doctor_refused(format!(
            "{} contains a `..` component; refusing to map it into the run directory",
            path.display()
        )));
    }
    Ok(path.strip_prefix("/").unwrap_or(path).to_path_buf())
}

fn mkdir_private(path: &Path) -> Result<(), Error> {
    if path.is_dir() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| Error::failure(format!("cannot create {}: {e}", path.display())))
}

// ------------------------------------------------------------------ the run directory

/// The per-run artifact directory (§7): `report.json`, `actions.jsonl`, `probes/`,
/// `backups/`. Append-only; never edited after the run.
pub struct RunDir {
    pub id: String,
    pub dir: PathBuf,
    actions: std::fs::File,
}

impl RunDir {
    /// `<UTC basic>__<run-id>` under `~/.local/state/fmonero/doctor/runs/`, where `<run-id>`
    /// is **8 lowercase hex characters from `/dev/urandom`** (§7) — so two runs inside one
    /// second get their own directory instead of appending to each other's `actions.jsonl`,
    /// which `undo` replays whole.
    pub fn create(ctx: &Context) -> Result<RunDir, Error> {
        let stamp = jiff::Timestamp::now()
            .strftime("%Y%m%dT%H%M%SZ")
            .to_string();
        let id = fmonero_core::rpc::random_hex(4)?;
        let dir = ctx.paths.doctor_runs_dir().join(format!("{stamp}__{id}"));
        for sub in ["probes", "backups", "quarantine"] {
            mkdir_private(&dir.join(sub))?;
        }
        // 0600 at creation, never chmod'ed afterwards (§2b.7).
        let actions = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(dir.join("actions.jsonl"))
            .map_err(|e| Error::failure(format!("cannot open actions.jsonl: {e}")))?;
        let run = RunDir { id, dir, actions };
        run.point_latest(ctx)?;
        prune(ctx);
        Ok(run)
    }

    /// `doctor/latest` → this run, by symlink-to-temp + rename (§7).
    fn point_latest(&self, ctx: &Context) -> Result<(), Error> {
        let latest = ctx.paths.doctor_latest();
        if let Some(parent) = latest.parent() {
            mkdir_private(parent)?;
        }
        let tmp = latest.with_extension(format!("tmp.{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(&self.dir, &tmp)
            .map_err(|e| Error::failure(format!("cannot create {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &latest).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::failure(format!("cannot move {} into place: {e}", tmp.display()))
        })
    }

    fn append(&mut self, line: &serde_json::Value) -> Result<(), Error> {
        writeln!(self.actions, "{line}")
            .map_err(|e| Error::failure(format!("cannot append to actions.jsonl: {e}")))?;
        self.actions
            .sync_all()
            .map_err(|e| Error::failure(format!("cannot fsync actions.jsonl: {e}")))
    }

    /// Backups live at `backups/<absolute path without its leading slash>`.
    fn backup_rel(path: &Path) -> Result<PathBuf, Error> {
        Ok(Path::new("backups").join(run_relative(path)?))
    }
}

/// Keep only the newest [`KEEP_RUNS`] run directories. Never touches anything else (§7).
fn prune(ctx: &Context) {
    let Ok(entries) = std::fs::read_dir(ctx.paths.doctor_runs_dir()) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    if dirs.len() <= KEEP_RUNS {
        return;
    }
    // Names begin with a UTC basic timestamp, so lexical order is chronological.
    dirs.sort();
    let drop = dirs.len() - KEEP_RUNS;
    for old in dirs.into_iter().take(drop) {
        let _ = std::fs::remove_dir_all(old);
    }
}

// ------------------------------------------------------------------ the chokepoint

/// One entry of the report's per-check `actions` array.
#[derive(Clone, Debug, Serialize)]
pub struct PlannedAction {
    pub seq: u64,
    pub check: String,
    pub class: String,
    pub action: String,
    pub writes_to: Vec<String>,
    pub outcome: String,
    pub error: Option<String>,
}

/// The bookkeeping every recorded action carries, in one value so a mutation helper takes
/// its own arguments rather than four of these.
#[derive(Copy, Clone)]
struct Step<'a> {
    seq: u64,
    check: &'a str,
    class: Class,
    started_at: &'a str,
}

pub struct Mutator<'a> {
    ctx: &'a Context,
    run: Option<RunDir>,
    dry_run: bool,
    seq: u64,
    /// One line per planned or applied action, for the report.
    pub log: Vec<PlannedAction>,
}

impl<'a> Mutator<'a> {
    pub fn new(ctx: &'a Context, run: Option<RunDir>, dry_run: bool) -> Mutator<'a> {
        Mutator {
            ctx,
            run,
            dry_run,
            seq: 0,
            log: Vec::new(),
        }
    }

    pub fn run_id(&self) -> Option<&str> {
        self.run.as_ref().map(|r| r.id.as_str())
    }
    pub fn run_dir(&self) -> Option<&Path> {
        self.run.as_ref().map(|r| r.dir.as_path())
    }
    pub fn applied(&self) -> usize {
        self.log.iter().filter(|a| a.outcome == "applied").count()
    }

    /// Apply (or, under `--dry-run`, only record) one action.
    pub fn mutate(&mut self, check: &str, class: Class, action: Action) -> Result<Outcome, Error> {
        self.seq += 1;
        let seq = self.seq;
        let started_at = fmonero_core::now_rfc3339();
        let describe = action.describe();
        let writes_to = action.writes_to();

        if self.dry_run {
            self.log.push(PlannedAction {
                seq,
                check: check.to_string(),
                class: class.name().to_string(),
                action: describe,
                writes_to,
                outcome: "planned".to_string(),
                error: None,
            });
            return Ok(Outcome::Planned);
        }

        let step = Step {
            seq,
            check,
            class,
            started_at: &started_at,
        };
        let result = self.apply(step, &action);
        let (outcome, error) = match &result {
            Ok(Outcome::Applied) => ("applied", None),
            Ok(Outcome::AlreadyDone) => ("already", None),
            Ok(Outcome::Planned) => ("planned", None),
            Err(e) => ("failed", Some(e.to_string())),
        };
        self.log.push(PlannedAction {
            seq,
            check: check.to_string(),
            class: class.name().to_string(),
            action: describe,
            writes_to,
            outcome: outcome.to_string(),
            error,
        });
        result
    }

    fn apply(&mut self, step: Step<'_>, action: &Action) -> Result<Outcome, Error> {
        match action {
            Action::WriteFile { path, bytes, mode } => self.write_file(step, path, bytes, *mode),
            Action::CreateDir { path, mode } => self.create_dir(step, path, *mode),
            Action::CreateSymlink { link, target } => self.create_symlink(step, link, target),
            Action::RunArgv { argv, inverse_argv } => self.run_argv(step, argv, inverse_argv),
        }
    }

    fn write_file(
        &mut self,
        step: Step<'_>,
        path: &Path,
        bytes: &[u8],
        mode: u32,
    ) -> Result<Outcome, Error> {
        let Step {
            seq,
            check,
            class,
            started_at,
        } = step;
        require_in_scope(self.ctx, path)?;
        let existing = std::fs::read(path).ok();
        let existing_mode = std::fs::metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o7777);
        if existing.as_deref() == Some(bytes) && existing_mode == Some(mode) {
            return Ok(Outcome::AlreadyDone);
        }
        let before_sha = existing.as_deref().map(sha256);

        // The backup is written and VERIFIED before anything is touched.
        let mut backup_rel: Option<String> = None;
        if let (Some(run), Some(old)) = (self.run.as_ref(), existing.as_deref()) {
            let rel = RunDir::backup_rel(path)?;
            let dest = run.dir.join(&rel);
            if let Some(parent) = dest.parent() {
                mkdir_private(parent)?;
            }
            fmonero_core::write_private(&dest, old)?;
            let written = std::fs::read(&dest)
                .map_err(|e| Error::failure(format!("cannot re-read the backup: {e}")))?;
            if written != old {
                return Err(Error::doctor_refused(format!(
                    "the backup of {} does not match the file; nothing was changed",
                    path.display()
                )));
            }
            backup_rel = Some(rel.display().to_string());
        } else if existing.is_some() && self.run.is_none() {
            return Err(Error::doctor_refused(
                "refusing to overwrite a file without a run directory to back it up into",
            ));
        }

        if let Some(parent) = path.parent() {
            mkdir_private(parent)?;
        }

        // The journal entry is written and fsynced BEFORE the rename. Applying first leaves
        // a window — a crash, a SIGKILL, ENOSPC — in which the file was replaced with
        // nothing in actions.jsonl, so `undo` silently skips it. The intent line carries
        // `ok: false`; the completion line after the rename carries `ok: true` and is what
        // `undo` acts on.
        let mut line = serde_json::json!({
            "seq": seq, "check": check, "class": class.name(), "started_at": started_at,
            "ok": false, "phase": "intent",
            "action": {"kind": "write_file", "path": path.display().to_string(),
                       "len": bytes.len(), "mode": format!("{mode:04o}")},
            "before": {"existed": existing.is_some(), "sha256": before_sha,
                       "mode": existing_mode.map(|m| format!("{m:04o}")), "backup": backup_rel},
            "after": {"sha256": sha256(bytes), "len": bytes.len()},
            "inverse": if existing.is_some() {
                serde_json::json!({"kind": "restore_file", "path": path.display().to_string(),
                                   "backup": backup_rel, "sha256": before_sha,
                                   "mode": existing_mode.map(|m| format!("{m:04o}"))})
            } else {
                // The doctor never removes a file (§7): a file this run created is moved
                // aside on undo, never unlinked.
                serde_json::json!({"kind": "quarantine_file", "path": path.display().to_string()})
            },
        });
        self.record(&line)?;

        write_at_mode(path, bytes, mode)?;

        if let Some(map) = line.as_object_mut() {
            map.insert("ok".into(), serde_json::json!(true));
            map.insert("phase".into(), serde_json::json!("done"));
        }
        self.record(&line)?;
        Ok(Outcome::Applied)
    }

    fn create_dir(&mut self, step: Step<'_>, path: &Path, mode: u32) -> Result<Outcome, Error> {
        let Step {
            seq,
            check,
            class,
            started_at,
        } = step;
        require_in_scope(self.ctx, path)?;
        if path.is_dir() {
            return Ok(Outcome::AlreadyDone);
        }
        if path.exists() {
            return Err(Error::doctor_refused(format!(
                "{} exists and is not a directory; the doctor never removes what it did not \
                 create",
                path.display()
            ))
            .with_hint(format!("ls -l {}", path.display())));
        }
        // Not `recursive`: one CreateDir per missing ancestor, each individually undoable
        // (§7.1). A missing parent is a refusal, which is what keeps the pairing honest.
        std::fs::DirBuilder::new()
            .mode(mode)
            .create(path)
            .map_err(|e| Error::failure(format!("cannot create {}: {e}", path.display())))?;
        let line = serde_json::json!({
            "seq": seq, "check": check, "class": class.name(), "started_at": started_at,
            "ok": true,
            "action": {"kind": "create_dir", "path": path.display().to_string(),
                       "mode": format!("{mode:04o}")},
            "before": {"existed": false},
            "after": {"existed": true},
            // The one removal §7 sanctions, and only while it is still empty.
            "inverse": {"kind": "remove_dir", "path": path.display().to_string()},
        });
        self.record(&line)?;
        Ok(Outcome::Applied)
    }

    fn create_symlink(
        &mut self,
        step: Step<'_>,
        link: &Path,
        target: &Path,
    ) -> Result<Outcome, Error> {
        let Step {
            seq,
            check,
            class,
            started_at,
        } = step;
        require_in_scope(self.ctx, link)?;
        if std::fs::read_link(link).ok().as_deref() == Some(target) {
            return Ok(Outcome::AlreadyDone);
        }
        if std::fs::symlink_metadata(link).is_ok() {
            return Err(Error::doctor_refused(format!(
                "{} already exists and is not the link fmonero would make; the doctor never \
                 removes what it did not create",
                link.display()
            ))
            .with_hint(format!("ls -l {}", link.display())));
        }
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::failure(format!("cannot create {}: {e}", parent.display())))?;
        }
        std::os::unix::fs::symlink(target, link)
            .map_err(|e| Error::failure(format!("cannot link {}: {e}", link.display())))?;
        let line = serde_json::json!({
            "seq": seq, "check": check, "class": class.name(), "started_at": started_at,
            "ok": true,
            "action": {"kind": "create_symlink", "link": link.display().to_string(),
                       "target": target.display().to_string()},
            "before": {"existed": false},
            "after": {"link": target.display().to_string()},
            "inverse": {"kind": "remove_symlink", "link": link.display().to_string()},
        });
        self.record(&line)?;
        Ok(Outcome::Applied)
    }

    fn run_argv(
        &mut self,
        step: Step<'_>,
        argv: &[String],
        inverse_argv: &[String],
    ) -> Result<Outcome, Error> {
        let Step {
            seq,
            check,
            class,
            started_at,
        } = step;
        // argv only, never a shell string (§2f).
        let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
        require_test_safe_argv(self.ctx, &borrowed)?;
        if proc::which(borrowed[0]).is_none() {
            return Err(Error::backend(format!("{} is not on PATH", borrowed[0])));
        }
        let out = proc::run_with_deadline(&borrowed, self.ctx.timeouts.systemctl, None)?;
        if out.timed_out {
            return Err(Error::upstream(format!(
                "`{}` was killed at its {}s deadline",
                argv.join(" "),
                self.ctx.timeouts.systemctl.as_secs()
            )));
        }
        if !out.success() {
            return Err(Error::upstream(format!(
                "`{}` failed: {}",
                argv.join(" "),
                out.stderr_tail().trim()
            )));
        }
        let line = serde_json::json!({
            "seq": seq, "check": check, "class": class.name(), "started_at": started_at,
            "ok": true,
            "action": {"kind": "run_argv", "argv": argv},
            // `before`/`after` are null for RunArgv, whose reversal is `inverse_argv` alone.
            "before": serde_json::Value::Null,
            "after": {"code": out.code},
            "inverse": if inverse_argv.is_empty() {
                serde_json::json!({"kind": "none", "note": "this step has no inverse"})
            } else {
                serde_json::json!({"kind": "run_argv", "argv": inverse_argv})
            },
        });
        self.record(&line)?;
        Ok(Outcome::Applied)
    }

    fn record(&mut self, line: &serde_json::Value) -> Result<(), Error> {
        match self.run.as_mut() {
            Some(run) => run.append(line),
            // A mutation with nowhere to record it is not allowed to happen.
            None => Err(Error::doctor_refused(
                "no run directory: a mutation must be recorded before it is applied",
            )),
        }
    }
}

/// Write `bytes` at exactly `mode`, through `.tmp.<pid>` + `rename(2)`, with the mode set at
/// creation and never `chmod`ed afterwards (§2b.7, §7's Axiom 5).
fn write_at_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<(), Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path
        .parent()
        .ok_or_else(|| Error::failure(format!("{} has no parent directory", path.display())))?;
    let tmp = dir.join(format!(
        ".{}.fmonero-doctor.tmp.{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into()),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
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

// ------------------------------------------------------------------ planning

/// Turn a detector's [`PlanKind`] into the actions that would repair it.
///
/// This is the impure half: it may re-read a file the probes only summarised. It writes
/// nothing — every real write goes through [`Mutator::mutate`].
pub fn plan(kind: PlanKind, ctx: &Context, p: &Probes) -> Result<Vec<Action>, Error> {
    let unit_dir = probes::systemd_user_dir(ctx);
    // A unit rewritten without a reload is a fixer that reports success and changes
    // nothing (§7.1), so every unit write is followed by one.
    let reload = || Action::RunArgv {
        argv: vec!["systemctl".into(), "--user".into(), "daemon-reload".into()],
        inverse_argv: vec!["systemctl".into(), "--user".into(), "daemon-reload".into()],
    };
    match kind {
        PlanKind::WriteNodeUnit => Ok(vec![
            Action::WriteFile {
                path: unit_dir.join(&ctx.settings.node.unit),
                bytes: units::render_node_unit(ctx).into_bytes(),
                mode: 0o644,
            },
            reload(),
        ]),
        PlanKind::WriteWalletUnit => Ok(vec![
            Action::WriteFile {
                path: unit_dir.join(&ctx.settings.wallet.unit),
                bytes: units::render_wallet_unit(ctx).into_bytes(),
                mode: 0o644,
            },
            reload(),
        ]),
        PlanKind::WriteBothUnits => Ok(vec![
            Action::WriteFile {
                path: unit_dir.join(&ctx.settings.node.unit),
                bytes: units::render_node_unit(ctx).into_bytes(),
                mode: 0o644,
            },
            Action::WriteFile {
                path: unit_dir.join(&ctx.settings.wallet.unit),
                bytes: units::render_wallet_unit(ctx).into_bytes(),
                mode: 0o644,
            },
            reload(),
        ]),

        // §7.1: `node.network` is the authority, not either observed value. The order is
        // fixed — settings first, then the unit — so a half-applied fix leaves the settings
        // and the login-file path agreeing with each other.
        PlanKind::NormaliseWalletPort => {
            let want = ctx.settings.node.network.wallet_rpc_port();
            let mut repaired = ctx.settings.clone();
            repaired.wallet.port = want;
            // Rewrite the port inside wallet.url and keep its host: §2c already refuses a
            // non-loopback one, so whatever host is there is loopback and the user's.
            let host = repaired
                .wallet
                .url
                .trim_start_matches("http://")
                .trim_end_matches('/')
                .rsplit_once(':')
                .map(|(h, _)| h.to_string())
                .unwrap_or_else(|| "127.0.0.1".to_string());
            repaired.wallet.url = format!("http://{host}:{want}");
            let mut ctx_after = ctx.clone();
            ctx_after.settings = repaired.clone();
            Ok(vec![
                Action::WriteFile {
                    path: ctx.paths.config_file(),
                    bytes: repaired.to_toml_bytes()?,
                    mode: 0o600,
                },
                Action::WriteFile {
                    path: unit_dir.join(&ctx.settings.wallet.unit),
                    bytes: units::render_wallet_unit(&ctx_after).into_bytes(),
                    mode: 0o644,
                },
                reload(),
            ])
        }

        // One CreateDir per missing ancestor, outermost first, so each level stays
        // individually undoable (§7.1).
        PlanKind::CreateWalletDir => Ok(p
            .wallet_dir_missing
            .iter()
            .map(|d| Action::CreateDir {
                path: PathBuf::from(d),
                mode: 0o700,
            })
            .collect()),

        PlanKind::DisableUnits => Ok([&p.node_unit_state, &p.wallet_unit_state]
            .iter()
            .filter(|s| s.enabled)
            .map(|s| Action::RunArgv {
                argv: vec![
                    "systemctl".into(),
                    "--user".into(),
                    "disable".into(),
                    s.name.clone(),
                ],
                inverse_argv: vec![
                    "systemctl".into(),
                    "--user".into(),
                    "enable".into(),
                    s.name.clone(),
                ],
            })
            .collect()),

        PlanKind::ChmodMonerodConf => {
            let path = ctx.paths.monerod_conf().display().to_string();
            let old = p
                .monerod_conf
                .mode
                .ok_or_else(|| Error::doctor_refused("monerod.conf has no mode to restore"))?;
            Ok(vec![Action::RunArgv {
                argv: vec!["chmod".into(), "600".into(), path.clone()],
                inverse_argv: vec!["chmod".into(), format!("{old:o}"), path],
            }])
        }

        // chmod with the prior modes as `inverse_argv`, one action per file, and never for
        // the two names the wallet daemon owns (§7.1, class D).
        PlanKind::FixStatePerms => Ok(p
            .perms
            .iter()
            .filter(|x| !x.ok() && !x.daemon_owned)
            .map(|x| Action::RunArgv {
                argv: vec!["chmod".into(), format!("{:o}", x.want), x.path.clone()],
                inverse_argv: vec!["chmod".into(), format!("{:o}", x.mode), x.path.clone()],
            })
            .collect()),

        PlanKind::ResetStateJson => Ok(vec![Action::WriteFile {
            path: ctx.paths.state_file(),
            bytes: fmonero_core::StateDoc::default().to_bytes()?,
            mode: 0o600,
        }]),

        // The whole document when nothing parsed; only the bad keys when it did.
        PlanKind::RepairSettings => Ok(vec![Action::WriteFile {
            path: ctx.paths.config_file(),
            bytes: repaired_settings(p)?,
            mode: 0o600,
        }]),

        PlanKind::LinkPlugin => {
            let target = super::repo_plugin_dir().ok_or_else(|| {
                Error::doctor_refused(
                    "cannot find this repository's plugin/oliver.monero, and a symlinked plugin \
                     directory has to point at a real checkout (§1.5)",
                )
                .with_hint("cd ~/Projects/odcpw/fmonero && fmonero setup --plugin --yes")
            })?;
            Ok(vec![Action::CreateSymlink {
                link: ctx.paths.plugin_link(),
                target,
            }])
        }

        PlanKind::EnablePlugin => Ok(vec![
            Action::RunArgv {
                argv: vec![
                    "omarchy".into(),
                    "plugin".into(),
                    "enable".into(),
                    PLUGIN_ID.into(),
                ],
                inverse_argv: vec![
                    "omarchy".into(),
                    "plugin".into(),
                    "disable".into(),
                    PLUGIN_ID.into(),
                ],
            },
            Action::RunArgv {
                argv: vec![
                    "omarchy".into(),
                    "bar".into(),
                    "put".into(),
                    PLUGIN_ID.into(),
                    "--before".into(),
                    "omarchy.tailscale".into(),
                ],
                // `omarchy bar` has no `remove`; disabling the plugin takes the widget out
                // of the bar, and that is the inverse of the step above.
                inverse_argv: Vec::new(),
            },
        ]),

        // `setup --menu` merges TEXTUALLY (§4.2): a serde round trip would delete every
        // comment in a file the owner maintains by hand, so the doctor shells out to the one
        // implementation rather than writing a second merger.
        PlanKind::MergeMenu => Ok(vec![Action::RunArgv {
            argv: vec![
                ctx.paths
                    .local_bin
                    .join(fmonero_core::NAME)
                    .display()
                    .to_string(),
                "setup".into(),
                "--menu".into(),
                "--yes".into(),
            ],
            // The merge only ever inserts lines, and `setup --menu` backs the file up
            // verbatim into `backups/` before touching it (§4.2). There is no inverse argv;
            // the recovery path is that backup, which the detail names.
            inverse_argv: Vec::new(),
        }]),
    }
}

/// Why a planned repair cannot run here, when a test state directory narrows the scopes.
///
/// Asked **before** the first mutation so a sandboxed `--fix` reports "skipped, and here is
/// why" rather than a failed fixer: the chokepoint would refuse the write anyway, but a
/// refusal counted as `fix_failed` would make every run under `FMONERO_STATE_DIR` exit 12 and
/// bury the class-A round trip the Phase 2 gate actually measures.
pub fn refused_here(ctx: &Context, actions: &[Action]) -> Option<String> {
    if !fmonero_core::paths::under_test_state_dir() {
        return None;
    }
    for action in actions {
        let outside = match action {
            Action::WriteFile { path, .. } | Action::CreateDir { path, .. } => {
                (!in_scope(ctx, path)).then(|| format!("touch {}", path.display()))
            }
            Action::CreateSymlink { link, .. } => {
                (!in_scope(ctx, link)).then(|| format!("touch {}", link.display()))
            }
            Action::RunArgv { argv, .. } => {
                let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
                require_test_safe_argv(ctx, &borrowed)
                    .err()
                    .map(|_| format!("run `{}`", argv.join(" ")))
            }
        };
        if let Some(what) = outside {
            return Some(format!(
                "skipped: FMONERO_STATE_DIR is set, so the doctor will not {what} — the same \
                 guard §4.2 puts on units::install"
            ));
        }
    }
    None
}

/// The repaired `config.toml`: every valid key kept, every invalid one reset to its default,
/// every unknown one dropped. A document that did not parse at all becomes the defaults.
///
/// It is the **same** replay `settings_valid` reports on — `probes::read_settings_document` —
/// rather than a second one, so the fixer can no longer reset a key the detector considers
/// fine. That is not tidiness: the two replays disagreeing is precisely how `doctor --fix`
/// came to rewrite `node.mode` from `remote` to `local` on a correctly configured machine
/// (§2m.1), because a `node.mode = remote` applied before `node.remote` fails to set and a
/// failed `set` here silently leaves the default behind.
fn repaired_settings(p: &Probes) -> Result<Vec<u8>, Error> {
    let Some(text) = &p.settings_file.text else {
        return Settings::default().to_toml_bytes();
    };
    if p.settings_parse_error.is_some() {
        return Settings::default().to_toml_bytes();
    }
    let doc = super::probes::read_settings_document(text);
    if let Some(e) = doc.parse_error {
        return Err(Error::doctor_refused(format!(
            "config.toml no longer parses: {e}"
        )));
    }
    doc.settings.to_toml_bytes()
}

// ------------------------------------------------------------------ undo

#[derive(Debug, Deserialize)]
struct ActionLine {
    seq: u64,
    /// Read from `actions.jsonl` and **deliberately not rendered**: it is a check id, and §2o rule
    /// 5 puts an id in `--json` and nowhere else. The journal on disk keeps it for the reader who
    /// wants to know which check wrote a line.
    #[serde(default)]
    #[allow(dead_code)]
    check: String,
    #[serde(default)]
    ok: bool,
    /// `"intent"` (written before the mutation) or `"done"` (written after it). Absent on
    /// the variants that record once, after the fact.
    #[serde(default)]
    phase: String,
    inverse: serde_json::Value,
}

/// One step of an undo, as §5c's `doctor undo` payload spells it: `{seq, action, path, ok}`.
#[derive(Debug, Serialize)]
pub struct UndoStep {
    pub seq: u64,
    pub action: String,
    pub path: String,
    pub ok: bool,
    /// Not part of §5c's key set; carried for the human rendering and skipped in JSON.
    #[serde(skip)]
    pub note: Option<String>,
}

/// Resolve `latest` or a run id to its directory.
pub fn resolve_run(ctx: &Context, run: &str) -> Result<PathBuf, Error> {
    if run == "latest" {
        let latest = ctx.paths.doctor_latest();
        let dir = std::fs::canonicalize(&latest).map_err(|_| {
            Error::not_found(format!("there is no {}", latest.display()))
                .with_hint("fmonero doctor --fix --dry-run")
        })?;
        return Ok(dir);
    }
    let root = ctx.paths.doctor_runs_dir();
    let entries = std::fs::read_dir(&root).map_err(|_| {
        Error::not_found(format!("there are no doctor runs under {}", root.display()))
            .with_hint("fmonero doctor --fix")
    })?;
    let mut matches: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            name == run || name.ends_with(&format!("__{run}"))
        })
        .collect();
    matches.sort();
    matches.pop().ok_or_else(|| {
        Error::not_found(format!("no doctor run matches '{run}'"))
            .with_hint(format!("ls {}", root.display()))
    })
}

/// Replay one run's `actions.jsonl` in reverse (§7).
///
/// Refuses with exit **13** when a `WriteFile` backup is missing or its hash does not match
/// what was recorded. Nothing is ever deleted: a file the run created is moved into the
/// run's `quarantine/`.
pub fn undo(ctx: &Context, dir: &Path) -> Result<Vec<UndoStep>, Error> {
    let path = dir.join("actions.jsonl");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| Error::not_found(format!("cannot read {}: {e}", path.display())))?;
    let mut lines: Vec<ActionLine> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // A crash mid-write leaves a torn last line; ignore it rather than refuse.
        if let Ok(parsed) = serde_json::from_str::<ActionLine>(line) {
            lines.push(parsed);
        }
    }
    lines.reverse();

    // Every seq that reached its completion line. An `intent` without one is a run that
    // stopped between the journal entry and the write.
    let completed: std::collections::HashSet<u64> =
        lines.iter().filter(|l| l.ok).map(|l| l.seq).collect();

    let mut steps = Vec::new();
    for line in lines {
        let kind = line
            .inverse
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("none")
            .to_string();
        let target = line
            .inverse
            .get("path")
            .or_else(|| line.inverse.get("link"))
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string();
        if !line.ok {
            if line.phase == "intent" && !completed.contains(&line.seq) {
                let backup = line
                    .inverse
                    .get("backup")
                    .and_then(|b| b.as_str())
                    .map(|b| format!("backup available at {}", dir.join(b).display()));
                steps.push(UndoStep {
                    seq: line.seq,
                    action: kind,
                    path: target,
                    ok: false,
                    // §2o rule 12 and rule 1: an em-dash joining two clauses, and a semicolon
                    // joining two sentences, in the message a person reads after an interrupted
                    // run. It is two sentences now.
                    note: Some(
                        backup.unwrap_or_default()
                            + ". The run stopped between writing the record and writing the file, \
                               so whether it applied is not known. Nothing was undone.",
                    ),
                });
            }
            continue;
        }
        let mut note = line
            .inverse
            .get("note")
            .and_then(|n| n.as_str())
            .map(str::to_string);
        let (ok, outcome) = match kind.as_str() {
            "restore_file" => (true, restore_file(ctx, dir, &line.inverse)?),
            "quarantine_file" => (true, quarantine(ctx, dir, &line.inverse)?),
            "remove_dir" => (true, remove_dir(ctx, &line.inverse)?),
            "remove_symlink" => (true, remove_symlink(ctx, &line.inverse)?),
            "run_argv" => {
                let argv: Vec<String> = line
                    .inverse
                    .get("argv")
                    .and_then(|a| serde_json::from_value(a.clone()).ok())
                    .unwrap_or_default();
                let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
                if borrowed.is_empty() {
                    (true, "nothing to undo".to_string())
                } else {
                    // An inverse argv runs the same commands the forward one did, so it is
                    // bounded by the same guard.
                    require_test_safe_argv(ctx, &borrowed)?;
                    let out = proc::run_with_deadline(&borrowed, ctx.timeouts.systemctl, None)?;
                    if out.success() {
                        (true, format!("ran {}", argv.join(" ")))
                    } else {
                        note = Some(out.stderr_tail().trim().to_string());
                        (false, "failed".to_string())
                    }
                }
            }
            _ => (true, "nothing to undo".to_string()),
        };
        if note.is_none() {
            note = Some(outcome);
        }
        steps.push(UndoStep {
            seq: line.seq,
            action: kind,
            path: target,
            ok,
            note,
        });
    }
    Ok(steps)
}

fn restore_file(ctx: &Context, dir: &Path, inverse: &serde_json::Value) -> Result<String, Error> {
    let target = inverse
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| Error::doctor_refused("an undo record has no path"))?;
    // Undo is bounded by the same write scopes as apply.
    require_in_scope(ctx, Path::new(target))?;
    let rel = inverse
        .get("backup")
        .and_then(|p| p.as_str())
        .ok_or_else(|| {
            Error::doctor_refused(format!(
                "no backup was recorded for {target}; refusing to guess"
            ))
        })?;
    let backup = dir.join(rel);
    let bytes = std::fs::read(&backup).map_err(|e| {
        Error::doctor_refused(format!(
            "the backup {} is missing or unreadable ({e}); {target} was left alone",
            backup.display()
        ))
    })?;
    if let Some(want) = inverse.get("sha256").and_then(|s| s.as_str()) {
        let got = sha256(&bytes);
        if got != want {
            return Err(Error::doctor_refused(format!(
                "the backup of {target} has changed since the run (sha256 {got} != {want}); \
                 nothing was restored"
            )));
        }
    }
    let mode = inverse
        .get("mode")
        .and_then(|m| m.as_str())
        .and_then(|m| u32::from_str_radix(m, 8).ok())
        .unwrap_or(0o600);
    write_at_mode(Path::new(target), &bytes, mode)?;
    Ok(format!("restored {target}"))
}

/// A file the run created: moved aside, never deleted (§7).
fn quarantine(ctx: &Context, dir: &Path, inverse: &serde_json::Value) -> Result<String, Error> {
    let target = inverse
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| Error::doctor_refused("an undo record has no path"))?;
    let path = Path::new(target);
    require_in_scope(ctx, path)?;
    if !path.exists() {
        return Ok("already absent".to_string());
    }
    let rel = run_relative(path)?;
    let dest = dir.join("quarantine").join(rel);
    if let Some(parent) = dest.parent() {
        mkdir_private(parent)?;
    }
    std::fs::rename(path, &dest)
        .map_err(|e| Error::failure(format!("cannot move {target} aside: {e}")))?;
    Ok(format!("moved to {}", dest.display()))
}

/// `CreateDir`'s inverse: `rmdir`, and **only while it is still empty** (§7).
fn remove_dir(ctx: &Context, inverse: &serde_json::Value) -> Result<String, Error> {
    let target = inverse
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| Error::doctor_refused("an undo record has no path"))?;
    let path = Path::new(target);
    require_in_scope(ctx, path)?;
    if !path.exists() {
        return Ok("already absent".to_string());
    }
    let empty = std::fs::read_dir(path)
        .map(|mut it| it.next().is_none())
        .unwrap_or(false);
    if !empty {
        return Ok(format!(
            "{target} is no longer empty; left in place (§7: rmdir only while empty)"
        ));
    }
    std::fs::remove_dir(path).map_err(|e| Error::failure(format!("cannot rmdir {target}: {e}")))?;
    Ok(format!("removed {target}"))
}

fn remove_symlink(ctx: &Context, inverse: &serde_json::Value) -> Result<String, Error> {
    let link = inverse
        .get("link")
        .and_then(|p| p.as_str())
        .ok_or_else(|| Error::doctor_refused("an undo record has no link"))?;
    let path = Path::new(link);
    require_in_scope(ctx, path)?;
    match std::fs::symlink_metadata(path) {
        Err(_) => Ok("already absent".to_string()),
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::remove_file(path)
                .map_err(|e| Error::failure(format!("cannot remove {link}: {e}")))?;
            Ok(format!("removed the link {link}"))
        }
        Ok(_) => Err(Error::doctor_refused(format!(
            "{link} is no longer a symlink; the doctor will not remove it"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx(name: &str) -> (Context, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "fmonero-doctor-mutate-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = Context::for_test(&dir).expect("test context");
        (ctx, dir)
    }

    #[test]
    fn a_write_outside_the_scopes_is_refused_and_a_parent_dir_component_cannot_escape() {
        let (ctx, dir) = test_ctx("scope");
        let run = RunDir::create(&ctx).expect("run dir");
        let mut m = Mutator::new(&ctx, Some(run), false);
        let err = m
            .mutate(
                "x",
                Class::A,
                Action::WriteFile {
                    path: PathBuf::from("/etc/passwd"),
                    bytes: b"nope".to_vec(),
                    mode: 0o600,
                },
            )
            .expect_err("must refuse");
        assert_eq!(err.exit_code(), 13, "{err}");

        // `Path::starts_with` compares components and does not resolve `..`, so this would
        // otherwise escape the guard while looking like an in-scope path.
        let escape = ctx.paths.state_dir.join("../../../tmp/fmonero-escape");
        let err = m
            .mutate(
                "x",
                Class::A,
                Action::WriteFile {
                    path: escape.clone(),
                    bytes: b"nope".to_vec(),
                    mode: 0o600,
                },
            )
            .expect_err("a `..` component must be refused");
        assert_eq!(err.exit_code(), 13, "{err}");
        assert!(!escape.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn undo_refuses_a_target_outside_the_write_scopes() {
        // Undo takes its targets from actions.jsonl, which is a file on disk like any other.
        let (ctx, dir) = test_ctx("undo-scope");
        let run = RunDir::create(&ctx).expect("run dir");
        let run_dir = run.dir.clone();
        drop(run);
        let forged = serde_json::json!({
            "seq": 1, "check": "forged", "ok": true, "phase": "done",
            "inverse": {"kind": "restore_file", "path": "/etc/passwd",
                        "backup": "backups/etc/passwd"},
        });
        std::fs::write(run_dir.join("actions.jsonl"), format!("{forged}\n")).unwrap();
        let err = undo(&ctx, &run_dir).expect_err("must refuse");
        assert_eq!(err.exit_code(), 13, "{err}");
        assert!(err.to_string().contains("write scopes"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_or_altered_backup_refuses_with_thirteen() {
        let (ctx, dir) = test_ctx("undo-hash");
        let target = ctx.paths.state_dir.join("state.json");
        std::fs::write(&target, b"original").unwrap();
        let run = RunDir::create(&ctx).expect("run dir");
        let run_dir = run.dir.clone();
        let mut m = Mutator::new(&ctx, Some(run), false);
        m.mutate(
            "state_valid",
            Class::A,
            Action::WriteFile {
                path: target.clone(),
                bytes: b"replacement".to_vec(),
                mode: 0o600,
            },
        )
        .expect("write");
        drop(m);
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement");

        // Tamper with the backup: undo must refuse rather than restore other bytes.
        let backup = run_dir
            .join("backups")
            .join(target.strip_prefix("/").unwrap_or(&target));
        std::fs::write(&backup, b"tampered").unwrap();
        let err = undo(&ctx, &run_dir).expect_err("must refuse");
        assert_eq!(err.exit_code(), 13, "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_journal_entry_is_written_before_the_mutation_and_the_round_trip_is_byte_exact() {
        let (ctx, dir) = test_ctx("roundtrip");
        let target = ctx.paths.config_dir.join("config.toml");
        std::fs::write(&target, b"original = true\n").unwrap();
        let run = RunDir::create(&ctx).expect("run dir");
        let run_dir = run.dir.clone();
        let mut m = Mutator::new(&ctx, Some(run), false);
        m.mutate(
            "settings_valid",
            Class::A,
            Action::WriteFile {
                path: target.clone(),
                bytes: b"repaired = true\n".to_vec(),
                mode: 0o600,
            },
        )
        .expect("write");
        // Run twice = run once.
        assert_eq!(
            m.mutate(
                "settings_valid",
                Class::A,
                Action::WriteFile {
                    path: target.clone(),
                    bytes: b"repaired = true\n".to_vec(),
                    mode: 0o600,
                },
            )
            .expect("second write"),
            Outcome::AlreadyDone
        );
        drop(m);

        let journal = std::fs::read_to_string(run_dir.join("actions.jsonl")).unwrap();
        let mut phases: Vec<String> = Vec::new();
        for line in journal.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            phases.push(v["phase"].as_str().unwrap_or("").to_string());
        }
        assert_eq!(phases, vec!["intent", "done"], "{journal}");

        let steps = undo(&ctx, &run_dir).expect("undo");
        assert_eq!(steps.len(), 1);
        assert!(steps[0].ok);
        assert_eq!(std::fs::read(&target).unwrap(), b"original = true\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_the_run_created_is_quarantined_on_undo_and_never_unlinked() {
        let (ctx, dir) = test_ctx("quarantine");
        let target = ctx.paths.state_dir.join("state.json");
        let run = RunDir::create(&ctx).expect("run dir");
        let run_dir = run.dir.clone();
        let mut m = Mutator::new(&ctx, Some(run), false);
        m.mutate(
            "state_valid",
            Class::A,
            Action::WriteFile {
                path: target.clone(),
                bytes: b"{}\n".to_vec(),
                mode: 0o600,
            },
        )
        .expect("write");
        drop(m);
        assert!(target.exists());
        undo(&ctx, &run_dir).expect("undo");
        assert!(!target.exists(), "the created file is moved aside");
        let moved = run_dir
            .join("quarantine")
            .join(target.strip_prefix("/").unwrap_or(&target));
        assert!(
            moved.exists(),
            "…and it still exists at {}",
            moved.display()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_created_directory_is_rmdired_only_while_empty() {
        let (ctx, dir) = test_ctx("createdir");
        let leaf = ctx.paths.state_dir.join("wallets-test");
        let run = RunDir::create(&ctx).expect("run dir");
        let run_dir = run.dir.clone();
        let mut m = Mutator::new(&ctx, Some(run), false);
        m.mutate(
            "wallet_dir_exists",
            Class::B,
            Action::CreateDir {
                path: leaf.clone(),
                mode: 0o700,
            },
        )
        .expect("mkdir");
        drop(m);
        assert_eq!(
            std::fs::metadata(&leaf).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        std::fs::write(leaf.join("keep"), b"x").unwrap();
        undo(&ctx, &run_dir).expect("undo");
        assert!(leaf.exists(), "a non-empty directory is left alone");
        std::fs::remove_file(leaf.join("keep")).unwrap();
        undo(&ctx, &run_dir).expect("undo again");
        assert!(!leaf.exists(), "an empty one is rmdir'ed");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The guard that keeps the Phase 2 gate's own `--fix` out of the owner's home.
    ///
    /// It is the one test here that touches the process environment, and it is safe to run
    /// beside the others precisely because of what it asserts: with `FMONERO_STATE_DIR` set
    /// the scopes are the three redirected directories, which is where every other test in
    /// this file writes.
    #[test]
    fn under_a_test_state_dir_no_fixer_may_reach_outside_it() {
        let (ctx, dir) = test_ctx("guard");
        std::env::set_var("FMONERO_STATE_DIR", &ctx.paths.state_dir);
        let scopes = write_scopes(&ctx);
        let unit = ctx.paths.systemd_user_dir.join(&ctx.settings.node.unit);
        let plugin = ctx.paths.plugin_link();
        let refused_write = refused_here(
            &ctx,
            &[Action::WriteFile {
                path: unit.clone(),
                bytes: b"[Unit]\n".to_vec(),
                mode: 0o644,
            }],
        );
        let refused_link = refused_here(
            &ctx,
            &[Action::CreateSymlink {
                link: plugin,
                target: PathBuf::from("/tmp"),
            }],
        );
        let refused_systemctl = refused_here(
            &ctx,
            &[Action::RunArgv {
                argv: vec![
                    "systemctl".into(),
                    "--user".into(),
                    "disable".into(),
                    "fmonero-monerod.service".into(),
                ],
                inverse_argv: Vec::new(),
            }],
        );
        let allowed_chmod = refused_here(
            &ctx,
            &[Action::RunArgv {
                argv: vec![
                    "chmod".into(),
                    "600".into(),
                    ctx.paths.state_file().display().to_string(),
                ],
                inverse_argv: Vec::new(),
            }],
        );
        let allowed_state = refused_here(
            &ctx,
            &[Action::WriteFile {
                path: ctx.paths.state_file(),
                bytes: b"{}\n".to_vec(),
                mode: 0o600,
            }],
        );
        std::env::remove_var("FMONERO_STATE_DIR");

        assert_eq!(scopes.len(), 3, "{scopes:?}");
        assert!(refused_write.is_some(), "a unit file must be refused");
        assert!(refused_link.is_some(), "the plugin symlink must be refused");
        assert!(
            refused_systemctl.is_some(),
            "systemctl --user disable must be refused"
        );
        assert!(allowed_chmod.is_none(), "an in-scope chmod stays allowed");
        assert!(
            allowed_state.is_none(),
            "state.json is exactly what a test state dir redirects"
        );
        // And the chokepoint itself refuses, not only the pre-check.
        std::env::set_var("FMONERO_STATE_DIR", &ctx.paths.state_dir);
        let run = RunDir::create(&ctx).expect("run dir");
        let mut m = Mutator::new(&ctx, Some(run), false);
        let err = m
            .mutate(
                "units_present",
                Class::B,
                Action::WriteFile {
                    path: unit.clone(),
                    bytes: b"[Unit]\n".to_vec(),
                    mode: 0o644,
                },
            )
            .expect_err("the chokepoint must refuse it too");
        std::env::remove_var("FMONERO_STATE_DIR");
        assert_eq!(err.exit_code(), 13, "{err}");
        assert!(!unit.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_dry_run_records_a_plan_and_touches_nothing() {
        let (ctx, dir) = test_ctx("dryrun");
        let target = ctx.paths.state_dir.join("state.json");
        let mut m = Mutator::new(&ctx, None, true);
        let outcome = m
            .mutate(
                "state_valid",
                Class::A,
                Action::WriteFile {
                    path: target.clone(),
                    bytes: b"{}".to_vec(),
                    mode: 0o600,
                },
            )
            .expect("plan");
        assert_eq!(outcome, Outcome::Planned);
        assert!(!target.exists());
        assert_eq!(m.log.len(), 1);
        assert_eq!(m.log[0].outcome, "planned");
        assert_eq!(m.log[0].writes_to, vec![target.display().to_string()]);
        assert!(m.run_dir().is_none(), "a dry run creates no run directory");
        let _ = std::fs::remove_dir_all(dir);
    }
}
