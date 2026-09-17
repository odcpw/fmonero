//! The two systemd **user** units, and the §2i layout detection.
//!
//! Types frozen at the Phase 1a gate; bodies are Phase 1b's (§8). The rules the bodies
//! honour, restated here because getting one of them wrong corrupts an LMDB (§1.2):
//!
//! * [`data_dir_in_use`] probes with `fcntl(F_GETLK)` and **`l_type: F_WRLCK`**, never
//!   `flock`: LMDB locks the environment with POSIX record locks
//!   (`mdb.c:5083-5099`), a running `monerod` holds a **shared** lock on that byte
//!   (`mdb_env_share_locks`, `mdb.c:5022-5057`), and only a write-lock *query* conflicts
//!   with a held read lock. An `flock` probe — or an `F_GETLK{F_RDLCK}` one — reports a live
//!   data dir as free, and `node up` then starts a second writer.
//! * It is used **only** by `node up` (as a start-time precondition) and by the doctor's
//!   `node_data_dir_single_writer`, **never by [`detect_layout`]** (§2i): it cannot tell our
//!   own monerod from a foreign one, so folding it into layout detection would disable every
//!   node mutator the moment our own node starts.
//! * [`install`] refuses to **write** anything when `FMONERO_STATE_DIR` is set, so a test can
//!   never half-write a real unit (§4.2).
//! * [`ensure_running`] **probes first** and starts the unit only when the probe fails —
//!   "start, then probe" would spawn `systemctl start` against a unit that does not exist on
//!   every AMBER wallet row of §4b — and when it does start one, [`preflight`] has already
//!   answered "no unit" and "no binary" in milliseconds (§2i, v5.1), so a machine that
//!   `setup` has configured and Monero has not reached says **exit 4 `not_installed`**
//!   instead of waiting 30 s for a unit that `203/EXEC`s every five seconds.
//! * **No systemd specifier may reach `monerod.conf`** — boost expands none, so a `%h` there
//!   makes monerod create a directory literally named `%h` and resync from zero. The guard is
//!   the permanent unit test [`render_node_conf_has_no_systemd_specifiers`]. The two
//!   `.service` files may still use `%S`/`%t` where §4.2 keeps them (the wallet unit's
//!   `StateDirectory`, `WorkingDirectory` and `--log-file`, none of which is compared against
//!   a setting); the node unit's `--config-file` is **rendered absolute** (v5.1), because a
//!   non-default `XDG_CONFIG_HOME` made `%h` name a file `setup` never wrote.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::context::{Context, Systemctl};
use crate::error::Error;
use crate::proc::{self, CancelToken};
use crate::rpc::Login;

/// Which node fmonero is looking at (§2i). **Configuration, not occupancy.**
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    UserUnit,
    SystemUnit,
    Remote,
    None,
}

impl Layout {
    /// The wire spelling, which §5's `node.layout` promises.
    pub fn as_str(self) -> &'static str {
        match self {
            Layout::UserUnit => "user_unit",
            Layout::SystemUnit => "system_unit",
            Layout::Remote => "remote",
            Layout::None => "none",
        }
    }
}

/// Arch's system unit, which fmonero detects, uses read-only and never fights (§1.2, §2i).
pub const SYSTEM_NODE_UNIT: &str = "monerod.service";
/// The system node's own config file, whose presence is half the `system_unit` test (§2i).
pub const SYSTEM_NODE_CONF: &str = "/etc/monerod.conf";

/// What `systemctl --user is-active` / `is-enabled` and a file check say about one unit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnitState {
    pub name: String,
    pub exists: bool,
    pub active: bool,
    pub enabled: bool,
}

impl UnitState {
    /// The state of a unit that is not installed — what §5 reports on a machine where
    /// `setup --units` has never run.
    pub fn missing(name: &str) -> UnitState {
        UnitState {
            name: name.to_string(),
            exists: false,
            active: false,
            enabled: false,
        }
    }
}

/// What [`install`] did, or would do, to one file. This **is** §5c's `setup.actions`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileAction {
    pub what: String,
    pub path: std::path::PathBuf,
    pub status: FileStatus,
    /// Present only for a dry run: the unified diff of the file's current bytes against the
    /// rendered bytes (§4.2), with the `rpc-login=` value replaced by `<redacted>`. `None`
    /// when `Unchanged`.
    pub diff: Option<String>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Written,
    Unchanged,
    Skipped,
    Planned,
}

impl FileStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            FileStatus::Written => "written",
            FileStatus::Unchanged => "unchanged",
            FileStatus::Skipped => "skipped",
            FileStatus::Planned => "planned",
        }
    }
}

// ---- layout (§2i) ------------------------------------------------------------

/// §2i: user unit / system unit / remote / none. Reads unit files and `systemctl`, and
/// **never** [`data_dir_in_use`] — occupancy is `node up`'s precondition, not a term here.
pub fn detect_layout(ctx: &Context) -> Result<Layout, Error> {
    // `remote` first, because it is a settings fact that makes every other row moot: a
    // non-loopback node.url means no client can be constructed at all (§2c).
    if crate::rpc::require_loopback(&ctx.settings.node.url).is_err() {
        return Ok(Layout::Remote);
    }
    if unit_file_exists(ctx, &ctx.settings.node.unit)? {
        return Ok(Layout::UserUnit);
    }
    if system_node_present(ctx)? {
        return Ok(Layout::SystemUnit);
    }
    Ok(Layout::None)
}

/// True only for [`Layout::UserUnit`]. Read by `NodeClient::mutators` (§2i) and by the
/// doctor; it is **not** the thing callers check by hand — the mutators live on a type they
/// cannot construct without it, and there is no `--force` override.
pub fn may_mutate_node(ctx: &Context) -> Result<bool, Error> {
    Ok(detect_layout(ctx)? == Layout::UserUnit)
}

