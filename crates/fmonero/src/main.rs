//! `fmonero` — a Monero node + wallet front end for Omarchy: CLI, doctor, FrankenTUI and the
//! backend of the `oliver.monero` bar widget.
//!
//! This file owns the whole command surface of DESIGN.md §4.1, the dispatch, the §5c payloads
//! and the §4a exit-code mapping. **It was frozen at the Phase 1a gate**: later phases fill in
//! `fmonero-core`, `output.rs`'s rendering, `setup::run`, `doctor::run` and `tui::run` without
//! touching it (§2k). That is why every payload below is built here, from the frozen §5b API,
//! even where the body it calls is still `Error::unimplemented`.

mod doctor;
mod output;
mod prompt;
mod setup;
mod tui;

use std::time::Duration;

use clap::{Parser, Subcommand};

use fmonero_core::history::{self, HistoryQuery, TransferKind};
use fmonero_core::rpc::node::BanRequest;
use fmonero_core::rpc::{NodeClient, WalletClient};
use fmonero_core::send::{self, Plan, SendRequest, SweepRequest};
use fmonero_core::state;
use fmonero_core::status::PendingStatus;
use fmonero_core::{
    address, agent, log as corelog, node_mode, node_session, nodes, settings as coresettings,
    units, wallet_dir, wallet_session, CancelToken, Context, Error, Piconero,
};

use output::{Out, Paint};

const ABOUT: &str = "A fast Monero node + wallet front end for Omarchy.";

const LONG_ABOUT: &str = "A fast Monero node + wallet front end for Omarchy.

Bare `fmonero` opens the TUI on a terminal; without a TTY it prints `status --json`.
Every command accepts --json, and a non-TTY stdout implies it. Agents: start with
`fmonero robot-docs` and `fmonero capabilities`, and branch on code_name + reason.

`status` starts nothing and changes nothing, so it answers even while the node is syncing
or the wallet service is busy.

Money safety: `wallet send` and `wallet sweep` prepare the payment without sending it,
print the real fee, and send only with --yes.";

const AFTER_HELP: &str =
    "Exit codes: 0 ok · 1 failure · 2 usage · 3 safety (needs --yes) · 4 backend ·
5 upstream (a background program errored or was killed at its deadline) · 6 not found ·
7 ambiguous ·
8 timeout · 9 wallet locked · 10 node syncing · 11/12/13 doctor · 14 insufficient funds ·
15 invalid address · 16 tx rejected · 130 interrupted.

Amounts: a decimal XMR string (0.5, 1.234567891234, at most 12 decimals) or raw piconero
with a `p` suffix (250000000000p). Recipients: an address, or @label from the address book.

--dry-run is implemented by exactly six commands: node remote, node local, wallet send,
wallet sweep, setup, doctor.";

#[derive(Parser)]
#[command(
    name = "fmonero",
    version,
    about = ABOUT,
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP,
    disable_help_subcommand = true
)]
struct Cli {
    // The seven global flags carry a display_order above any command's own arguments, so that
    // `fmonero wallet send --help` reads as "what this command does" and then "what every
    // command does".
    /// Machine-readable JSON on stdout (errors too, with the same exit code)
    #[arg(long, global = true, display_order = 100)]
    json: bool,
    /// Lower every deadline to N seconds (never raises one; 0 is refused)
    #[arg(long, global = true, value_name = "SECONDS", display_order = 101)]
    timeout: Option<u64>,
    /// Diagnostics on stderr; repeat for more
    #[arg(short = 'v', long, global = true, action = clap::ArgAction::Count, display_order = 102)]
    verbose: u8,
    /// Never emit colour (also honoured: NO_COLOR, CI, a non-TTY stdout)
    #[arg(long, global = true, display_order = 103)]
    no_color: bool,
    /// Confirm an irreversible action: send, sweep, create, clear-pending, set-daemon, node down/restart, setup
    #[arg(long, global = true, display_order = 104)]
    yes: bool,
    /// Build and print, change nothing (wallet send, wallet sweep, setup, doctor only)
    #[arg(long, global = true, display_order = 105)]
    dry_run: bool,
    /// Ignore an armed 60 s stall breaker for this invocation (nothing else)
    #[arg(long, global = true, display_order = 106)]
    force: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// The whole Status document (schema 1). Starts nothing and changes nothing
    #[command(
        after_help = "Three questions to the wallet and the node with no wallet open, five
with one, plus two checks on the background services. A node that is not answering is
`reachable: false`, not an error: `status` exits 0 with warnings.

--check-node prints exactly the same document and, when a public node is configured and the
last check of it is older than node.remote_probe_sec, starts one in the background and returns
at once. This call is not made fresher by it; the next one is."
    )]
    Status {
        /// Start a background check of the configured public node when the last one is stale
        #[arg(long)]
        check_node: bool,
    },
    /// The local monerod: start, stop, inspect, peers, bans, limits
    Node {
        #[command(subcommand)]
        cmd: NodeCmd,
    },
    /// The wallet, through monero-wallet-rpc
    Wallet {
        #[command(subcommand)]
        cmd: WalletCmd,
    },
    /// fmonero's own settings (~/.config/fmonero/config.toml)
    Settings {
        #[command(subcommand)]
        cmd: SettingsCmd,
    },
    /// Write the systemd user units, link the bar widget, merge the menu entries
    Setup(setup::SetupArgs),
    /// Diagnose the installation, and optionally repair it
    Doctor(doctor::DoctorArgs),
    /// Terminal UI: Node · Wallet · Send · Log
    Tui(tui::TuiArgs),
    /// The machine contract: exit codes, reasons, settings keys, warning ids
    Capabilities,
    /// Agent handbook, plain text
    #[command(alias = "docs")]
    RobotDocs {
        /// Only `guide` exists today
        section: Option<String>,
    },
    /// Version, build stage and what this build has actually been seen to do
    Version,
}

#[derive(Subcommand)]
enum NodeCmd {
    /// Start the node's systemd user unit and wait for it to answer
    #[command(
        after_help = "Refuses (exit 3) when another program is already using the chain folder,
and when another program on this machine runs the node: two of them on one chain corrupt it.
Without --wait a still-syncing node is exit 0 plus the node_syncing warning."
    )]
    Up {
        /// Also wait until the node reports itself synchronized
        #[arg(long)]
        wait: bool,
        /// Bound for --wait; 0 means "until interrupted" and is refused without a TTY
        #[arg(long, value_name = "SECONDS", default_value_t = 1800)]
        wait_timeout: u64,
    },
    /// Stop the node: stop_daemon over RPC first, so LMDB closes cleanly, then systemctl stop
    Down,
    /// Stop, then start
    Restart,
    /// Everything the node knows about itself: sync, fork, fees, txpool, limits
    Info,
    /// Tail the node's log file
    Log {
        /// How many lines from the end
        #[arg(long, value_name = "N", default_value_t = 200)]
        lines: usize,
        /// Follow — only from the TUI's Log screen; refused here, where a pipe would hang
        #[arg(long)]
        follow: bool,
        /// Keep only lines containing this text
        #[arg(long, value_name = "TEXT")]
        grep: Option<String>,
    },
    /// Live connections and the node's peer lists
    Peers {
        /// Only the white list
        #[arg(long, conflicts_with = "gray")]
        white: bool,
        /// Only the gray list
        #[arg(long)]
        gray: bool,
        /// Keep only the first N rows of each list
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },
    /// The node's ban list
    Bans,
    /// Ban a host
    Ban {
        /// A host or IP, e.g. 1.2.3.4
        host: String,
        /// How long, in seconds
        #[arg(long, value_name = "N", default_value_t = 86400)]
        seconds: u64,
    },
    /// Unban a host
    Unban {
        /// A host or IP, e.g. 1.2.3.4
        host: String,
    },
    /// Point monero-wallet-rpc at a REMOTE node and keep it there (§2m)
    #[command(
        after_help = "Writes node.mode, node.remote and node.remote_trusted, then sets up both
background services again and restarts the wallet service if it was running. fmonero itself
never talks to the public node; the wallet service does.

A public node sees this machine's IP address and when you ask it anything, supplies the
decoys every payment is made from, and can lie about the chain tip and the fee. So never pass
--trusted to one, and bound a send with --max-fee. The encryption here is opportunistic and
nothing checks the certificate, and Tor is not in v0.1.

  fmonero node remote node.example.com:18089 --yes      # a public node
  fmonero node local --yes                              # the way back"
    )]
    Remote {
        /// [http[s]://]host:port — the port is required, no path, no credential
        addr: String,
        /// Send `trusted` — only ever for a node YOU run (§2m.6)
        #[arg(long)]
        trusted: bool,
    },
    /// Go back to fmonero's own local monerod (§2m)
    Local,
    /// Public nodes from monero.fail, cached — and, with --probe, measured from here (§2n)
    #[command(
        after_help = "Uses the saved list and fetches a new one only when --refresh is
given or the saved list is absent or older than 24 h. --probe then checks the top --limit
candidates from here, 8 at a time with a 5 s deadline each.

Getting the list tells monero.fail this machine's IP address. Checking a node tells that node
too. Nothing is contacted automatically except the node already configured, and that only
while the TUI's Node screen is showing. monero.fail's figures are its own measurements. They
are not a recommendation, and not this machine's.

Needs curl, as a child process: fmonero's own HTTP client only ever talks to this machine.

  fmonero node list                       # the cache, instantly
  fmonero node list --refresh --probe     # refetch, then measure the top 20 from here
  fmonero node remote <host:port> --yes   # use one"
    )]
    List {
        /// Refetch nodes.json and health.json from monero.fail now
        #[arg(long)]
        refresh: bool,
        /// Ask each listed node for get_info from THIS machine (each learns your IP)
        #[arg(long)]
        probe: bool,
        /// How many rows to print, and to probe
        #[arg(long, value_name = "N", default_value_t = fmonero_core::nodes::DEFAULT_LIMIT)]
        limit: usize,
        /// Every usable candidate, not just --limit of them
        #[arg(long, conflicts_with = "limit")]
        all: bool,
    },
    /// Read or set the node's rate limits and peer counts
    #[command(
        after_help = "With no arguments this only reads, and changes nothing. All limits are
kB/s, and -1 puts a limit back to the node's own default."
    )]
    Limit {
        /// Upload limit in kB/s (-1 = default)
        #[arg(long, value_name = "KBPS", allow_negative_numbers = true)]
        up: Option<i64>,
        /// Download limit in kB/s (-1 = default)
        #[arg(long, value_name = "KBPS", allow_negative_numbers = true)]
        down: Option<i64>,
        /// Maximum outgoing connections
        #[arg(long, value_name = "N")]
        out_peers: Option<u32>,
        /// Maximum incoming connections
        #[arg(long, value_name = "N")]
        in_peers: Option<u32>,
    },
}

#[derive(Subcommand)]
enum WalletCmd {
    /// The wallets in wallet.dir (*.keys basenames). Reads no wallet bytes
    List,
    /// Open a wallet: start the unit if needed, open it, install the node credential, auto-refresh
    #[command(
        after_help = "The password comes from --password-file, then $FMONERO_WALLET_PASSWORD,
then a prompt in the terminal with echo off. It is never passed on a command line, fmonero's
or the wallet service's."
    )]
    Open {
        /// The wallet's name inside wallet.dir (no `/`)
        name: String,
        /// Read the password from this file (must have no group or other permission bit)
        #[arg(long, value_name = "PATH")]
        password_file: Option<std::path::PathBuf>,
    },
    /// Close the open wallet
    Close,
    /// Create a new wallet. Does NOT print the seed
    #[command(
        after_help = "In a terminal the password is asked for twice and compared. An empty one
