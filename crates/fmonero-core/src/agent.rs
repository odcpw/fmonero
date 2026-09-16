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

/// What this build actually is. `version` is the crate version; this names the phase, so a
/// `capabilities` dump can never be mistaken for a finished tool.
pub const BUILD_STAGE: &str = "phase-1b-core";

/// The §4b summary, in prose, and honest about the one thing that matters: **no fmonero build
/// has ever spoken to a real `monerod` or `monero-wallet-rpc`**, because neither is installed
/// on the machine this was written on (§1.1).
pub const BUILD_NOTE: &str = "\
Core implemented (DESIGN.md §8, Phase 1b). What has actually been observed working:

GREEN — verified end to end on a machine with no Monero installed: status with both daemons \
absent, wallet list, wallet validate's structural path, settings list|get|set, capabilities, \
robot-docs, version, setup --dry-run, wallet clear-pending, node log with no log file, every \
--help, every usage error, and amount and address parsing.

AMBER — verified against crates/fmonero-mock serving fixtures DERIVED FROM THE MONERO SOURCE \
at v0.18.5.1, which proves fmonero's wire format and parsing and proves NOTHING about a real \
daemon: node info|peers|bans|ban|unban|limit, wallet open|close|balance|address|new-address|\
history|refresh|book|notes|set-daemon|create, wallet send and sweep including the \
do_not_relay build, the real fee, the no---yes refusal and the relay sequence, MD5 Digest \
authentication, the wallet-RPC error map, the restricted-node degradation and the deadline, \
breaker and --force paths.

RED — never verified, and not claimed anywhere: a real node start, stop or restart, real sync \
progress, real peers, a real balance, a real refresh, A REAL SEND OR SWEEP OF ANY AMOUNT, a \
real open_wallet against a real wallet file, a real setup run, and the warm timing target. \
The owner turns these green by installing Monero and running the §4b command list.";

/// The agent handbook, plain text (§4). Claims only GREEN and AMBER, and says which (§2j).
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
  Every failure is {ok:false, schema, code, code_name, error, hint, reason}. `reason` is a
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
    4 unreachable        -> the daemon is not listening
    4 auth_refused       -> it is listening and refused our credential
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

TIMEOUTS AND HANGS
  --timeout SECONDS lowers every deadline and never raises one. `node up --wait` is bounded by
  --wait-timeout (default 1800; 0 means until interrupted and is refused without a TTY).
  `node log --follow` is refused in the CLI, where a pipe would hang.

WALLETS WITHOUT A TERMINAL
  FMONERO_WALLET_PASSWORD=… fmonero wallet open <name>    # or --password-file <0600 file>
  The password never reaches argv, and no seed, spend key, view key or transaction key ever
  appears in output, in state or in a log line.

WHAT THIS BUILD HAS BEEN SEEN TO DO
  `fmonero version --json` carries build_stage and build_note. Read it: the node and wallet
  paths are verified against a fixture-serving mock, not against a real daemon, and the note
  says exactly which rows those are.
";

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
    /// True for exactly four commands (§4).
    accepts_dry_run: bool,
    /// The §4b class this command's *lowest* scenario reaches (§2j).
    verified: &'static str,
}

/// The whole §4.1 table, as data. `capabilities` prints it and `scripts/check.sh` counts it,
/// so a command that exists in `main.rs` and not here is visible rather than undocumented.
const COMMANDS: &[Command] = &[
    cmd("status", "", false, false, false, false, false, "green"),
    cmd("node up", "[--wait] [--wait-timeout SECONDS]", true, false, false, false, false, "amber"),
    cmd("node down", "", true, true, false, false, false, "amber"),
    cmd("node restart", "", true, true, false, false, false, "red"),
    cmd("node info", "", false, false, false, true, false, "amber"),
    cmd("node log", "[--lines N] [--grep TEXT]", false, false, false, false, false, "green"),
    cmd("node peers", "[--white|--gray] [--limit N]", false, false, false, true, false, "amber"),
    cmd("node bans", "", false, false, false, true, false, "amber"),
    cmd("node ban", "HOST [--seconds N]", true, false, false, true, false, "amber"),
    cmd("node unban", "HOST", true, false, false, true, false, "amber"),
    cmd(
        "node limit",
        "[--up KBPS] [--down KBPS] [--out-peers N] [--in-peers N]",
        true, false, false, true, false, "amber",
    ),
    cmd("wallet list", "", false, false, false, false, false, "green"),
    cmd("wallet open", "NAME [--password-file P]", true, false, false, false, false, "amber"),
    cmd("wallet close", "", true, false, true, false, false, "amber"),
    cmd(
        "wallet create",
        "NAME [--language L] [--allow-empty-password] [--password-file P]",
        true, true, false, false, false, "amber",
    ),
    cmd("wallet balance", "[--account N] [--subaddresses]", false, false, true, false, false, "amber"),
    cmd("wallet address", "[--account N] [--index N] [--qr]", false, false, true, false, false, "amber"),
    cmd("wallet new-address", "[--account N] [--label TEXT]", true, false, true, false, false, "amber"),
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
    cmd("wallet book", "list | add ADDRESS DESC | remove INDEX", true, false, true, false, false, "amber"),
    cmd("wallet refresh", "[--start-height H]", true, false, true, true, false, "amber"),
    cmd("wallet notes", "get TXID | set TXID TEXT", true, false, true, false, false, "amber"),
    cmd("wallet set-daemon", "ADDRESS [--trusted]", true, true, true, false, false, "amber"),
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
        true, false, false, false, true, "green",
    ),
    Command {
        name: "tui",
        args: "[--node|--wallet|--send|--log]",
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
];

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
                "verified": c.verified,
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
        assert_eq!(v["settings"].as_array().unwrap().len(), 20);
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
    fn exactly_four_commands_accept_dry_run() {
        let accepting: Vec<&str> = COMMANDS
            .iter()
            .filter(|c| c.accepts_dry_run)
            .map(|c| c.name)
            .collect();
        assert_eq!(
            accepting,
            vec!["wallet send", "wallet sweep", "setup", "doctor"]
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
        // And it must not claim a real send: §4b's RED list is the one thing §2j is about.
        assert!(
            BUILD_NOTE.contains("A REAL SEND OR SWEEP OF ANY AMOUNT"),
            "the note must say the send path has never been broadcast"
        );
        assert!(
            COMMANDS
                .iter()
                .any(|c| c.name == "wallet send" && c.verified == "red"),
            "`wallet send` is RED until a real broadcast has happened (§4b)"
        );
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
