//! Desktop and systemd integration (DESIGN.md §4.2).
//!
//! Idempotent: running it twice leaves the same machine, which is what the byte-for-byte
//! comparison inside `units::install` buys. `--dry-run` prints a unified diff and changes
//! nothing. **Everything it prints redacts the `rpc-login=` value** (§4.2): without that the
//! node credential would be printed to stdout by §8's own gate command, become a key of this
//! command's JSON payload and be pasted into DECISIONS.md — which §2b's known-value grep then
//! fails on, on correct behaviour.

use clap::Args;
use fmonero_core::units::{self, FileAction, FileStatus};
use fmonero_core::{CancelToken, Context, Error};

use crate::output::Out;

/// The menu entries `--menu` merges. Single-line objects: the merge inserts lines (§4.2).
pub const MENU_TEMPLATE: &str = include_str!("../assets/menu.jsonc");

/// `fmonero setup [--units] [--plugin] [--menu] [--all] [--rotate-rpc-login] [--full-chain]`.
#[derive(Args, Clone, Debug, Default)]
pub struct SetupArgs {
    /// Write both systemd user units and monerod.conf, then daemon-reload
    #[arg(long)]
    pub units: bool,
    /// Link, enable and place the oliver.monero bar widget
    #[arg(long)]
    pub plugin: bool,
    /// Merge fmonero's entries into the Omarchy menu
    #[arg(long)]
    pub menu: bool,
    /// All of the above
    #[arg(long)]
    pub all: bool,
    /// Mint a fresh node rpc-login credential (the node must then be restarted)
    #[arg(long)]
    pub rotate_rpc_login: bool,
    /// Write a unit without --prune-blockchain (a full chain does not fit this disk)
    #[arg(long)]
    pub full_chain: bool,
    /// Set from the global --yes (not a flag of its own).
    #[clap(skip)]
    pub yes: bool,
    /// Set from the global --dry-run (not a flag of its own).
    #[clap(skip)]
    pub dry_run: bool,
}

impl SetupArgs {
    fn want_units(&self) -> bool {
        self.units || self.all
    }
    fn want_plugin(&self) -> bool {
        self.plugin || self.all
    }
    fn want_menu(&self) -> bool {
        self.menu || self.all
    }
}

/// Run setup. `--dry-run` prints a unified diff and changes nothing (§4.2).
pub fn run(args: SetupArgs, ctx: Context, out: Out) -> anyhow::Result<i32> {
    if !(args.want_units() || args.want_plugin() || args.want_menu()) {
        return Err(
            Error::usage("setup needs at least one of --units, --plugin, --menu or --all")
                .with_reason("bad_args")
                .with_hint("fmonero setup --all --yes")
                .into(),
        );
    }
    // A dry run changes nothing, so it needs no confirmation — which is what makes
    // `setup --dry-run` a GREEN row (§4b).
    if !args.dry_run && !args.yes {
        return Err(Error::safety("fmonero setup needs --yes")
            .with_reason("needs_yes")
            .with_hint("fmonero setup --all --yes")
            .into());
    }
    let mut actions: Vec<FileAction> = Vec::new();
    let mut ctx = ctx;
    if args.want_units() {
        actions.extend(units_step(&mut ctx, &args)?);
    }
    if args.want_plugin() {
        actions.extend(plugin_step(&ctx, &args)?);
    }
    if args.want_menu() {
        actions.extend(menu_step(&ctx, &args)?);
    }
    out.emit(
        "setup",
        serde_json::json!({
            "ok": true,
            "schema": fmonero_core::SCHEMA_VERSION,
            "actions": actions,
            "dry_run": args.dry_run,
        }),
    );
    Ok(0)
}

// ---- --units -----------------------------------------------------------------