is refused without --allow-empty-password. The seed is never printed: back it up with
`monero-wallet-cli --wallet-file <wallet.dir>/<name> --offline`, then type `seed`."
    )]
    Create {
        /// The wallet's name inside wallet.dir (no `/`)
        name: String,
        /// Seed language; `fmonero wallet create --help` lists what the daemon accepts
        #[arg(long, value_name = "L", default_value = coresettings::DEFAULT_SEED_LANGUAGE)]
        language: String,
        /// Create an UNENCRYPTED wallet on purpose
        #[arg(long)]
        allow_empty_password: bool,
        /// Read the password from this file instead of prompting
        #[arg(long, value_name = "PATH")]
        password_file: Option<std::path::PathBuf>,
    },
    /// Balance and unlocked balance
    Balance {
        /// Account index
        #[arg(long, value_name = "N")]
        account: Option<u32>,
        /// Also list the funded subaddresses
        #[arg(long)]
        subaddresses: bool,
    },
    /// The account's primary address and its subaddresses
    Address {
        /// Account index
        #[arg(long, value_name = "N")]
        account: Option<u32>,
        /// Select one subaddress by index
        #[arg(long, value_name = "N")]
        index: Option<u32>,
        /// Also draw the selected address as a QR (skipped when stdout is not a TTY)
        #[arg(long)]
        qr: bool,
    },
    /// Generate a new subaddress
    NewAddress {
        /// Account index
        #[arg(long, value_name = "N")]
        account: Option<u32>,
        /// A label for the new subaddress
        #[arg(long, value_name = "TEXT")]
        label: Option<String>,
    },
    /// Transfer history, from a local snapshot plus one incremental fetch
    #[command(
        after_help = "--in selects `in` AND `block` (coinbase) rows: they arrive in the same
list. --cached asks the wallet service nothing at all, and then `confirmations` and `locked`
are null. Asking nothing and a live confirmation count cannot both be had."
    )]
    History {
        /// Account index
        #[arg(long, value_name = "N")]
        account: Option<u32>,
        /// Keep only the newest N rows
        #[arg(long, value_name = "N", default_value_t = 20)]
        limit: usize,
        /// Incoming rows, coinbase included
        #[arg(long = "in")]
        r#in: bool,
        /// Outgoing rows
        #[arg(long)]
        out: bool,
        /// Not yet confirmed, built by this wallet
        #[arg(long)]
        pending: bool,
        /// Built but never accepted
        #[arg(long)]
        failed: bool,
        /// In the node's txpool (this one needs the node)
        #[arg(long)]
        pool: bool,
        /// Only rows at or above this height (sent as an explicit height filter)
        #[arg(long, value_name = "HEIGHT")]
        since: Option<u64>,
        /// Serve the snapshot alone: no RPC, and confirmations/locked are null
        #[arg(long)]
        cached: bool,
    },
    /// Send an exact amount to one recipient
    #[command(
        after_help = "Prepares the payment without sending it and prints the real fee first.
Without --yes it prints that fee and exits 3. --dry-run beats --yes: it never sends. --max-fee
refuses to send when the prepared fee exceeds it. v0.1 prepares exactly ONE transaction; when
the wallet needs more, the hint names `wallet sweep`.

Tested against fmonero-mock only: no send has ever gone out from this version."
    )]
    Send {
        /// A Monero address, or @label from the address book
        to: String,
        /// XMR (0.01) or raw piconero (10000000000p)
        amount: String,
        /// Account index
        #[arg(long, value_name = "N")]
        account: Option<u32>,
        /// Fee level 0-4: 0 and 1 cheapest · 2 normal · 3 fast · 4 fastest
        #[arg(long, value_name = "0-4")]
        priority: Option<u32>,
        /// A note stored with the payment once it has gone out
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        /// Refuse the relay when the built total fee exceeds this
        #[arg(long, value_name = "AMOUNT")]
        max_fee: Option<String>,
    },
    /// Sweep an account to one recipient
    #[command(
        after_help = "Sweeps the whole account (bounded by --below) and may need several
payments. Each is sent in order under one crash-safe record. There is no amount argument and
no way to pick one address: every address of the account is included.

Tested against fmonero-mock only: no sweep has ever gone out from this version."
    )]
    Sweep {
        /// A Monero address, or @label from the address book
        to: String,
        /// Account index
        #[arg(long, value_name = "N")]
        account: Option<u32>,
        /// Fee level 0-4: 0 and 1 cheapest · 2 normal · 3 fast · 4 fastest
        #[arg(long, value_name = "0-4")]
        priority: Option<u32>,
        /// Only sweep outputs below this amount
        #[arg(long, value_name = "AMOUNT")]
        below: Option<String>,
        /// Refuse the relay when the built total fee exceeds this
        #[arg(long, value_name = "AMOUNT")]
        max_fee: Option<String>,
    },
    /// Clear the pending-send record after checking whether the transaction landed
    #[command(
        after_help = "The manual escape hatch after a crash. A normal successful send clears
the record itself, so if you find yourself typing this after every send, something is wrong."
    )]
    ClearPending,
    /// Check an address: structurally, then against the wallet daemon
    Validate {
        /// The address to check
        address: String,
    },
    /// The wallet's address book
    Book {
        #[command(subcommand)]
        cmd: BookCmd,
    },
    /// Scan for new blocks now
    Refresh {
        /// Start scanning from this height
        #[arg(long, value_name = "H")]
        start_height: Option<u64>,
    },
    /// Read or write the note stored with a transaction
    Notes {
        #[command(subcommand)]
        cmd: NotesCmd,
    },
    /// Point the open wallet at another daemon
    #[command(
        after_help = "This is the only way a public node enters the picture: the wallet service
reaches it, not fmonero, whose own HTTP client only ever speaks to this machine."
    )]
    SetDaemon {
        /// host:port
        address: String,
        /// Treat it as trusted (it may see which outputs are yours)
        #[arg(long)]
        trusted: bool,
    },
}

#[derive(Subcommand)]
enum BookCmd {
    /// Every entry
    List,
    /// Add an entry
    Add {
        /// A Monero address
        address: String,
        /// The description `@label` matches against, case-insensitively
        description: String,
    },
    /// Remove an entry by index
    Remove {
        /// The index `wallet book list` printed
        index: u64,
    },
}

#[derive(Subcommand)]
enum NotesCmd {
    /// Read the note of one transaction
    Get {
        /// A 64-character transaction id
        txid: String,
    },
    /// Write the note of one transaction
    Set {
        /// A 64-character transaction id
        txid: String,
        /// The note
        text: String,
    },
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// Every key, with its value, default and one-line doc
    List,
    /// One key
    Get {
        /// A dotted key, e.g. wallet.priority
        key: String,
    },
    /// Change one key
    Set {
        /// A dotted key, e.g. wallet.priority
        key: String,
        /// The new value
        value: String,
    },
}

// ---------------------------------------------------------------- clap plumbing

/// Turn clap's own rendering into one line: keep `error:` and `tip:`, drop the ANSI, the
/// `Usage:` block and the "For more information" footer (the hint replaces it).
fn clap_message(e: &clap::Error) -> String {
    let rendered = e.render().to_string();
    let mut plain = String::with_capacity(rendered.len());
    let mut chars = rendered.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for line in plain.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Usage:") || line.starts_with("For more information")
        {
            continue;
        }
        match parts.last_mut() {
            // clap wraps a list (`the following required arguments were not provided:` then
            // one indented item per line) — fold it back up.
            Some(last) if last.ends_with(':') => {
                last.push(' ');
                last.push_str(line);
            }
            _ => parts.push(line.trim_start_matches("error: ").to_string()),
        }
    }
    if parts.is_empty() {
        "invalid arguments".to_string()
    } else {
        parts.join("; ")
    }
}

/// The command path the user was reaching for, e.g. `["wallet", "send"]`, so that both the
/// message and the hint can name it. Empty when no command was recognised.
fn clap_path(argv: &[std::ffi::OsString]) -> Vec<&'static str> {
    const TOP: [&str; 10] = [
        "status",
        "node",
        "wallet",
        "settings",
        "setup",
        "doctor",
        "tui",
        "capabilities",
        "robot-docs",
        "version",
    ];
    const NESTED: [(&str, &[&str]); 4] = [
        (
            "node",
            &[
                "up", "down", "restart", "info", "log", "peers", "bans", "ban", "unban", "limit",
            ],
        ),
        (
            "wallet",
            &[
                "list",
                "open",
                "close",
                "create",
                "balance",
                "address",
                "new-address",
                "history",
                "send",
                "sweep",
                "clear-pending",
                "validate",
                "book",
                "refresh",
                "notes",
                "set-daemon",
            ],
        ),
        ("settings", &["list", "get", "set"]),
        ("doctor", &["undo", "capabilities"]),
    ];
    let mut path: Vec<&'static str> = Vec::new();
    for arg in argv.iter().skip(1) {
        let Some(arg) = arg.to_str() else { continue };
        if arg.starts_with('-') {
            continue;
        }
        match path.as_slice() {
            [] => {
                let name = match arg {
                    "docs" => "robot-docs",
                    other => match TOP.iter().find(|s| **s == other) {
                        Some(s) => s,
                        None => return path,
                    },
                };
                path.push(name);
            }
            [top] => {
                let Some((_, subs)) = NESTED.iter().find(|(n, _)| n == top) else {
                    return path;
                };
                match subs.iter().find(|s| **s == arg) {
                    Some(s) => path.push(s),
                    None => return path,
                }
            }
            _ => return path,
        }
    }
    path
}

/// Always a command that actually runs (§4: hints are runnable).
fn clap_hint(path: &[&str]) -> String {
    if path.is_empty() {
        "fmonero --help".to_string()
    } else {
        format!("fmonero {} --help", path.join(" "))
    }
}

/// Parse, or turn clap's failure into the §4 error envelope, so that a piping agent gets one
/// JSON object for **every** failure, a bad flag value included.
/// `fmonero help`, `fmonero help wallet`, `fmonero help wallet send` → the same help
/// `--help` prints. The `help` **subcommand** stays disabled (`disable_help_subcommand`), so it
/// never appears in the command tree, in `capabilities.commands` or in §5c's key-set gate — but
/// `help <sub>` is what a person and an agent both type first, and answering it with
/// "unrecognized subcommand 'help'" and a generic `fmonero --help` spends a round trip on
/// vocabulary. The rewrite is positional: only a leading `help` is treated this way, so
/// `fmonero wallet notes get help` is still a txid argument.
fn rewrite_help_argv(argv: &mut Vec<std::ffi::OsString>) {
    if argv.len() < 2 || argv[1] != *"help" {
        return;
    }
    argv.remove(1);
    argv.push(std::ffi::OsString::from("--help"));
}

fn parse_or_report() -> Cli {
    let mut argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    rewrite_help_argv(&mut argv);
    match Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(e) => {
            use clap::error::ErrorKind;
            // `--help` and `--version` are successes clap reports as errors.
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                e.exit();
            }
            // The policy has to be read off the raw argv: there is no parsed Cli yet.
            let flag = |name: &str| argv.iter().any(|a| a == name);
            let out = Out::new(flag("--json"), flag("--no-color"), 0);
            let path = clap_path(&argv);
            let message = if e.kind() == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand {
                match path.as_slice() {
                    [top] => format!("`fmonero {top}` needs a subcommand"),
                    _ => "a subcommand is required".to_string(),
                }
            } else {
                clap_message(&e)
            };
            let err = Error::usage(message)
                .with_reason("bad_args")
                .with_hint(clap_hint(&path));
            std::process::exit(out.report(&err));
        }
    }
}

/// The signal a handler saw, or 0. An `AtomicI32` store is async-signal-safe; nothing else in
/// a handler is allowed to be.
static SIGNALLED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn note_signal(sig: libc::c_int) {
    SIGNALLED.store(sig, std::sync::atomic::Ordering::SeqCst);
}

