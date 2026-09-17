//! The **only** place the doctor spawns a process, reads a file or issues an RPC
//! (DESIGN.md §7). Everything here produces captured facts; `checks.rs` is a set of pure
//! functions over the result.
//!
//! Every probe is bounded: commands run under `fmonero_core::proc::run_with_deadline` (its
//! own process group, SIGTERM then SIGKILL), every HTTP call goes through a client built
//! with a deadline (§2d), and a probe **never fails a run** — a command that cannot be
//! spawned, times out or errors is recorded as such and the detector decides what that
//! means.
//!
//! Two §2b rules are enforced here rather than downstream, because "redacted at capture"
//! is the only form of redaction that works:
//!
//! * every captured line matching a credential key is rewritten to `<key>=<redacted>`
//!   **before** the bytes are stored ([`redact`], §2b.8);
//! * the **contents** of `paths.monerod_conf()` and `paths.wallet_login_file(port)` are
//!   never read into a probe at all — only their modes and two derived booleans. Key-name
//!   redaction cannot fire on a login file whose single line is `monero:<base64>`: there is
//!   no key to match, so the only safe rule is not to capture it (§2b.8).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fmonero_core::units::{Layout, UnitState};
use fmonero_core::{history, proc, settings::Settings, state, units, Context, PendingSend};
use serde::{Deserialize, Serialize};

use super::DoctorGroup;

/// `monerod --version` / `monero-wallet-rpc --version` deadline (§7.1).
pub const VERSION_PROBE_SEC: u64 = 3;
/// A probe artifact is truncated at 64 KiB with a trailer (§7).
pub const PROBE_TRUNCATE: usize = 64 * 1024;
/// The major.minor fmonero was built against (§7.1 `version_supported`).
pub const SUPPORTED_MONERO: &str = "0.18";
/// The bar widget's id, which is also its directory name (§6).
pub const PLUGIN_ID: &str = "oliver.monero";
/// §7.1 `wallet_daemon_link` / `wallet_daemon_auth`: the one constant, shared with §5's
/// `wallet_behind` warning and §2d's syncing classification.
pub use fmonero_core::WALLET_BEHIND_BLOCKS;
/// §7.1 `disk_space`: 120 GB for a pruned chain plus headroom — an **estimate** (§1.3).
pub const DISK_FLOOR_BYTES: u64 = 120 * 1_000_000_000;
/// §7.1 `history_cache_fresh`: the high-water height must be within this of the wallet's.
pub const CACHE_STALE_BLOCKS: u64 = 1_000;
/// §7.1 `node_clock`: the daemon's adjusted time must be within this of ours.
pub const CLOCK_SKEW_SEC: i64 = 60;

// ------------------------------------------------------------------ captured shapes

/// One captured external command.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CmdProbe {
    pub argv: Vec<String>,
    pub code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    /// Set when the command could not be spawned at all (missing binary, …).
    pub error: Option<String>,
    /// False when this probe was not taken because its group was not selected.
    pub taken: bool,
}

impl CmdProbe {
    /// Exit 0, not killed, spawned.
    pub fn success(&self) -> bool {
        self.taken && self.error.is_none() && !self.timed_out && self.code == Some(0)
    }
    /// The most useful one-line explanation of a non-success.
    pub fn failure_detail(&self) -> String {
        if let Some(e) = &self.error {
            return e.clone();
        }
        if self.timed_out {
            return format!("killed at its deadline after {} ms", self.duration_ms);
        }
        let tail = self
            .stderr
            .lines()
            .chain(self.stdout.lines())
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no output")
            .trim()
            .to_string();
        match self.code {
            Some(c) => format!("exit {c}: {tail}"),
            None => format!("killed by a signal: {tail}"),
        }
    }
    /// The first non-empty line of stdout — what a `--version` probe is for.
    pub fn first_line(&self) -> String {
        self.stdout
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string()
    }
}

/// One captured file. `text` is `None` for a file the doctor deliberately does not read
/// (§2b.8) and for one that does not exist.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileProbe {
    pub path: String,
    pub exists: bool,
    pub len: u64,
    /// The permission bits, `st_mode & 0o7777`.
    pub mode: Option<u32>,
    pub text: Option<String>,
    pub error: Option<String>,
    pub taken: bool,
}

impl FileProbe {
    /// Metadata **and** contents, redacted at capture (§2b.8).
    fn read(path: &Path) -> FileProbe {
        let mut p = FileProbe::stat(path);
        if !p.exists || p.error.is_some() {
            return p;
        }
        match std::fs::read_to_string(path) {
            Ok(t) => p.text = Some(redact(&t)),
            Err(e) => p.error = Some(e.to_string()),
        }
        p
    }

    /// Metadata only: for anything whose body the doctor must not carry (§2b.8).
    fn stat(path: &Path) -> FileProbe {
        let mut p = FileProbe {
            path: path.display().to_string(),
            taken: true,
            ..FileProbe::default()
        };
        match std::fs::symlink_metadata(path) {
            Ok(m) => {
                p.exists = true;
                p.len = m.len();
                p.mode = Some(m.permissions().mode() & 0o7777);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => p.error = Some(e.to_string()),
        }
        p
    }

    /// No group and no other permission bits — §2c's rule for a file whose exact mode
    /// upstream chooses (`monero-wallet-rpc`'s login file settles at 0400 through
    /// `private_file`'s own dance, so a literal `== 0600` would fail on a healthy machine).
    pub fn owner_only(&self) -> bool {
        self.mode.is_some_and(|m| m & 0o077 == 0)
    }
}

/// What `~/.config/omarchy/plugins/oliver.monero` is, if anything.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LinkProbe {
    pub path: String,
    pub exists: bool,
    pub is_symlink: bool,
    /// `readlink(2)`, verbatim.
    pub target: Option<String>,
    /// The fully resolved path — what §7.1 says to validate.
    pub resolved: Option<String>,
    /// Where `setup --plugin` would point it, when this checkout can be found.
    pub want_target: Option<String>,
    pub taken: bool,
}