/// Arch's `monerod.service`: active, or installed **and** enabled (§2i).
fn system_node_present(ctx: &Context) -> Result<bool, Error> {
    if let Systemctl::Fixture(path) = &ctx.systemctl {
        let row = fixture_row(path, SYSTEM_NODE_UNIT)?;
        return Ok(match row {
            Some(r) => r.active || (r.exists && r.enabled),
            None => false,
        });
    }
    // No `--user`: this is the system manager.
    if systemctl_word(ctx, &["systemctl", "is-active", SYSTEM_NODE_UNIT])? == "active" {
        return Ok(true);
    }
    if Path::new(SYSTEM_NODE_CONF).exists() {
        let enabled = systemctl_word(ctx, &["systemctl", "is-enabled", SYSTEM_NODE_UNIT])?;
        // `enabled`, `enabled-runtime`, `alias`, `static`… anything but the failure words.
        if !matches!(
            enabled.as_str(),
            "" | "disabled" | "masked" | "not-found" | "invalid"
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---- unit state --------------------------------------------------------------

/// `systemctl --user is-active` / `is-enabled`, or the fixture file under
/// `Systemctl::Fixture` (§2i).
pub fn state(ctx: &Context, unit: &str) -> Result<UnitState, Error> {
    if let Systemctl::Fixture(path) = &ctx.systemctl {
        return Ok(match fixture_row(path, unit)? {
            Some(r) => UnitState {
                name: unit.to_string(),
                exists: r.exists,
                active: r.active,
                enabled: r.enabled,
            },
            None => UnitState::missing(unit),
        });
    }
    let exists = ctx.paths.systemd_user_dir.join(unit).exists();
    let active = systemctl_word(ctx, &["systemctl", "--user", "is-active", unit])? == "active";
    let enabled = systemctl_word(ctx, &["systemctl", "--user", "is-enabled", unit])? == "enabled";
    Ok(UnitState {
        name: unit.to_string(),
        exists,
        active,
        enabled,
    })
}

/// `systemctl --user show -p MainPID <unit>`, so `node up` can tell "our node is already
/// running" (exit 0) from "someone else's monerod holds this data dir" (exit 3, §2i). Under
/// `Systemctl::Fixture` it reads the `<main-pid>` column of that file instead (§2i).
pub fn main_pid(ctx: &Context, unit: &str) -> Result<Option<i32>, Error> {
    if let Systemctl::Fixture(path) = &ctx.systemctl {
        return Ok(fixture_row(path, unit)?.and_then(|r| r.main_pid));
    }
    let out = systemctl(ctx, &["systemctl", "--user", "show", "-p", "MainPID", unit])?;
    let text = out.stdout_string();
    let pid = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("MainPID="))
        .and_then(|v| v.trim().parse::<i32>().ok())
        .filter(|p| *p > 0);
    Ok(pid)
}

pub fn start(ctx: &Context, unit: &str, cancel: &CancelToken) -> Result<(), Error> {
    if let Systemctl::Fixture(path) = &ctx.systemctl {
        return fixture_set_active(path, unit, true);
    }
    run_unit_verb(ctx, "start", unit, cancel)
}

pub fn stop(ctx: &Context, unit: &str, cancel: &CancelToken) -> Result<(), Error> {
    if let Systemctl::Fixture(path) = &ctx.systemctl {
        return fixture_set_active(path, unit, false);
    }
    run_unit_verb(ctx, "stop", unit, cancel)
}

fn run_unit_verb(ctx: &Context, verb: &str, unit: &str, cancel: &CancelToken) -> Result<(), Error> {
    // The only argv fmonero builds from data, and the unit name is one of two constants (§2f).
    let out = proc::run_with_deadline(
        &["systemctl", "--user", verb, unit],
        ctx.timeouts.systemctl,
        Some(cancel),
    )?;
    if out.timed_out {
        return Err(Error::timeout(format!(
            "systemctl --user {verb} {unit} did not finish in time"
        ))
        .with_hint("fmonero status --json"));
    }
    if out.success() {
        return Ok(());
    }
    Err(Error::backend(format!(
        "systemctl --user {verb} {unit} failed: {}",
        one_line(&out.stderr_tail())
    ))
    .with_reason("unit_missing")
    .with_hint("fmonero setup --units --yes"))
}

/// The two questions that must be answered **before any `systemctl` call** (§2i, §4.1, v5.1),
/// each in a fraction of a millisecond:
///
/// 1. **is the unit file there?** Else exit **4 `unit_missing`**, hint `fmonero setup --units`.
/// 2. **is the daemon binary on `PATH`?** Else exit **4 `not_installed`**, hint
///    `omarchy pkg add monero` — the same probe the doctor's `monerod_installed` makes.
///
/// Without (2), a machine that `setup --units` has configured and `omarchy pkg add monero` has
/// not reached answers `node up` with **exit 8** after thirty seconds — "nothing is broken",
/// pointing at `status --json`, which will say `reachable: false` without saying why — and
/// leaves the unit restarting every five seconds on `203/EXEC`. §4a assigns that condition
/// exit 4. This is the first state the owner of this build will meet, and it was finding 1 of
/// the Phase 4 verification.
///
/// (2) is skipped under [`Systemctl::Fixture`], the §2i test seam: a fixture that reports a
/// unit's existence and activity is standing in for the machine's daemon as well, and
/// `fmonero-mock` is what answers the RPC in every one of those rows. The seam is honoured
/// only under `FMONERO_STATE_DIR` (§2i), so no real invocation can reach the exemption — and
/// [`tests::preflight_wants_a_unit_file_and_a_binary`] drives (2) directly, so it is not
/// exempt from being tested.
pub fn preflight(ctx: &Context, unit: &str, binary: &str) -> Result<(), Error> {
    if !unit_file_exists(ctx, unit)? {
        return Err(Error::backend(format!("{unit} is not installed in {}", {
            ctx.paths.systemd_user_dir.display()
        }))
        .with_reason("unit_missing")
        .with_hint("fmonero setup --units --yes"));
    }
    if matches!(ctx.systemctl, Systemctl::Real) && proc::which(binary).is_none() {
        return Err(Error::backend(format!(
            "{binary} is not installed, so {unit} would exit 203/EXEC and be restarted every \
             {RESTART_SEC}s without ever answering"
        ))
        .with_reason("not_installed")
        .with_hint("omarchy pkg add monero   # then: fmonero node up"));
    }
    Ok(())
}

/// `RestartSec=` in both rendered units, quoted in [`preflight`]'s message so the two cannot
/// disagree about what a failing unit does.
pub const RESTART_SEC: u32 = 5;

/// The daemon one of the two units runs, for [`preflight`]'s second question. Derived from the
/// unit name rather than passed in, because [`ensure_running`]'s signature is frozen (§5b) and
/// its two callers — `wallet_session::open` and `wallet create` — would otherwise each have to
/// name the binary, which is the "invariant that lives in two places" §2a.3 rejects.
pub fn unit_binary(ctx: &Context, unit: &str) -> &'static str {
    if unit == ctx.settings.wallet.unit {
        "monero-wallet-rpc"
    } else {
        "monerod"
    }
}

/// Probe first; start the unit only when the probe fails; then poll until it succeeds or the
/// deadline. Returns whether a start was needed. fbittorrent's `backend::ensure_running`
/// shape (`fbittorrent/src/backend.rs:17-29`).
pub fn ensure_running(
    ctx: &Context,
    unit: &str,
    probe: &dyn Fn() -> bool,
    deadline: Duration,
    cancel: &CancelToken,
) -> Result<bool, Error> {
    // This order is what lets every AMBER wallet row of §4b run against `fmonero-mock` with
    // no unit installed at all (§5b).
    if probe() {
        return Ok(false);
    }
    // Only now, when a start is actually about to happen (§2i, v5.1).
    preflight(ctx, unit, unit_binary(ctx, unit))?;
    start(ctx, unit, cancel)?;
    let started = std::time::Instant::now();
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        if probe() {
            return Ok(true);
        }
        if started.elapsed() >= deadline {
            // We started it and it never answered, so we own it: leave nothing flapping in a
            // `Restart=on-failure` loop behind a command that has already failed (v5.1).
            let stopped = stop_after_failed_start(ctx, unit, cancel);
            return Err(Error::timeout(format!(
                "{unit} started but did not answer within {}s{stopped}",
                deadline.as_secs()
            ))
            .with_hint("fmonero doctor --only units"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// `systemctl --user stop` a unit **we** started that never answered, then clear the failed
/// state so the next attempt is not refused by the start limit the rendered units now set
/// (§4.2). Returns the clause to append to the caller's error message, because an error that
/// does not say the unit was stopped leaves the user looking for a daemon that is no longer
/// there. Best-effort: a stop that fails is not worth losing the real error over.
///
/// Its other caller is `node_session::up`, whose own `get_info` poll has the same shape and the
/// same obligation: the Phase 4 verifier's runs left both real units flapping, and that — a
/// test-suite and command-path defect, not an accident — is what this closes (§2i, v5.1).
pub fn stop_after_failed_start(ctx: &Context, unit: &str, cancel: &CancelToken) -> String {
    if stop(ctx, unit, cancel).is_err() {
        return String::new();
    }
    if matches!(ctx.systemctl, Systemctl::Real) {
        let _ = proc::run_with_deadline(
            &["systemctl", "--user", "reset-failed", unit],
            ctx.timeouts.systemctl,
            Some(cancel),
        );
    }
    format!(", so it was stopped again ({unit} is inactive)")
}

// ---- rendering (§4.2) --------------------------------------------------------

/// The node unit (§4.2).
///
/// **`--config-file` carries the absolute, XDG-resolved `paths.monerod_conf()`, not
/// `%h/.config/fmonero/monerod.conf`** (v5.1, closing `secrets-5`). The two agree only while
/// `XDG_CONFIG_HOME` is `~/.config`: with it set anywhere else, `setup --units` writes the
/// conf where `Paths` says and `%h` names a file that is not there, `monerod` exits on a
/// missing **named** config file (§1b.6), `Restart=on-failure` retries it, and `node up`
/// reports "did not answer get_info within 30s" — the symptom pointing away from the cause.
/// systemd has no specifier for `XDG_CONFIG_HOME`, so the fix is the one
/// [`render_wallet_unit`] already applies to `--wallet-dir`: render the path.
///
/// `%S` still appears in the wallet unit, where nothing is compared against a setting.
///
/// `StartLimitIntervalSec`/`StartLimitBurst` are explicit because the manager defaults
/// (10 s / 5 starts) can never be reached by a unit with `RestartSec=5`: two starts per
/// interval, so a `203/EXEC` — the answer when the Monero binary is absent — restarts every
/// five seconds forever. §4.2 already describes the failure as retrying "into systemd's start
/// limit"; these two lines are what make that true.
pub fn render_node_unit(ctx: &Context) -> String {
    format!(
        "\
[Unit]
Description=Monero daemon (fmonero)
After=network-online.target
Wants=network-online.target
# Three tries in a minute, then stop: RestartSec=5 never reaches the 10 s/5 default (§4.2).
StartLimitIntervalSec=60
StartLimitBurst=3

[Service]
Type=simple
ExecStart=/usr/bin/monerod --config-file {conf} --non-interactive
Restart=on-failure
RestartSec=5
# monerod is I/O-heavy and must never compete with the desktop
Nice=5
IOSchedulingClass=best-effort
IOSchedulingPriority=6

[Install]
WantedBy=default.target
",
        conf = ctx.paths.monerod_conf().display(),
    )
}

/// `~/.config/fmonero/monerod.conf` (§4.2, §2c). **Every path in it is ABSOLUTE**: boost's
/// `program_options` does no specifier expansion, so a `%h` here would make `monerod` create
/// a directory literally named `%h` and resync the chain from zero — a failure no mock can
/// catch, which is why its guard is the permanent unit test
/// [`tests::render_node_conf_has_no_systemd_specifiers`].
pub fn render_node_conf(ctx: &Context, login: &Login) -> String {
    let net = ctx.settings.node.network;
    let mut s = String::new();
    s.push_str(
        "# written by `fmonero setup --units` (DESIGN.md §4.2). Mode 0600: it carries rpc-login.\n",
    );
    s.push_str(
        "# Read by monerod through boost::program_options, NOT by systemd: every path here is\n",
    );
    s.push_str("# ABSOLUTE, because boost does no specifier expansion.\n");
    s.push_str(&format!(
        "data-dir={}\n",
        ctx.settings.node.data_dir.display()
    ));
    if ctx.settings.node.prune {
        s.push_str("prune-blockchain=1\n");
    }
    s.push_str("rpc-bind-ip=127.0.0.1\n");
    s.push_str(&format!("rpc-bind-port={}\n", net.node_rpc_port()));
    s.push_str(&format!("rpc-login={}:{}\n", login.user, login.pass));
    s.push_str("no-zmq=1\n");
    s.push_str("# The update check is a DNS TXT lookup, so it is off — which makes node.update_available\n");
    s.push_str("# permanently false under OUR unit, by design (§5).\n");
    s.push_str("check-updates=disabled\n");
    s.push_str(&format!(
        "log-file={}\n",
        ctx.settings.node.log_file.display()
    ));
    s.push_str("log-level=0\n");
    s.push_str("max-log-files=3\n");
    s.push_str(
        "# Narrower than monerod's -1 auto default on purpose: the desktop is the primary\n",
    );
    s.push_str("# workload and connection count is what monerod costs at idle (§4.2).\n");
    s.push_str("out-peers=16\n");
    s.push_str("in-peers=32\n");
    match net {
        crate::settings::Network::Mainnet => {}
        crate::settings::Network::Stagenet => s.push_str("stagenet=1\n"),
        crate::settings::Network::Testnet => s.push_str("testnet=1\n"),
    }
    s
}

/// The wallet unit (§4.2). `--wallet-dir` is rendered from `settings.wallet.dir` **and
/// therefore always carries the network segment**, never from a template constant, and
/// never as `%h/...`: the doctor's `wallet_dir_matches_unit` compares it against that
/// setting literally.
pub fn render_wallet_unit(ctx: &Context) -> String {
    let net = ctx.settings.node.network;
    let extra = match net {
        crate::settings::Network::Mainnet => "",
        crate::settings::Network::Stagenet => "  --stagenet \\\n",
        crate::settings::Network::Testnet => "  --testnet \\\n",
    };
    format!(
        "\
[Unit]
Description=Monero wallet RPC (fmonero)
After={node_unit}
# Three tries in a minute, then stop: RestartSec=5 never reaches the 10 s/5 default (§4.2).
StartLimitIntervalSec=60
StartLimitBurst=3

[Service]
Type=simple
StateDirectory=fmonero/run
StateDirectoryMode=0700
WorkingDirectory=%S/fmonero/run
ExecStart=/usr/bin/monero-wallet-rpc \\
  --rpc-bind-ip 127.0.0.1 \\
  --rpc-bind-port {port} \\
  --wallet-dir {dir} \\
  --daemon-address {daemon} \\
  --trusted-daemon \\
{extra}  --non-interactive \\
  --log-file %S/fmonero/run/monero-wallet-rpc.log \\
  --log-level 0 \\
  --max-concurrency 4
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
",
        node_unit = ctx.settings.node.unit,
        port = ctx.settings.wallet.port,
        dir = ctx.settings.wallet.dir.display(),
        daemon = host_port(&ctx.settings.node.url),
    )
}

/// `host:port` from a loopback http URL, for the wallet unit's `--daemon-address` and for
/// `set_daemon` (§4.2).
pub fn host_port(url: &str) -> String {
    let rest = url.trim_start_matches("http://").trim_end_matches('/');
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_string()
}

/// Carries an existing valid `rpc-login=fmonero:<32 hex>` through unchanged; draws 16 fresh
/// bytes from `/dev/urandom` only when the line is absent, unreadable or malformed. This is
/// what makes `setup --units` idempotent and what stops a rewrite from locking out a
/// **running** node (§4.2). `rotate` forces a new one.
pub fn existing_or_new_node_login(ctx: &Context, rotate: bool) -> Result<Login, Error> {
    if !rotate {
        if let Ok(text) = std::fs::read_to_string(ctx.paths.monerod_conf()) {
            if let Some(login) = Login::from_monerod_conf_text(&text) {
                // "Syntactically valid" is `fmonero:<32 hex>` (§4.2): a hand-edited value of
                // some other shape is not one fmonero minted, and rewriting it would lock
                // out whoever did.
                if login.user == LOGIN_USER
                    && login.pass.len() == 32
                    && login.pass.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Ok(login);
                }
            }
        }
    }
    Ok(Login {
        user: LOGIN_USER.to_string(),
        pass: crate::rpc::random_hex(16)?,
    })
}

/// The username in `rpc-login=fmonero:<32 hex>` (§2c, §4.2).
pub const LOGIN_USER: &str = "fmonero";

/// Replace the `rpc-login=` value with `<redacted>`, for everything `setup` **prints**
/// (§4.2). The byte-for-byte comparison that decides `Written`/`Unchanged` runs on the real
/// bytes inside [`install`], so idempotence is unaffected; only the display is redacted.
pub fn redact_login(text: &str) -> String {
    text.lines()
        .map(|l| {
            let trimmed = l.trim_start();
            let indent = &l[..l.len() - trimmed.len()];
            match trimmed.strip_prefix("rpc-login=") {
                Some(_) => format!("{indent}rpc-login=<redacted>"),
                None => l.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if text.ends_with('\n') { "\n" } else { "" }
}

// ---- install (§4.2) ----------------------------------------------------------

/// Renders the three files, compares byte-for-byte against what is on disk, and (unless
/// `dry_run`) writes the changed ones (the conf at 0600), daemon-reloads and **disables**
/// both units — they are on-demand (§4.2). Returns one [`FileAction`] per file, plus one for
/// `settings.wallet.dir` and every missing ancestor.
///
/// `rotate` mints a fresh node credential (§4.2's `--rotate-rpc-login`).
pub fn install(ctx: &Context, dry_run: bool) -> Result<Vec<FileAction>, Error> {
    install_with(ctx, dry_run, false)
}

/// [`install`] plus `--rotate-rpc-login` (§4.2). Separate rather than a second parameter on
/// `install`, whose signature is frozen (§5b).
pub fn install_rotating(ctx: &Context, dry_run: bool) -> Result<Vec<FileAction>, Error> {
    install_with(ctx, dry_run, true)
}

fn install_with(ctx: &Context, dry_run: bool, rotate: bool) -> Result<Vec<FileAction>, Error> {
    // §4.2: a test never installs a unit. A dry run writes nothing, so it is allowed to run
    // anywhere — which is what lets `tests/e2e.sh` assert the diff and the CreateDir action
    // in its own state dir instead of against $HOME. The deviation is in DECISIONS.md.
    if !dry_run && crate::paths::under_test_state_dir() {
        return Err(Error::safety(
            "units::install refuses to write a real systemd unit while FMONERO_STATE_DIR is set",
        )
        .with_hint("fmonero setup --units --dry-run"));
    }
    let mut actions = Vec::new();

    // The wallet directory first: `monero-wallet-rpc` creates only the LEAF, with one
    // mkdir(dir, 0700), and treats any failure but EEXIST as fatal (§4.2). Without this the
    // first `systemctl --user start` fails with ENOENT and every command then reports "the
    // wallet daemon is not running" — the symptom pointing away from the cause.
    for dir in missing_ancestors(&ctx.settings.wallet.dir) {
        if dry_run {
            actions.push(FileAction {
                what: "CreateDir wallet.dir".to_string(),
                path: dir,
                status: FileStatus::Planned,
                diff: None,
            });
        } else {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .map_err(|e| Error::failure(format!("cannot create {}: {e}", dir.display())))?;
            actions.push(FileAction {
                what: "CreateDir wallet.dir".to_string(),
                path: dir,
                status: FileStatus::Written,
                diff: None,
            });
        }
    }

    let login = existing_or_new_node_login(ctx, rotate)?;
    let node_unit_path = ctx.paths.systemd_user_dir.join(&ctx.settings.node.unit);
    let wallet_unit_path = ctx.paths.systemd_user_dir.join(&ctx.settings.wallet.unit);
    let files: [(&str, std::path::PathBuf, String, u32); 3] = [
        ("node unit", node_unit_path, render_node_unit(ctx), 0o644),
        (
            "wallet unit",
            wallet_unit_path,
            render_wallet_unit(ctx),
            0o644,
        ),
        (
            "monerod.conf",
            ctx.paths.monerod_conf(),
            render_node_conf(ctx, &login),
            0o600,
        ),
    ];
    let mut wrote_any = false;
    for (what, path, rendered, mode) in files {
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current == rendered {
            actions.push(FileAction {
                what: what.to_string(),
                path,
                status: FileStatus::Unchanged,
                diff: None,
            });
            continue;
        }
        if dry_run {
            actions.push(FileAction {
                what: what.to_string(),
                path,
                status: FileStatus::Planned,
                // Everything `setup` PRINTS redacts the credential (§4.2).
                diff: Some(unified_diff(
                    &redact_login(&current),
                    &redact_login(&rendered),
                )),
            });
            continue;
        }
        write_at_mode(&path, rendered.as_bytes(), mode)?;
        wrote_any = true;
        actions.push(FileAction {
            what: what.to_string(),
            path,
            status: FileStatus::Written,
            diff: None,
        });
    }
    if !dry_run && wrote_any {
        let cancel = CancelToken::new();
        // A unit written without a reload is a change that reports success and does nothing.
        let _ = proc::run_with_deadline(
            &["systemctl", "--user", "daemon-reload"],
            ctx.timeouts.systemctl,
            Some(&cancel),
        );
        // Both units are written DISABLED: they are started on demand (§4.2).
        for unit in [&ctx.settings.node.unit, &ctx.settings.wallet.unit] {
            let _ = proc::run_with_deadline(
                &["systemctl", "--user", "disable", unit],
                ctx.timeouts.systemctl,
                Some(&cancel),
            );
        }
    }
    Ok(actions)
}

/// Every ancestor of `dir` that does not exist, outermost first, `dir` included. The doctor's
/// `wallet_dir_exists` fixer emits one `CreateDir` per level for the same reason: each stays
/// individually undoable (§7.1).
fn missing_ancestors(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut missing = Vec::new();
    let mut cur = Some(dir);
    while let Some(p) = cur {
        if p.exists() || p.parent().is_none() {
            break;
        }
        missing.push(p.to_path_buf());
        cur = p.parent();
    }
    missing.reverse();
    missing
}

fn write_at_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<(), Error> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::failure(format!("cannot create {}: {e}", parent.display())))?;
    }
    let tmp = path.with_extension("fmonero-tmp");
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

/// A minimal unified diff: enough for a human to see what `setup --units` would change, and
/// deliberately not a library (§3.1 takes no diff dependency).
fn unified_diff(before: &str, after: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    let mut out = String::new();
    out.push_str("--- current\n+++ rendered\n");
    // Longest common prefix and suffix, then everything between is the hunk. Two rendered
    // versions of one template differ in a handful of adjacent lines, so this is exact for
    // the case that matters and honest (a whole-file hunk) for any other.
    let mut head = 0;
    while head < a.len() && head < b.len() && a[head] == b[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < a.len() - head
        && tail < b.len() - head
        && a[a.len() - 1 - tail] == b[b.len() - 1 - tail]
    {
        tail += 1;
    }
    out.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        head + 1,
        a.len() - head - tail,
        head + 1,
        b.len() - head - tail
    ));
    for line in &a[head..a.len() - tail] {
        out.push_str(&format!("-{line}\n"));
    }
    for line in &b[head..b.len() - tail] {
        out.push_str(&format!("+{line}\n"));
    }
    out
}

// ---- the single-writer probe (§2i) -------------------------------------------

/// Two probes (§2i): `<paths.proc_root>/*/cmdline` for a `monerod` already serving
/// `data_dir`, and `fcntl(F_GETLK)` with **`l_type: F_WRLCK`** on
/// `<data_dir>/lmdb/lock.mdb`. `Some(pid)` names the holder.
pub fn data_dir_in_use(ctx: &Context, data_dir: &Path) -> Result<Option<i32>, Error> {
    if let Some(pid) = monerod_on_data_dir(ctx, data_dir)? {
        return Ok(Some(pid));
    }
    Ok(lmdb_lock_holder(&ctx.paths.lmdb_lock(data_dir)))
}

/// Probe 1: a `monerod` whose `--data-dir`, or whose `--config-file`'s `data-dir=`, resolves
/// to the same path. `monerod-not-really` must not match, so the comparison is on the
/// **basename of argv[0]**, not on a substring.
fn monerod_on_data_dir(ctx: &Context, data_dir: &Path) -> Result<Option<i32>, Error> {
    let entries = match std::fs::read_dir(&ctx.paths.proc_root) {
        Ok(e) => e,
        // No /proc (or no fixture) is not a failure: the fcntl probe is the authority.
        Err(_) => return Ok(None),
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let argv: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let Some(exe) = argv.first() else { continue };
        if Path::new(exe).file_name().and_then(|n| n.to_str()) != Some("monerod") {
            continue;
        }
        if argv_data_dir(&argv).is_some_and(|d| same_dir(&d, data_dir)) {
            return Ok(Some(pid));
        }
    }
    Ok(None)
}

/// `--data-dir DIR` from an argv, or `data-dir=` from the `--config-file` it names. **argv
/// wins over the file**, because `po::store` is called on argv first and boost never
/// overwrites an already-set value (`src/daemon/main.cpp:215-241`, §1b.6).
fn argv_data_dir(argv: &[String]) -> Option<std::path::PathBuf> {
    let flag = |name: &str| {
        argv.iter()
            .position(|a| a == name)
            .and_then(|i| argv.get(i + 1))
            .cloned()
            .or_else(|| {
                argv.iter()
                    .find_map(|a| a.strip_prefix(&format!("{name}=")).map(str::to_string))
            })
    };
    if let Some(d) = flag("--data-dir") {
        return Some(std::path::PathBuf::from(d));
    }
    let conf = flag("--config-file")?;
    let text = std::fs::read_to_string(conf).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some(v) = line.strip_prefix("data-dir") {
            if let Some(v) = v.trim_start().strip_prefix('=') {
                return Some(std::path::PathBuf::from(v.trim()));
            }
        }
    }
    None
}

/// Two paths that name the same directory. `canonicalize` when both exist — a symlinked
/// `~/.bitmonero` is the case that matters — and a lexical comparison otherwise, because the
/// §2i fixtures name `/var/lib/monero`, which does not exist on this machine.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => normalize(a) == normalize(b),
    }
}

fn normalize(p: &Path) -> String {
    let s = p.to_string_lossy();
    let s = s.trim_end_matches('/');
    s.replace("/./", "/")
}

/// Probe 2: the POSIX **record** lock LMDB holds on `<data-dir>/lmdb/lock.mdb`
/// (`mdb.c:5083-5099`), queried with `l_type: F_WRLCK`.
///
/// The query type is load-bearing. A running `monerod` holds a **shared** (`F_RDLCK`) lock on
/// that byte for the life of the process: `mdb_env_share_locks` downgrades the setup-time
/// exclusive lock (`mdb.c:5022-5057`). A write-lock query conflicts with a held read lock; a
/// read-lock query does **not**, so `F_GETLK{F_RDLCK}` — the natural reading of "is this file
/// locked?" — reports a live data dir as free and `node up` starts a second writer on the
/// LMDB. `flock` is worse still: on Linux it is an independent namespace and would always
/// succeed.
fn lmdb_lock_holder(lock_file: &Path) -> Option<i32> {
    use std::os::unix::io::AsRawFd;
    // A missing lock.mdb means the directory was never opened (§2i).
    let f = std::fs::File::open(lock_file).ok()?;
    let mut fl = libc::flock {
        l_type: libc::F_WRLCK as libc::c_short,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: 0,
        l_len: 1,
        l_pid: 0,
    };
    // SAFETY: `f` is open for the duration and `fl` is a valid, fully initialised struct.
    let rc = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETLK, &mut fl) };
    if rc != 0 || fl.l_type == libc::F_UNLCK as libc::c_short {
        return None;
    }
    Some(fl.l_pid)
}