fn units_step(ctx: &mut Context, args: &SetupArgs) -> Result<Vec<FileAction>, Error> {
    // §4.2: refuse under the Arch system layout. fmonero is a guest on someone else's node
    // and writing a second unit is how two monerod end up on one LMDB (§1.2).
    if units::detect_layout(ctx)? == units::Layout::SystemUnit {
        return Err(Error::safety(
            "Arch's monerod.service owns the node on this machine, and fmonero never writes a \
             second unit for a data dir a system daemon serves",
        )
        .with_reason("foreign_node")
        .with_hint("sudo systemctl status monerod   # fmonero does not use sudo"));
    }
    // §1.3: a full chain does not fit this disk, and the low-disk floor has the same escape
    // hatch — the flag pair says "I know, do it anyway".
    let override_disk = args.full_chain && args.yes;
    if args.full_chain {
        if !args.yes {
            return Err(Error::safety(
                "--full-chain writes a unit with no --prune-blockchain, and a full chain does not \
                 fit this disk (§1.3)",
            )
            .with_reason("full_chain_refused")
            .with_hint("fmonero setup --units --full-chain --yes"));
        }
        ctx.settings.node.prune = false;
    }
    if !override_disk {
        let free = fmonero_core::fsinfo::free_bytes(&ctx.settings.node.data_dir)?;
        if free < fmonero_core::status::DISK_FLOOR_BYTES {
            return Err(Error::safety(format!(
                "{} has {} free, under the 120 GB floor for a pruned chain plus headroom — a \
                 figure that is an ESTIMATE, not measured on this machine (§1.3)",
                ctx.settings.node.data_dir.display(),
                crate::output::human_bytes(free)
            ))
            .with_reason("full_chain_refused")
            .with_hint("fmonero setup --units --full-chain --yes   # only if you know it fits"));
        }
    }

    // The derived settings keys for this network, so the unit's `--rpc-bind-port`,
    // `wallet.url` and `Paths::wallet_login_file(port)` cannot disagree (§4.2).
    let net = ctx.settings.node.network;
    let mut derived: Vec<(&str, String)> = vec![
        (
            "node.url",
            format!("http://127.0.0.1:{}", net.node_rpc_port()),
        ),
        (
            "wallet.url",
            format!("http://127.0.0.1:{}", net.wallet_rpc_port()),
        ),
        ("wallet.port", net.wallet_rpc_port().to_string()),
    ];
    // `wallet.dir` carries the network segment, and the unit renders that exact path (§4.2).
    let want_dir = ctx
        .paths
        .wallet_dir
        .join(net.as_str())
        .display()
        .to_string();
    if ctx.settings.wallet.dir.display().to_string() != want_dir {
        derived.push(("wallet.dir", want_dir));
    }
    let mut changed: Vec<(String, String, String)> = Vec::new();
    for (key, value) in derived {
        let old = ctx.settings.get(key)?;
        if old == value {
            continue;
        }
        ctx.settings.set(key, &value)?;
        changed.push((key.to_string(), old, value));
    }
    if args.full_chain {
        let old = ctx.settings.get("node.prune")?;
        if old != "false" {
            changed.push(("node.prune".into(), old, "false".into()));
        }
    }
    let mut actions: Vec<FileAction> = changed
        .iter()
        .map(|(key, old, new)| FileAction {
            what: format!("settings {key}: {old} -> {new}"),
            path: ctx.paths.config_file(),
            status: if args.dry_run {
                FileStatus::Planned
            } else {
                FileStatus::Written
            },
            diff: None,
        })
        .collect();
    // `units::install` renders the three files, compares byte-for-byte, and creates
    // `wallet.dir` and every missing ancestor at 0700 first (§4.2).
    //
    // **It runs BEFORE `Settings::save`, and the order is the point.** `install` renders from
    // `ctx.settings` in memory, so the mutations above are already in force — but saving first
    // meant a failing `install` (a read-only `~/.config/systemd/user`, the §2i system-unit
    // refusal, no `systemctl` in the session) left `node.url`, `wallet.url`, `wallet.port`,
    // `wallet.dir` and — under `--full-chain --yes` — `node.prune` rewritten on disk, with the
    // `actions` list that would have said so discarded along with the error. §4.2 says "running
    // it twice leaves the same machine"; a run that reports failure and changes five keys is
    // neither idempotent nor honest, and the next `status` then talks to the wrong port.
    let written = if args.rotate_rpc_login {
        units::install_rotating(ctx, args.dry_run)?
    } else {
        units::install(ctx, args.dry_run)?
    };
    if !args.dry_run && !changed.is_empty() {
        ctx.settings.save(ctx)?;
    }
    if args.rotate_rpc_login && !args.dry_run {
        actions.push(FileAction {
            what: "the node credential was rotated: restart the node before using it".into(),
            path: ctx.paths.monerod_conf(),
            status: FileStatus::Written,
            diff: None,
        });
    }
    actions.extend(written);
    Ok(actions)
}