/// One listening socket, joined from `<proc_root>/net/tcp` and `<proc_root>/*/fd`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortProbe {
    pub port: u16,
    /// The dotted bind address, decoded from the kernel's `inet_addr`-order hex.
    pub address: String,
    pub loopback: bool,
    pub inode: u64,
    pub pid: Option<i32>,
    /// argv[0]'s basename, when a pid was found.
    pub comm: Option<String>,
    /// True when `pid` is the `MainPID` of one of fmonero's own two units.
    pub ours: bool,
}

/// One file or directory fmonero itself wrote, with the mode §2b.7 requires of it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermProbe {
    pub path: String,
    pub is_dir: bool,
    pub mode: u32,
    /// `0700` for a directory, `0600` for a file — except the two run-directory names
    /// whose mode the wallet daemon chooses, which are checked for owner-only instead.
    pub want: u32,
    /// True for `monero-wallet-rpc.*.login` and `monero-wallet-rpc.log`: the daemon
    /// recreates them on every start and `StateDirectoryMode=0700` is the real fix, so
    /// they are never `chmod`ed (§7.1, class D).
    pub daemon_owned: bool,
}

impl PermProbe {
    pub fn ok(&self) -> bool {
        if self.daemon_owned {
            self.mode & 0o077 == 0
        } else {
            self.mode & 0o7777 == self.want
        }
    }
}

/// The §5b `history::CacheHeader`, as facts rather than as a struct the detector would
/// have to re-derive.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheFacts {
    pub magic_ok: bool,
    pub format_version: u32,
    pub expected_format_version: u32,
    pub wallet_fp: String,
    pub high_water: u64,
    pub built_at: i64,
    /// `Some(false)` when a wallet is open and the fingerprint is another wallet's.
    pub fingerprint_matches: Option<bool>,
}

// ------------------------------------------------------------------ redaction (§2b.8)

/// The credential key names §2b.8 names, lower-cased.
const SECRET_KEYS: &[&str] = &[
    "rpc-login",
    "daemon-login",
    "password",
    "login",
    "rpc_login",
    "daemon_password",
];

/// Rewrite any line that carries a credential so the **value** becomes `<redacted>`,
/// **before** the bytes are stored (§2b.8).
///
/// §2b.8's form is `<key>=<redacted>`: the key survives and the secret does not. That is
/// not a detail — `unit_no_password_in_argv` (§7.1) has to report *which* flag it found, so
/// a redactor that replaced the whole line with a generic marker would destroy the one thing
/// the check exists to name, and the check would report `pass` on a unit carrying
/// `--rpc-login`. Both `key=value` and `--flag value` forms are handled, and a line with no
/// credential key is returned byte for byte.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let lower = line.to_ascii_lowercase();
        if !SECRET_KEYS.iter().any(|k| lower.contains(*k)) {
            out.push_str(line);
            continue;
        }
        let nl = if line.ends_with('\n') { "\n" } else { "" };
        let body = line.trim_end_matches('\n').trim_end_matches('\r');
        let indent: String = body.chars().take_while(|c| c.is_whitespace()).collect();
        let mut tokens: Vec<String> = body.split_whitespace().map(str::to_string).collect();
        let mut i = 0;
        while i < tokens.len() {
            let name = tokens[i]
                .to_ascii_lowercase()
                .split('=')
                .next()
                .unwrap_or_default()
                .to_string();
            if SECRET_KEYS.iter().any(|k| name.contains(*k)) {
                match tokens[i].find('=') {
                    Some(eq) => tokens[i] = format!("{}=<redacted>", &tokens[i][..eq]),
                    // `--rpc-login fmonero:…`: the secret is the NEXT token.
                    None => {
                        if i + 1 < tokens.len() {
                            tokens[i + 1] = "<redacted>".to_string();
                            i += 1;
                        }
                    }
                }
            }
            i += 1;
        }
        out.push_str(&format!("{indent}{}{nl}", tokens.join(" ")));
    }
    out
}

/// Truncate an artifact at [`PROBE_TRUNCATE`] with a trailer that says so (§7).
pub fn truncate(text: String) -> String {
    if text.len() <= PROBE_TRUNCATE {
        return text;
    }
    let mut cut = PROBE_TRUNCATE;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!(
        "{}\n… truncated at {PROBE_TRUNCATE} bytes of {} (DESIGN.md §7)\n",
        &text[..cut],
        text.len()
    )
}

// ------------------------------------------------------------------ the capture