// ---- systemctl plumbing ------------------------------------------------------

fn systemctl(ctx: &Context, argv: &[&str]) -> Result<proc::Output, Error> {
    let cancel = CancelToken::new();
    proc::run_with_deadline(argv, ctx.timeouts.systemctl, Some(&cancel))
}

/// The first word of a `systemctl is-*` answer. A non-zero exit is **not** an error here:
/// `is-active` exits 3 for an inactive unit and prints `inactive`, which is the answer.
fn systemctl_word(ctx: &Context, argv: &[&str]) -> Result<String, Error> {
    if proc::which(argv[0]).is_none() {
        return Ok(String::new());
    }
    let out = systemctl(ctx, argv)?;
    Ok(out.stdout_string().trim().to_string())
}

fn one_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// One line of a `Systemctl::Fixture` file (§2i):
/// `<unit> <is-active> <is-enabled> <exists> <main-pid>`, `-` for an absent pid.
struct FixtureRow {
    active: bool,
    enabled: bool,
    exists: bool,
    main_pid: Option<i32>,
}

fn fixture_row(path: &Path, unit: &str) -> Result<Option<FixtureRow>, Error> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        Error::failure(format!(
            "cannot read the systemctl fixture {}: {e}",
            path.display()
        ))
    })?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut f = line.split_whitespace();
        let Some(name) = f.next() else { continue };
        if name != unit {
            continue;
        }
        let word = |o: Option<&str>| o.unwrap_or("").to_string();
        let active = word(f.next()) == "active";
        let enabled = word(f.next()) == "enabled";
        let exists = matches!(word(f.next()).as_str(), "yes" | "true");
        let main_pid = f
            .next()
            .and_then(|v| v.parse::<i32>().ok())
            .filter(|p| *p > 0);
        return Ok(Some(FixtureRow {
            active,
            enabled,
            exists,
            main_pid,
        }));
    }
    Ok(None)
}

