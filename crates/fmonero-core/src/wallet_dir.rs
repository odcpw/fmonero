//! The wallet directory listing (DESIGN.md §5b, §2b.6).
//!
//! Signature-only at the Phase 1a gate (§8). It is a core function rather than a subcommand
//! body because `wallet list` **and** the TUI's open-wallet modal both consume it (§7a).

use crate::context::Context;
use crate::error::Error;

/// The `*.keys` basenames in `settings.wallet.dir`, sorted.
///
/// **No wallet bytes are read** (§2b.6): fmonero reads the *directory* and nothing else.
/// Only `monero-wallet-rpc` ever touches a wallet file.
pub fn list(_ctx: &Context) -> Result<Vec<String>, Error> {
    Err(Error::unimplemented("wallet_dir::list"))
}