/// Everything one doctor run saw. Every detector in `checks.rs` is a pure function of this.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Probes {
    pub taken_at: String,
    /// Now, in Unix seconds, so every time comparison is a pure function of the probes.
    pub now_unix: i64,
    pub groups: Vec<String>,

    // ---- binaries
    pub which_monerod: Option<String>,
    pub monerod_version: CmdProbe,
    pub which_wallet_rpc: Option<String>,
    pub wallet_rpc_version_cmd: CmdProbe,

    // ---- node
    pub layout: String,
    pub layout_error: Option<String>,
    pub node_url: String,
    /// v5.2, §2m: `"local"` or `"remote"` — the fact a third of §7.1 now branches on.
    pub node_mode: String,
    /// `units::daemon_target(ctx)` — the address, the trust flag and the ssl mode the wallet
    /// unit renders and `set_daemon` sends, from the ONE derivation both read (§2m.3).
    pub daemon_address: String,
    pub daemon_trusted: bool,
    pub daemon_ssl: String,
    pub node_taken: bool,
    pub node_reachable: bool,
    pub node_error: Option<String>,
    pub node_height: Option<u64>,
    pub node_target_height: Option<u64>,
    pub node_synchronized: Option<bool>,
    pub node_offline: Option<bool>,
    pub node_restricted: Option<bool>,
    pub node_outgoing_connections: Option<u64>,
    pub node_adjusted_time: Option<i64>,
    pub node_version: Option<String>,
    pub node_authenticated: Option<bool>,
    pub node_nettype: Option<String>,
    pub data_dir: String,
    pub data_dir_holder: Option<i32>,
    pub data_dir_probe_error: Option<String>,

    // ---- wallet
    pub wallet_url: String,
    pub wallet_taken: bool,
    pub wallet_reachable: bool,
    pub wallet_error: Option<String>,
    pub wallet_rpc_version: Option<String>,
    pub wallet_authenticated: Option<bool>,
    pub wallet_open: bool,
    pub wallet_height: Option<u64>,
    pub wallet_primary_address: Option<String>,
    /// Mode only — never the bytes (§2b.8).
    pub wallet_login_file: FileProbe,
    pub wallet_login_port: u16,
    pub last_open_name: Option<String>,
    pub last_open_nettype: Option<String>,
    pub wallet_dir: String,
    /// Ancestors of `wallet.dir` that do not exist, **outermost first** — one `CreateDir`
    /// each, so every level stays individually undoable (§7.1).
    pub wallet_dir_missing: Vec<String>,
    /// Existing levels of `wallet.dir` whose mode is not 0700.
    pub wallet_dir_bad_modes: Vec<(String, u32)>,
    pub pending: Option<PendingSend>,

    // ---- units
    pub systemd_user_dir: String,
    pub node_unit_name: String,
    pub wallet_unit_name: String,
    pub node_unit: FileProbe,
    pub wallet_unit: FileProbe,
    pub node_unit_expected: String,
    pub wallet_unit_expected: String,
    /// The byte-for-byte comparison against `units::render_*`, made **on the unredacted
    /// bytes at capture time** (§7.1). `FileProbe::text` is redacted (§2b.8), so comparing
    /// that copy would report drift on a correct machine the day a unit ever carried a
    /// credential flag — a check that fails on a correct machine is the check an agent
    /// relaxes.
    pub node_unit_matches: bool,
    pub wallet_unit_matches: bool,
    pub node_unit_state: UnitState,
    pub wallet_unit_state: UnitState,
    /// `--wallet-dir`'s argument as the unit actually carries it.
    pub unit_wallet_dir: Option<String>,
    /// `--rpc-bind-port`'s argument as the unit actually carries it.
    pub unit_wallet_port: Option<u16>,
    /// `--daemon-address`, the trust flag and `--daemon-ssl` **as the installed unit carries
    /// them** — the three `wallet_daemon_matches_unit` compares against `daemon_target` (§7.1,
    /// v5.2). A `monero-wallet-rpc` started before any `wallet open` auto-refreshes against
    /// whatever its argv said (§1b.9 item 3), so a unit naming the old node keeps a wallet
    /// syncing from a node `status` does not name — a skew with a SILENT symptom.
    pub unit_daemon_address: Option<String>,
    pub unit_daemon_trusted: Option<bool>,
    pub unit_daemon_ssl: Option<String>,

    // ---- security / storage
    /// Mode only — never the bytes (§2b.8).
    pub monerod_conf: FileProbe,
    pub monerod_conf_prune: Option<bool>,
    pub monerod_conf_has_login: bool,
    pub ports: Vec<PortProbe>,
    /// The ports §7.1 names, resolved for this network and deduplicated — so the `pass`
    /// detail can say what was looked at rather than only "ok".
    pub ports_watched: Vec<u16>,
    pub ports_taken: bool,
    pub perms: Vec<PermProbe>,
    pub free_bytes: Option<u64>,
    pub free_bytes_error: Option<String>,
    pub prune_setting: bool,

    // ---- install
    pub local_bin_fmonero: String,
    pub local_bin_exists: bool,
    pub state_file: FileProbe,
    pub state_parse_error: Option<String>,
    pub settings_file: FileProbe,
    pub settings_parse_error: Option<String>,
    pub settings_unknown_keys: Vec<String>,
    pub settings_problems: Vec<(String, String)>,
    pub cache: Option<CacheFacts>,
    pub cache_present: bool,
    pub plugin_link: LinkProbe,
    pub omarchy_plugin_list: CmdProbe,
    pub menu_file: FileProbe,
    pub menu_missing_ids: Vec<String>,

    // ---- settings facts every group reads
    pub network: String,
    pub expected_wallet_port: u16,
    pub settings_wallet_port: u16,
    pub settings_wallet_url_port: Option<u16>,
    pub node_url_port: Option<u16>,
}

fn group_selected(only: Option<DoctorGroup>, g: DoctorGroup) -> bool {
    only.is_none() || only == Some(g)
}

fn run(argv: &[&str], deadline: Duration) -> CmdProbe {
    let started = Instant::now();
    let mut probe = CmdProbe {
        argv: argv.iter().map(|s| (*s).to_string()).collect(),
        taken: true,
        ..CmdProbe::default()
    };
    match proc::run_with_deadline(argv, deadline, None) {
        Ok(out) => {
            probe.code = out.code;
            probe.timed_out = out.timed_out;
            probe.stdout = redact(&out.stdout_string());
            probe.stderr = redact(&out.stderr_string());
        }
        Err(e) => probe.error = Some(e.to_string()),
    }
    probe.duration_ms = started.elapsed().as_millis() as u64;
    probe
}

impl Probes {
    /// Capture everything the selected groups need. Never fails: a probe that cannot be
    /// taken records why (§7).
    pub fn capture(ctx: &Context, only: Option<DoctorGroup>) -> Probes {
        let mut p = Probes::empty(ctx, only);

        // ---- binaries -------------------------------------------------------
        p.which_monerod = proc::which("monerod").map(|x| x.display().to_string());
        p.which_wallet_rpc = proc::which("monero-wallet-rpc").map(|x| x.display().to_string());
        let vdl = Duration::from_secs(VERSION_PROBE_SEC)
            .min(ctx.timeouts.systemctl.max(Duration::from_secs(1)));
        if p.which_monerod.is_some() {
            p.monerod_version = run(&["monerod", "--version"], vdl);
        }
        if p.which_wallet_rpc.is_some() {
            p.wallet_rpc_version_cmd = run(&["monero-wallet-rpc", "--version"], vdl);
        }

        // ---- units (read first: the node and wallet groups quote them) -------
        p.capture_units(ctx);

        // ---- node -----------------------------------------------------------
        // **Not in remote mode** (§2m.3): there is no local daemon to ask, and
        // `NodeClient::from_ctx` refuses outright — so probing would spend a `detect_layout`
        // and a `data_dir_in_use` to learn nothing. Every `node` check but `node_layout` and
        // `remote_node_untrusted` reports `skip:remote_node` from the mode alone.
        let remote = ctx.settings.node.mode.is_remote();
        if group_selected(only, DoctorGroup::Node) && p.which_monerod.is_some() && !remote {
            p.capture_node(ctx);
        }
        if remote {
            // `node_layout` still needs the layout string, and it costs nothing here.
            p.layout = Layout::Remote.as_str().to_string();
        }

        // ---- wallet ---------------------------------------------------------
        if group_selected(only, DoctorGroup::Wallet) && p.which_wallet_rpc.is_some() {
            p.capture_wallet(ctx);
        }
        // The wallet directory, the login file's mode and the money record are local facts
        // and are captured whether or not the daemon answered.
        p.capture_wallet_local(ctx);

        // ---- security, storage, install -------------------------------------
        p.capture_security(ctx, only);
        p.capture_storage(ctx);
        p.capture_install(ctx);
        p
    }