/// `units::{start, stop}` under `Systemctl::Fixture` are **no-ops that rewrite the
/// `<is-active>` column** of the named line (§2i). Without them nothing could reach
/// `Layout::UserUnit` with a running unit in a test.
fn fixture_set_active(path: &Path, unit: &str, active: bool) -> Result<(), Error> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        Error::failure(format!(
            "cannot read the systemctl fixture {}: {e}",
            path.display()
        ))
    })?;
    let mut out = String::with_capacity(text.len());
    let mut found = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut f: Vec<&str> = trimmed.split_whitespace().collect();
        if f.first() == Some(&unit) && f.len() >= 2 {
            f[1] = if active { "active" } else { "inactive" };
            found = true;
            out.push_str(&f.join(" "));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !found {
        return Err(Error::backend(format!(
            "the systemctl fixture {} has no line for {unit}",
            path.display()
        ))
        .with_reason("unit_missing"));
    }
    std::fs::write(path, out)
        .map_err(|e| Error::failure(format!("cannot write {}: {e}", path.display())))
}

fn unit_file_exists(ctx: &Context, unit: &str) -> Result<bool, Error> {
    if let Systemctl::Fixture(path) = &ctx.systemctl {
        return Ok(fixture_row(path, unit)?.is_some_and(|r| r.exists));
    }
    Ok(ctx.paths.systemd_user_dir.join(unit).exists())
}

