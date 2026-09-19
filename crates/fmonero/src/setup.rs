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

    /// The user's **own** invocation with `--yes` appended (§4), for the `needs_yes` refusal.
    ///
    /// A template hint widens what the user asked for: `fmonero setup --units` refused with
    /// `hint: fmonero setup --all --yes` prescribes the plugin and the menu as well, which is
    /// harmless only because `setup` is idempotent — it is still not the command the user ran.
    /// The bad-args refusal above keeps `--all`, because there the user named no target at all.
    pub(crate) fn own_command(&self) -> String {
        let mut s = String::from("fmonero setup");
        for (flag, on) in [
            ("--units", self.units),
            ("--plugin", self.plugin),
            ("--menu", self.menu),
            ("--all", self.all),
            ("--rotate-rpc-login", self.rotate_rpc_login),
            ("--full-chain", self.full_chain),
        ] {
            if on {
                s.push(' ');
                s.push_str(flag);
            }
        }
        s.push_str(" --yes");
        s
    }

    /// The same invocation with `--dry-run` instead of `--yes` — the safe half of what the
    /// user asked for, and the hint the sandbox refusal carries (v5.3, §4.2).
    pub(crate) fn own_dry_run(&self) -> String {
        let mut s = self.own_command();
        debug_assert!(s.ends_with(" --yes"));
        s.truncate(s.len() - " --yes".len());
        s.push_str(" --dry-run");
        s
    }
}

