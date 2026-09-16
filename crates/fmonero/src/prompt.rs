//! The TTY password prompt (DESIGN.md §2b.1).
//!
//! **Signature-only at the Phase 1a gate** (§8): Phase 1b writes the body, against the three
//! rules restated here, because `main.rs` is frozen and must call it by name today.
//!
//! Three sources, consulted in this order, first hit wins:
//!
//! 1. `--password-file PATH` — read, **one** trailing newline stripped, and refused with
//!    exit 3 if the file has any group or other permission bit;
//! 2. `$FMONERO_WALLET_PASSWORD`;
//! 3. a TTY prompt with echo off (`termios` through `libc`; `SIGINT` restores the terminal).
//!
//! With no TTY and neither of the first two, the caller exits **9 `wallet_locked`** with
//! `hint: FMONERO_WALLET_PASSWORD=… fmonero wallet open <name>   # or --password-file`, so an
//! agent is never left waiting on a prompt that cannot appear. The password never reaches
//! argv — not fmonero's, and not `monero-wallet-rpc`'s (§2b.1).

use std::path::Path;

use fmonero_core::Error;

/// Read the wallet password (§2b.1).
///
/// `confirm` prompts **twice and compares** — `wallet create` on a TTY, where a mistyped
/// password creates a wallet that can never be opened and whose seed can never be read
/// (§2b.2). A mismatch is exit 2 and nothing is created.
pub fn wallet_password(_password_file: Option<&Path>, _confirm: bool) -> Result<String, Error> {
    Err(Error::unimplemented("prompt::wallet_password"))
}