/// Ctrl+C must cancel the action, not abandon it: `proc::run_with_deadline` polls the token
/// every 25 ms, tears the child's process group down and returns `Interrupted` (exit 130).
fn install_cancel_on_signal(cancel: &CancelToken) {
    unsafe {
        libc::signal(libc::SIGINT, note_signal as *const () as libc::sighandler_t);
        libc::signal(
            libc::SIGTERM,
            note_signal as *const () as libc::sighandler_t,
        );
    }
    let cancel = cancel.clone();
    std::thread::spawn(move || {
        let sig = loop {
            let sig = SIGNALLED.load(std::sync::atomic::Ordering::SeqCst);
            if sig != 0 {
                break sig;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        cancel.cancel();
        // Never become unkillable: a command that polls no token still dies on the default
        // disposition.
        std::thread::sleep(Duration::from_secs(3));
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    });
}

fn main() {
    // Behave like a Unix tool when stdout is a closed pipe (`fmonero node log | head`).
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = parse_or_report();
    let out = Out::new(cli.json, cli.no_color, cli.verbose);
    match run(cli, out) {
        Ok(code) => std::process::exit(code),
        Err(e) => std::process::exit(out.report(&e)),
    }
}

/// Anything that escaped a `tui`/`doctor`/`setup` run keeps its §4a code when it has one.
fn from_anyhow(e: anyhow::Error) -> Error {
    match e.downcast::<Error>() {
        Ok(typed) => typed,
        Err(other) => Error::failure(format!("{other:#}")),
    }
}

/// Stamp the envelope keys every fmonero success object carries (§4).
fn ok(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(map) = value.as_object_mut() {
        map.insert("ok".into(), serde_json::json!(true));
        map.insert(
            "schema".into(),
            serde_json::json!(fmonero_core::SCHEMA_VERSION),
        );
    }
    value
}

/// `--dry-run` is implemented by exactly six commands since v5.2 (§4). Anywhere else it exits 2 rather
/// than being silently ignored: a flag that means "change nothing", accepted globally and
/// ignored on a mutating command, is worse than a flag that does not exist.
fn dry_run_is_supported(cmd: &Cmd) -> bool {
    match cmd {
        Cmd::Setup(_) | Cmd::Doctor(_) => true,
        Cmd::Wallet { cmd } => matches!(cmd, WalletCmd::Send { .. } | WalletCmd::Sweep { .. }),
        // v5.2, §2m.5 step 2: on both mode commands `--dry-run` prints the plan and the unit
        // diff and exits 0 **without** `--yes`, exactly as `setup --dry-run` does. Six commands
        // accept it, and `capabilities` is generated from this same table (§4).
        Cmd::Node { cmd } => matches!(cmd, NodeCmd::Remote { .. } | NodeCmd::Local),
        _ => false,
    }
}

/// The node client the money path uses, or `None` in remote mode (v5.2, §2m.3).
///
/// A failure that is **not** the remote-mode gate keeps its own exit code: a non-loopback
/// `node.url` is exit 2 and an unreadable `monerod.conf` is exit 1, and reporting either as
/// exit 4 `unreachable` would point the user at `fmonero node up` — which exits 2 on the same
/// configuration (§2c).
fn node_for_send(ctx: &Context) -> Result<Option<NodeClient>, Error> {
    match NodeClient::from_ctx(ctx) {
        Ok(n) => Ok(Some(n)),
        Err(e) if e.reason() == Some("remote_node") => Ok(None),
        Err(e) => Err(e),
    }
}

/// Step 7's situation, named by the caller (§2a.1, §2m.3).
///
/// `Remote` and `Unreachable` are opposite instructions — proceed by another gate, versus
/// refuse — which is why §5b made this an enum instead of leaving both behind one `None`.
fn node_gate<'a>(ctx: &Context, n: Option<&'a NodeClient>) -> send::NodeGate<'a> {
    match n {
        Some(n) => send::NodeGate::Local(n),
        None if ctx.settings.node.mode.is_remote() => send::NodeGate::Remote,
        None => send::NodeGate::Unreachable,
    }
}

/// The name of the command the user typed, for the `--dry-run` refusal's hint.
fn command_path(cmd: &Cmd) -> String {
    match cmd {
        Cmd::Status { .. } => "status".into(),
        Cmd::Node { cmd } => format!(
            "node {}",
            match cmd {
                NodeCmd::Up { .. } => "up",
                NodeCmd::Down => "down",
                NodeCmd::Restart => "restart",
                NodeCmd::Info => "info",
                NodeCmd::Log { .. } => "log",
                NodeCmd::Peers { .. } => "peers",
                NodeCmd::Bans => "bans",
                NodeCmd::Ban { .. } => "ban",
                NodeCmd::Unban { .. } => "unban",
                NodeCmd::Limit { .. } => "limit",
                NodeCmd::Remote { .. } => "remote",
                NodeCmd::Local => "local",
                NodeCmd::List { .. } => "list",
            }
        ),
        Cmd::Wallet { cmd } => format!(
            "wallet {}",
            match cmd {
                WalletCmd::List => "list",
                WalletCmd::Open { .. } => "open",
                WalletCmd::Close => "close",
                WalletCmd::Create { .. } => "create",
                WalletCmd::Balance { .. } => "balance",
                WalletCmd::Address { .. } => "address",
                WalletCmd::NewAddress { .. } => "new-address",
                WalletCmd::History { .. } => "history",
                WalletCmd::Send { .. } => "send",
                WalletCmd::Sweep { .. } => "sweep",
                WalletCmd::ClearPending => "clear-pending",
                WalletCmd::Validate { .. } => "validate",
                WalletCmd::Book { .. } => "book",
                WalletCmd::Refresh { .. } => "refresh",
                WalletCmd::Notes { .. } => "notes",
                WalletCmd::SetDaemon { .. } => "set-daemon",
            }
        ),
        Cmd::Settings { cmd } => format!(
            "settings {}",
            match cmd {
                SettingsCmd::List => "list",
                SettingsCmd::Get { .. } => "get",
                SettingsCmd::Set { .. } => "set",
            }
        ),
        Cmd::Setup(_) => "setup".into(),
        Cmd::Doctor(_) => "doctor".into(),
        Cmd::Tui(_) => "tui".into(),
        Cmd::Capabilities => "capabilities".into(),
        Cmd::RobotDocs { .. } => "robot-docs".into(),
        Cmd::Version => "version".into(),
    }
}

// ---------------------------------------------------------------- the money payload

/// The §5c key set for `wallet send` / `wallet sweep`, identical for a dry run, for the
/// no-`--yes` refusal and for a relayed success, so an agent parses **one** shape and reads
/// `relayed` to learn what happened.
fn plan_payload(plan: &Plan, txids: &[String], relayed: bool, dry_run: bool) -> serde_json::Value {
    // §5's shape rule: "Unknown is `null` — never an absent key, never `{}`, never a zero
    // standing in for unknown." `amount + fee` can only exceed `u64::MAX` on a malformed or
    // hostile wallet-rpc answer, and `Piconero::checked_add` exists precisely so money is never
    // wrapped or invented — `u64::MAX` here rendered a fabricated `18446744.073709551615 XMR`
    // total beside a real amount and a real fee.
    let total = plan.total().ok();
    let txs: Vec<serde_json::Value> = plan
        .txs
        .iter()
        .map(|t| {
            serde_json::json!({
                "txid": t.txid,
                "amount_pico": t.amount.0,
                "amount_xmr": t.amount.to_xmr_string(),
                "fee_pico": t.fee.0,
                "fee_xmr": t.fee.to_xmr_string(),
                "weight": t.weight,
                "relayed": txids.contains(&t.txid),
            })
        })
        .collect();
    serde_json::json!({
        "relayed": relayed,
        "dry_run": dry_run,
        "txids": txids,
        "amount_pico": plan.total_amount.0,
        "amount_xmr": plan.total_amount.to_xmr_string(),
        "fee_pico": plan.total_fee.0,
        "fee_xmr": plan.total_fee.to_xmr_string(),
        "total_pico": total.map(|t| t.0),
        "total_xmr": total.map(Piconero::to_xmr_string),
        "dest": plan.dest,
        "account_index": plan.account_index,
        "priority": plan.priority,
        "weight": plan.total_weight,
        "txs": txs,
    })
}

/// The §5c money exception: a `send`/`sweep` refused at step 10 (`needs_yes`) emits the
/// failure envelope **plus** the complete key set above, with the **built** transaction's
/// real numbers — the fee §2a.1 exists to show may not be human-mode-only.
fn refuse_with_plan(out: &Out, e: &Error, payload: serde_json::Value) -> i32 {
    if out.json {
        let mut merged = e.json();
        if let (Some(m), Some(p)) = (merged.as_object_mut(), payload.as_object()) {
            for (k, v) in p {
                m.insert(k.clone(), v.clone());
            }
            // The ENVELOPE wins. The payload arrives stamped `ok: true` by `ok()`, and §5c is
            // explicit that this refusal carries `ok: false` with the exit code 3 — a failure
            // envelope that says `ok: true` on the money surface is the §2j defect, and it is
            // the one an agent branches on.
            for (k, v) in e.json().as_object().into_iter().flatten() {
                m.insert(k.clone(), v.clone());
            }
        }
        println!("{merged}");
    } else {
        output::human(out, "wallet send", &payload);
        out.note(format!(
            "{}: {}",
            out.paint(&format!("fmonero: error[{}]", e.code_name()), Paint::Red),
            e
        ));
        if let Some(reason) = e.reason() {
            out.note(format!("  reason: {reason}"));
        }
        out.note(format!("  hint: {}", e.hint()));
    }
    e.exit_code()
}