    pub(crate) fn empty(ctx: &Context, only: Option<DoctorGroup>) -> Probes {
        let groups = super::GROUPS
            .iter()
            .filter(|g| group_selected(only, **g))
            .map(|g| g.name().to_string())
            .collect();
        Probes {
            taken_at: fmonero_core::now_rfc3339(),
            now_unix: fmonero_core::now_unix(),
            groups,
            which_monerod: None,
            monerod_version: CmdProbe::default(),
            which_wallet_rpc: None,
            wallet_rpc_version_cmd: CmdProbe::default(),
            layout: Layout::None.as_str().to_string(),
            layout_error: None,
            node_url: ctx.settings.node.url.clone(),
            node_mode: ctx.settings.node.mode.to_string(),
            // Pure settings — no process, no socket — so they belong to `empty`, which is also
            // what the §7.1 fixture harness builds from. `node_layout` names all three in
            // remote mode, where the group's every other source is gone (§2m.3).
            daemon_address: units::daemon_target(ctx).address,
            daemon_trusted: units::daemon_target(ctx).trusted,
            daemon_ssl: units::daemon_target(ctx).ssl.to_string(),
            node_taken: false,
            node_reachable: false,
            node_error: None,
            node_height: None,
            node_target_height: None,
            node_synchronized: None,
            node_offline: None,
            node_restricted: None,
            node_outgoing_connections: None,
            node_adjusted_time: None,
            node_version: None,
            node_authenticated: None,
            node_nettype: None,
            data_dir: ctx.settings.node.data_dir.display().to_string(),
            data_dir_holder: None,
            data_dir_probe_error: None,
            wallet_url: ctx.settings.wallet.url.clone(),
            wallet_taken: false,
            wallet_reachable: false,
            wallet_error: None,
            wallet_rpc_version: None,
            wallet_authenticated: None,
            wallet_open: false,
            wallet_height: None,
            wallet_primary_address: None,
            wallet_login_file: FileProbe::default(),
            wallet_login_port: ctx.settings.wallet.port,
            last_open_name: None,
            last_open_nettype: None,
            wallet_dir: ctx.settings.wallet.dir.display().to_string(),
            wallet_dir_missing: Vec::new(),
            wallet_dir_bad_modes: Vec::new(),
            pending: None,
            systemd_user_dir: systemd_user_dir(ctx).display().to_string(),
            node_unit_name: ctx.settings.node.unit.clone(),
            wallet_unit_name: ctx.settings.wallet.unit.clone(),
            node_unit: FileProbe::default(),
            wallet_unit: FileProbe::default(),
            node_unit_expected: String::new(),
            wallet_unit_expected: String::new(),
            node_unit_matches: false,
            wallet_unit_matches: false,
            node_unit_state: UnitState::missing(&ctx.settings.node.unit),
            wallet_unit_state: UnitState::missing(&ctx.settings.wallet.unit),
            unit_wallet_dir: None,
            unit_wallet_port: None,
            unit_daemon_address: None,
            unit_daemon_trusted: None,
            unit_daemon_ssl: None,
            monerod_conf: FileProbe::default(),
            monerod_conf_prune: None,
            monerod_conf_has_login: false,
            ports: Vec::new(),
            ports_watched: Vec::new(),
            ports_taken: false,
            perms: Vec::new(),
            free_bytes: None,
            free_bytes_error: None,
            prune_setting: ctx.settings.node.prune,
            local_bin_fmonero: ctx
                .paths
                .local_bin
                .join(fmonero_core::NAME)
                .display()
                .to_string(),
            local_bin_exists: false,
            state_file: FileProbe::default(),
            state_parse_error: None,
            settings_file: FileProbe::default(),
            settings_parse_error: None,
            settings_unknown_keys: Vec::new(),
            settings_problems: Vec::new(),
            cache: None,
            cache_present: false,
            plugin_link: LinkProbe::default(),
            omarchy_plugin_list: CmdProbe::default(),
            menu_file: FileProbe::default(),
            menu_missing_ids: Vec::new(),
            network: format!("{}", ctx.settings.node.network),
            expected_wallet_port: ctx.settings.node.network.wallet_rpc_port(),
            settings_wallet_port: ctx.settings.wallet.port,
            settings_wallet_url_port: url_port(&ctx.settings.wallet.url),
            node_url_port: url_port(&ctx.settings.node.url),
        }
    }

    // ---- node ---------------------------------------------------------------

    pub(crate) fn capture_node(&mut self, ctx: &Context) {
        self.node_taken = true;
        match units::detect_layout(ctx) {
            Ok(l) => self.layout = l.as_str().to_string(),
            Err(e) => {
                self.layout_error = Some(e.to_string());
                self.layout = Layout::None.as_str().to_string();
            }
        }
        match fmonero_core::NodeClient::from_ctx(ctx) {
            Ok(n) => match n.get_info() {
                Ok(info) => {
                    self.node_reachable = true;
                    self.node_height = Some(info.height);
                    self.node_target_height = Some(info.target_height);
                    self.node_synchronized = Some(info.synchronized);
                    self.node_offline = Some(info.offline);
                    self.node_restricted = Some(info.restricted);
                    self.node_adjusted_time = Some(info.adjusted_time as i64);
                    self.node_nettype = Some(info.nettype.clone());
                    // §5: a restricted node poisons these, so they are not facts there.
                    if !info.restricted {
                        self.node_outgoing_connections = Some(info.outgoing_connections_count);
                        self.node_version = Some(info.version.clone());
                    }
                    self.node_authenticated = n.authenticated();
                }
                Err(e) => self.node_error = Some(e.to_string()),
            },
            Err(e) => self.node_error = Some(e.to_string()),
        }
        let data_dir = ctx.settings.node.data_dir.clone();
        match units::data_dir_in_use(ctx, &data_dir) {
            Ok(holder) => self.data_dir_holder = holder,
            Err(e) => self.data_dir_probe_error = Some(e.to_string()),
        }
    }

