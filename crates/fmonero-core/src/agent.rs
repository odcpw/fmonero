//! The machine contract: `capabilities` and `robot-docs` (DESIGN.md §5b, §4b).
//!
//! Everything a machine reads is **generated from the tables that define it** — the exit-code
//! dictionary from [`EXIT_CODES`], the `reason` discriminators from [`REASONS`], the settings
//! keys from `Settings::KEYS`, the warning ids from [`WARNING_IDS`] — so a documented value
//! that cannot be constructed would be a drift those tables exist to prevent (§4).
//!
//! Nothing in this file may claim more than §4b marks GREEN or AMBER, and every command says
//! which (§2j). That is why each row of [`COMMANDS`] carries a `verified` class: "honest" is
//! not a property of prose here, it is a column.

use crate::context::Context;
use crate::error::{EXIT_CODES, REASONS};
use crate::settings::Settings;
use crate::status::WARNING_IDS;

/// This build's scope and evidence, kept with the machine contract.
pub const BUILD_STAGE: &str = "wallet-workflows-0.2";

pub const BUILD_SUMMARY: &str = "Wallet workflows 0.2: restore, labeled receiving addresses, payment requests, accounts, contacts, live logs, privacy, maintenance and advanced wallet tools.

Qualified with isolated software tests and real Monero 0.18.5.3 scratch wallets. Native restore, message signatures, encrypted recovery, view-only creation, artifact exchange and password reopening have passed. See docs/QUALIFICATION.md for the current evidence. Public-network payments with valued funds and physical hardware-wallet operation remain unqualified.";

// copy-exempt-begin: BUILD_NOTE is the machine verification contract, not product UI copy.
pub const BUILD_NOTE: &str = "GREEN — local checksum/URI/integer contracts, protected input, local status/presentation, bounded artifacts/logs and plugin structural checks are exercised by deterministic tests. Workspace tests, real PTY/QML checks and installation results are recorded in docs/QUALIFICATION.md with their scope.

AMBER — protocol fixtures exercise exact request shapes, guards, split/partial/uncertain relay and injected directory durability failures. Real isolated Monero 0.18.5.3 wallets exercise seed restore, labeled subaddresses, account/contact changes, exact payment URIs, message signing and tamper rejection, encrypted recovery/password reopen, view-only creation, output/key-image export and password change. Scratch test results prove only those workflows and inputs.

RED — public-network payments with valued funds and physical Ledger/Trezor operation remain unqualified. Real owned node start, stop or restart; full-chain real sync progress and real peers; production funded balance, refresh and open_wallet scenarios; setup's system_unit refusal; the 120 GB disk floor as a capacity guarantee; production warm timing target; and every doctor check against a live production daemon remain outside this feature qualification. Local valueless regtest interoperability, when recorded, does not promote these production scenarios. Hardware workflows are implemented but physical acceptance is separate.";
// copy-exempt-end

/// Conservative rows retain red when a production or physical scenario is unrun.
const RED_COMMANDS: &[&str] = &[
    "wallet tools care hardware",
    "launch",
    "status",
    "node up",
    "node down",
    "node restart",
    "node info",
    "node peers",
    "wallet open",
    "wallet create",
    "wallet balance",
    "wallet refresh",
    "wallet send",
    "wallet sweep",
    "setup",
    "doctor",
];

/// The agent handbook, plain text (§4). Claims only GREEN and AMBER, and says which (§2j).
// copy-exempt-begin: ROBOT_GUIDE is `fmonero robot-docs`, the handbook written FOR agents — its
// audience branches on the ids and method names in it, and §2o's first paragraph exempts the agent
// contract by name. A person's version of this text is `--help` and `BUILD_SUMMARY`.
pub const ROBOT_GUIDE: &str = "\
fmonero — robot docs

A Monero node + wallet front end. One JSON object per invocation on stdout; stderr carries
progress and diagnostics. A non-TTY stdout implies --json for every command that has a JSON
payload; `robot-docs` and `--help` are always plain text and `tui` exits 2 without a terminal.

START HERE
  fmonero capabilities        # exit codes, reasons, settings keys, warning ids, env, commands
  fmonero status --json       # the whole schema-1 Status document; starts nothing
  fmonero doctor --json       # the health idiom is: fmonero doctor --json >/dev/null; echo $?