/// Run setup. `--dry-run` prints a unified diff and changes nothing (§4.2).
pub fn run(args: SetupArgs, ctx: Context, out: Out) -> anyhow::Result<i32> {
    if !(args.want_units() || args.want_plugin() || args.want_menu()) {
        return Err(Error::usage(
            "fmonero setup needs to be told what to set up. Use --all, or one of \
                 --units, --plugin and --menu.",
        )
        .with_reason("bad_args")
        .with_hint("fmonero setup --all --yes")
        .into());
    }
    // A dry run changes nothing, so it needs no confirmation — which is what makes
    // `setup --dry-run` a GREEN row (§4b).
    if !args.dry_run && !args.yes {
        return Err(Error::safety(
            "fmonero setup writes files on this machine, so it needs --yes.",
        )
        .with_reason("needs_yes")
        .with_hint(args.own_command())
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
            "Another program on this machine already runs the node, and fmonero will not set \
             up a second one over the same copy of the blockchain.",
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
                "--full-chain keeps the whole blockchain, and fmonero's estimate is that it \
                 does not fit on this disk. Add --yes if you know it does.",
            )
            .with_reason("full_chain_refused")
            .with_hint("fmonero setup --units --full-chain --yes"));
        }
        ctx.settings.node.prune = false;
    }
    // **The floor is a CHAIN floor, and remote mode stores no chain** (v5.2, §2m.3). Refusing
    // to write the two units on a machine configured for a remote node, because there is no
    // room for blocks it will never download, is a refusal on a correct machine — the same
    // argument §7.1 makes for `disk_space` reporting `skip:remote_node`. The node unit and
    // `monerod.conf` are still rendered, because `fmonero node local --yes` must find them.
    if !override_disk && !ctx.settings.node.mode.is_remote() {
        let free = fmonero_core::fsinfo::free_bytes(&ctx.settings.node.data_dir)?;
        if free < fmonero_core::status::DISK_FLOOR_BYTES {
            return Err(Error::safety(format!(
                "{} has {} free. A smaller copy of the blockchain needs about 120 GB, and that \
                 figure is an estimate rather than something measured here.",
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
    // v5.3: a `sandboxed_units` refusal's hint becomes the user's **own** command with
    // `--dry-run`, which is the safe half of what they asked for (§4.2, §4).
    let written = if args.rotate_rpc_login {
        units::install_rotating(ctx, args.dry_run)
    } else {
        units::install(ctx, args.dry_run)
    }
    .map_err(|e| e.retarget_sandbox_hint(args.own_dry_run()))?;
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
                .map_err(|e| Error::io("create", "the folder it needs", parent, &e))?;
        }
        // A pre-existing link to somewhere else is replaced; a real directory is not touched.
        if link.is_symlink() {
            std::fs::remove_file(&link)
                .map_err(|e| Error::io("replace", "the link it manages", &link, &e))?;
        } else if link.exists() {
            return Err(Error::safety(format!(
                "{} is already there and is not a link, so fmonero will not replace it.",
                link.display()
            ))
            .with_hint(format!("mv {} {}.bak", link.display(), link.display())));
        }
        std::os::unix::fs::symlink(&source, &link)
            .map_err(|e| Error::io("create", "the link it manages", &link, &e))?;
        actions.push(FileAction {
            what: format!("CreateSymlink -> {}", source.display()),
            path: link.clone(),
            status: FileStatus::Written,
            diff: None,
        });
    }
    // argv only, never a shell string (§2f), and each step is skipped when already done — read
    // from Omarchy's own `shell.json`, which is where enabling a bar widget records itself
    // (§1.5, §4.2). Both commands are themselves idempotent, so what was wrong before v5.1 was
    // the REPORT: they spawned and said `written` on every run, and §8's idempotence assertion
    // reads exactly that (finding 2 of the Phase 4 verification).
    let on_bar = plugin_on_bar(ctx);
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
        // For a bar widget the two questions are one fact: `omarchy plugin enable` puts the id
        // into a section of `bar.layout` and `omarchy bar put` "leaves a widget that is already
        // on the bar where it is" (`/usr/share/omarchy/bin/omarchy-bar`), so a widget already in
        // that document is what both commands would produce.
        if on_bar == Some(true) {
            actions.push(FileAction {
                what: format!("{what} (already: {PLUGIN_ID} is on the bar in shell.json)"),
                path: ctx.paths.omarchy_shell_json(),
                status: FileStatus::Unchanged,
                diff: None,
            });
            continue;
        }
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

/// Is `oliver.monero` already a widget of Omarchy's bar?
///
/// Read from `~/.config/omarchy/shell.json`, which is where `omarchy plugin enable` records a
/// bar widget — §1.5 says so in as many words ("Enabling writes `{\"id\":\"oliver.monero\"}`
/// into `~/.config/omarchy/shell.json`") and `omarchy-plugin-enable` reaches it through
/// `omarchy-shell shell enablePlugin`. The file rather than `omarchy plugin list`, for two
/// reasons: the listing needs a **running** shell (it is an IPC call), and `setup` must answer
/// this question on a machine whose shell has not started; and reading the state costs no
/// process spawn, while `plugin list` costs one before the two it exists to avoid.
///
/// `None` means "cannot tell" — no file, or a document this code does not recognise — and the
/// caller then runs the command as it always did. Guessing `true` there would skip a step that
/// was never taken; guessing `false` is what the old code effectively did.
fn plugin_on_bar(ctx: &Context) -> Option<bool> {
    let text = std::fs::read_to_string(ctx.paths.omarchy_shell_json()).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
    let layout = doc.get("bar")?.get("layout")?.as_object()?;
    Some(layout.values().any(|section| {
        section.as_array().is_some_and(|widgets| {
            widgets
                .iter()
                .any(|w| w.get("id") == Some(&PLUGIN_ID.into()))
        })
    }))
}

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
    Err(Error::not_found(
        "fmonero could not find its own plugin folder. Run fmonero setup --plugin from inside the \
         repository, because the link it makes has to point at a real checkout.",
    )
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
    // 0700, not `create_dir_all`'s `0777 & ~umask` — the directory half of DEFECT F, which only
    // `install.sh`'s `umask 077` was hiding.
    fmonero_core::paths::ensure_private_dir(&backups)?;
    let backup = backups.join(format!("omarchy-menu.jsonc.{stamp}"));
    if path.exists() {
        // **Read and `write_private`, never `fs::copy`** (DECISIONS: DEFECT F). `fs::copy`
        // carries the SOURCE's mode to the destination, and the source is Omarchy's own file at
        // 0644 — so the backup landed 0644 inside fmonero's state directory, where §2b.7 says
        // everything fmonero writes under `$HOME` is 0600 and where the doctor's `state_perms`
        // check says the same thing at `error` severity. `install.sh`'s `umask 077` cannot help:
        // `fs::copy` calls `set_permissions` from the source metadata after creating the file.
        // `write_private` sets 0600 at `O_EXCL` creation time and never chmods.
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::io("read", "the file it is about to back up", &path, &e))?;
        fmonero_core::write_private(&backup, &bytes)?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::io("create", "the folder it needs", parent, &e))?;
    }
    std::fs::write(&path, merged.as_bytes())
        .map_err(|e| Error::io("write", "one of its own files", &path, &e))?;
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
        Error::failure(
            "fmonero could not make sense of the Omarchy menu file, so it changed \
             nothing.",
        )
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
    fn the_six_menu_entries_come_from_the_asset() {
        let entries = menu_entries();
        let ids: Vec<&str> = entries.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids,
            vec![
                "monero",
                "monero.open",
                "monero.send",
                // v5.5, §6.5: the Wallet screen is a menu destination now, because it is the
                // screen a user with no wallet has to reach, and `Node` becomes `Nodes` — the
                // picker made "which node" the question that screen answers.
                "monero.wallet",
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
        assert_eq!(missing.len(), 5, "one of the six is already there");
        let merged = insert_before_last_brace(current, &missing).unwrap();
        assert!(merged.contains("// the user's own note"));
        assert!(merged.contains("// fmonero"));
        assert!(merged.contains("\"monero.open\""));
        // The pre-existing entry is untouched, and the result still parses.
        assert!(merged.contains("\"monero\": {\"icon\":\"x\"}"));
        let ids = existing_ids(&merged);
        assert_eq!(ids.len(), 6, "{merged}");
        // Running it again is a no-op.
        let again: Vec<(&str, &str)> = menu_entries()
            .into_iter()
            .filter(|(id, _)| !ids.contains(&(*id).to_string()))
            .collect();
        assert!(again.is_empty(), "idempotent (§4.2)");
    }

    /// Finding 2 of the Phase 4 verification: the two `omarchy` steps reported `written` on
    /// every run, so §8's idempotence assertion could never hold. The pre-check reads the same
    /// document Omarchy writes, and the three answers it has to distinguish are "on the bar",
    /// "not on the bar" and "cannot tell" — the last one running the command as before, because
    /// skipping a step that was never taken is the one wrong answer here.
    #[test]
    fn the_bar_pre_check_reads_omarchys_own_shell_json() {
        let dir = std::env::temp_dir().join(format!("fmonero-setup-bar-{}", std::process::id()));
        let mut ctx = Context::for_test(&dir).expect("test context");
        ctx.paths.omarchy_config_dir = dir.join("omarchy");
        std::fs::create_dir_all(&ctx.paths.omarchy_config_dir).unwrap();
        let shell = ctx.paths.omarchy_shell_json();

        // No file at all: "cannot tell".
        assert_eq!(plugin_on_bar(&ctx), None);

        // A real bar with other widgets and not ours.
        std::fs::write(
            &shell,
            br#"{"version":1,"bar":{"layout":{"left":[{"id":"omarchy.menu"}],
                 "center":[],"right":[{"id":"omarchy.tray"},{"id":"omarchy.tailscale"}]}},
                 "plugins":[]}"#,
        )
        .unwrap();
        assert_eq!(plugin_on_bar(&ctx), Some(false), "not on the bar yet");

        // Ours, in the section `--before omarchy.tailscale` puts it in.
        std::fs::write(
            &shell,
            br#"{"version":1,"bar":{"layout":{"left":[{"id":"omarchy.menu"}],
                 "center":[],"right":[{"id":"omarchy.tray"},{"id":"oliver.monero"},
                 {"id":"omarchy.tailscale"}]}},"plugins":[]}"#,
        )
        .unwrap();
        assert_eq!(plugin_on_bar(&ctx), Some(true), "already on the bar");

        // A document this code does not recognise is "cannot tell", never a guess.
        std::fs::write(&shell, b"{\"version\":1}\n").unwrap();
        assert_eq!(plugin_on_bar(&ctx), None);
        std::fs::write(&shell, b"not json at all\n").unwrap();
        assert_eq!(plugin_on_bar(&ctx), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_merge_into_an_empty_document_still_parses() {
        let merged = insert_before_last_brace("{\n}\n", &menu_entries()).unwrap();
        let ids = existing_ids(&merged);
        assert_eq!(ids.len(), 6, "{merged}");
    }

    /// One sandbox with a 0644 Omarchy menu file — the ordinary state of that file on any
    /// machine Omarchy has written it — run through the real `menu_step`.
    fn menu_sandbox(name: &str, mode: u32) -> (Context, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "fmonero-setup-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ctx = Context::for_test(&dir).expect("test context");
        ctx.paths.omarchy_config_dir = dir.join("omarchy");
        let menu = ctx.paths.omarchy_menu();
        std::fs::create_dir_all(menu.parent().expect("parent")).expect("extensions dir");
        std::fs::write(&menu, b"{\n  \"user.thing\": {\"label\":\"Thing\"}\n}\n").expect("menu");
        std::fs::set_permissions(&menu, std::fs::Permissions::from_mode(mode)).expect("chmod");
        (ctx, dir)
    }

    /// **DEFECT F.** `setup --menu` backed the Omarchy menu file up with `std::fs::copy`, which
    /// carries the SOURCE's mode to the destination: a 0644 source — Omarchy's file, not
    /// fmonero's — produced a 0644 backup inside fmonero's own state directory, where §2b.7 says
    /// everything fmonero writes is 0600 and where the doctor's `state_perms` check fails at
    /// `error` severity. The directory half is the same fault: `create_dir_all` uses
    /// `0777 & ~umask`, so `backups/` landed 0755 under the ordinary 022 and only
    /// `install.sh`'s own `umask 077` was hiding it.
    #[test]
    fn the_menu_backup_is_0600_whatever_mode_the_source_file_has() {
        use std::os::unix::fs::PermissionsExt;
        let (ctx, dir) = menu_sandbox("backup-mode", 0o644);
        let source_mode = std::fs::metadata(ctx.paths.omarchy_menu())
            .expect("stat")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(source_mode, 0o644, "the source must really be 0644");
        let args = SetupArgs {
            menu: true,
            yes: true,
            ..SetupArgs::default()
        };
        let actions = menu_step(&ctx, &args).expect("the merge");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].status, FileStatus::Written);
        let backups: Vec<std::path::PathBuf> = std::fs::read_dir(ctx.paths.backups_dir())
            .expect("the backups directory")
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(backups.len(), 1, "one backup, named by the UTC stamp");
        let mode = std::fs::metadata(&backups[0])
            .expect("stat")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o600, "§2b.7: {} is {mode:04o}", backups[0].display());
        let dir_mode = std::fs::metadata(ctx.paths.backups_dir())
            .expect("stat")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(dir_mode, 0o700, "§2b.7: the directory too, {dir_mode:04o}");
        // Verbatim: the backup is the bytes the merge was computed from.
        assert_eq!(
            std::fs::read(&backups[0]).expect("read the backup"),
            b"{\n  \"user.thing\": {\"label\":\"Thing\"}\n}\n".to_vec()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **FINDING E, recorded as a test rather than closed as a defect** (DECISIONS: the fix-up
    /// round). §4.2's merge leaves an existing id byte-for-byte alone, deliberately, so a
    /// *label* change — `Node` → `Nodes` — reaches a machine that already has the entry only if
    /// the line is removed first. This pins that behaviour, on the line the owner actually has,
    /// so the choice is a property of the code and not an accident of it.
    #[test]
    fn an_existing_entry_keeps_its_own_label_even_when_the_asset_renames_it() {
        let (ctx, dir) = menu_sandbox("label-kept", 0o600);
        let menu = ctx.paths.omarchy_menu();
        // The exact line the previous fmonero version wrote, `Node` and all.
        let old = "  \"monero.node\": {\"icon\":\"\u{f048b}\",\"label\":\"Node\",\"action\":\"omarchy-launch-or-focus-tui --app-id=fmonero fmonero tui --node\"},";
        std::fs::write(&menu, format!("{{\n{old}\n}}\n")).expect("menu");
        let args = SetupArgs {
            menu: true,
            yes: true,
            ..SetupArgs::default()
        };
        menu_step(&ctx, &args).expect("the merge");
        let after = std::fs::read_to_string(&menu).expect("read back");
        assert!(
            after.contains(old),
            "the existing line must be byte-for-byte unchanged:\n{after}"
        );
        assert!(
            !after.contains("\"label\":\"Nodes\""),
            "the asset's new label must NOT be merged over an existing id (§4.2):\n{after}"
        );
        // The five ids that were absent are added, so the merge did run.
        assert_eq!(existing_ids(&after).len(), 6, "{after}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
