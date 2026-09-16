//! The doctor: detect, report, and apply a small set of reversible repairs (DESIGN.md §7).
//!
//! **Stub at the Phase 1a gate.** [`DoctorArgs`] and its subcommands are frozen here because
//! `main.rs`'s clap tree references them and `main.rs` may not be edited afterwards (§2k,
//! §5b); the Phase 2 doctor agent owns everything else under `src/doctor/` and may add
//! fields to `DoctorArgs`.
//!
//! The shape §7 fixes, so the Phase 2 agent inherits it rather than inventing one:
//! `probes.rs` is the only place a process is spawned, a file read or an RPC issued;
//! `checks.rs` holds every detector as a **pure function** of the captured probes;
//! `mutate.rs` is the single chokepoint every `--fix` mutation flows through, with verbatim
//! backups, sha256 before and after, an append-only `actions.jsonl` and a recorded inverse
//! for `doctor undo`; `report.rs` renders. Read-only by default. The doctor never stops a
//! daemon it did not start, never touches a wallet file, **never removes a file**, and never
//! runs sudo.

use clap::{Args, Subcommand};
use fmonero_core::{Context, Error};

/// `fmonero doctor [--fix] [--only GROUP] [--save]`, plus `doctor undo` and
/// `doctor capabilities`.
#[derive(Args, Clone, Debug, Default)]
pub struct DoctorArgs {
    /// Apply the fixable findings (each one reversible with `doctor undo latest`)
    #[arg(long)]
    pub fix: bool,
    /// Only run one group: binaries | node | wallet | units | storage | install | security
    #[arg(long, value_name = "GROUP")]
    pub only: Option<String>,
    /// Write the run artifacts even without --fix
    #[arg(long)]
    pub save: bool,
    #[command(subcommand)]
    pub cmd: Option<DoctorCmd>,
    /// Set from the global --yes (not a flag of its own); accepted and ignored by --fix (§7).
    #[clap(skip)]
    pub yes: bool,
    /// Set from the global --dry-run (not a flag of its own).
    #[clap(skip)]
    pub dry_run: bool,
}

#[derive(Subcommand, Clone, Debug)]
pub enum DoctorCmd {
    /// Replay a `--fix` run in reverse (exit 13 if a backup is missing or altered)
    Undo {
        /// A run id, or `latest`
        #[arg(default_value = "latest")]
        run: String,
    },
    /// List the checks, groups and safety classes
    Capabilities,
}

/// Run the doctor. Exit **11** if any check is `fail` and no `--fix` was asked for; **12**
/// if a fixer failed or fixed only part; **0** only when no `fail` remains (§7).
pub fn run(_args: DoctorArgs, _ctx: Context) -> anyhow::Result<i32> {
    Err(Error::unimplemented("fmonero doctor").into())
}
