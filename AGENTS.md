# fmonero implementation session

Read CLAUDE.md, README.md and docs/IMPLEMENTATION.md. The owner's current instruction authorizes br/bv, an NTM team, the complete daily-use and advanced wallet scope, UI changes, and Omarchy installation. That instruction overrides DESIGN.md's older no-beads/no-NTM and feature cut-list restrictions. Keep Monero as the cryptographic and wallet-file authority.

## Coordination and ownership

- Use br with --json; never bare bv. The orchestrator owns dependencies and closure. Agents claim only their assigned issues; report implementation and test coverage, never self-close.
- Two implementing agents at most: core owns crates/fmonero-core plus workspace manifests; UI owns crates/fmonero/src/tui. The orchestrator owns main.rs, plugin, install/setup, documentation and central verification. Agree APIs before touching a shared boundary. Do not edit another domain or overwrite existing dirty changes.
- No subagent may compile, run cargo test, spawn more agents, change services, open owner wallets, or perform owner wallet transactions. Write real code and meaningful tests; the orchestrator runs one serialized build/test wave. No stubs, fixture-as-live proof, weakened assertions, or hard-coded successful results.
- Never include unrelated files in a commit. Agents hand off paths and test names; the orchestrator records verification before br close. No push without an explicit instruction.

## Safety and compatibility

- Passwords, seeds and private keys enter only masked TUI fields or bounded stdin prompts/pipes. Never arguments, environment, output, logs, clipboard history, state, debug formatters, or checked-in fixtures. Clear buffers on completion/cancel. New restore/import/signing operations must use explicit user input and preserve payment locks and pending-payment guards.
- Never read or modify the owner's wallet files directly. Backend wallet operations go through Monero; exported artifacts are explicit, bounded, owner-only, atomic writes. Existing wallets must not be overwritten.
- No test may operate real systemd units: set all FMONERO_STATE_DIR, FMONERO_CONFIG_DIR, FMONERO_CACHE_DIR, XDG_DATA_HOME, FMONERO_SYSTEMD_USER_DIR, FMONERO_SYSTEMCTL_FIXTURE and FMONERO_NODES_URL together. Tests use scratch wallets and isolated processes. No funded broadcasts or physical hardware interaction are authorized by this development task.
- Broadcasts keep exact recipient/amount/fee review, typed send confirmation or --yes, do_not_relay preparation, one lock, persisted uncertainty, and no ambiguous retry. Do not silently fall back from required TLS to plaintext, or make node requests while the app is closed.
- Preserve existing settings through defaulted migrations. Keep decimals in integer piconero. Core stays silent; every process/HTTP call is deadline-bounded; argv only. Never add a generic arbitrary wallet RPC escape hatch.

## Resources and UX

- One shared target directory; no worktree build trees. Orchestrator only, jobs=2, no incremental/debug symbols for development. Use scripts/cargo-serial.sh. Keep target protected during active builds and remove disposable build artifacts only after compiler/test processes finish. Never delete global caches or another project's files.
- Use mnemonic labeled buttons such as (O)pen, (R)estore, (N)ew address; keyboard and mouse dispatch the same action. Text entry owns its keystrokes. Explain each action, show work/busy/error states, preserve Escape/cancel, support narrow/short terminals, use semantic colors plus text, and make every advertised action clickable.
- Omarchy plugin extends the existing oliver.monero installation, not a duplicate. Sensitive advanced workflows remain in the TUI, with useful widget shortcuts, privacy mode, notifications and on-demand lifecycle preserved.
- Profile before optimizing. The gauntlet is scoped to these changed features and their reverse dependencies, with Monero 0.18.5.3 as the backend oracle. Report software, offline backend, funded-payment and hardware evidence separately; do not claim full GUI certification.