/// `wallet address --qr`: the address as a half-block QR on stdout. Skipped when stdout is not
/// a TTY — the payload is then the ordinary JSON (§4).
fn print_qr(out: &Out, text: &str) -> Result<(), Error> {
    use qrcode::{Color, QrCode};
    let code = QrCode::new(text.as_bytes())
        .map_err(|e| Error::failure(format!("cannot make a QR code for that address: {e}")))?;
    let width = code.width();
    let modules = code.to_colors();
    let dark = |x: usize, y: usize| -> bool {
        y < width && x < width && modules[y * width + x] == Color::Dark
    };
    // Two module rows per text row, so the QR stays square in a terminal cell grid.
    let quiet = 2;
    let side = width + quiet * 2;
    let dark_at =
        |x: usize, y: usize| -> bool { x >= quiet && y >= quiet && dark(x - quiet, y - quiet) };
    // The glyphs draw the DARK modules, so the code only scans if the ink is dark and the paper
    // light. A bare glyph takes the terminal's own colours, which on Omarchy's dark theme renders
    // the whole code inverted — including a dark quiet zone, which is the part a decoder uses to
    // find the code at all. So each row carries an explicit black-on-white. `print_qr` runs only
    // on a TTY (§4), so colour is available unless the user turned it off, and with it off the
    // glyphs are still the honest best a monochrome terminal allows.
    let (open, close) = if out.color {
        ("\x1b[30;47m", "\x1b[0m")
    } else {
        ("", "")
    };
    for row in (0..side).step_by(2) {
        let mut line = String::with_capacity(side + open.len() + close.len());
        line.push_str(open);
        for x in 0..side {
            let top = dark_at(x, row);
            let bottom = row + 1 < side && dark_at(x, row + 1);
            line.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        line.push_str(close);
        out.line(line);
    }
    Ok(())
}

// ---------------------------------------------------------------- dispatch

fn run(cli: Cli, out: Out) -> Result<i32, Error> {
    if cli.timeout == Some(0) {
        return Err(Error::usage("--timeout 0 is not a deadline")
            .with_reason("bad_args")
            .with_hint("fmonero --timeout 5 status --json"));
    }
    let dry_run = cli.dry_run;
    let yes = cli.yes;

    // Bare invocation: the TUI on a terminal, `status --json` without one (§4).
    let cmd = match cli.cmd {
        Some(c) => c,
        None if output::stdout_is_tty() => Cmd::Tui(tui::TuiArgs::default()),
        None => Cmd::Status { check_node: false },
    };
    if dry_run && !dry_run_is_supported(&cmd) {
        let path = command_path(&cmd);
        return Err(Error::usage("--dry-run is not supported by this command")
            .with_reason("unsupported_flag")
            .with_hint(format!("fmonero {path} --help")));
    }

    let ctx = Context::load()?.with_flags(out.json, cli.verbose, cli.timeout, cli.force);
    let cancel = CancelToken::new();
    install_cancel_on_signal(&cancel);
    out.verbose_note(format!(
        "state {} · config {} · cache {} · node {}s wallet {}s",
        ctx.paths.state_dir.display(),
        ctx.paths.config_dir.display(),
        ctx.paths.cache_dir.display(),
        ctx.timeouts.node.as_secs(),
        ctx.timeouts.wallet.as_secs()
    ));
    let default_account = |given: Option<u32>| given.unwrap_or(ctx.settings.wallet.account);

    match cmd {
        Cmd::Status { check_node } => {
            // §5, v5.6: the identical document either way. The flag decides whether a
            // detached check of the configured public node is started on the way out — the one
            // spawn any form of `status` performs, and the reason the bar widget can show a
            // public node's height with an age a person would call current.
            let s = if check_node {
                fmonero_core::status_checking_node(&ctx)?
            } else {
                fmonero_core::status(&ctx)?
            };
            out.emit("status", serde_json::to_value(&s)?);
            Ok(0)
        }

        // ---- node ----------------------------------------------------------
        Cmd::Node { cmd } => {
            // **Every node command that needs the local `monerod` refuses in remote mode, in
            // one place** (§2m.3, §4.1): exit 4 `backend`, `reason: remote_node`, **before any
            // RPC is issued and before any `systemctl` spawn**. Core holds the structural gate
            // — `NodeClient::from_ctx` and `node_session`'s `Layout::Remote` arm both refuse,
            // which is what a unit test asserts — and this guard exists only to name the verb
            // the user typed in the message §2m.3 prescribes. `node info` (settings-only in
            // remote mode), `node remote` and `node local` are the three that go through.
            if ctx.settings.node.mode.is_remote() {
                let verb = match &cmd {
                    NodeCmd::Up { .. } => Some("up"),
                    NodeCmd::Down => Some("down"),
                    NodeCmd::Restart => Some("restart"),
                    NodeCmd::Log { .. } => Some("log"),
                    NodeCmd::Peers { .. } => Some("peers"),
                    NodeCmd::Bans => Some("bans"),
                    NodeCmd::Ban { .. } => Some("ban"),
                    NodeCmd::Unban { .. } => Some("unban"),
                    NodeCmd::Limit { .. } => Some("limit"),
                    // `node list` is mode-independent: a list of remote nodes is exactly as
                    // meaningful in local mode, where it is how someone decides to switch, and
                    // it touches no local `monerod` at all (§4.1, §2n).
                    NodeCmd::Info
                    | NodeCmd::Remote { .. }
                    | NodeCmd::Local
                    | NodeCmd::List { .. } => None,
                };
                if let Some(verb) = verb {
                    return Err(units::remote_node_error(&ctx, Some(verb)));
                }
            }
            match cmd {
                NodeCmd::Up { wait, wait_timeout } => {
                    if wait && wait_timeout == 0 && !output::stdout_is_tty() {
                        return Err(Error::usage(
                        "--wait-timeout 0 polls until interrupted, and nothing can interrupt a non-interactive caller",
                    )
                    .with_reason("bad_args")
                    .with_hint("fmonero node up --wait --wait-timeout 86400"));
                    }
                    let wait = wait.then(|| Duration::from_secs(wait_timeout));
                    let started = node_session::up(&ctx, wait, &cancel)?;
                    let unit = ctx.settings.node.unit.clone();
                    let active = units::state(&ctx, &unit)?.active;
                    out.emit(
                        "node up",
                        ok(serde_json::json!({
                            "unit": unit,
                            "active": active,
                            "reachable": started.reachable,
                            "synchronized": started.synchronized,
                            "waited_sec": started.waited_sec,
                            "already_running": started.already_running,
                        })),
                    );
                    Ok(0)
                }
                NodeCmd::Down => {
                    require_yes(yes, "fmonero node down", &[])?;
                    node_session::down(&ctx, &cancel)?;
                    out.emit(
                        "node down",
                        ok(serde_json::json!({
                            "unit": ctx.settings.node.unit,
                            "active": false,
                            "reachable": false,
                            "synchronized": serde_json::Value::Null,
                            "waited_sec": 0,
                            "already_running": false,
                        })),
                    );
                    Ok(0)
                }
                NodeCmd::Restart => {
                    require_yes(yes, "fmonero node restart", &[])?;
                    let started = node_session::restart(&ctx, &cancel)?;
                    let unit = ctx.settings.node.unit.clone();
                    let active = units::state(&ctx, &unit)?.active;
                    out.emit(
                        "node restart",
                        ok(serde_json::json!({
                            "unit": unit,
                            "active": active,
                            "reachable": started.reachable,
                            "synchronized": started.synchronized,
                            "waited_sec": started.waited_sec,
                            "already_running": started.already_running,
                        })),
                    );
                    Ok(0)
                }
                NodeCmd::Info => {
                    let s = fmonero_core::status(&ctx)?;
                    // **The one node command that still exits 0 in remote mode** (§2m.3): it takes
                    // a settings-only path and never calls `NodeClient::from_ctx` at all, printing
                    // the address, the trusted flag and the mode with the four measured sub-objects
                    // `null`. That is a deliberate branch in one command, not a hole in the gate:
                    // "which node am I using" is the question remote mode makes people ask, and a
                    // command that answers it by refusing is a command people stop running. **The
                    // key set is identical to local mode's**, so one parser serves both (§5c).
                    if ctx.settings.node.mode.is_remote() {
                        out.emit(
                            "node info",
                            ok(serde_json::json!({
                                "node": s.node,
                                "txpool": serde_json::Value::Null,
                                "sync": serde_json::Value::Null,
                                "hard_fork": serde_json::Value::Null,
                                "rpc": serde_json::Value::Null,
                            })),
                        );
                        // **The sentence that contradicted the row above it is gone** (X7). It
                        // read `fmonero cannot see this node's own block` directly under a row
                        // printing that block, unpunctuated, at column 0 against an eight-space
                        // indent. What is true is in `node_info_human`, indented with the rows it
                        // is about: the figures come from the last check rather than from now.
                        out.note("Try: fmonero status --check-node");
                        return Ok(0);
                    }
                    let n = NodeClient::from_ctx(&ctx)?;
                    let txpool = n.get_txpool_stats()?;
                    let hf = n.hard_fork_info(0)?;
                    let rpc = n.get_version()?;
                    // `sync_info` is a !m_restricted method: on a restricted node the data has no
                    // source at all, and a zero would be a rendered untruth (§1b.2, §5).
                    let sync = if s.node.restricted == Some(true) {
                        serde_json::Value::Null
                    } else {
                        let si = n.sync_info()?;
                        serde_json::json!({
                            "height": si.height,
                            "target_height": si.target_height,
                            "spans": si.spans,
                            "peers": si.peers,
                        })
                    };
                    out.emit(
                        "node info",
                        ok(serde_json::json!({
                            "node": s.node,
                            "txpool": {
                                "txs_total": txpool.txs_total,
                                "bytes_total": txpool.bytes_total,
                                "fee_total_pico": txpool.fee_total.0,
                                "fee_total_xmr": txpool.fee_total.to_xmr_string(),
                                "oldest": txpool.oldest,
                                "num_10m": txpool.num_10m,
                                "num_double_spends": txpool.num_double_spends,
                            },
                            "sync": sync,
                            // Two DIFFERENT fork versions, kept apart (§1b.3): `version` is the
                            // current fork, the voting fields describe the NEXT one.
                            "hard_fork": {
                                "current_version": hf.version,
                                "state": hf.state,
                                "voting_on": {
                                    "version": hf.voting,
                                    "enabled": hf.enabled,
                                    "earliest_height": hf.earliest_height,
                                    "votes": hf.votes,
                                    "threshold": hf.threshold,
                                    "window": hf.window,
                                    "voting": hf.voting,
                                }
                            },
                            "rpc": {
                                "version_major": rpc.version >> 16,
                                "version_minor": rpc.version & 0xffff,
                                "release": rpc.release,
                                "hard_forks": rpc.hard_forks,
                            }
                        })),
                    );
                    Ok(0)
                }
                NodeCmd::Log {
                    lines,
                    follow,
                    grep,
                } => {
                    if follow {
                        return Err(Error::usage(
                        "--follow is only available in the TUI's Log screen; in the CLI a pipe would hang",
                    )
                    .with_reason("unsupported_flag")
                    .with_hint("fmonero tui --log"));
                    }
                    let tail = corelog::tail(&ctx.settings.node.log_file, lines, grep.as_deref())?;
                    out.emit(
                        "node log",
                        ok(serde_json::json!({
                            "path": tail.path.display().to_string(),
                            "lines": tail.lines,
                            "truncated": tail.truncated,
                        })),
                    );
                    Ok(0)
                }
                NodeCmd::Peers { white, gray, limit } => {
                    let n = NodeClient::from_ctx(&ctx)?;
                    let conns = n.get_connections()?;
                    let lists = n.get_peer_list(true)?;
                    let cap = |mut v: Vec<serde_json::Value>| {
                        if let Some(k) = limit {
                            v.truncate(k);
                        }
                        v
                    };
                    let connections = cap(conns
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "address": c.address,
                                "host": c.host,
                                // Parsed from the daemon's STRING and rendered as an integer, so
                                // both arrays of this payload agree (§1b.3, §5c).
                                "port": c.port.parse::<u64>().ok(),
                                "state": c.state,
                                "height": c.height,
                                "pruning_seed": c.pruning_seed,
                                "incoming": c.incoming,
                                "live_time": c.live_time,
                                "avg_download": c.avg_download,
                                "avg_upload": c.avg_upload,
                            })
                        })
                        .collect());
                    let row = |p: &fmonero_core::rpc::node::Peer| serde_json::json!({"host": p.host, "port": p.port, "last_seen": p.last_seen});
                    let white_rows = cap(lists.white_list.iter().map(row).collect());
                    let gray_rows = cap(lists.gray_list.iter().map(row).collect());
                    out.emit(
                    "node peers",
                    ok(serde_json::json!({
                        "connections": connections,
                        "white": if gray && !white { serde_json::Value::Null } else { serde_json::json!(white_rows) },
                        "gray": if white && !gray { serde_json::Value::Null } else { serde_json::json!(gray_rows) },
                    })),
                );
                    Ok(0)
                }
                NodeCmd::Bans => {
                    let n = NodeClient::from_ctx(&ctx)?;
                    let bans = n.get_bans()?;
                    out.emit(
                        "node bans",
                        ok(serde_json::json!({
                            "bans": bans.iter().map(|b| serde_json::json!({
                                "host": b.host, "ip": b.ip, "seconds": b.seconds
                            })).collect::<Vec<_>>(),
                        })),
                    );
                    Ok(0)
                }
                NodeCmd::Ban { host, seconds } => {
                    let n = NodeClient::from_ctx(&ctx)?;
                    // The §2i layout gate is the TYPE: `mutators` refuses unless fmonero owns the
                    // node, before any RPC is issued.
                    n.mutators(&ctx)?.set_bans(&[BanRequest {
                        host: host.clone(),
                        ip: 0,
                        ban: true,
                        seconds,
                    }])?;
                    out.emit(
                        "node ban",
                        ok(serde_json::json!({"host": host, "banned": true, "seconds": seconds})),
                    );
                    Ok(0)
                }
                NodeCmd::Unban { host } => {
                    let n = NodeClient::from_ctx(&ctx)?;
                    n.mutators(&ctx)?.set_bans(&[BanRequest {
                        host: host.clone(),
                        ip: 0,
                        ban: false,
                        seconds: 0,
                    }])?;
                    out.emit(
                        "node unban",
                        ok(serde_json::json!({"host": host, "banned": false, "seconds": 0})),
                    );
                    Ok(0)
                }
                NodeCmd::Limit {
                    up,
                    down,
                    out_peers,
                    in_peers,
                } => {
                    let n = NodeClient::from_ctx(&ctx)?;
                    let changed =
                        up.is_some() || down.is_some() || out_peers.is_some() || in_peers.is_some();
                    let (limits, peers) = if changed {
                        let m = n.mutators(&ctx)?;
                        // 0 means "unchanged" on both sides of set_limit (§1b.3).
                        let limits = m.set_limit(down.unwrap_or(0), up.unwrap_or(0))?;
                        let peers = if out_peers.is_some() || in_peers.is_some() {
                            m.set_peers_count(out_peers, in_peers)?
                        } else {
                            n.peers_count()?
                        };
                        (limits, peers)
                    } else {
                        (n.get_limit()?, n.peers_count()?)
                    };
                    out.emit(
                        "node limit",
                        ok(serde_json::json!({
                            "limit_up_kbps": limits.limit_up,
                            "limit_down_kbps": limits.limit_down,
                            "out_peers": peers.0,
                            "in_peers": peers.1,
                            "changed": changed,
                        })),
                    );
                    Ok(0)
                }
                // v5.2, §2m.5: the two mode commands. Both are in core (`node_mode`), because they
                // write settings, render units and call `systemctl` — and because the money guards
                // they take belong with the send path's, not in a second copy here (§2a.3).
                NodeCmd::Remote { addr, trusted } => {
                    let mut own = vec![addr.clone()];
                    if trusted {
                        own.push("--trusted".into());
                    }
                    // `--dry-run` comes FIRST and needs no `--yes` (§2m.5 step 2): "show me what
                    // this would do" is the safe half of the command, and a `--dry-run` that
                    // exits 3 teaches the user to type `--yes` to see a preview.
                    if !dry_run {
                        require_yes(yes, "fmonero node remote", &own)?;
                    }
                    // v5.3: `units::install`'s sandbox refusal is raised in core and cannot
                    // know what the user typed; here it becomes their own command with
                    // `--dry-run` (§4.2, §4).
                    let change = node_mode::set_remote(&ctx, &addr, trusted, dry_run, &cancel)
                        .map_err(|e| {
                            e.retarget_sandbox_hint(dry_run_form("fmonero node remote", &own))
                        })?;
                    out.emit("node remote", ok(serde_json::to_value(&change)?));
                    Ok(0)
                }
                NodeCmd::List {
                    refresh,
                    probe,
                    limit,
                    all,
                } => {
                    let opts = nodes::ListOpts {
                        refresh,
                        probe,
                        limit,
                        all,
                    };
                    // The §2n.5 disclosure, on the surface that performs the act (revision 7).
                    // It is computed BEFORE the call, because a fetch the user did not ask for —
                    // an absent or day-old cache under a bare `node list` — is exactly the case a
                    // flag-shaped consent record misses, and README's own first quickstart line
                    // is `node list --probe`. `--json` stays silent: the envelope carries
                    // `refreshed` and `probed`.
                    let before = nodes::cached(&ctx)?;
                    let will_fetch = refresh
                        || before
                            .as_ref()
                            .map(nodes::NodeList::is_stale)
                            .unwrap_or(true);
                    let printable = before.as_ref().map(|d| d.nodes.len()).unwrap_or(0);
                    if !ctx.json {
                        if will_fetch {
                            // X2, rule 12: an em-dash is not prose punctuation here, and this
                            // is the disclosure that stands in front of a third-party fetch.
                            out.note(
                                "Getting the list from monero.fail. This tells it your IP address.",
                            );
                        }
                        if probe {
                            let n = nodes::effective_limit(&opts, printable.max(1));
                            // X2: rule 7 (`probing`) and rule 12 in one string.
                            out.note(format!(
                                "Checking up to {n} nodes. Each one learns your IP address."
                            ));
                        }
                    }
                    let mut doc = nodes::list(&ctx, opts, &cancel)?;
                    let total = doc.nodes.len();
                    let configured = nodes::configured_address(&ctx);
                    nodes::sort_for_display(&mut doc.nodes, configured.as_deref());
                    let shown = nodes::effective_limit(&opts, total);
                    let probed = doc
                        .nodes
                        .iter()
                        .take(shown)
                        .filter(|n| n.probe.is_some())
                        .count();
                    let rows: Vec<serde_json::Value> = doc
                        .nodes
                        .iter()
                        .take(shown)
                        .map(|n| {
                            let mut row =
                                serde_json::to_value(n).unwrap_or(serde_json::Value::Null);
                            if let Some(obj) = row.as_object_mut() {
                                obj.insert(
                                    "configured".into(),
                                    serde_json::Value::Bool(
                                        configured.as_deref() == Some(n.address.as_str()),
                                    ),
                                );
                            }
                            row
                        })
                        .collect();
                    out.emit(
                        "node list",
                        ok(serde_json::json!({
                            "nodes": rows,
                            "source": doc.source,
                            "fetched_at": doc.fetched_at,
                            "cache_age_sec": doc.cache_age_sec,
                            "skipped_no_port": doc.skipped_no_port,
                            "skipped_invalid": doc.skipped_invalid,
                            "probed": probed,
                            "total": total,
                            "refreshed": will_fetch,
                        })),
                    );
                    Ok(0)
                }
                NodeCmd::Local => {
                    if !dry_run {
                        require_yes(yes, "fmonero node local", &[])?;
                    }
                    let change = node_mode::set_local(&ctx, dry_run, &cancel).map_err(|e| {
                        e.retarget_sandbox_hint(dry_run_form("fmonero node local", &[]))
                    })?;
                    // §2m.5: it ends by naming `fmonero node up` when the node unit is inactive —
                    // the switch starts no daemon, because §2i's single-writer precondition is the
                    // only thing allowed to start `monerod`.
                    if !dry_run && !units::state(&ctx, &ctx.settings.node.unit)?.active {
                        out.note("the local node is not running: fmonero node up");
                    }
                    out.emit("node local", ok(serde_json::to_value(&change)?));
                    Ok(0)
                }
            }
        }

        // ---- wallet --------------------------------------------------------
        Cmd::Wallet { cmd } => match cmd {
            WalletCmd::List => {
                let wallets = wallet_dir::list(&ctx)?;
                out.emit(
                    "wallet list",
                    ok(serde_json::json!({
                        "wallets": wallets,
                        "dir": ctx.settings.wallet.dir.display().to_string(),
                    })),
                );
                Ok(0)
            }
            WalletCmd::Open {
                name,
                password_file,
            } => {
                check_wallet_name(&name)?;
                // v5.3: the open sequence runs under `timeouts.refresh_sec`, because every one
                // of its calls can be waiting behind the wallet's own first refresh against the
                // daemon — in remote mode, over the internet (§4.2, §2m.3).
                let ctx = ctx.clone().with_wallet_probe_at_refresh();
                let w = WalletClient::from_ctx(&ctx)?;
                let password = prompt::wallet_password(password_file.as_deref(), false)?;
                let opened = wallet_session::open(&ctx, &w, &name, password, &cancel)?;
                out.emit(
                    "wallet open",
                    ok(serde_json::json!({
                        "name": opened.name,
                        "open": true,
                        "unit_started": opened.unit_started,
                        "nettype": opened.net.as_str(),
                    })),
                );
                Ok(0)
            }
            WalletCmd::Close => {
                let name = state::last_open(&ctx)?.map(|(n, _)| n);
                let w = WalletClient::from_ctx(&ctx)?;
                wallet_session::close(&ctx, &w)?;
                out.emit(
                    "wallet close",
                    ok(serde_json::json!({
                        "name": name, "open": false, "unit_started": false
                    })),
                );
                Ok(0)
            }
            WalletCmd::Create {
                name,
                language,
                allow_empty_password,
                password_file,
            } => {
                check_wallet_name(&name)?;
                coresettings::validate_seed_language(&language)?;
                let mut own = vec![name.clone()];
                // `--language` is omitted when it is the default clap filled in: the hint is
                // then the same command, and a hint is read as well as run.
                if language != coresettings::DEFAULT_SEED_LANGUAGE {
                    own.push("--language".into());
                    own.push(language.clone());
                }
                if allow_empty_password {
                    own.push("--allow-empty-password".into());
                }
                if let Some(p) = &password_file {
                    own.push("--password-file".into());
                    own.push(p.display().to_string());
                }
                require_yes(yes, "fmonero wallet create", &own)?;
                // On a TTY the password is asked for twice and compared (§2b.2): a wallet whose
                // password the user does not have can never be opened and its seed can never be
                // read, and `create_wallet` takes the password verbatim with no emptiness check.
                let password =
                    prompt::wallet_password(password_file.as_deref(), output::stdout_is_tty())?;
                if password.is_empty() && !allow_empty_password {
                    return Err(Error::usage(
                        "an empty password would create an UNENCRYPTED wallet",
                    )
                    .with_reason("bad_args")
                    .with_hint(format!(
                        "fmonero wallet create {name} --allow-empty-password --yes"
                    )));
                }
                // v5.3, and the defect a live rehearsal against a real public node found:
                // `on_create_wallet` leaves the new wallet open and wallet-rpc immediately
                // starts refreshing it from the restore height the **node** just handed it
                // (§2m.3), so every call of the sequence below — `ensure_running`'s own
                // `get_version` probe, `set_daemon`, `auto_refresh`, `get_address` — is waiting
                // behind an internet round trip. Under the 5 s probe deadline one create in
                // three failed AFTER the wallet file existed (§4.2).
                let ctx = ctx.clone().with_wallet_probe_at_refresh();
                let w = WalletClient::from_ctx(&ctx)?;
                // **v5.5: one sequence, in core, for both callers** (§2a.3). The TUI creates
                // wallets too now (§7a.5), and two implementations of "ensure_running,
                // create_wallet, then the last three steps of open" are two places that can each
                // forget the `set_daemon` step whose absence means the wallet can never sync.
                //
                // `announce` is why it is a parameter and not a return value: from the moment
                // `create_wallet` returns the wallet file exists and every later step can still
                // fail — the first `wallet create` against a real public node exited 5 on
                // `set_daemon`, because wallet-rpc held its daemon mutex across the new wallet's
                // first refresh. Emitting "back up your seed before funding this wallet" only on
                // the success path would make the one output that protects money conditional on
                // a step that has nothing to do with it (§2a). It goes to stderr, so the §4 data
                // stream is untouched in both modes and the payload still carries the key.
                let created = wallet_session::create(
                    &ctx,
                    &w,
                    &name,
                    password,
                    &language,
                    &|line| out.note(line),
                    &cancel,
                )?;
                out.emit(
                    "wallet create",
                    ok(serde_json::json!({
                        "name": created.name,
                        "open": true,
                        "unit_started": created.unit_started,
                        "seed_backup_hint": created.seed_backup_hint,
                    })),
                );
                Ok(0)
            }
            WalletCmd::Balance {
                account,
                subaddresses,
            } => {
                let acct = default_account(account);
                let w = WalletClient::from_ctx(&ctx)?;
                let b = w.get_balance(acct)?;
                let subs = if subaddresses {
                    serde_json::json!(b
                        .per_subaddress
                        .iter()
                        .map(|s| serde_json::json!({
                            "address_index": s.address_index,
                            "address": s.address,
                            "label": s.label,
                            "balance_pico": s.balance.0,
                            "balance_xmr": s.balance.to_xmr_string(),
                            "unlocked_pico": s.unlocked.0,
                            "unlocked_xmr": s.unlocked.to_xmr_string(),
                            "num_unspent_outputs": s.num_unspent_outputs,
                        }))
                        .collect::<Vec<_>>())
                } else {
                    serde_json::Value::Null
                };
                out.emit(
                    "wallet balance",
                    ok(serde_json::json!({
                        "account_index": acct,
                        "balance_pico": b.balance.0,
                        "balance_xmr": b.balance.to_xmr_string(),
                        "unlocked_pico": b.unlocked.0,
                        "unlocked_xmr": b.unlocked.to_xmr_string(),
                        "blocks_to_unlock": b.blocks_to_unlock,
                        "time_to_unlock_sec": b.time_to_unlock_sec,
                        "subaddresses": subs,
                    })),
                );
                Ok(0)
            }
            WalletCmd::Address { account, index, qr } => {
                let acct = default_account(account);
                let w = WalletClient::from_ctx(&ctx)?;
                let set = w.get_address(acct)?;
                let selected = match index {
                    Some(i) => set
                        .addresses
                        .iter()
                        .find(|a| a.address_index == i)
                        .map(|a| a.address.clone())
                        .ok_or_else(|| {
                            Error::usage(format!("this account has no address {i}"))
                                .with_reason("bad_args")
                                .with_hint("fmonero wallet address --json")
                        })?,
                    None => set.primary.clone(),
                };
                if qr && output::stdout_is_tty() && !out.json {
                    print_qr(&out, &selected)?;
                }
                out.emit(
                    "wallet address",
                    ok(serde_json::json!({
                        "primary": set.primary,
                        "addresses": set.addresses.iter().map(|a| serde_json::json!({
                            "address_index": a.address_index,
                            "address": a.address,
                            "label": a.label,
                            "used": a.used,
                        })).collect::<Vec<_>>(),
                        "selected": selected,
                    })),
                );
                Ok(0)
            }
            WalletCmd::NewAddress { account, label } => {
                let acct = default_account(account);
                let w = WalletClient::from_ctx(&ctx)?;
                let created = w.create_address(acct, label.as_deref().unwrap_or(""))?;
                let set = w.get_address(acct)?;
                out.emit(
                    "wallet new-address",
                    ok(serde_json::json!({
                        "primary": set.primary,
                        "addresses": set.addresses.iter().map(|a| serde_json::json!({
                            "address_index": a.address_index,
                            "address": a.address,
                            "label": a.label,
                            "used": a.used,
                        })).collect::<Vec<_>>(),
                        "selected": created.address,
                    })),
                );
                Ok(0)
            }
            WalletCmd::History {
                account,
                limit,
                r#in,
                out: outgoing,
                pending,
                failed,
                pool,
                since,
                cached,
            } => {
                let acct = default_account(account);
                let mut kinds: Vec<TransferKind> = Vec::new();
                if r#in {
                    // `--in` selects `in` AND `block`: coinbase rows arrive in the same list and
                    // there is no separate request flag (§1b.4, §4.1).
                    kinds.push(TransferKind::In);
                    kinds.push(TransferKind::Block);
                }
                if outgoing {
                    kinds.push(TransferKind::Out);
                }
                if pending {
                    kinds.push(TransferKind::Pending);
                }
                if failed {
                    kinds.push(TransferKind::Failed);
                }
                if pool {
                    kinds.push(TransferKind::Pool);
                }
                if kinds.is_empty() {
                    // X17: one default set for the CLI and the TUI, so the same wallet cannot
                    // read `5 payments` here and six rows on the Payments pane.
                    kinds = TransferKind::DEFAULT_ROWS.to_vec();
                }
                let mut q = HistoryQuery {
                    account_index: acct,
                    limit,
                    kinds,
                    since_height: since,
                    cached,
                    ..HistoryQuery::default()
                };
                let w = if cached {
                    None
                } else {
                    Some(WalletClient::from_ctx(&ctx)?)
                };
                let rows = history::query(&ctx, w.as_ref(), &mut q)?;
                let header = history::header(&ctx)?;
                let transfers: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "txid": t.txid,
                            "type": t.kind.as_str(),
                            "height": t.height,
                            "timestamp": t.timestamp,
                            "amount_pico": t.amount.0,
                            "amount_xmr": t.amount.to_xmr_string(),
                            "fee_pico": t.fee.0,
                            "fee_xmr": t.fee.to_xmr_string(),
                            // Never recomputed here: one implementation, in core (§5b).
                            "confirmations": history::confirmations(t, q.wallet_height),
                            "locked": history::locked(t, q.wallet_height, q.adjusted_now),
                            "note": t.note,
                            "address": t.address,
                            "subaddr_index": {"major": t.subaddr_index.major, "minor": t.subaddr_index.minor},
                            "destinations": t.destinations.iter().map(|d| serde_json::json!({
                                "address": d.address,
                                "amount_pico": d.amount.0,
                                "amount_xmr": d.amount.to_xmr_string(),
                            })).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                let now = fmonero_core::now_unix();
                out.emit(
                    "wallet history",
                    ok(serde_json::json!({
                        "transfers": transfers,
                        "cache_age_sec": header.as_ref().map(|h| (now - h.built_at).max(0)),
                        "high_water": header.as_ref().map(|h| h.high_water),
                        "cached": cached,
                        "pool_unavailable": q.pool_unavailable,
                    })),
                );
                Ok(0)
            }
            WalletCmd::Send {
                to,
                amount,
                account,
                priority,
                note,
                max_fee,
            } => {
                let acct = default_account(account);
                let prio = priority.unwrap_or(ctx.settings.wallet.priority);
                check_priority(prio)?;
                let req = SendRequest {
                    dest: to.clone(),
                    amount: Piconero::parse(&amount)?,
                    account_index: acct,
                    priority: prio,
                    note: note.clone(),
                    max_fee: max_fee.as_deref().map(Piconero::parse).transpose()?,
                };
                let w = WalletClient::from_ctx(&ctx)?;
                // §2c: `NodeClient::from_ctx` fails only for a non-loopback URL (exit 2
                // `non_loopback_url`), an unreadable monerod.conf (exit 1) or remote mode
                // (exit 4 `remote_node`, v5.2) — never because the node is down. Discarding
                // that error made the first two report as exit 4 `unreachable` with
                // `fmonero node up` as the hint, which itself exits 2 on the same
                // configuration: a diagnosis pointing away from the cause, on the money path.
                let n = node_for_send(&ctx)?;
                let plan = send::build(&ctx, &w, node_gate(&ctx, n.as_ref()), &req, &cancel)?;
                // §2a.1 step 9, and §5c's money exception: the refusal must carry the BUILT
                // transaction's key set, so it is raised here — with the plan in hand — rather
                // than inside `build`, whose `Err` would destroy it (§2a.2).
                if let Err(e) = send::check_max_fee(&plan) {
                    let payload = plan_payload(&plan, &[], false, false);
                    return Ok(refuse_with_plan(&out, &e, ok(payload)));
                }
                if dry_run {
                    // --dry-run beats --yes: it never relays (§4).
                    out.emit("wallet send", ok(plan_payload(&plan, &[], false, true)));
                    return Ok(0);
                }
                if !yes {
                    let payload = plan_payload(&plan, &[], false, false);
                    let e = Error::safety(format!(
                        "this would send {} XMR to {} with a fee of {} XMR (total {})",
                        plan.total_amount.to_xmr_string(),
                        plan.dest,
                        plan.total_fee.to_xmr_string(),
                        plan.total().map(|t| t.to_xmr_string()).unwrap_or_default()
                    ))
                    .with_reason("needs_yes")
                    .with_hint(send_hint(
                        "send",
                        &to,
                        Some(&amount),
                        acct,
                        prio,
                        &[
                            ("--max-fee", max_fee.as_deref()),
                            ("--note", note.as_deref()),
                        ],
                    ));
                    return Ok(refuse_with_plan(&out, &e, ok(payload)));
                }
                let payload = plan_payload(&plan, &[], true, false);
                let txids = send::relay(&ctx, &w, plan, &|s| eprintln!("{s}"), &cancel)?;
                let mut payload = payload;
                if let Some(m) = payload.as_object_mut() {
                    m.insert("txids".into(), serde_json::json!(txids));
                    if let Some(txs) = m.get_mut("txs").and_then(|t| t.as_array_mut()) {
                        for tx in txs {
                            let relayed = tx["txid"]
                                .as_str()
                                .map(|id| txids.iter().any(|t| t == id))
                                .unwrap_or(false);
                            tx["relayed"] = serde_json::json!(relayed);
                        }
                    }
                }
                out.emit("wallet send", ok(payload));
                Ok(0)
            }
            WalletCmd::Sweep {
                to,
                account,
                priority,
                below,
                max_fee,
            } => {
                let acct = default_account(account);
                let prio = priority.unwrap_or(ctx.settings.wallet.priority);
                check_priority(prio)?;
                let req = SweepRequest {
                    dest: to.clone(),
                    account_index: acct,
                    priority: prio,
                    below: below.as_deref().map(Piconero::parse).transpose()?,
                    max_fee: max_fee.as_deref().map(Piconero::parse).transpose()?,
                };
                let w = WalletClient::from_ctx(&ctx)?;
                // As `wallet send` above: a loopback or config failure keeps its own exit code
                // rather than becoming exit 4 `unreachable` (§2c).
                let n = node_for_send(&ctx)?;
                let plan = send::build_sweep(&ctx, &w, node_gate(&ctx, n.as_ref()), &req, &cancel)?;
                if let Err(e) = send::check_max_fee(&plan) {
                    let payload = plan_payload(&plan, &[], false, false);
                    return Ok(refuse_with_plan(&out, &e, ok(payload)));
                }
                if dry_run {
                    out.emit("wallet sweep", ok(plan_payload(&plan, &[], false, true)));
                    return Ok(0);
                }
                if !yes {
                    let payload = plan_payload(&plan, &[], false, false);
                    let e = Error::safety(format!(
                        "this would sweep {} XMR to {} in {} transaction(s) with a total fee of {} XMR",
                        plan.total_amount.to_xmr_string(),
                        plan.dest,
                        plan.txs.len(),
                        plan.total_fee.to_xmr_string()
                    ))
                    .with_reason("needs_yes")
                    .with_hint(send_hint(
                        "sweep",
                        &to,
                        None,
                        acct,
                        prio,
                        &[
                            ("--below", below.as_deref()),
                            ("--max-fee", max_fee.as_deref()),
                        ],
                    ));
                    return Ok(refuse_with_plan(&out, &e, ok(payload)));
                }
                let payload = plan_payload(&plan, &[], true, false);
                let txids = send::relay(&ctx, &w, plan, &|s| eprintln!("{s}"), &cancel)?;
                let mut payload = payload;
                if let Some(m) = payload.as_object_mut() {
                    m.insert("txids".into(), serde_json::json!(txids));
                    if let Some(txs) = m.get_mut("txs").and_then(|t| t.as_array_mut()) {
                        for tx in txs {
                            let relayed = tx["txid"]
                                .as_str()
                                .map(|id| txids.iter().any(|t| t == id))
                                .unwrap_or(false);
                            tx["relayed"] = serde_json::json!(relayed);
                        }
                    }
                }
                out.emit("wallet sweep", ok(payload));
                Ok(0)
            }
            WalletCmd::ClearPending => {
                // §2a.11: the record's third writer enters the same critical section. Without
                // this a `clear-pending` between two `relay_tx` calls of a sweep removes the
                // record under the relay, `state::mark_relayed` fails for a transaction that
                // really went out, and nothing is left on disk to block the next send.
                let _lock = send::take_send_lock(&ctx)?;
                // §2a.4 makes this command "the manual escape hatch after a crash", and §4
                // makes every hint a runnable command — so the one command the `pending_send`
                // refusal points at must work even on a record fmonero cannot parse. A
                // hand-edited record, one written by a future build with a new field, or a
                // truncated copy would otherwise block every `send` and every `sweep` for good,
                // with the printed hint failing the same way.
                let (record, unreadable) = match state::pending_send(&ctx) {
                    Ok(r) => (r, false),
                    Err(_) => (None, true),
                };
                let rendered = record
                    .as_ref()
                    .map(|r| PendingStatus::from_record(r, fmonero_core::now_unix()));
                if record.is_some() {
                    out.note("check whether these transactions landed before clearing the record:");
                    out.note("  fmonero wallet history --limit 5");
                } else if unreadable {
                    out.note(format!(
                        "{} exists but cannot be parsed, so it names no transaction to check:",
                        ctx.paths.pending_file().display()
                    ));
                    out.note("  fmonero wallet history --limit 5   # check the last few by hand");
                }
                require_yes(yes, "fmonero wallet clear-pending", &[])?;
                let cleared = record.is_some() || unreadable;
                state::clear_pending_send(&ctx)?;
                out.emit(
                    "wallet clear-pending",
                    ok(serde_json::json!({
                        "cleared": cleared,
                        "pending": rendered,
                    })),
                );
                Ok(0)
            }
            WalletCmd::Validate { address } => {
                let structural = address::parse(&address);
                match WalletClient::from_ctx(&ctx).and_then(|w| w.validate_address(&address)) {
                    Ok(check) => {
                        out.emit(
                            "wallet validate",
                            ok(serde_json::json!({
                                "address": address,
                                "valid": check.valid,
                                "integrated": check.integrated,
                                "subaddress": check.subaddress,
                                "nettype": if check.nettype.is_empty() { serde_json::Value::Null } else { serde_json::json!(check.nettype) },
                                "structural_only": false,
                            })),
                        );
                        Ok(0)
                    }
                    // Only an unreachable wallet daemon falls back to the structural verdict,
                    // and it says so (§4.1). Any other failure is the daemon's answer.
                    Err(e) if e.exit_code() == 4 => {
                        let a = structural?;
                        out.verbose_note(format!("the wallet service could not check it: {e}"));
                        out.emit(
                            "wallet validate",
                            ok(serde_json::json!({
                                "address": a.text,
                                "valid": true,
                                "integrated": a.kind == address::AddrKind::Integrated,
                                "subaddress": a.kind == address::AddrKind::Subaddress,
                                "nettype": a.net.as_str(),
                                "structural_only": true,
                            })),
                        );
                        Ok(0)
                    }
                    Err(e) => Err(e),
                }
            }
            WalletCmd::Book { cmd } => {
                let w = WalletClient::from_ctx(&ctx)?;
                match cmd {
                    BookCmd::List => {
                        let entries = w.get_address_book()?;
                        out.emit(
                            "wallet book list",
                            ok(serde_json::json!({
                                "entries": entries.iter().map(|e| serde_json::json!({
                                    "index": e.index,
                                    "address": e.address,
                                    "description": e.description,
                                })).collect::<Vec<_>>(),
                            })),
                        );
                        Ok(0)
                    }
                    BookCmd::Add {
                        address,
                        description,
                    } => {
                        address::parse(&address)?;
                        let index = w.add_address_book(&address, &description)?;
                        out.emit(
                            "wallet book add",
                            ok(serde_json::json!({
                                "index": index, "address": address, "description": description
                            })),
                        );
                        Ok(0)
                    }
                    BookCmd::Remove { index } => {
                        require_yes(yes, "fmonero wallet book remove", &[index.to_string()])?;
                        let entry = w
                            .get_address_book()?
                            .into_iter()
                            .find(|e| e.index == index)
                            .ok_or_else(|| {
                                Error::not_found(format!("no address-book entry {index}"))
                                    .with_hint("fmonero wallet book list")
                            })?;
                        w.delete_address_book(index)?;
                        out.emit(
                            "wallet book remove",
                            ok(serde_json::json!({
                                "index": index,
                                "address": entry.address,
                                "description": entry.description,
                            })),
                        );
                        Ok(0)
                    }
                }
            }
            WalletCmd::Refresh { start_height } => {
                let w = WalletClient::from_ctx(&ctx)?;
                // Through `wallet_session::refresh`, never `WalletClient::refresh` directly:
                // it is the one writer of §5's `wallet.last_refresh_at`, shared with the TUI's
                // `R` and with `send`'s remote gate (§5b, §2m.3).
                let r = wallet_session::refresh(&ctx, &w, start_height.unwrap_or(0), &cancel)?;
                out.emit(
                    "wallet refresh",
                    ok(serde_json::json!({
                        "blocks_fetched": r.blocks_fetched,
                        "received_money": r.received_money,
                    })),
                );
                Ok(0)
            }
            WalletCmd::Notes { cmd } => {
                let w = WalletClient::from_ctx(&ctx)?;
                match cmd {
                    NotesCmd::Get { txid } => {
                        check_txid(&txid)?;
                        let notes = w.get_tx_notes(std::slice::from_ref(&txid))?;
                        out.emit(
                            "wallet notes get",
                            ok(serde_json::json!({"txids": [txid], "notes": notes})),
                        );
                        Ok(0)
                    }
                    NotesCmd::Set { txid, text } => {
                        check_txid(&txid)?;
                        w.set_tx_notes(std::slice::from_ref(&txid), std::slice::from_ref(&text))?;
                        out.emit(
                            "wallet notes set",
                            ok(serde_json::json!({"txids": [txid], "notes": [text]})),
                        );
                        Ok(0)
                    }
                }
            }
            WalletCmd::SetDaemon { address, trusted } => {
                // The SAME grammar `node.remote` uses (§2m.2, §5b), so the two commands cannot
                // accept different strings — and it runs before the `--yes` gate, because an
                // unparseable address costs nothing to refuse.
                let address = coresettings::Settings::parse_remote_address(&address)?;
                let mut own = vec![address.clone()];
                if trusted {
                    own.push("--trusted".into());
                }
                require_yes(yes, "fmonero wallet set-daemon", &own)?;
                let w = WalletClient::from_ctx(&ctx)?;
                // Omitting the two credential fields does not KEEP the wallet's daemon login,
                // it clears it: `on_set_daemon` builds the optional login only from a non-empty
                // username/password (`wallet_rpc_server.cpp:4754-4757`) and `wallet2::set_daemon`
                // then assigns `m_daemon_login = std::move(daemon_login)` unconditionally
                // (`wallet2.cpp:1393`). fmonero's own node requires `rpc-login` (§4.2), so a
                // loopback target carries it — from the same `monerod.conf` that
                // `wallet_session::open` reads — and a remote one still receives nothing,
                // because a remote node is reached by monero-wallet-rpc, not by fmonero (§2c).
                let login =
                    if fmonero_core::rpc::require_loopback(&format!("http://{address}")).is_ok() {
                        fmonero_core::rpc::Login::from_monerod_conf(&ctx.paths.monerod_conf())?
                    } else {
                        None
                    };
                // `ssl_support` is chosen from the ADDRESS's own shape, by the same derivation
                // the unit and `wallet_session::open` use (§2m.3): `disabled` for a loopback
                // literal, `autodetect` otherwise. Keying it on the mode instead would leave
                // this command — whose address is in neither settings key — with a rule of its
                // own.
                let ssl = units::ssl_for_address(&address);
                w.set_daemon(&address, trusted, login.as_ref(), ssl)?;
                out.emit(
                    "wallet set-daemon",
                    ok(serde_json::json!({
                        "address": address,
                        "trusted": trusted,
                        "ssl_support": ssl,
                        "authenticated": w.authenticated(),
                        // **A one-off, and the payload says so** (§5c): `wallet_session::open`
                        // issues its own `set_daemon` after every `open_wallet` (§4.2), so the
                        // next `wallet open` replaces whatever this set. An agent that set a
                        // daemon and then opened a wallet would otherwise have no way to learn
                        // that its change is gone. The persistent form is `node remote`.
                        "persistent": false,
                    })),
                );
                Ok(0)
            }
        },

        // ---- settings ------------------------------------------------------
        Cmd::Settings { cmd } => match cmd {
            SettingsCmd::List => {
                let rows: Vec<serde_json::Value> = fmonero_core::Settings::KEYS
                    .iter()
                    .map(|(key, default, doc)| {
                        serde_json::json!({
                            "key": key,
                            "value": ctx.settings.get(key).unwrap_or_default(),
                            // The table's own default, unchanged: it is the agent contract, and
                            // three of them are patterns (`~/.bitmonero`, `…/wallets/<network>`).
                            "default": default,
                            // What that pattern resolves to on THIS machine, which is what the
                            // human row compares against and prints — otherwise a value nobody
                            // changed is marked `*` (self-read, round 3).
                            "default_here": fmonero_core::Settings::default_here(
                                key,
                                &ctx.paths,
                                ctx.settings.node.network,
                            ),
                            "doc": doc,
                        })
                    })
                    .collect();
                out.emit("settings list", ok(serde_json::json!({"settings": rows})));
                Ok(0)
            }
            SettingsCmd::Get { key } => {
                let value = ctx.settings.get(&key)?;
                out.emit(
                    "settings get",
                    ok(serde_json::json!({
                        "key": key,
                        "value": value,
                        "default": fmonero_core::Settings::default_of(&key),
                    })),
                );
                Ok(0)
            }
            SettingsCmd::Set { key, value } => {
                // `set` validates the key first, so an unknown one is exit 2 `unknown_key`
                // (§4) rather than the exit 6 a `get` would answer for the same typo.
                let mut s = ctx.settings.clone();
                s.set(&key, &value)?;
                let old = ctx.settings.get(&key)?;
                s.save(&ctx)?;
                let new = s.get(&key)?;
                out.emit(
                    "settings set",
                    ok(serde_json::json!({"key": key, "old": old, "new": new})),
                );
                Ok(0)
            }
        },

        // ---- setup · doctor · tui -----------------------------------------
        Cmd::Setup(mut args) => {
            args.yes = yes;
            args.dry_run = dry_run;
            setup::run(args, ctx, out).map_err(from_anyhow)
        }
        Cmd::Doctor(mut args) => {
            args.yes = yes;
            args.dry_run = dry_run;
            doctor::run(args, ctx).map_err(from_anyhow)
        }
        Cmd::Tui(args) => {
            if !output::stdout_is_tty() {
                return Err(
                    Error::usage("the TUI needs a terminal; stdout is not a TTY")
                        .with_reason("bad_args")
                        .with_hint("fmonero status --json"),
                );
            }
            tui::run(args, ctx).map_err(from_anyhow)
        }

        // ---- the machine contract -----------------------------------------
        Cmd::Capabilities => {
            // Always machine output, whatever the flags say (§4).
            out.forced_json()
                .emit("capabilities", agent::capabilities_json(&ctx));
            Ok(0)
        }
        Cmd::RobotDocs { section } => {
            let section = section.unwrap_or_else(|| "guide".into());
            if section != "guide" {
                return Err(Error::usage(format!("unknown docs section '{section}'"))
                    .with_reason("bad_args")
                    .with_hint("fmonero robot-docs guide"));
            }
            // Always plain text, never JSON (§4).
            print!("{}", agent::ROBOT_GUIDE);
            Ok(0)
        }
        Cmd::Version => {
            out.emit(
                "version",
                ok(serde_json::json!({
                    "version": fmonero_core::VERSION,
                    "build_stage": agent::BUILD_STAGE,
                    "build_note": agent::BUILD_NOTE,
                })),
            );
            Ok(0)
        }
    }
}