BRANCH ON code_name + reason, NEVER ON PROSE
  Every failure is {ok:false, schema, code, code_name, error, hint, reason, detail, action}.
  `detail` carries the technical text `error` no longer does — the transport's own words, the
  RPC method, a serde message — and `action` is the in-app sentence the TUI renders instead of
  `hint`. Both are null for most errors and never absent.
  `status.warnings[]` carries the same pair plus one: `hint` for a terminal, `action` for the TUI
  and `panel_action` for the bar panel, whose keymap is a different one. Render the field that
  belongs to your surface; ids and reasons are what you branch on. `reason` is a
  stable discriminator WITHIN a code, because exit 3 alone covers eight unrelated conditions
  and the reflex for one of them (append --yes and retry) is wrong for the rest:
    3 needs_yes          -> re-run with --yes
    3 pending_send       -> a previous send may be in flight; DO NOT retry. Check
                            `fmonero wallet history --limit 5`, then
                            `fmonero wallet clear-pending --yes`
    3 send_in_progress   -> another fmonero process is between a build and a relay; wait
    3 fee_above_max      -> the built fee exceeded --max-fee; nothing was relayed
    3 foreign_node       -> this monerod is not fmonero's; it is never mutated
    3 data_dir_in_use    -> another monerod already holds the data dir
    4 not_installed     -> monerod / monero-wallet-rpc is not on PATH: `omarchy pkg add monero`.
                            `node up`, `node restart`, `wallet open` and `wallet create` answer
                            this in milliseconds and start nothing
    4 unit_missing       -> no fmonero unit is installed: `fmonero setup --units --yes`
    4 unreachable        -> the daemon is not listening
    4 auth_refused       -> it is listening and refused our credential
    4 remote_node        -> node.mode is remote and that command needs the LOCAL monerod:
                            `fmonero node local --yes`. Nine node commands answer this
    4 remote_address_rejected -> monero-wallet-rpc refused the remote address itself
    2 bad_remote_address -> the address is not [http[s]://]host:port with an explicit port
    2 remote_unset       -> node.mode = remote needs node.remote, and vice versa
   10 node_not_synchronized -> OUR node is behind; wait, then retry
   10 wallet_catching_up -> the wallet did not finish a refresh against the remote node inside
                            timeouts.refresh_sec; wait, then retry
    5 stalled            -> a call was killed at its deadline less than a minute ago: wait,
                            or pass --force for this one invocation
    5 rpc_error          -> the daemon answered and refused; retrying changes nothing
    5 restricted         -> the node runs --restricted-rpc and does not serve that method
    9 not_open           -> open a wallet
    9 bad_password       -> ask the user again; the same password will not work
   16 too_large          -> this send needs more than one transaction; use `wallet sweep`
    1 txid_mismatch      -> the relayed id differs from the built one. Check what landed.
  `fmonero capabilities` prints the full (code, reason) table, generated from the same
  constant the errors are built from.

MONEY, AND WHAT MAKES IT SAFE
  `wallet send` and `wallet sweep` build the transaction with do_not_relay FIRST, so the fee
  they print is the fee of a transaction that exists. Without --yes they print it and exit 3
  `needs_yes` — and in --json that refusal carries the COMPLETE send key set, so the fee is
  machine-readable without parsing stderr. --dry-run beats --yes and never relays.
    fmonero wallet send <addr|@label> <amount> --dry-run --json   # build, print, relay nothing
    fmonero wallet send <addr|@label> <amount> --yes --json       # build and relay
  --max-fee AMOUNT refuses the relay when the built total fee exceeds it. Amounts are a
  decimal XMR string (at most 12 decimals) or raw piconero with a `p` suffix. `all` is not an
  amount: `wallet sweep <addr>` moves everything, bounded by --below.
  Every monetary value appears twice in output: `*_pico` (integer) and `*_xmr` (string). Do
  not do arithmetic on the string.
  A crash between build and relay leaves a record that BLOCKS the next send. A normal send
  clears it itself; if a command exits 3 `pending_send`, a human decides.

REMOTE NODE (§2m)
  fmonero can point monero-wallet-rpc at somebody else\'s node instead of running one here.
  **fmonero itself never speaks to it**: its own HTTP client is loopback-plaintext-only and
  contains no TLS stack. Three settings keys, and nothing else is consulted anywhere:
    node.mode            local | remote (default local)
    node.remote          [http[s]://]host:port — the port is REQUIRED, no path, no credential
    node.remote_trusted  false by default. NEVER true for a public node
  Two more keys, both TUI-only readers:
    node.remote_probe_sec  how often the TUI probes THE CONFIGURED NODE while its Node screen
                           is showing. 60 by default; 0 = off; 1..9 refused. Nothing else
                           reads it: status, the widget and the doctor spawn nothing, ever.
    wallet.autostart       start the wallet daemon when `fmonero tui` launches. false.
  Two commands, both --yes, both --dry-run capable, both idempotent:
    fmonero node remote <host:port> --yes     # writes the keys, re-renders the units,
                                              # updates the open wallet without restarting it
                                              # AND the rendered unit changed
    fmonero node local --yes                  # the way back
  And, since v5.5, the command that finds the address the first one needs:
    fmonero node list [--refresh] [--probe] [--limit N] [--all] [--json]
  In remote mode: `node up|down|restart|peers|bans|ban|unban|limit|log` exit 4 `remote_node`
  before any RPC or systemctl spawn; `node info` still exits 0 and reports the address, the
  trusted flag and the mode with its four measured sub-objects null; `status` reports
  node.layout \"remote\", node.reachable NULL (not false) and every measured node field null.
  A send\'s step-7 gate becomes ONE bounded `refresh` that must succeed.
  WHAT THE NODE SEES AND CAN DO, because none of it is hypothetical:
    * your IP address and the timing of every request, including the burst around a send;
    * which blocks the wallet asks for, which bounds the wallet\'s age;
    * with trusted, the amounts the wallet holds — they travel INSIDE the request;
    * the decoys: it supplies the output distribution and the ring members every transaction
      is built from, so a hostile node can degrade the ring signature. trusted:false does not
      reduce this.
  It can lie about the chain tip (a send built on a stale tip is rejected on relay: exit 16,
  funds unaffected), about whether a transaction was relayed (the pending record covers it),
  and about the FEE — the one lie that costs real money, because the wallet takes the
  fee-per-byte verbatim and the transaction relays successfully. Bound every send with
  --max-fee. In remote mode `wallet create` also takes the new wallet\'s restore height from
  that node, before fmonero\'s own set_daemon runs: create while it is answering.
  TLS here is --daemon-ssl autodetect: OPPORTUNISTIC AND UNAUTHENTICATED. It tries TLS, falls
  back to plaintext, and does not verify the certificate. Tor is not in v0.1.

PUBLIC NODE DISCOVERY (§2n, v5.5)
  `fmonero node list` serves cache_dir/public-nodes.json and fetches monero.fail's nodes.json
  and health.json only when --refresh is given or the cache is absent or older than 24 h;
  --probe then asks the top --limit candidates (20 by default) for get_info from THIS machine,
  with at most 8 probes in flight and a 5 s deadline each, cancellable. Payload keys: nodes[], source, fetched_at,
  cache_age_sec, skipped_no_port, skipped_invalid, probed, total, refreshed.
  WHAT IT COSTS, and it is the same four sentences README carries:
    * fetching the list tells MONERO.FAIL this machine's IP address and the time;
    * probing tells EACH PROBED NODE the same;
    * NOTHING is contacted automatically except the node already configured, and that only
      while the TUI's Node screen is showing (node.remote_probe_sec);
    * monero.fail's available/checks/last_height are MONERO.FAIL'S MEASUREMENTS FROM ITS OWN
      VANTAGE POINT — not a recommendation and not this machine's. A node rated healthy there
      may be unreachable from here, which is why --probe exists and why its numbers are printed
      in their own columns and never averaged into a score.
  In human mode `node list` prints that disclosure on stderr whenever the invocation actually
  fetches or probes. --json is silent: the envelope carries `refreshed` and `probed`.
  The fetch and the probes are `curl` CHILD PROCESSES through the one deadline helper — argv
  only, three shapes, carrying no credential, password, wallet name, address, amount or txid,
  because /proc/<pid>/cmdline is world-readable. fmonero's own client is unchanged: loopback
  http only, no TLS stack, no port bound. No curl on PATH is exit 4 curl_missing, raised before
  any network call. If monero.fail is down or changes shape, `node list` reports it and nothing
  else in fmonero is affected: the cache is served stale with its age.
  One honest discrepancy: curl verifies TLS certificates and --daemon-ssl autodetect does not,
  so an https node with a bad certificate probes reachable:false reason:tls while the wallet
  would have reached it by falling back to plaintext. Neither figure is adjusted to match.

TIMEOUTS AND HANGS
  --timeout SECONDS lowers every deadline and never raises one. `node up --wait` is bounded by
  --wait-timeout (default 1800; 0 means until interrupted and is refused without a TTY).
  `node log --follow` is refused in the CLI, where a pipe would hang.

APPLICATION LIFETIME
  fmonero app status --json reports {running, frontend_running, wallet_running, node_running}.
  It inspects local TUI leases and the two owned units; it starts nothing and makes no RPC or
  public-node request. Opening the TUI or starting an owned service makes the widget appear.
  fmonero app stop --yes --json saves/stops both owned services and asks existing TUI windows
  to exit. It refuses while a payment is being prepared or remains unresolved. Payload:
  {ok, schema, stopped: true}. The bar icon disappears; opening fmonero brings it back.
  Installation disables login startup even for unchanged, previously-enabled unit files.

WALLETS WITHOUT A TERMINAL
  FMONERO_WALLET_PASSWORD=… fmonero wallet open <name>    # or --password-file <0600 file>
  fmonero wallet open <name> --password-stdin --json
  With --password-stdin, supply a single newline-terminated UTF-8 line over a pipe. Trailing
  spaces are preserved; missing newline, more than 16384 bytes, or a 10-second input timeout
  is an error. Interactive terminals should use the ordinary prompt. The widget uses this pipe
  and clears its masked input immediately after submission; it writes no password file.
  fmonero wallet stop --yes --json saves and stops the wallet service, leaving the local node
  alone. Payload: {ok, schema, stopped: true, open: false}. Open, create, close, and stop all
  refuse while a payment is pending or being prepared.
  The password never reaches argv, and no seed, spend key, view key or transaction key ever
  appears in output, in state or in a log line.

WHAT THIS BUILD HAS BEEN SEEN TO DO
  `fmonero version --json` carries build_stage and build_note. Read it: the node and wallet
  paths are verified against a fixture-serving mock, not against a real daemon, and the note
  says exactly which rows those are.
";
// copy-exempt-end

/// One row of `capabilities.commands` (§5b).
struct Command {
    name: &'static str,
    args: &'static str,
    mutating: bool,
    needs_yes: bool,
    needs_wallet: bool,
    needs_node: bool,
    /// `json` everywhere but `robot-docs`, `--help` and `tui` (§4).
    output_format: &'static str,
    /// True for exactly six commands since v5.2 (§4).
    accepts_dry_run: bool,
    /// The §4b class this command's *lowest* scenario reaches (§2j).
    verified: &'static str,
}

/// The whole §4.1 table, as data. `capabilities` prints it and `scripts/check.sh` counts it,
/// so a command that exists in `main.rs` and not here is visible rather than undocumented.
const COMMANDS: &[Command] = &[
    cmd("status", "", false, false, false, false, false, "red"),
    cmd("node up", "[--wait] [--wait-timeout SECONDS]", true, false, false, false, false, "red"),
    cmd("node down", "", true, true, false, false, false, "red"),
    cmd("node restart", "", true, true, false, false, false, "red"),
    cmd("node info", "", false, false, false, true, false, "red"),
    cmd("node log", "[--lines N] [--grep TEXT]", false, false, false, false, false, "green"),
    cmd("node peers", "[--white|--gray] [--limit N]", false, false, false, true, false, "red"),
    cmd("node bans", "", false, false, false, true, false, "amber"),
    cmd("node ban", "HOST [--seconds N]", true, false, false, true, false, "amber"),
    cmd("node unban", "HOST", true, false, false, true, false, "amber"),
    cmd(
        "node limit",
        "[--up KBPS] [--down KBPS] [--out-peers N] [--in-peers N]",
        true, false, false, true, false, "amber",
    ),
    // v5.2, §2m.5. `needs_node: false` for both: they touch settings, unit files and
    // `systemctl`, and start no daemon. `accepts_dry_run: true` — two of the six commands
    // that take it (§4).
    cmd(
        "node remote",
        "ADDR [--trusted] [--yes] [--dry-run]",
        true, true, false, false, true, "amber",
    ),
    cmd("node local", "[--yes] [--dry-run]", true, true, false, false, true, "amber"),
    // v5.5, §2n. `mutating: true` because it WRITES THE CACHE, not because it needs `--yes`;
    // `needs_node: false` in both modes — it never touches a local `monerod` (§4.1).
    cmd(
        "node list",
        "[--refresh] [--probe] [--limit N] [--all]",
        true, false, false, false, false, "amber",
    ),
    cmd("wallet list", "", false, false, false, false, false, "green"),
    cmd("wallet open", "NAME [--password-file P | --password-stdin]", true, false, false, false, false, "red"),
    cmd("wallet close", "", true, false, true, false, false, "amber"),
    cmd("app status", "", false, false, false, false, false, "green"),
    cmd("app presentation", "", false, false, false, false, false, "green"),
    cmd("app stop", "--yes", true, true, false, false, false, "amber"),
    cmd("wallet stop", "--yes", true, true, false, false, false, "amber"),
    cmd(
        "wallet create",
        "NAME [--language L] [--allow-empty-password] [--password-file P|--password-stdin]",
        true, true, false, false, false, "red",
    ),
    cmd("wallet balance", "[--account N] [--subaddresses]", false, false, true, false, false, "red"),
    cmd("wallet address", "[--account N] [--index N] [--qr]", false, false, true, false, false, "amber"),
    cmd("wallet new-address", "[--account N] [--label TEXT]", true, false, true, false, false, "amber"),
    cmd("wallet restore", "NAME [--restore-height N] [--language L] [--seed-offset] [--secret-stdin] --yes", true, true, false, false, false, "amber"),
    cmd("wallet request create", "[--address ADDRESS] [--account N] [--index N] [--amount AMOUNT] [--description TEXT] [--recipient-name TEXT] [--qr]", false, false, false, false, false, "amber"),
    cmd("wallet request parse", "URI", false, false, false, false, false, "amber"),
    cmd("wallet logs", "[--source wallet|node|wallet-service|node-service|application] [--lines N] [--grep TEXT]", false, false, false, false, false, "amber"),
    cmd("wallet accounts list", "", false, false, true, false, false, "amber"),
    cmd("wallet accounts create", "LABEL", true, false, true, false, false, "amber"),
    cmd("wallet accounts rename", "INDEX LABEL", true, false, true, false, false, "amber"),
    cmd("wallet accounts select", "INDEX", true, false, true, false, false, "amber"),
    cmd("wallet book edit", "INDEX ADDRESS DESCRIPTION --yes", true, true, true, false, false, "amber"),
    cmd("wallet notifications enable", "", true, false, false, false, false, "amber"),
    cmd("wallet notifications disable", "", true, false, false, false, false, "amber"),
    cmd("wallet notifications status", "", false, false, false, false, false, "green"),
    cmd("wallet notifications poll", "", true, false, false, false, false, "amber"),
    cmd("wallet tools care capabilities", "", false, false, true, false, false, "amber"),
    cmd("wallet tools care rescan", "[--spent|--hard] --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools care password", "[--secret-stdin] --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools care backup", "DIRECTORY [--restore-height N] [--secret-stdin] --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools care view-only", "NAME ADDRESS [--restore-height N] [--secret-stdin] --yes", true, true, false, false, false, "amber"),
    cmd("wallet tools care hardware", "NAME ledger|trezor [--restore-height N] [--secret-stdin] --yes", true, true, false, false, false, "red"),
    cmd("wallet tools care export-outputs", "PATH --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools care import-outputs", "PATH --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools care export-key-images", "PATH --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools care import-key-images", "PATH --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools care export-history", "PATH [--format csv|json] [--account N] [--limit N] [--cached] --yes", true, true, false, false, false, "amber"),
    cmd("wallet tools message sign", "DATA --output PATH [--account N] [--index N] [--view] --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools message verify", "ADDRESS DATA SIGNATURE", false, false, true, false, false, "amber"),
    cmd("wallet tools proof transaction", "TXID ADDRESS --output PATH [--message PATH] --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools proof check-transaction", "TXID ADDRESS SIGNATURE [--message PATH]", false, false, true, false, false, "amber"),
    cmd("wallet tools proof spend", "TXID --output PATH [--message PATH] --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools proof check-spend", "TXID SIGNATURE [--message PATH]", false, false, true, false, false, "amber"),
    cmd("wallet tools proof reserve", "--output PATH (--all|--amount AMOUNT [--account N]) [--message PATH] --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools proof check-reserve", "ADDRESS SIGNATURE [--message PATH]", false, false, true, false, false, "amber"),
    cmd("wallet tools offline prepare", "TO AMOUNT --output PATH [--account N] [--priority N] [--max-fee AMOUNT] --yes", true, true, true, true, false, "amber"),
    cmd("wallet tools offline describe", "PATH", false, false, true, false, false, "amber"),
    cmd("wallet tools offline sign", "PATH --output PATH [--secret-stdin] --yes", true, true, true, false, false, "amber"),
    cmd("wallet tools offline submit", "PATH [--secret-stdin] [--max-fee AMOUNT] --yes", true, true, true, true, false, "amber"),
    cmd("wallet tools offline pending", "", false, false, false, false, false, "green"),
    cmd("wallet tools offline inspect-pending", "[--secret-stdin] --yes", true, true, false, false, false, "amber"),
    cmd("wallet tools offline clear-pending", "--yes", true, true, false, false, false, "amber"),
    cmd("launch", "[wallet|node|send|logs|receive|requests|new-address|restore|tools]", false, false, false, false, false, "red"),
    cmd(
        "wallet history",
        "[--account N] [--limit N] [--in] [--out] [--pending] [--failed] [--pool] [--since HEIGHT] [--cached]",
        false, false, true, false, false, "amber",
    ),
    cmd(
        "wallet send",
        "TO AMOUNT [--account N] [--priority 0-4] [--note TEXT] [--max-fee AMOUNT]",
        true, true, true, true, true, "red",
    ),
    cmd(
        "wallet sweep",
        "TO [--account N] [--priority 0-4] [--below AMOUNT] [--max-fee AMOUNT]",
        true, true, true, true, true, "red",
    ),
    cmd("wallet clear-pending", "", true, true, false, false, false, "green"),
    cmd("wallet validate", "ADDRESS", false, false, false, false, false, "green"),
    // §4.1's `wallet book` row reads "on add/remove" in its M column and "on remove" in its Y
    // column, and one bool per row cannot say either. `capabilities` is the surface robot-docs
    // tells an agent to branch on, so a flattened `needs_yes: false` buys it an exit 3 on a
    // mutating command; the row is split per subcommand exactly as `settings list|get|set`
    // already is, which keeps `needs_yes` the boolean an agent branches on.
    cmd("wallet book list", "", false, false, true, false, false, "amber"),
    cmd("wallet book add", "ADDRESS DESC", true, false, true, false, false, "amber"),
    cmd("wallet book remove", "INDEX", true, true, true, false, false, "amber"),
    cmd("wallet refresh", "[--start-height H]", true, false, true, true, false, "red"),
    cmd("wallet notes", "get TXID | set TXID TEXT", true, false, true, false, false, "amber"),
    cmd("wallet use-node", "ADDRESS [--trusted]", true, true, true, false, false, "amber"),
    cmd("settings list", "", false, false, false, false, false, "green"),
    cmd("settings get", "KEY", false, false, false, false, false, "green"),
    cmd("settings set", "KEY VALUE", true, false, false, false, false, "green"),
    cmd(
        "setup",
        "[--units] [--plugin] [--menu] [--all] [--rotate-rpc-login] [--full-chain]",
        true, true, false, false, true, "red",
    ),
    cmd(
        "doctor",
        "[--fix] [--only GROUP] [--save] | undo RUN|latest | capabilities",
        true, false, false, false, true, "red",
    ),
    Command {
        name: "tui",
        args: "[--node|--wallet|--send|--log|--receive|--requests|--new-address|--restore|--tools]",
        mutating: true,
        needs_yes: false,
        needs_wallet: false,
        needs_node: false,
        // Interactive, and exits 2 on a non-TTY stdout (§4).
        output_format: "tty",
        accepts_dry_run: false,
        verified: "green",
    },
    cmd("capabilities", "", false, false, false, false, false, "green"),
    Command {
        name: "robot-docs",
        args: "[SECTION]",
        mutating: false,
        needs_yes: false,
        needs_wallet: false,
        needs_node: false,
        // A handbook, not a record: always plain text (§4).
        output_format: "text",
        accepts_dry_run: false,
        verified: "green",
    },
    cmd("version", "", false, false, false, false, false, "green"),
];

#[allow(clippy::too_many_arguments)]
const fn cmd(
    name: &'static str,
    args: &'static str,
    mutating: bool,
    needs_yes: bool,
    needs_wallet: bool,
    needs_node: bool,
    accepts_dry_run: bool,
    verified: &'static str,
) -> Command {
    Command {
        name,
        args,
        mutating,
        needs_yes,
        needs_wallet,
        needs_node,
        output_format: "json",
        accepts_dry_run,
        verified,
    }
}

/// One row of `capabilities.env` (§5b).
struct EnvVar {
    name: &'static str,
    purpose: &'static str,
    default: &'static str,
    test_only: bool,
}

/// Every environment variable fmonero reads. Phase 3's ergonomics pass asserts the round trip
/// — every documented variable is read somewhere under `crates/`, and every variable read is
/// documented — which is what stops this list drifting (§5b).
// copy-exempt-begin: `capabilities.env` is forced JSON (§4) — the caller who reads these rows is
// the one setting the variables, and a purpose that did not name the file it moves would be
// useless to them. No person sees this text.
const ENV: &[EnvVar] = &[
    EnvVar {
        name: "FMONERO_STATE_DIR",
        purpose: "override $XDG_STATE_HOME/fmonero: state.json, pending.json, send.lock, doctor runs",
        default: "~/.local/state/fmonero",
        test_only: false,
    },
    EnvVar {
        name: "FMONERO_CONFIG_DIR",
        purpose: "override $XDG_CONFIG_HOME/fmonero: config.toml and monerod.conf",
        default: "~/.config/fmonero",
        test_only: false,
    },
    EnvVar {
        name: "FMONERO_CACHE_DIR",
        purpose: "override $XDG_CACHE_HOME/fmonero: the postcard transfer snapshot",
        default: "~/.cache/fmonero",
        test_only: false,
    },
    EnvVar {
        name: "FMONERO_WALLET_PASSWORD",
        purpose: "the documented way to open a wallet with no TTY; never passed to any child process",
        default: "",
        test_only: false,
    },
    EnvVar {
        name: "NO_COLOR",
        purpose: "never emit colour (so does CI, --no-color, and a non-TTY stdout)",
        default: "",
        test_only: false,
    },
    EnvVar {
        name: "CI",
        purpose: "never emit colour",
        default: "",
        test_only: false,
    },
    // X3, v5.6.3: the width the human CLI output wraps to. A shell exports it, a test sets it, and
    // without either the width comes from the terminal itself and falls back to 100 on a pipe.
    // It is documented rather than hidden because an agent capturing output at a fixed width is
    // exactly the caller who needs to know it exists.
    EnvVar {
        name: "COLUMNS",
        purpose: "wrap human output to this many columns (40-200); otherwise the terminal's own width, or 100 without one",
        default: "",
        test_only: false,
    },
    EnvVar {
        name: "FMONERO_PROC_ROOT",
        purpose: "read /proc from here instead, for the node-layout fixtures; honoured ONLY when FMONERO_STATE_DIR is also set",
        default: "/proc",
        test_only: true,
    },
    EnvVar {
        name: "FMONERO_SYSTEMCTL_FIXTURE",
        purpose: "a file of `<unit> <is-active> <is-enabled> <exists> <main-pid>` lines instead of systemctl; honoured ONLY when FMONERO_STATE_DIR is also set",
        default: "",
        test_only: true,
    },
    // Added by Phase 3's ergonomics pass, not by §5b's own list: the Phase 2 doctor introduced
    // this third test seam (`doctor/probes.rs::systemd_user_dir`, a deviation it recorded) so
    // that the six §7.1 unit-file checks have something to read, since `units::install` refuses
    // to run under FMONERO_STATE_DIR. §5b asks the ergonomics pass to assert that "every
    // variable read is documented", and `scripts/check.sh` now checks that direction too — this
    // row is what that check found missing, and documenting it is the fix.
    EnvVar {
        name: "FMONERO_SYSTEMD_USER_DIR",
        purpose: "read the systemd user units from here instead, for the doctor's unit-file fixtures; honoured ONLY when FMONERO_STATE_DIR is also set",
        default: "~/.config/systemd/user",
        test_only: true,
    },
    // v5.5, §2n.7. The fourth test seam, and the one that makes the whole of §2n testable with no
    // internet: it replaces the `https://monero.fail` base of argv shape 1 so the fetch reads a
    // local fixture server. With it set the list fetch's `--proto` becomes `=http,https` and
    // nothing else changes, because a seam that cannot be reached is not a seam.
    EnvVar {
        name: "FMONERO_NODES_URL",
        purpose: "fetch nodes.json and health.json from this base instead of https://monero.fail, for the §2n parser fixtures; read in nodes.rs and nowhere else",
        default: "https://monero.fail",
        test_only: true,
    },
];
// copy-exempt-end

/// The machine contract (§5b).
pub fn capabilities_json(ctx: &Context) -> serde_json::Value {
    let exit_codes: Vec<serde_json::Value> = EXIT_CODES
        .iter()
        .map(|(code, name, meaning)| {
            serde_json::json!({"code": code, "code_name": name, "meaning": meaning})
        })
        .collect();
    let mut reasons: Vec<serde_json::Value> = Vec::new();
    for (code, _, name) in EXIT_CODES {
        let within: Vec<&str> = REASONS
            .iter()
            .filter(|(c, _)| c == code)
            .map(|(_, r)| *r)
            .collect();
        if !within.is_empty() {
            reasons.push(serde_json::json!({
                "code": code, "code_name": name, "reasons": within
            }));
        }
    }
    let settings: Vec<serde_json::Value> = Settings::KEYS
        .iter()
        .map(|(key, default, doc)| {
            serde_json::json!({
                "key": key,
                "default": default,
                "doc": doc,
                "value": ctx.settings.get(key).unwrap_or_default(),
            })
        })
        .collect();
    let commands: Vec<serde_json::Value> = COMMANDS
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "args": c.args,
                "mutating": c.mutating,
                "needs_yes": c.needs_yes,
                "needs_wallet": c.needs_wallet,
                "needs_node": c.needs_node,
                "output_format": c.output_format,
                "accepts_dry_run": c.accepts_dry_run,
                // The lowest class wins (§4b), and `RED_COMMANDS` is applied here rather than
                // trusted in the column so that a row cannot be promoted above its lowest
                // scenario by editing one literal.
                "verified": if RED_COMMANDS.contains(&c.name) { "red" } else { c.verified },
            })
        })
        .collect();
    let env: Vec<serde_json::Value> = ENV
        .iter()
        .map(|e| {
            serde_json::json!({
                "name": e.name,
                "purpose": e.purpose,
                "default": e.default,
                "test_only": e.test_only,
            })
        })
        .collect();
    serde_json::json!({
        // The §4 envelope: every success object carries `ok` and `schema`. `main.rs` emits this
        // object verbatim, so the two keys belong here.
        "ok": true,
        "tool": crate::NAME,
        "version": crate::VERSION,
        "schema": crate::SCHEMA_VERSION,
        "contract_version": crate::CONTRACT_VERSION,
        "build_stage": BUILD_STAGE,
        "build_note": BUILD_NOTE,
        "exit_codes": exit_codes,
        "reasons": reasons,
        "settings": settings,
        "warnings": WARNING_IDS,
        "commands": commands,
        "env": env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contract_is_generated_from_the_frozen_tables() {
        let dir = std::env::temp_dir().join(format!("fmonero-caps-{}", std::process::id()));
        let ctx = Context::for_test(&dir).expect("context");
        let v = capabilities_json(&ctx);
        assert_eq!(v["exit_codes"].as_array().unwrap().len(), EXIT_CODES.len());
        assert_eq!(
            v["settings"].as_array().unwrap().len(),
            Settings::KEYS.len()
        );
        // Twenty before v5.2; §2m.1 added node.mode/node.remote/node.remote_trusted; v5.5 adds
        // node.remote_probe_sec, wallet.autostart and tui.mouse.
        for key in [
            "tui.privacy",
            "node.tls",
            "node.ca_file",
            "node.fingerprint",
            "node.proxy",
        ] {
            assert!(
                Settings::KEYS.iter().any(|row| row.0 == key),
                "{key} must be discoverable"
            );
        }
        assert!(!v["reasons"].as_array().unwrap().is_empty());
        assert_eq!(v["warnings"].as_array().unwrap().len(), WARNING_IDS.len());
        assert!(!v["env"].as_array().unwrap().is_empty());
        assert!(!v["commands"].as_array().unwrap().is_empty());
        assert_eq!(v["build_stage"], BUILD_STAGE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// §4: `--dry-run` is implemented by **exactly four** commands, and `capabilities` lists
    /// them — so the documented set and `main.rs`'s `dry_run_is_supported` cannot drift
    /// without this failing.
    #[test]
    fn exactly_six_commands_accept_dry_run() {
        let accepting: Vec<&str> = COMMANDS
            .iter()
            .filter(|c| c.accepts_dry_run)
            .map(|c| c.name)
            .collect();
        // Four before v5.2; §2m.5 step 2 puts `--dry-run` on both mode commands, ahead of
        // their `--yes` gate, exactly as `setup --dry-run` is (§4).
        assert_eq!(
            accepting,
            vec![
                "node remote",
                "node local",
                "wallet send",
                "wallet sweep",
                "setup",
                "doctor"
            ]
        );
    }

    /// §4: `output_format` is `json` everywhere but the three exceptions §4 names.
    #[test]
    fn only_three_commands_are_not_json() {
        let other: Vec<(&str, &str)> = COMMANDS
            .iter()
            .filter(|c| c.output_format != "json")
            .map(|c| (c.name, c.output_format))
            .collect();
        assert_eq!(other, vec![("tui", "tty"), ("robot-docs", "text")]);
    }

    /// §2j: nothing here may claim more than §4b marks GREEN or AMBER, and every row says
    /// which — so a command with no class is a command whose honesty nobody checked.
    #[test]
    fn every_command_carries_a_verification_class_and_the_note_names_all_three() {
        for c in COMMANDS {
            assert!(
                matches!(c.verified, "green" | "amber" | "red"),
                "{} has no §4b class",
                c.name
            );
        }
        for word in ["GREEN", "AMBER", "RED"] {
            assert!(BUILD_NOTE.contains(word), "the note must name {word}");
        }
        // Local interoperability never establishes funded public-network qualification.
        assert!(
            BUILD_NOTE.contains("public-network payments with valued funds"),
            "the note must retain the funded public-network evidence limit"
        );
        assert!(
            COMMANDS
                .iter()
                .any(|c| c.name == "wallet send" && c.verified == "red"),
            "wallet send retains its unqualified production scenario"
        );
    }

    /// §4b: `verified` is "the §4b class this command's *lowest* scenario reaches", and §4b's
    /// own rule is that a class "mirrors the *lowest* class any of its scenarios reaches". This
    /// ties the column to [`RED_COMMANDS`] in both directions, so a row cannot be promoted
    /// above its lowest class and a name cannot be added to the RED list without the column
    /// following — the drift that had `node down` reading `amber` beside `node restart`'s `red`
    /// when both have exactly one mock scenario and one unrun real one.
    #[test]
    fn verified_is_the_lowest_class_every_scenario_reaches() {
        for name in RED_COMMANDS {
            let c = COMMANDS
                .iter()
                .find(|c| &c.name == name)
                .unwrap_or_else(|| panic!("RED_COMMANDS names {name}, which is not a command"));
            assert_eq!(
                c.verified, "red",
                "{name} has a §4b RED scenario, so its lowest class is red"
            );
        }
        for c in COMMANDS {
            if c.verified == "red" {
                assert!(
                    RED_COMMANDS.contains(&c.name),
                    "{} is marked red but RED_COMMANDS does not say why (§2j)",
                    c.name
                );
            }
        }
        // And the two halves of this file agree: every clause of BUILD_NOTE's RED sentence has
        // a red row, so the prose and the column cannot drift apart.
        let red = BUILD_NOTE
            .split("RED —")
            .nth(1)
            .expect("BUILD_NOTE has a RED sentence");
        for clause in [
            "node start, stop or restart",
            "real sync progress",
            "real peers",
            "funded balance",
            "refresh and open_wallet",
            "public-network payments with valued funds",
            "physical Ledger/Trezor",
            "setup's system_unit refusal",
            "the 120 GB disk floor",
            "every doctor check",
        ] {
            assert!(red.contains(clause), "the RED sentence must name {clause}");
        }
    }

    /// The guide is what an agent reads first, so it has to name the two things that decide
    /// whether it can act: the discriminator to branch on, and what has not been verified.
    #[test]
    fn the_guide_names_the_reasons_and_is_honest_about_the_mock() {
        for reason in [
            "pending_send",
            "send_in_progress",
            "fee_above_max",
            "auth_refused",
            "stalled",
            "txid_mismatch",
        ] {
            assert!(ROBOT_GUIDE.contains(reason), "the guide must name {reason}");
        }
        assert!(ROBOT_GUIDE.contains("code_name"));
        assert!(
            ROBOT_GUIDE.contains("not against a real daemon"),
            "§2j: the guide may not present the mocked rows as verified"
        );
        // Every `reason` the guide lists must be constructible (§4's anti-drift rule).
        for line in ROBOT_GUIDE.lines() {
            let t = line.trim();
            let Some((code, rest)) = t.split_once(' ') else {
                continue;
            };
            let Ok(code) = code.parse::<i32>() else {
                continue;
            };
            let Some(reason) = rest.split_whitespace().next() else {
                continue;
            };
            if !reason.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                continue;
            }
            assert!(
                REASONS.contains(&(code, reason)),
                "the guide lists ({code}, {reason}), which Error::REASONS cannot construct"
            );
        }
    }
}