    // ---- wallet -------------------------------------------------------------

    fn capture_wallet(&mut self, ctx: &Context) {
        self.wallet_taken = true;
        let Ok(w) = fmonero_core::WalletClient::from_ctx(ctx) else {
            self.wallet_error = Some("cannot construct a wallet client".to_string());
            return;
        };
        match w.get_version() {
            Ok((major, minor, _)) => {
                self.wallet_reachable = true;
                self.wallet_rpc_version = Some(format!("{major}.{minor}"));
            }
            Err(e) => self.wallet_error = Some(e.to_string()),
        }
        match w.get_height() {
            Ok(h) => {
                self.wallet_reachable = true;
                self.wallet_open = true;
                self.wallet_height = Some(h);
            }
            // `-13 NOT_OPEN` maps to WalletLocked: reachable, but nothing is open (§5).
            Err(e) if e.exit_code() == 9 => self.wallet_reachable = true,
            Err(e) => {
                if self.wallet_error.is_none() {
                    self.wallet_error = Some(e.to_string());
                }
            }
        }
        self.wallet_authenticated = w.authenticated();
        if self.wallet_open {
            if let Ok(set) = w.get_address(ctx.settings.wallet.account) {
                self.wallet_primary_address = Some(set.primary);
            }
        }
    }

    pub(crate) fn capture_wallet_local(&mut self, ctx: &Context) {
        // Mode only. Never the bytes: its single line is `monero:<base64>`, so there is no
        // key for the redactor to match and the only safe rule is not to read it (§2b.8).
        self.wallet_login_file =
            FileProbe::stat(&ctx.paths.wallet_login_file(ctx.settings.wallet.port));
        if let Ok(Some((name, net))) = state::last_open(ctx) {
            self.last_open_name = Some(name);
            self.last_open_nettype = Some(net.as_str().to_string());
        }
        self.pending = state::pending_send(ctx).ok().flatten();

        // Every ancestor, outermost first, so the fixer emits one CreateDir per level.
        let dir = ctx.settings.wallet.dir.clone();
        let mut chain: Vec<PathBuf> = Vec::new();
        let mut cur = Some(dir.as_path());
        while let Some(d) = cur {
            chain.push(d.to_path_buf());
            cur = d.parent().filter(|p| !p.as_os_str().is_empty());
        }
        chain.reverse();
        for level in chain {
            match std::fs::metadata(&level) {
                Ok(m) if m.is_dir() => {
                    let mode = m.permissions().mode() & 0o7777;
                    // Only fmonero's own levels are held to 0700: `/home`, `$HOME` and
                    // `~/.local/share` belong to the system and to the user.
                    if level.starts_with(&ctx.paths.wallet_dir) && mode != 0o700 {
                        self.wallet_dir_bad_modes
                            .push((level.display().to_string(), mode));
                    }
                }
                Ok(_) => self
                    .wallet_dir_bad_modes
                    .push((level.display().to_string(), 0)),
                Err(_) => self.wallet_dir_missing.push(level.display().to_string()),
            }
        }
    }

    // ---- units --------------------------------------------------------------