// ---------------------------------------------------------------- small guards

/// Refuse an irreversible action that was not confirmed (§4.1). The money path does **not**
/// use this: `send`/`sweep` refuse only after a build, so that the fee they print is real
/// (§2a.1 step 10).
///
/// `args` carries the user's **own** arguments, and the hint is their own command with `--yes`
/// appended — the §4 rule [`send_hint`] already follows on the money path. A template hint
/// (`format!("{what} --yes")`) is not runnable for the three commands that take a required
/// positional: `fmonero wallet create --yes` exits **2** with "the following required arguments
/// were not provided: <NAME>", so the one reflex §4 promises an agent on exit 3 `needs_yes` —
/// append `--yes` and retry — costs it a round trip and a second error instead. Every flag the
/// user passed comes back too, for the reason [`send_hint`] gives: a hint that drops a bound is
/// a different command. [`tests::every_needs_yes_hint_parses_under_clap`] drives the clap tree
/// with each hint's own argv.
/// The user's own command with `--dry-run` appended — the hint a `sandboxed_units` refusal
/// carries (v5.3, §4.2). Built exactly as [`require_yes`] builds its `--yes` form, so the two
/// cannot drift.
fn dry_run_form(what: &str, args: &[String]) -> String {
    let mut s = String::from(what);
    for a in args {
        s.push(' ');
        s.push_str(&shell_quote(a));
    }
    s.push_str(" --dry-run");
    s
}

