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
pub fn list(ctx: &Context) -> Result<Vec<String>, Error> {
    let dir = &ctx.settings.wallet.dir;
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // A directory that does not exist yet is an empty list, not a failure: `wallet list`
        // on a freshly installed machine is a GREEN row (§4b), and its own output names the
        // command that creates the first wallet.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        // **S6 and S1.** This read `cannot read <dir>: Permission denied (os error 13)` with a
        // hint that only printed the path back, and the TUI then threw the whole error away and
        // told the owner he had no wallet (S1). The sentence is plain now, the operating system's
        // words are in `detail`, and the hint names the command that shows what is wrong with a
        // folder rather than the one that repeats its name.
        Err(e) => {
            return Err(Error::io("read", "your wallet folder", dir, &e)
                .with_reason("wallet_dir_unreadable")
                .with_action("Press o on the Wallet screen once you can read that folder.")
                .with_hint("fmonero doctor --only wallet"))
        }
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".keys").map(str::to_string)
        })
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_keys_basenames_are_listed_sorted_and_a_missing_dir_is_empty() {
        let root = std::env::temp_dir().join(format!("fmonero-wdir-{}", std::process::id()));
        let mut ctx = Context::for_test(&root).unwrap();
        ctx.settings.wallet.dir = root.join("wallets/mainnet");
        assert!(list(&ctx).unwrap().is_empty(), "a missing dir is empty");
        std::fs::create_dir_all(&ctx.settings.wallet.dir).unwrap();
        for name in ["main.keys", "main", "zeta.keys", "notes.txt", ".keys"] {
            std::fs::write(ctx.settings.wallet.dir.join(name), b"").unwrap();
        }
        assert_eq!(list(&ctx).unwrap(), vec!["main", "zeta"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