    pub(crate) fn capture_units(&mut self, ctx: &Context) {
        let dir = systemd_user_dir(ctx);
        self.systemd_user_dir = dir.display().to_string();
        self.node_unit = FileProbe::read(&dir.join(&ctx.settings.node.unit));
        self.wallet_unit = FileProbe::read(&dir.join(&ctx.settings.wallet.unit));
        self.node_unit_expected = units::render_node_unit(ctx);
        self.wallet_unit_expected = units::render_wallet_unit(ctx);
        // On the real bytes, before redaction — see the field's own note.
        self.node_unit_matches = std::fs::read(dir.join(&ctx.settings.node.unit))
            .is_ok_and(|raw| raw == self.node_unit_expected.as_bytes());
        self.wallet_unit_matches = std::fs::read(dir.join(&ctx.settings.wallet.unit))
            .is_ok_and(|raw| raw == self.wallet_unit_expected.as_bytes());
        self.node_unit_state = units::state(ctx, &ctx.settings.node.unit)
            .unwrap_or_else(|_| self.node_unit_state.clone());
        self.wallet_unit_state = units::state(ctx, &ctx.settings.wallet.unit)
            .unwrap_or_else(|_| self.wallet_unit_state.clone());
        if let Some(text) = self.wallet_unit.text.clone() {
            let argv = exec_start_argv(&text);
            self.unit_wallet_dir = flag_value(&argv, "--wallet-dir");
            self.unit_wallet_port =
                flag_value(&argv, "--rpc-bind-port").and_then(|v| v.parse().ok());
            self.unit_daemon_address = flag_value(&argv, "--daemon-address");
            self.unit_daemon_ssl = flag_value(&argv, "--daemon-ssl");
            // Exactly one of the two flags is always rendered (§4.2). `None` here means the
            // unit carries neither, which is NOT "untrusted": `make_basic` falls back to
            // `is_local_address(daemon_address)` (`wallet2.cpp:451-462`).
            self.unit_daemon_trusted = match (
                argv.iter().any(|a| a == "--trusted-daemon"),
                argv.iter().any(|a| a == "--untrusted-daemon"),
            ) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                _ => None,
            };
        }
    }

    // ---- security -----------------------------------------------------------

    pub(crate) fn capture_security(&mut self, ctx: &Context, only: Option<DoctorGroup>) {
        // Mode only for the conf's FileProbe; the two facts below are DERIVED and the
        // bytes are dropped immediately, so no `rpc-login=` value can reach an artifact
        // (§2b.8, §2c).
        let conf = ctx.paths.monerod_conf();
        self.monerod_conf = FileProbe::stat(&conf);
        if self.monerod_conf.exists {
            if let Ok(text) = std::fs::read_to_string(&conf) {
                self.monerod_conf_has_login = text
                    .lines()
                    .any(|l| l.trim_start().starts_with("rpc-login="));
                self.monerod_conf_prune = Some(text.lines().any(|l| {
                    l.trim() == "prune-blockchain=1" || l.trim() == "prune-blockchain=true"
                }));
            }
        }

        if group_selected(only, DoctorGroup::Security) {
            let (watched, ports) = self.scan_ports(ctx);
            self.ports_watched = watched;
            self.ports = ports;
            self.ports_taken = true;
        }
        self.perms = scan_perms(ctx);
    }

    /// Who listens on the three ports §7.1 names, read **through `paths.proc_root`** so the
    /// fixture drives it: `<proc_root>/net/tcp` gives (port → inode) and `<proc_root>/*/fd`
    /// gives (inode → pid). Never `ss` or `lsof`, neither of which is guaranteed installed
    /// (the same reason §2i rejects them).
    fn scan_ports(&self, ctx: &Context) -> (Vec<u16>, Vec<PortProbe>) {
        let mut wanted: Vec<u16> = vec![
            ctx.settings.node.network.node_p2p_port(),
            ctx.settings.wallet.port,
        ];
        if let Some(p) = self.node_url_port {
            wanted.push(p);
        } else {
            wanted.push(ctx.settings.node.network.node_rpc_port());
        }
        wanted.sort_unstable();
        wanted.dedup();

        let mut listeners: Vec<(u16, String, bool, u64)> = Vec::new();
        for name in ["net/tcp", "net/tcp6"] {
            let Ok(text) = std::fs::read_to_string(ctx.paths.proc_root.join(name)) else {
                continue;
            };
            for line in text.lines().skip(1) {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() < 10 {
                    continue;
                }
                // `st = 0A` is TCP_LISTEN; anything else is not a bound port.
                if f[3] != "0A" {
                    continue;
                }
                let Some((addr_hex, port_hex)) = f[1].split_once(':') else {
                    continue;
                };
                // The port is HEX. Reading it as decimal finds nothing and the check
                // reports `pass` on every machine — the vacuous gate §2j forbids.
                let Ok(port) = u16::from_str_radix(port_hex, 16) else {
                    continue;
                };
                if !wanted.contains(&port) {
                    continue;
                }
                let Ok(inode) = f[9].parse::<u64>() else {
                    continue;
                };
                let (addr, loopback) = decode_addr(addr_hex);
                listeners.push((port, addr, loopback, inode));
            }
        }
        if listeners.is_empty() {
            return (wanted, Vec::new());
        }

        let ours: Vec<i32> = [&self.node_unit_name, &self.wallet_unit_name]
            .iter()
            .filter_map(|u| units::main_pid(ctx, u).ok().flatten())
            .collect();
        let owners = socket_owners(&ctx.paths.proc_root);
        let probes = listeners
            .into_iter()
            .map(|(port, address, loopback, inode)| {
                let pid = owners.get(&inode).map(|(pid, _)| *pid);
                let comm = owners.get(&inode).map(|(_, c)| c.clone());
                PortProbe {
                    port,
                    address,
                    loopback,
                    inode,
                    pid,
                    comm,
                    ours: pid.is_some_and(|p| ours.contains(&p)),
                }
            })
            .collect();
        (wanted, probes)
    }

    // ---- storage ------------------------------------------------------------

    pub(crate) fn capture_storage(&mut self, ctx: &Context) {
        // Measured locally, never from `get_info.free_space`, which a restricted node
        // reports as 2^64-1 (§1b.3, §7.1).
        match fmonero_core::fsinfo::free_bytes(&ctx.settings.node.data_dir) {
            Ok(b) => self.free_bytes = Some(b),
            Err(e) => self.free_bytes_error = Some(e.to_string()),
        }
    }

    // ---- install ------------------------------------------------------------

    pub(crate) fn capture_install(&mut self, ctx: &Context) {
        self.local_bin_exists = ctx.paths.local_bin.join(fmonero_core::NAME).exists();

        self.state_file = FileProbe::read(&ctx.paths.state_file());
        if let Some(text) = &self.state_file.text {
            if let Err(e) = serde_json::from_str::<fmonero_core::StateDoc>(text) {
                self.state_parse_error = Some(e.to_string());
            }
        }

        self.settings_file = FileProbe::read(&ctx.paths.config_file());
        if let Some(text) = &self.settings_file.text {
            let (err, unknown, problems) = inspect_settings(text);
            self.settings_parse_error = err;
            self.settings_unknown_keys = unknown;
            self.settings_problems = problems;
        }

        self.cache_present = ctx.paths.history_cache().exists();
        if let Ok(Some(h)) = history::header(ctx) {
            let fp = self
                .wallet_primary_address
                .as_deref()
                .map(|a| fingerprint(a) == h.wallet_fp);
            self.cache = Some(CacheFacts {
                magic_ok: h.magic_ok,
                format_version: h.format_version,
                expected_format_version: history::CACHE_FORMAT_VERSION,
                wallet_fp: hex(&h.wallet_fp),
                high_water: h.high_water,
                built_at: h.built_at,
                fingerprint_matches: fp,
            });
        }

        self.plugin_link = capture_link(ctx);
        if proc::which("omarchy").is_some() {
            self.omarchy_plugin_list = run(
                &["omarchy", "plugin", "list"],
                ctx.timeouts.systemctl.max(Duration::from_secs(1)),
            );
        }
        self.menu_file = FileProbe::read(&ctx.paths.omarchy_menu());
        let present = self.menu_file.text.clone().unwrap_or_default();
        self.menu_missing_ids = menu_template_ids()
            .into_iter()
            .filter(|id| !present.contains(&format!("\"{id}\"")))
            .collect();
    }
}

// ------------------------------------------------------------------ helpers