fn require_yes(yes: bool, what: &str, args: &[String]) -> Result<(), Error> {
    if yes {
        return Ok(());
    }
    let mut hint = String::from(what);
    for a in args {
        hint.push(' ');
        hint.push_str(&shell_quote(a));
    }
    hint.push_str(" --yes");
    Err(Error::safety(format!("{what} needs --yes"))
        .with_reason("needs_yes")
        .with_hint(hint))
}

/// The `--yes` hint is the user's **own** command with `--yes` appended **verbatim** (§4).
///
/// Every bound the user gave has to come back, not just the ones a template happened to carry:
/// a hint that drops `--below` turns a bounded sweep into the whole account, and one that drops
/// `--max-fee` prescribes the rebuild §2a.2 warns may charge a different fee — with the user's
/// own ceiling removed. `--note` is quoted because it is free text.
fn send_hint(
    verb: &str,
    to: &str,
    amount: Option<&str>,
    account: u32,
    priority: u32,
    extra: &[(&str, Option<&str>)],
) -> String {
    let mut s = format!("fmonero wallet {verb} {to}");
    if let Some(a) = amount {
        s.push(' ');
        s.push_str(a);
    }
    s.push_str(&format!(" --account {account} --priority {priority}"));
    for (flag, value) in extra {
        if let Some(v) = value {
            s.push_str(&format!(" {flag} {}", shell_quote(v)));
        }
    }
    s.push_str(" --yes");
    s
}