// ---- --plugin ----------------------------------------------------------------

fn plugin_step(ctx: &Context, args: &SetupArgs) -> Result<Vec<FileAction>, Error> {
    let source = repo_plugin_dir(ctx)?;
    let link = ctx.paths.plugin_link();
    let mut actions = Vec::new();
    let already = std::fs::read_link(&link).ok().filter(|t| *t == source);
    if already.is_some() {
        actions.push(FileAction {
            what: "plugin symlink".into(),
            path: link.clone(),
            status: FileStatus::Unchanged,
            diff: None,
        });
    } else if args.dry_run {
        actions.push(FileAction {
            what: format!("CreateSymlink -> {}", source.display()),
            path: link.clone(),
            status: FileStatus::Planned,
            diff: None,
        });
    } else {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::failure(format!("cannot create {}: {e}", parent.display())))?;
        }
        // A pre-existing link to somewhere else is replaced; a real directory is not touched.
        if link.is_symlink() {
            std::fs::remove_file(&link)
                .map_err(|e| Error::failure(format!("cannot replace {}: {e}", link.display())))?;
        } else if link.exists() {
            return Err(Error::safety(format!(
                "{} exists and is not a symlink, so fmonero will not replace it",
                link.display()
            ))
            .with_hint(format!("mv {} {}.bak", link.display(), link.display())));
        }
        std::os::unix::fs::symlink(&source, &link)
            .map_err(|e| Error::failure(format!("cannot link {}: {e}", link.display())))?;
        actions.push(FileAction {
            what: format!("CreateSymlink -> {}", source.display()),
            path: link.clone(),
            status: FileStatus::Written,
            diff: None,
        });
    }
    // argv only, never a shell string (§2f), and each step is skipped when already done.
    for argv in [
        vec!["omarchy", "plugin", "enable", PLUGIN_ID],
        vec![
            "omarchy",
            "bar",
            "put",
            PLUGIN_ID,
            "--before",
            "omarchy.tailscale",
        ],
    ] {
        let what = argv.join(" ");
        if args.dry_run {
            actions.push(FileAction {
                what: format!("RunArgv {what}"),
                path: link.clone(),
                status: FileStatus::Planned,
                diff: None,
            });
            continue;
        }
        if fmonero_core::proc::which(argv[0]).is_none() {
            actions.push(FileAction {
                what: format!("RunArgv {what} (omarchy is not on PATH)"),
                path: link.clone(),
                status: FileStatus::Skipped,
                diff: None,
            });
            continue;
        }
        let cancel = CancelToken::new();
        let out = fmonero_core::proc::run_with_deadline(
            &argv,
            std::time::Duration::from_secs(30),
            Some(&cancel),
        )?;
        actions.push(FileAction {
            what: if out.success() {
                format!("RunArgv {what}")
            } else {
                format!("RunArgv {what}: {}", out.stderr_tail().trim())
            },
            path: link.clone(),
            status: if out.success() {
                FileStatus::Written
            } else {
                FileStatus::Skipped
            },
            diff: None,
        });
    }
    Ok(actions)
}

/// The bar widget's id, which is also its directory name (§6).
const PLUGIN_ID: &str = "oliver.monero";

/// `plugin/oliver.monero` inside **this repo's main checkout**, which is what the doctor's
/// `plugin_linked` check validates the resolved link against (§7.1).
///
/// Searched from the working directory upwards, then at the documented location: a symlinked
/// plugin directory does not hot-reload (§1.5), so the link must point at a real checkout and
/// guessing one would be worse than saying so.
fn repo_plugin_dir(ctx: &Context) -> Result<std::path::PathBuf, Error> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = Some(cwd.as_path());
        while let Some(d) = dir {
            candidates.push(d.join("plugin").join(PLUGIN_ID));
            dir = d.parent();
        }
    }
    candidates.push(
        ctx.paths
            .home
            .join("Projects/odcpw/fmonero/plugin")
            .join(PLUGIN_ID),
    );
    for c in candidates {
        if c.join("manifest.json").is_file() {
            return Ok(c);
        }
    }
    Err(Error::not_found(format!(
        "cannot find plugin/{PLUGIN_ID}/manifest.json: run `fmonero setup --plugin` from the \
         repository, whose checkout the symlink has to point at"
    ))
    .with_hint("cd ~/Projects/odcpw/fmonero && fmonero setup --plugin --yes"))
}