/// **The permanent guard of §4.2.** boost's `program_options` does no specifier
/// expansion, so a `%h` in `monerod.conf` would make monerod create a directory
/// literally named `%h` under its working directory and resync the chain from zero — a
/// failure no mock can catch. `--dry-run` writes no file, so grepping the file could
/// never fail; this calls the renderer directly.
#[cfg(test)]
#[test]
fn render_node_conf_has_no_systemd_specifiers() {
    let dir = std::env::temp_dir().join(format!("fmonero-conf-{}", std::process::id()));
    let ctx = Context::for_test(&dir).expect("test context");
    let login = Login {
        user: LOGIN_USER.into(),
        pass: "0".repeat(32),
    };
    let text = render_node_conf(&ctx, &login);
    for bad in ["%h", "%S", "%t"] {
        assert!(
            !text.contains(bad),
            "monerod.conf must carry no systemd specifier, found {bad}:\n{text}"
        );
    }
    // Every path in it is absolute.
    for line in text.lines() {
        for key in ["data-dir=", "log-file="] {
            if let Some(v) = line.strip_prefix(key) {
                assert!(
                    Path::new(v).is_absolute(),
                    "{key}{v} must be absolute (boost does not expand ~ either)"
                );
            }
        }
    }
    assert!(text.contains("rpc-bind-ip=127.0.0.1"));
    assert!(text.contains("rpc-bind-port=18081"));
    assert!(text.contains("no-zmq=1"));
    assert!(text.contains("check-updates=disabled"));
    assert!(text.contains("out-peers=16") && text.contains("in-peers=32"));
    assert!(text.contains("prune-blockchain=1"));
    assert!(!text.contains("stagenet") && !text.contains("testnet"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5b rule 3: `Layout::UserUnit` serializes as `"user_unit"`. A plain derive would emit
    /// `"UserUnit"`, which is a JSON document no section of the design describes — and the
    /// plugin's parser, the doctor's output and the e2e assertions all read this key.
    #[test]
    fn the_layout_spellings_are_snake_case() {
        for (layout, wire) in [
            (Layout::UserUnit, "user_unit"),
            (Layout::SystemUnit, "system_unit"),
            (Layout::Remote, "remote"),
            (Layout::None, "none"),
        ] {
            assert_eq!(serde_json::to_value(layout).unwrap(), wire);
            assert_eq!(layout.as_str(), wire);
            let back: Layout = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(back, layout);
        }
    }

    #[test]
    fn the_file_status_spellings_are_lowercase() {
        for (status, wire) in [
            (FileStatus::Written, "written"),
            (FileStatus::Unchanged, "unchanged"),
            (FileStatus::Skipped, "skipped"),
            (FileStatus::Planned, "planned"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), wire);
            assert_eq!(status.as_str(), wire);
            let back: FileStatus = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn a_missing_unit_reports_itself_as_missing() {
        let u = UnitState::missing("fmonero-monerod.service");
        assert_eq!(u.name, "fmonero-monerod.service");
        assert!(!u.exists && !u.active && !u.enabled);
        let v = serde_json::to_value(&u).unwrap();
        assert_eq!(v["exists"], false);
    }

    fn ctx_in(dir: &Path) -> Context {
        Context::for_test(dir).expect("test context")
    }

    /// The wallet unit is the **only** place a specifier belongs (§4.2, v5.1), and both units
    /// render the paths that are compared against a setting — `--wallet-dir` against
    /// `settings.wallet.dir` (the doctor's `wallet_dir_matches_unit`) and `--config-file`
    /// against `paths.monerod_conf()`.
    #[test]
    fn the_units_carry_the_specifiers_and_the_literal_wallet_dir() {
        let dir = std::env::temp_dir().join(format!("fmonero-units-{}", std::process::id()));
        let mut ctx = ctx_in(&dir);
        let node = render_node_unit(&ctx);
        // v5.1, closing `secrets-5`: the ABSOLUTE, XDG-resolved conf path, never `%h`. With a
        // non-default XDG_CONFIG_HOME the `%h` literal named a file `setup --units` never
        // wrote, and monerod exits on a missing named config file (§1b.6).
        assert!(
            node.contains(&format!(
                "--config-file {} --non-interactive",
                ctx.paths.monerod_conf().display()
            )),
            "{node}"
        );
        assert!(
            !node.contains('%'),
            "the node unit compares its only path against a setting, so it carries no \
             specifier at all (§4.2 v5.1):\n{node}"
        );
        // A 203/EXEC must not restart every five seconds forever: RestartSec=5 can never reach
        // the manager's 10 s / 5-start default, so both units set the limit themselves (§4.2).
        assert!(node.contains("StartLimitIntervalSec=60"), "{node}");
        assert!(node.contains("StartLimitBurst=3"), "{node}");
        assert!(node.contains("Restart=on-failure"));
        assert!(node.contains(&format!("RestartSec={RESTART_SEC}")));
        assert!(!node.contains("Wants=fmonero-monerod"));
        let wallet = render_wallet_unit(&ctx);
        // §2b's test (4): asserted on the rendered text, so it needs no systemd run.
        assert!(wallet.contains("StateDirectoryMode=0700"), "{wallet}");
        assert!(wallet.contains("StartLimitIntervalSec=60"), "{wallet}");
        assert!(wallet.contains("StartLimitBurst=3"), "{wallet}");
        // §4.2 keeps `%S` exactly where nothing is compared against a setting.
        assert!(wallet.contains("WorkingDirectory=%S/fmonero/run"));
        assert!(wallet.contains("--log-level 0"), "a secrets control (§4.2)");
        assert!(wallet.contains("--max-concurrency 4"));
        assert!(wallet.contains("--rpc-bind-port 18083"));
        assert!(wallet.contains(&format!(
            "--wallet-dir {}",
            ctx.settings.wallet.dir.display()
        )));
        assert!(!wallet.contains("%h/"), "§4.2: never %h for --wallet-dir");
        // §4.2: `Wants=fmonero-monerod.service` is deliberately absent — it would start a
        // node around the §2i single-writer check.
        assert!(!wallet.contains("Wants="));
        assert!(wallet.contains("After=fmonero-monerod.service"));
        // Nothing the daemon would log a secret through, and no password in argv (§7.1).
        for flag in [
            "--password",
            "--rpc-login",
            "--daemon-login",
            "--disable-rpc-login",
            "--restricted-rpc",
            "--detach",
        ] {
            assert!(!wallet.contains(flag), "{flag} must not be in the argv");
        }
        // The network changes three things and nothing else (§4).
        ctx.settings.node.network = crate::settings::Network::Stagenet;
        ctx.settings.wallet.port = 38083;
        ctx.settings.node.url = "http://127.0.0.1:38081".into();
        let wallet = render_wallet_unit(&ctx);
        assert!(wallet.contains("--stagenet"), "{wallet}");
        assert!(wallet.contains("--rpc-bind-port 38083"));
        assert!(wallet.contains("--daemon-address 127.0.0.1:38081"));
        let conf = render_node_conf(
            &ctx,
            &Login {
                user: LOGIN_USER.into(),
                pass: "1".repeat(32),
            },
        );
        assert!(conf.contains("stagenet=1"));
        assert!(conf.contains("rpc-bind-port=38081"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn everything_setup_prints_redacts_the_credential() {
        let text = "data-dir=/x\nrpc-login=fmonero:0123456789abcdef0123456789abcdef\nno-zmq=1\n";
        let shown = redact_login(text);
        assert!(shown.contains("rpc-login=<redacted>"), "{shown}");
        assert!(!shown.contains("0123456789abcdef"), "{shown}");
        assert!(shown.contains("data-dir=/x") && shown.contains("no-zmq=1"));
        assert!(shown.ends_with('\n'));
    }

    /// Idempotence rests on this: an existing valid token is carried through, so a second
    /// `setup --units` neither changes the file nor locks out a running node (§4.2).
    #[test]
    fn an_existing_credential_is_carried_through_and_only_rotate_replaces_it() {
        let dir = std::env::temp_dir().join(format!("fmonero-login2-{}", std::process::id()));
        let ctx = ctx_in(&dir);
        let first = existing_or_new_node_login(&ctx, false).unwrap();
        assert_eq!(first.user, "fmonero");
        assert_eq!(first.pass.len(), 32);
        std::fs::write(
            ctx.paths.monerod_conf(),
            render_node_conf(&ctx, &first).as_bytes(),
        )
        .unwrap();
        let again = existing_or_new_node_login(&ctx, false).unwrap();
        assert_eq!(
            again.pass, first.pass,
            "an existing token is carried through"
        );
        let rotated = existing_or_new_node_login(&ctx, true).unwrap();
        assert_ne!(
            rotated.pass, first.pass,
            "--rotate-rpc-login mints a new one"
        );
        // A malformed line draws a fresh one rather than being carried through.
        std::fs::write(ctx.paths.monerod_conf(), b"rpc-login=someoneelse:short\n").unwrap();
        let fresh = existing_or_new_node_login(&ctx, false).unwrap();
        assert_eq!(fresh.user, "fmonero");
        assert_eq!(fresh.pass.len(), 32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `--data-dir` reading of §1b.6, including the `--config-file` fallback and the
    /// case a substring match would get wrong.
    #[test]
    fn the_data_dir_is_read_from_argv_first_and_from_the_config_file_second() {
        let argv: Vec<String> = ["/usr/bin/monerod", "--data-dir", "/var/lib/monero"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            argv_data_dir(&argv),
            Some(std::path::PathBuf::from("/var/lib/monero"))
        );
        let joined: Vec<String> = ["/usr/bin/monerod", "--data-dir=/srv/x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            argv_data_dir(&joined),
            Some(std::path::PathBuf::from("/srv/x"))
        );
        let dir = std::env::temp_dir().join(format!("fmonero-argv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("monerod.conf");
        std::fs::write(
            &conf,
            b"# a comment\ndata-dir=/var/lib/monero\nlog-level=0\n",
        )
        .unwrap();
        let via_conf: Vec<String> = vec![
            "/usr/bin/monerod".into(),
            "--config-file".into(),
            conf.display().to_string(),
        ];
        assert_eq!(
            argv_data_dir(&via_conf),
            Some(std::path::PathBuf::from("/var/lib/monero"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The case that distinguishes a correct probe from every wrong one.** A running
    /// `monerod` holds a SHARED (`F_RDLCK`) lock on byte 0 of `lmdb/lock.mdb`
    /// (`mdb.c:5050`); only an `F_GETLK{F_WRLCK}` query conflicts with it. A forked child
    /// holds the lock, because POSIX record locks are per process and a second lock from this
    /// process would silently succeed.
    #[test]
    fn a_shared_fcntl_lock_is_detected_and_an_flock_only_holder_is_not() {
        use std::os::unix::io::AsRawFd;
        let dir = std::env::temp_dir().join(format!("fmonero-lmdb-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("lmdb")).unwrap();
        let lock = dir.join("lmdb/lock.mdb");
        std::fs::write(&lock, b"\0\0\0\0").unwrap();

        // Nothing holds it yet.
        assert_eq!(lmdb_lock_holder(&lock), None);

        for (l_type, what) in [
            (libc::F_RDLCK, "the SHARED lock a live monerod holds"),
            (libc::F_WRLCK, "an exclusive lock"),
        ] {
            let child = fork_holding(&lock, l_type);
            let seen = wait_for(|| lmdb_lock_holder(&lock));
            assert_eq!(seen, Some(child), "{what} must be detected");
            reap(child);
        }

        // And the negative case that documents why `flock` was rejected: on Linux the two
        // families are independent namespaces, so an flock-only holder is invisible here —
        // which is exactly why an flock PROBE would report a live data dir as free.
        let f = std::fs::File::open(&lock).unwrap();
        // SAFETY: `f` is open for the duration of the call.
        assert_eq!(
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert_eq!(
            lmdb_lock_holder(&lock),
            None,
            "an flock holder is not an fcntl holder (§2i)"
        );
        // SAFETY: same descriptor, still open.
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fork a child that takes an `fcntl` lock of `l_type` on byte 0 and then blocks.
    ///
    /// POSIX record locks are per **process**, so a lock taken from this process would silently
    /// succeed and the test would prove nothing. The child closes every inherited descriptor
    /// above stderr **before** opening anything and calls only async-signal-safe functions —
    /// `fork` duplicates descriptors into the same open file descriptions, so a child that kept
    /// them would go on holding another test's `flock` for as long as it lived.
    fn fork_holding(lock: &Path, l_type: libc::c_int) -> i32 {
        use std::os::unix::ffi::OsStrExt;
        let mut c: Vec<u8> = lock.as_os_str().as_bytes().to_vec();
        c.push(0);
        // SAFETY: after the fork the child calls only async-signal-safe functions, over a path
        // buffer allocated before it.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            // SAFETY: every call below is async-signal-safe and `c` is NUL-terminated.
            unsafe {
                let mut fd = 3;
                while fd < 1024 {
                    libc::close(fd);
                    fd += 1;
                }
                let f = libc::open(c.as_ptr() as *const libc::c_char, libc::O_RDWR);
                if f < 0 {
                    libc::_exit(8);
                }
                let mut fl = libc::flock {
                    l_type: l_type as libc::c_short,
                    l_whence: libc::SEEK_SET as libc::c_short,
                    l_start: 0,
                    l_len: 1,
                    l_pid: 0,
                };
                if libc::fcntl(f, libc::F_SETLK, &mut fl) != 0 {
                    libc::_exit(9);
                }
                loop {
                    libc::sleep(1);
                }
            }
        }
        pid
    }

    fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> Option<T> {
        for _ in 0..200 {
            if let Some(v) = f() {
                return Some(v);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn reap(pid: i32) {
        // SAFETY: `pid` is a child of this process.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            let mut st = 0;
            libc::waitpid(pid, &mut st, 0);
        }
    }

    /// The §2i seam, end to end: a fixture file decides `detect_layout`, `state`, `main_pid`
    /// and whether the mutators may be handed out — and `start`/`stop` rewrite the
    /// `<is-active>` column. Without all four nothing can reach `Layout::UserUnit` in a test.
    #[test]
    fn the_systemctl_fixture_drives_layout_state_and_start() {
        let dir = std::env::temp_dir().join(format!("fmonero-fixture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("systemctl.txt");
        std::fs::write(
            &fixture,
            "# a comment line consumers skip\n\
             fmonero-monerod.service inactive disabled yes 0\n\
             fmonero-wallet-rpc.service active disabled yes 51002\n\
             monerod.service inactive disabled no -\n",
        )
        .unwrap();
        let mut ctx = Context::for_test(&dir).unwrap();
        ctx.systemctl = Systemctl::Fixture(fixture.clone());

        assert_eq!(detect_layout(&ctx).unwrap(), Layout::UserUnit);
        assert!(may_mutate_node(&ctx).unwrap());
        let node = state(&ctx, "fmonero-monerod.service").unwrap();
        assert!(node.exists && !node.active && !node.enabled);
        assert_eq!(main_pid(&ctx, "fmonero-monerod.service").unwrap(), None);
        assert_eq!(
            main_pid(&ctx, "fmonero-wallet-rpc.service").unwrap(),
            Some(51002)
        );

        let cancel = CancelToken::new();
        start(&ctx, "fmonero-monerod.service", &cancel).unwrap();
        assert!(state(&ctx, "fmonero-monerod.service").unwrap().active);
        stop(&ctx, "fmonero-monerod.service", &cancel).unwrap();
        assert!(!state(&ctx, "fmonero-monerod.service").unwrap().active);
        // The comment line survives the rewrite.
        assert!(std::fs::read_to_string(&fixture)
            .unwrap()
            .contains("# a comment"));

        // A unit the fixture does not list is absent, not unknown.
        assert!(!state(&ctx, "nothing.service").unwrap().exists);

        // `system_unit`: neither fmonero unit exists and Arch's is active.
        std::fs::write(
            &fixture,
            "fmonero-monerod.service inactive disabled no -\n\
             fmonero-wallet-rpc.service inactive disabled no -\n\
             monerod.service active enabled yes 8421\n",
        )
        .unwrap();
        assert_eq!(detect_layout(&ctx).unwrap(), Layout::SystemUnit);
        assert!(!may_mutate_node(&ctx).unwrap());

        // `none`: nothing anywhere.
        std::fs::write(
            &fixture,
            "fmonero-monerod.service inactive disabled no -\n\
             monerod.service inactive disabled no -\n",
        )
        .unwrap();
        assert_eq!(detect_layout(&ctx).unwrap(), Layout::None);

        // `remote` is decided by the SETTING alone, even with the unit present and active —
        // the case a detector that reads only the unit file gets wrong (§2i).
        std::fs::write(
            &fixture,
            "fmonero-monerod.service active disabled yes 41233\n\
             monerod.service inactive disabled no -\n",
        )
        .unwrap();
        ctx.settings.node.url = "http://10.0.0.5:18081".into();
        assert_eq!(detect_layout(&ctx).unwrap(), Layout::Remote);
        assert!(!may_mutate_node(&ctx).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A dry run writes nothing and is allowed anywhere, which is what lets `tests/e2e.sh`
    /// assert the diff and the `CreateDir` action in its own state dir. The **refusal** to
    /// write under `FMONERO_STATE_DIR` is asserted by `tests/e2e.sh` instead: it needs the
    /// environment variable set, and mutating the process environment from a test would race
    /// every other test in this binary.
    #[test]
    fn a_dry_run_plans_the_three_files_and_redacts_the_credential() {
        let dir = std::env::temp_dir().join(format!("fmonero-install-{}", std::process::id()));
        let ctx = Context::for_test(&dir).unwrap();
        let planned = install(&ctx, true).expect("a dry run writes nothing and is allowed");
        assert!(planned
            .iter()
            .all(|a| a.status == FileStatus::Planned || a.status == FileStatus::Unchanged));
        // The diff is redacted, and the CreateDir action for wallet.dir is listed (§4.2).
        let text = serde_json::to_string(&planned).unwrap();
        assert!(text.contains("CreateDir wallet.dir"), "{text}");
        assert!(!text.contains("rpc-login=fmonero:"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Finding 1 of the Phase 4 verification, as a test.** Before v5.1 neither question was
    /// asked: on a machine `setup --units` had configured and `omarchy pkg add monero` had not
    /// reached, `node up` spent thirty seconds waiting for a unit that exits `203/EXEC`, exited
    /// **8** ("nothing is broken") and left the unit restarting every five seconds. §4a assigns
    /// that condition exit **4**.
    ///
    /// Both cases are machine-independent: the unit file is one this test writes into its own
    /// sandboxed `systemd_user_dir`, and the binary is named explicitly, so neither answer
    /// depends on what is installed here.
    #[test]
    fn preflight_wants_a_unit_file_and_a_binary() {
        let dir = std::env::temp_dir().join(format!("fmonero-preflight-{}", std::process::id()));
        let ctx = ctx_in(&dir);
        let unit = ctx.settings.node.unit.clone();

        // (1) No unit file: exit 4 `unit_missing`, and the hint writes one.
        let e = preflight(&ctx, &unit, "sh").expect_err("no unit file");
        assert_eq!(e.exit_code(), 4, "{e}");
        assert_eq!(e.reason(), Some("unit_missing"));
        assert!(e.hint().contains("fmonero setup --units"), "{}", e.hint());

        // (2) The unit file is there and the binary is not: exit 4 `not_installed`, with the
        // install command as the hint — not exit 8 thirty seconds later.
        std::fs::create_dir_all(&ctx.paths.systemd_user_dir).unwrap();
        std::fs::write(ctx.paths.systemd_user_dir.join(&unit), b"[Unit]\n").unwrap();
        let started = std::time::Instant::now();
        let e = preflight(&ctx, &unit, "monerod-does-not-exist-anywhere").expect_err("no binary");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "it must answer within milliseconds, not at a deadline"
        );
        assert_eq!(e.exit_code(), 4, "{e}");
        assert_eq!(e.reason(), Some("not_installed"));
        assert!(e.hint().contains("omarchy pkg add monero"), "{}", e.hint());
        assert!(e.to_string().contains("203/EXEC"), "{e}");

        // (3) Both present: nothing to say. `sh` stands in for the daemon binary because it is
        // on every PATH this code can run on.
        preflight(&ctx, &unit, "sh").expect("a unit file and a binary is all it asks for");

        // (4) The §2i fixture seam stands in for the machine's daemon as well — that is what
        // lets every AMBER `node up` row of §4b run against `fmonero-mock` — but it never
        // excuses a MISSING unit, which the fixture itself reports.
        let fixture = dir.join("systemctl.txt");
        std::fs::write(&fixture, format!("{unit} inactive disabled yes 0\n")).unwrap();
        let mut fx = ctx.clone();
        fx.systemctl = Systemctl::Fixture(fixture.clone());
        preflight(&fx, &unit, "monerod-does-not-exist-anywhere")
            .expect("the fixture stands in for the daemon");
        std::fs::write(&fixture, format!("{unit} inactive disabled no -\n")).unwrap();
        let e = preflight(&fx, &unit, "sh").expect_err("but not for the unit file");
        assert_eq!(e.reason(), Some("unit_missing"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The binary each unit runs, so `ensure_running` — whose signature is frozen (§5b) — can
    /// ask [`preflight`]'s second question without a second parameter.
    #[test]
    fn each_unit_names_its_own_daemon() {
        let dir = std::env::temp_dir().join(format!("fmonero-unitbin-{}", std::process::id()));
        let ctx = ctx_in(&dir);
        assert_eq!(unit_binary(&ctx, &ctx.settings.node.unit), "monerod");
        assert_eq!(
            unit_binary(&ctx, &ctx.settings.wallet.unit),
            "monero-wallet-rpc"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A unit **we** started that never answered is stopped again, and the error says so — so a
    /// failed `wallet open` or `node up` never leaves a `Restart=on-failure` loop behind. Driven
    /// through the §2i fixture, whose `<is-active>` column is the observable.
    #[test]
    fn a_unit_that_never_answered_is_stopped_again() {
        let dir = std::env::temp_dir().join(format!("fmonero-flap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("systemctl.txt");
        let mut ctx = ctx_in(&dir);
        let unit = ctx.settings.wallet.unit.clone();
        std::fs::write(&fixture, format!("{unit} inactive disabled yes 0\n")).unwrap();
        ctx.systemctl = Systemctl::Fixture(fixture.clone());
        let cancel = CancelToken::new();

        let e = ensure_running(&ctx, &unit, &|| false, Duration::from_millis(300), &cancel)
            .expect_err("a probe that never succeeds is a timeout");
        assert_eq!(e.exit_code(), 8, "{e}");
        assert!(e.to_string().contains("was stopped again"), "{e}");
        assert!(
            !state(&ctx, &unit).unwrap().active,
            "the unit must be inactive again, not flapping"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_daemon_address_is_the_node_urls_host_and_port() {
        assert_eq!(host_port("http://127.0.0.1:18081"), "127.0.0.1:18081");
        assert_eq!(host_port("http://127.0.0.1:38081/"), "127.0.0.1:38081");
        assert_eq!(host_port("http://[::1]:18081"), "[::1]:18081");
    }
}