/// Single-quote a value for a hint the user pastes into a shell. fmonero itself never builds a
/// shell string (§2f) — this is the one place a *printed* command has to survive a copy-paste,
/// and a note is the only free-text argument on the money path.
fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./@:=+,".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A wallet **filename** may not contain `/`: `on_open_wallet` and friends refuse it with a
/// bare `-1` "Invalid filename" (§1b.5), so fmonero refuses it locally with exit 2 instead of
/// round-tripping it.
fn check_wallet_name(name: &str) -> Result<(), Error> {
    if name.is_empty() || name.contains('/') {
        return Err(Error::usage(format!(
            // X2: rule 7 (`daemon`) and rule 6 (`<wallet.dir>/<name>` as the subject of a
            // sentence). Rule 10's shape instead: what was typed, then what a good one looks like.
            "'{name}' is not a wallet name. A name is one word with no '/' in it, and the wallet \
             file of that name sits in your wallet folder."
        ))
        .with_reason("bad_args")
        .with_hint("fmonero wallet list"));
    }
    Ok(())
}

/// `--priority 0..4`, where **0 is an alias of the lowest tier** and not a "wallet default"
/// (§1b.4).
fn check_priority(priority: u32) -> Result<(), Error> {
    if priority > 4 {
        return Err(Error::usage(format!(
            // X2, §2o.2: the fee level's four words, not the wire's five numbers.
            "--priority {priority} is not a fee level. Use 0 to 4: 0 and 1 are cheapest, \
             2 normal, 3 fast, 4 fastest."
        ))
        .with_reason("bad_args")
        .with_hint("fmonero settings set wallet.priority 2"));
    }
    Ok(())
}