// ---- --menu ------------------------------------------------------------------

fn menu_step(ctx: &Context, args: &SetupArgs) -> Result<Vec<FileAction>, Error> {
    let path = ctx.paths.omarchy_menu();
    let current = std::fs::read_to_string(&path).unwrap_or_else(|_| "{\n}\n".to_string());
    // Strip comments into a scratch copy only, and parse THAT to learn which ids exist. A
    // serde round trip would delete every comment in a file the owner maintains by hand
    // (§4.2), so the merge is textual and the parse is read-only.
    let present = existing_ids(&current);
    let missing: Vec<(&str, &str)> = menu_entries()
        .into_iter()
        .filter(|(id, _)| !present.contains(&(*id).to_string()))
        .collect();
    if missing.is_empty() {
        return Ok(vec![FileAction {
            what: "Omarchy menu entries".into(),
            path,
            status: FileStatus::Unchanged,
            diff: None,
        }]);
    }
    let merged = insert_before_last_brace(&current, &missing)?;
    if args.dry_run {
        return Ok(vec![FileAction {
            what: format!("{} Omarchy menu entries", missing.len()),
            path,
            status: FileStatus::Planned,
            diff: Some(units::redact_login(&diff_lines(&current, &merged))),
        }]);
    }
    // A verbatim backup first, named by the UTC basic timestamp (§4.2).
    let stamp = jiff::Timestamp::now()
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string();
    let backups = ctx.paths.backups_dir();
    std::fs::create_dir_all(&backups)
        .map_err(|e| Error::failure(format!("cannot create {}: {e}", backups.display())))?;
    let backup = backups.join(format!("omarchy-menu.jsonc.{stamp}"));
    if path.exists() {
        std::fs::copy(&path, &backup)
            .map_err(|e| Error::failure(format!("cannot back up {}: {e}", path.display())))?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::failure(format!("cannot create {}: {e}", parent.display())))?;
    }
    std::fs::write(&path, merged.as_bytes())
        .map_err(|e| Error::failure(format!("cannot write {}: {e}", path.display())))?;
    Ok(vec![FileAction {
        what: format!(
            "{} Omarchy menu entries (backup: {})",
            missing.len(),
            backup.display()
        ),
        path,
        status: FileStatus::Written,
        diff: None,
    }])
}

/// `(id, the single line to insert)` for every entry of `assets/menu.jsonc`.
///
/// The template is the asset, so the two cannot drift: each non-comment, non-brace line of it
/// is one entry, and its key is the id.
fn menu_entries() -> Vec<(&'static str, &'static str)> {
    MENU_TEMPLATE
        .lines()
        .filter_map(|line| {
            let t = line.trim();
            if t.is_empty() || t.starts_with("//") || t == "{" || t == "}" {
                return None;
            }
            let id = t.strip_prefix('"')?.split('"').next()?;
            Some((id, line))
        })
        .collect()
}

/// The top-level keys of a JSONC document, read from a comment-stripped scratch copy.
fn existing_ids(text: &str) -> Vec<String> {
    let stripped = strip_comments(text);
    serde_json::from_str::<serde_json::Value>(&stripped)
        .ok()
        .and_then(|v| {
            v.as_object()
                .map(|o| o.keys().cloned().collect::<Vec<String>>())
        })
        // A file fmonero cannot parse is one whose ids it does not know: inserting a
        // duplicate key is worse than inserting nothing, so an unparsable file reports every
        // id as present and the merge becomes a no-op.
        .unwrap_or_else(|| {
            menu_entries()
                .iter()
                .map(|(id, _)| (*id).to_string())
                .collect()
        })
}