/// Where the two units live — **`ctx.paths.systemd_user_dir`, and nothing else**.
///
/// It used to read `FMONERO_SYSTEMD_USER_DIR` here, a Phase 2 deviation recorded in
/// `decisions/phase2-doctor.md`: §2i froze `FMONERO_PROC_ROOT` and
/// `FMONERO_SYSTEMCTL_FIXTURE`, neither of which points anything at a directory of unit
/// **files**, and this phase did not own `Paths`. v5.1 moves the seam into
/// `Paths::discover`, under the same `FMONERO_STATE_DIR` guard, so the doctor and every
/// other reader of that directory — `units::detect_layout`, `units::state`,
/// `units::preflight`, `units::install` — agree about which directory a test is looking at.
/// This function stays because six §7.1 checks name it (`units_present`, `unit_loopback`,
/// `unit_no_password_in_argv`, `unit_no_node_wants`, `wallet_dir_matches_unit`,
/// `wallet_port_consistent`) and one place to read it from is still worth having.
pub fn systemd_user_dir(ctx: &Context) -> PathBuf {
    ctx.paths.systemd_user_dir.clone()
}

/// The `ExecStart=` line's argv, with systemd's trailing `\` continuations joined.
pub fn exec_start_argv(unit: &str) -> Vec<String> {
    let mut joined = String::new();
    let mut in_exec = false;
    for line in unit.lines() {
        let t = line.trim();
        if !in_exec {
            let Some(rest) = t.strip_prefix("ExecStart=") else {
                continue;
            };
            in_exec = true;
            joined.push_str(rest.trim_end_matches('\\').trim());
            if !t.ends_with('\\') {
                break;
            }
            joined.push(' ');
            continue;
        }
        joined.push_str(t.trim_end_matches('\\').trim());
        if !t.ends_with('\\') {
            break;
        }
        joined.push(' ');
    }
    joined.split_whitespace().map(str::to_string).collect()
}

/// `--flag VALUE` or `--flag=VALUE` out of an argv.
pub fn flag_value(argv: &[String], flag: &str) -> Option<String> {
    if let Some(i) = argv.iter().position(|a| a == flag) {
        return argv.get(i + 1).cloned();
    }
    argv.iter()
        .find_map(|a| a.strip_prefix(&format!("{flag}=")).map(str::to_string))
}

/// The port of a `http://host:port` URL.
pub fn url_port(url: &str) -> Option<u16> {
    let rest = url.trim_start_matches("http://").trim_end_matches('/');
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    hostport.rsplit_once(':').and_then(|(_, p)| p.parse().ok())
}

/// `/proc/net/tcp`'s `local_address` half: hex in **`inet_addr` byte order**, so
/// `127.0.0.1` prints as `0100007F`. A parser that reads it big-endian sees `1.0.0.127`
/// and the loopback test silently inverts.
fn decode_addr(hex: &str) -> (String, bool) {
    if hex.len() == 8 {
        let Ok(raw) = u32::from_str_radix(hex, 16) else {
            return (hex.to_string(), false);
        };
        let b = raw.to_le_bytes();
        let addr = std::net::Ipv4Addr::new(b[0], b[1], b[2], b[3]);
        let loopback = addr.is_loopback();
        return (addr.to_string(), loopback);
    }
    if hex.len() == 32 {
        // IPv6, four little-endian 32-bit words. `::1` is the only one we care about.
        let mut bytes = [0u8; 16];
        for (i, chunk) in hex.as_bytes().chunks(8).enumerate() {
            let Ok(word) = u32::from_str_radix(std::str::from_utf8(chunk).unwrap_or("0"), 16)
            else {
                return (hex.to_string(), false);
            };
            bytes[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        let addr = std::net::Ipv6Addr::from(bytes);
        let loopback = addr.is_loopback();
        return (addr.to_string(), loopback);
    }
    (hex.to_string(), false)
}

/// `(socket inode) -> (pid, argv[0] basename)` from `<proc_root>/*/fd/*`.
///
/// The fd entries are symlinks to `socket:[<inode>]`; the fixture's are **dangling on
/// purpose** — git stores the target, which is the only thing this reads.
fn socket_owners(proc_root: &Path) -> std::collections::HashMap<u64, (i32, String)> {
    let mut map = std::collections::HashMap::new();
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return map;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        let comm = std::fs::read(entry.path().join("cmdline"))
            .ok()
            .and_then(|raw| {
                raw.split(|b| *b == 0)
                    .find(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).into_owned())
            })
            .map(|exe| {
                Path::new(&exe)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or(exe)
            })
            .unwrap_or_default();
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let t = target.to_string_lossy();
            let Some(inode) = t
                .strip_prefix("socket:[")
                .and_then(|r| r.strip_suffix(']'))
                .and_then(|n| n.parse::<u64>().ok())
            else {
                continue;
            };
            map.insert(inode, (pid, comm.clone()));
        }
    }
    map
}

/// Every file and directory **fmonero itself writes**, with the mode §2b.7 requires.
///
/// The run directory's two daemon-owned names are excluded from the literal rule and
/// checked for owner-only instead: `monero-wallet-rpc.*.login` settles at 0400 through
/// `private_file`'s own dance (§2c) and `monero-wallet-rpc.log` is whatever the daemon
/// chose, so "every file is 0600" would report `fail` on a healthy machine (§7.1).
fn scan_perms(ctx: &Context) -> Vec<PermProbe> {
    let mut out = Vec::new();
    for root in [
        &ctx.paths.state_dir,
        &ctx.paths.config_dir,
        &ctx.paths.cache_dir,
    ] {
        walk_perms(root, &mut out, 0);
    }
    out
}

fn walk_perms(path: &Path, out: &mut Vec<PermProbe>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        // `doctor/latest` is a symlink; its mode is not a property of anything.
        return;
    }
    let mode = meta.permissions().mode() & 0o7777;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let daemon_owned = name.starts_with("monero-wallet-rpc.")
        && (name.ends_with(".login") || name.ends_with(".log"));
    out.push(PermProbe {
        path: path.display().to_string(),
        is_dir: meta.is_dir(),
        mode,
        want: if meta.is_dir() { 0o700 } else { 0o600 },
        daemon_owned,
    });
    if !meta.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        walk_perms(&entry.path(), out, depth + 1);
    }
}