/// A transaction id is 64 hex characters; the daemon's own answer to a malformed one is a
/// `-8` variant we would have to translate anyway (§1b.4).
fn check_txid(txid: &str) -> Result<(), Error> {
    if txid.len() == 64 && txid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(Error::usage(format!(
        "'{txid}' is not a transaction id (64 hex characters)"
    ))
    .with_reason("bad_args")
    .with_hint("fmonero wallet history --limit 20"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use clap::Parser;

    /// Every `require_yes` call site in this file, as the arguments the user's own command
    /// carried and the hint the refusal must print (§4: "the hint is the user's own command
    /// with `--yes` appended verbatim").
    ///
    /// [`every_require_yes_call_site_is_listed_here`] ties this table to the source, so a
    /// seventh call site cannot arrive without a row here, and
    /// [`every_needs_yes_hint_parses_under_clap`] runs every hint through the clap tree.
    const NEEDS_YES: &[(&str, &[&str], &str)] = &[
        ("fmonero node down", &[], "fmonero node down --yes"),
        ("fmonero node restart", &[], "fmonero node restart --yes"),
        (
            "fmonero wallet create",
            &["zzhint"],
            "fmonero wallet create zzhint --yes",
        ),
        (
            "fmonero wallet clear-pending",
            &[],
            "fmonero wallet clear-pending --yes",
        ),
        (
            "fmonero wallet book remove",
            &["3"],
            "fmonero wallet book remove 3 --yes",
        ),
        (
            "fmonero wallet set-daemon",
            &["127.0.0.1:18081", "--trusted"],
            "fmonero wallet set-daemon 127.0.0.1:18081 --trusted --yes",
        ),
        // v5.2, §2m.5: both mode commands need `--yes` — they decide which node the next send
        // is built against, they restart a daemon that may be holding an open wallet, and they
        // change the node an already-relayed send would be resolved against (§4.1).
        (
            "fmonero node remote",
            &["node.example.com:18089", "--trusted"],
            "fmonero node remote node.example.com:18089 --trusted --yes",
        ),
        ("fmonero node local", &[], "fmonero node local --yes"),
    ];

    /// Every `needs_yes` hint this binary can print, from the three places that build one:
    /// [`super::require_yes`], `setup::run` (§4.2) and [`super::send_hint`] (§2a.1 step 10).
    fn every_needs_yes_hint() -> Vec<String> {
        let mut hints: Vec<String> = Vec::new();
        for (what, args, expected) in NEEDS_YES {
            let own: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
            let e = super::require_yes(false, what, &own).expect_err("no --yes is a refusal");
            assert_eq!(e.reason(), Some("needs_yes"), "{what}");
            assert_eq!(
                &e.hint(),
                expected,
                "{what}: the hint is not the user's command"
            );
            hints.push(e.hint());
        }
        hints.push(
            crate::setup::SetupArgs {
                units: true,
                ..Default::default()
            }
            .own_command(),
        );
        hints.push(super::send_hint(
            "send",
            "@friend",
            Some("0.1"),
            0,
            2,
            &[("--max-fee", Some("1")), ("--note", Some("coffee"))],
        ));
        hints.push(super::send_hint(
            "sweep",
            "@friend",
            None,
            0,
            2,
            &[("--below", Some("0.5"))],
        ));
        hints
    }

    /// **V1 of the fix-up verification.** A refusal hint is a command an agent runs: the one
    /// reflex §4 promises it on exit 3 `needs_yes` is "append `--yes` and retry", and a hint
    /// that drops the command's required positional answers that reflex with exit 2, "the
    /// following required arguments were not provided". This drives the real clap tree with
    /// each hint's own argv, so a template hint cannot come back unnoticed.
    #[test]
    fn every_needs_yes_hint_parses_under_clap() {
        let hints = every_needs_yes_hint();
        assert_eq!(
            hints.len(),
            NEEDS_YES.len() + 3,
            "a hint source went missing"
        );
        for hint in hints {
            let argv: Vec<&str> = hint.split_whitespace().collect();
            assert_eq!(argv.first(), Some(&"fmonero"), "{hint}");
            assert!(argv.contains(&"--yes"), "{hint} does not carry --yes");
            if let Err(e) = super::Cli::try_parse_from(&argv) {
                panic!("the hint `{hint}` does not parse: {e}");
            }
        }
    }

    /// The table above is the source's, not a copy that can go stale: every `require_yes(yes,`
    /// line in this file must be one of its rows.
    #[test]
    fn every_require_yes_call_site_is_listed_here() {
        let src = include_str!("main.rs");
        // A call site is a line that *starts* with the call, so this test's own mentions of
        // the helper — this filter included — are not counted as one.
        let sites: Vec<&str> = src
            .lines()
            .map(str::trim_start)
            .filter(|l| l.starts_with("require_yes(yes, \""))
            .collect();
        assert_eq!(
            sites.len(),
            NEEDS_YES.len(),
            "a require_yes call site has no row in NEEDS_YES: {sites:#?}"
        );
        for (what, _, _) in NEEDS_YES {
            assert!(
                sites.iter().any(|l| l.contains(&format!("\"{what}\""))),
                "no call site names {what}"
            );
        }
    }

    /// **V3 of the fix-up verification.** `capabilities.commands[].needs_yes` is what an agent
    /// branches on before it calls a mutating command, so it has to match the handlers in both
    /// directions: `wallet book` read `needs_yes: false` while `wallet book remove` refused
    /// with exit 3.
    #[test]
    fn capabilities_needs_yes_matches_the_handlers() {
        let dir = std::env::temp_dir().join(format!("fmonero-needs-yes-{}", std::process::id()));
        let ctx = fmonero_core::Context::for_test(&dir).expect("a test context");
        let caps = fmonero_core::agent::capabilities_json(&ctx);
        let _ = std::fs::remove_dir_all(&dir);
        let rows: Vec<(&str, bool)> = caps["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .map(|c| {
                (
                    c["name"].as_str().expect("name"),
                    c["needs_yes"].as_bool().expect("needs_yes"),
                )
            })
            .collect();
        assert!(rows.len() > 30, "the table is not vacuous");
        for (what, _, _) in NEEDS_YES {
            let name = what.strip_prefix("fmonero ").expect("a fmonero command");
            let row = rows
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("capabilities has no row for `{name}`"));
            assert!(
                row.1,
                "`{name}` refuses without --yes, and capabilities says needs_yes: false"
            );
        }
        // The other direction: nothing claims `--yes` that no handler demands. The three that
        // do not go through `require_yes` refuse elsewhere — `send`/`sweep` at §2a.1 step 10,
        // after the build, so the fee they print is real, and `setup` in `setup::run`.
        let mut declared: Vec<&str> = rows
            .iter()
            .filter(|(_, y)| *y)
            .map(|(n, _)| *n)
            .collect::<Vec<_>>();
        declared.sort_unstable();
        let mut expected: Vec<&str> = NEEDS_YES
            .iter()
            .map(|(w, _, _)| w.strip_prefix("fmonero ").expect("a fmonero command"))
            .collect();
        expected.extend(["wallet send", "wallet sweep", "setup"]);
        expected.sort_unstable();
        assert_eq!(declared, expected, "capabilities and the handlers disagree");
    }

    fn rewrite(args: &[&str]) -> Vec<String> {
        let mut argv: Vec<OsString> = args.iter().map(OsString::from).collect();
        super::rewrite_help_argv(&mut argv);
        argv.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// Phase 3's ergonomics pass. `fmonero help <sub>` is the first thing both a person and an
    /// agent type, and before this it answered "unrecognized subcommand 'help'".
    #[test]
    fn a_leading_help_becomes_the_help_flag_for_that_command() {
        assert_eq!(rewrite(&["fmonero", "help"]), ["fmonero", "--help"]);
        assert_eq!(
            rewrite(&["fmonero", "help", "wallet"]),
            ["fmonero", "wallet", "--help"]
        );
        assert_eq!(
            rewrite(&["fmonero", "help", "wallet", "send"]),
            ["fmonero", "wallet", "send", "--help"]
        );
    }

    /// Positional, so an argument that happens to be the word `help` is left alone — otherwise
    /// `wallet notes get help` would print help instead of reporting a bad txid.
    #[test]
    fn help_anywhere_but_first_is_an_ordinary_argument() {
        assert_eq!(
            rewrite(&["fmonero", "wallet", "notes", "get", "help"]),
            ["fmonero", "wallet", "notes", "get", "help"]
        );
        assert_eq!(rewrite(&["fmonero"]), ["fmonero"]);
        assert_eq!(
            rewrite(&["fmonero", "status", "--json"]),
            ["fmonero", "status", "--json"]
        );
    }
}