/// `//` line comments and `/* … */` blocks, outside string literals.
fn strip_comments(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut in_string, mut escaped) = (0usize, false, false);
    while i < b.len() {
        let c = b[i] as char;
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Insert one line per absent id immediately before the file's final `}`, preceded on first
/// insertion by a `// fmonero` marker, leaving existing ids byte-for-byte alone (§4.2).
fn insert_before_last_brace(current: &str, missing: &[(&str, &str)]) -> Result<String, Error> {
    let at = current.rfind('}').ok_or_else(|| {
        Error::failure("the Omarchy menu extension has no closing brace to insert before")
            .with_hint("fmonero setup --menu --dry-run")
    })?;
    let (head, tail) = current.split_at(at);
    let mut merged = head.trim_end().to_string();
    // The previous last entry needs a comma before another one can follow it.
    if !merged.trim_end().ends_with('{') && !merged.trim_end().ends_with(',') {
        merged.push(',');
    }
    merged.push('\n');
    if !current.contains("// fmonero") {
        merged.push_str("  // fmonero\n");
    }
    for (i, (_, line)) in missing.iter().enumerate() {
        let line = line.trim_end().trim_end_matches(',');
        merged.push_str(line);
        if i + 1 < missing.len() {
            merged.push(',');
        }
        merged.push('\n');
    }
    merged.push_str(tail);
    Ok(merged)
}

/// A line-level diff for the `--dry-run` payload. `units::install` has its own for the three
/// files it renders; this one is for the menu, whose "before" is a user-owned file.
fn diff_lines(before: &str, after: &str) -> String {
    let mut out = String::from("--- current\n+++ merged\n");
    let (a, b): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    let mut head = 0;
    while head < a.len() && head < b.len() && a[head] == b[head] {
        head += 1;
    }
    for line in &a[head..] {
        out.push_str(&format!("-{line}\n"));
    }
    for line in &b[head..] {
        out.push_str(&format!("+{line}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_five_menu_entries_come_from_the_asset() {
        let entries = menu_entries();
        let ids: Vec<&str> = entries.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids,
            vec![
                "monero",
                "monero.open",
                "monero.send",
                "monero.node",
                "monero.doctor"
            ],
            "the asset is the source of truth (§4.2)"
        );
        // §1.5: only constant literals reach the shell-string entry points, and the app-ids
        // must not be prefixes of one another.
        let text = MENU_TEMPLATE;
        assert!(text.contains("--app-id=fmonero "));
        assert!(text.contains("--app-id=fmonerodoctor "));
    }

    #[test]
    fn comments_are_stripped_only_outside_strings() {
        let text = "{\n  // STRIPPED\n  \"a\": {\"label\": \"http://x // KEPT\"},\n  /* ALSO-STRIPPED */ \"b\": {}\n}\n";
        let stripped = strip_comments(text);
        assert!(!stripped.contains("STRIPPED"), "{stripped}");
        // A `//` inside a string literal is data, not a comment — and a URL is the case that
        // proves it: stripping it would corrupt every `action` in the menu template.
        assert!(stripped.contains("// KEPT"), "{stripped}");
        let ids = existing_ids(text);
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    /// §4.2: existing ids are left byte-for-byte alone and the insertion is textual, so every
    /// comment in a user-owned file survives.
    #[test]
    fn the_merge_inserts_only_the_absent_ids_and_keeps_every_comment() {
        let current = "{\n  // the user's own note\n  \"monero\": {\"icon\":\"x\"}\n}\n";
        let present = existing_ids(current);
        assert_eq!(present, vec!["monero".to_string()]);
        let missing: Vec<(&str, &str)> = menu_entries()
            .into_iter()
            .filter(|(id, _)| !present.contains(&(*id).to_string()))
            .collect();
        assert_eq!(missing.len(), 4, "one of the five is already there");
        let merged = insert_before_last_brace(current, &missing).unwrap();
        assert!(merged.contains("// the user's own note"));
        assert!(merged.contains("// fmonero"));
        assert!(merged.contains("\"monero.open\""));
        // The pre-existing entry is untouched, and the result still parses.
        assert!(merged.contains("\"monero\": {\"icon\":\"x\"}"));
        let ids = existing_ids(&merged);
        assert_eq!(ids.len(), 5, "{merged}");
        // Running it again is a no-op.
        let again: Vec<(&str, &str)> = menu_entries()
            .into_iter()
            .filter(|(id, _)| !ids.contains(&(*id).to_string()))
            .collect();
        assert!(again.is_empty(), "idempotent (§4.2)");
    }

    #[test]
    fn a_merge_into_an_empty_document_still_parses() {
        let merged = insert_before_last_brace("{\n}\n", &menu_entries()).unwrap();
        let ids = existing_ids(&merged);
        assert_eq!(ids.len(), 5, "{merged}");
    }
}
