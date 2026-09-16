//! Desktop and systemd integration (DESIGN.md §4.2).
//!
//! **Stub at the Phase 1a gate.** [`SetupArgs`] is frozen here because `main.rs`'s clap tree
//! references it and `main.rs` may not be edited afterwards (§2k, §5b); Phase 1b writes the
//! body.
//!
//! What it will do, so the shape is not re-derived: `--units` writes both units plus
//! `~/.config/fmonero/monerod.conf` (0600), runs `systemctl --user daemon-reload`, and leaves
//! both units **disabled** — they are started on demand. It creates `settings.wallet.dir` and
//! every missing parent at 0700 first, because `monero-wallet-rpc` creates only the leaf and
//! treats any other failure as fatal. It rewrites the derived settings keys for
//! `settings.node.network` — `node.url`, `wallet.url` **and `wallet.port`** — so that the
//! unit's `--rpc-bind-port`, `wallet.url` and `Paths::wallet_login_file(port)` cannot
//! disagree. Everything it prints redacts the `rpc-login=` value. `--plugin` links and
//! enables the bar widget; `--menu` merges `assets/menu.jsonc` **textually**, after a verbatim
//! backup, because a serde round trip would delete every comment in a file the owner
//! maintains by hand. Idempotent: running it twice leaves the same machine.

use clap::Args;
use fmonero_core::{Context, Error};

use crate::output::Out;

/// The menu entries `--menu` merges. Single-line objects: the merge inserts lines (§4.2).
///
/// It lives here rather than in Phase 1b's code because `include_str!` resolves against this
/// file, and `main.rs` — frozen at the 1a gate — must be able to name this module. Phase 1b's
/// `run` is its consumer; until then nothing reads it.
#[allow(dead_code)]
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

/// Run setup. `--dry-run` prints a unified diff and changes nothing (§4.2).
pub fn run(_args: SetupArgs, _ctx: Context, _out: Out) -> anyhow::Result<i32> {
    Err(Error::unimplemented("fmonero setup").into())
}