fn capture_link(ctx: &Context) -> LinkProbe {
    let link = ctx.paths.plugin_link();
    let mut p = LinkProbe {
        path: link.display().to_string(),
        taken: true,
        want_target: super::repo_plugin_dir().map(|d| d.display().to_string()),
        ..LinkProbe::default()
    };
    if let Ok(meta) = std::fs::symlink_metadata(&link) {
        p.exists = true;
        p.is_symlink = meta.file_type().is_symlink();
    }
    p.target = std::fs::read_link(&link)
        .ok()
        .map(|t| t.display().to_string());
    p.resolved = std::fs::canonicalize(&link)
        .ok()
        .map(|t| t.display().to_string());
    p
}

/// `config.toml`'s three failure kinds, as data: a parse error, unknown keys, and known
/// keys whose **value** is invalid.
///
/// Parsed leniently on purpose. `Settings` derives `deny_unknown_fields`, so a strict
/// deserialize of a document with one stray key answers "does not parse" and hides the four
/// real defects behind it — and the repaired document §7.1's fixer writes has to keep every
/// key that was fine.
pub fn inspect_settings(text: &str) -> (Option<String>, Vec<String>, Vec<(String, String)>) {
    let value: toml::Value = match toml::from_str(text) {
        Ok(v) => v,
        Err(e) => return (Some(e.to_string()), Vec::new(), Vec::new()),
    };
    let mut unknown = Vec::new();
    let mut problems = Vec::new();
    let mut probe = Settings::default();
    let known: Vec<&str> = Settings::KEYS.iter().map(|(k, _, _)| *k).collect();
    if let Some(table) = value.as_table() {
        for (section, body) in table {
            let Some(inner) = body.as_table() else {
                unknown.push(section.clone());
                continue;
            };
            for (leaf, v) in inner {
                let key = format!("{section}.{leaf}");
                if !known.contains(&key.as_str()) {
                    unknown.push(key);
                    continue;
                }
                let text = match v {
                    toml::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if let Err(e) = probe.set(&key, &text) {
                    problems.push((key, e.to_string()));
                }
            }
        }
    }
    unknown.sort();
    problems.sort();
    (None, unknown, problems)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The first 16 bytes of `sha256(primary_address)` — §5b's history-cache fingerprint. The
/// address is public (§2b) and the fingerprint only has to notice that this is a
/// **different wallet**. Recomputed here rather than imported because `history`'s own copy
/// is private and `crates/fmonero-core` is frozen (§2k, §5b).
fn fingerprint(primary_address: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(primary_address.as_bytes());
    let mut fp = [0u8; 16];
    fp.copy_from_slice(&digest[..16]);
    fp
}

/// The ids `setup --menu` merges, read from the asset itself so the two cannot drift —
/// `crate::setup::MENU_TEMPLATE` is the same `include_str!` the merge uses. Each
/// non-comment, non-brace line is one entry and its key is the id (§4.2).
pub fn menu_template_ids() -> Vec<String> {
    crate::setup::MENU_TEMPLATE
        .lines()
        .filter_map(|line| {
            let t = line.trim();
            if t.is_empty() || t.starts_with("//") || t == "{" || t == "}" {
                return None;
            }
            Some(t.strip_prefix('"')?.split('"').next()?.to_string())
        })
        .collect()
}

/// The command probes worth writing into `runs/<id>/probes/` (§7).
pub fn named_commands(p: &Probes) -> Vec<(&'static str, &CmdProbe)> {
    vec![
        ("monerod-version", &p.monerod_version),
        ("monero-wallet-rpc-version", &p.wallet_rpc_version_cmd),
        ("omarchy-plugin-list", &p.omarchy_plugin_list),
    ]
}

/// One probe artifact's text: the argv, the outcome and the (already redacted) output.
pub fn artifact_text(p: &CmdProbe) -> String {
    if !p.taken {
        return "not taken: this probe's group was not selected\n".to_string();
    }
    let mut s = format!("$ {}\n", p.argv.join(" "));
    s.push_str(&format!(
        "exit {:?} · {} ms{}\n",
        p.code,
        p.duration_ms,
        if p.timed_out { " · TIMED OUT" } else { "" }
    ));
    if let Some(e) = &p.error {
        s.push_str(&format!("error: {e}\n"));
    }
    s.push_str("--- stdout\n");
    s.push_str(&p.stdout);
    s.push_str("\n--- stderr\n");
    s.push_str(&p.stderr);
    s.push('\n');
    truncate(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redactor_fires_on_every_documented_key_and_keeps_the_rest() {
        let text = "data-dir=/x\nrpc-login=fmonero:deadbeef\n  --password-file /y\nout-peers=16\n";
        let got = redact(text);
        assert!(!got.contains("deadbeef"), "{got}");
        assert!(got.contains("data-dir=/x"), "{got}");
        assert!(got.contains("out-peers=16"), "{got}");
        assert!(got.contains("<redacted>"), "{got}");
    }

    #[test]
    fn a_proc_net_tcp_address_is_decoded_in_inet_addr_order() {
        // 127.0.0.1 is 0100007F, not 7F000001: a big-endian reading inverts the test.
        assert_eq!(decode_addr("0100007F"), ("127.0.0.1".to_string(), true));
        assert_eq!(decode_addr("00000000"), ("0.0.0.0".to_string(), false));
    }

    #[test]
    fn the_exec_start_argv_joins_systemds_line_continuations() {
        let unit = "[Service]\nExecStart=/usr/bin/monero-wallet-rpc \\\n  --rpc-bind-port 18083 \\\n  --wallet-dir /w \\\n  --log-level 0\nRestart=on-failure\n";
        let argv = exec_start_argv(unit);
        assert_eq!(
            flag_value(&argv, "--rpc-bind-port").as_deref(),
            Some("18083")
        );
        assert_eq!(flag_value(&argv, "--wallet-dir").as_deref(), Some("/w"));
        assert_eq!(flag_value(&argv, "--log-level").as_deref(), Some("0"));
        assert!(!argv.contains(&"Restart=on-failure".to_string()));
    }

    #[test]
    fn a_url_port_is_read_from_the_authority_only() {
        assert_eq!(url_port("http://127.0.0.1:18081"), Some(18081));
        assert_eq!(url_port("http://127.0.0.1:38083/json_rpc"), Some(38083));
        assert_eq!(url_port("http://127.0.0.1"), None);
    }
}
