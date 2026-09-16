//! The FrankenTUI app: Node · Wallet · Send · Log (DESIGN.md §7a).
//!
//! **Stub at the Phase 1a gate.** [`TuiArgs`] is frozen here because `main.rs`'s clap tree
//! references it and `main.rs` may not be edited afterwards (§2k, §5b); the Phase 2 TUI
//! agent owns everything else under `src/tui/` and **may add fields** to `TuiArgs` without
//! touching `main.rs` — which is exactly why the struct lives here.
//!
//! The app will own a `Context` clone and call `fmonero-core` **in process**; it never spawns
//! `fmonero`, and it never prints (the runtime owns the terminal).

use clap::Args;
use fmonero_core::{Context, Error};

/// `fmonero tui [--node] [--wallet] [--send] [--log]`.
#[derive(Args, Clone, Debug, Default)]
pub struct TuiArgs {
    /// Open on the Node screen (the default)
    #[arg(long)]
    pub node: bool,
    /// Open on the Wallet screen
    #[arg(long)]
    pub wallet: bool,
    /// Open on the Send screen
    #[arg(long)]
    pub send: bool,
    /// Open on the Log screen
    #[arg(long)]
    pub log: bool,
}

/// Run the TUI. Returns the process exit code: `0` on a clean quit, the last failed action's
/// own §4a code, or `130` on Ctrl+C — the TUI never invents an exit code (§7a).
pub fn run(_args: TuiArgs, _ctx: Context) -> anyhow::Result<i32> {
    Err(Error::unimplemented("fmonero tui").into())
}
