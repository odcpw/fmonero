# fmonero: complete wallet workflows and intuitive desktop integration

Requested 2026-10-13 by Oliver. Implement the daily-use improvements and the advanced GUI features in this round. Use the installed jsm-managed br/bv, NTM/vibing, UX/UI, Rust optimization and gauntlet skills. Plan and refine the dependency graph before coding. Keep the team small and centralize builds.

## Product outcome

A person can open, create or restore a wallet, create labeled receiving subaddresses and payment requests, see received/confirmed/spendable state, safely prepare/send/sweep payments, manage contacts/accounts, inspect useful live logs, and use advanced watch-only, cold-signing, hardware, signing and proof tools without memorizing command-line management. The app remains on demand and the Omarchy widget remains a thin useful companion.

## Scope and boundaries

1. Local checksum validation: verify Keccak-256 after Base58 decoding for every supported address form/network; reject a changed checksum while offline; retain Monero's authoritative validation before sending. No accidental SHA3 substitution.
2. Remote transport: explicit auto/required/disabled TLS policy, CA/fingerprint options and SOCKS proxy; apply identically at startup and set_daemon. Preserve old config migration; HTTPS means required TLS; certificate failure cannot silently fall back.
3. Privacy: one persisted setting shared by TUI/widget; mask amounts, addresses, wallet names, notes, payment details and notification contents, with deliberate reveal for payment review. No hidden destination at final confirmation.
4. Receive/requests: new labeled subaddress, selected/copy/QR after creation; exact integer-amount monero: URI generation/parsing, reject unknown/unsafe parameters; review pasted requests before preparing payments; track receiving/confirmed/spendable through real wallet observations.
5. Restore: real deterministic-seed restoration with name, masked seed/password, height or date guidance, no overwrites, backend startup/node setup, clear progress/cancel/error states and discoverable opening of the restored wallet. Input protection is a hard requirement, not a stub or merely an external documentation link.
6. Logs: bounded live wallet/node/service/application sources; follow/pause/search, source labels, secret redaction, resource limits, remote-mode usefulness and no service starts from log viewing.
7. Send/sweep: TUI send-all and normal multi-transaction preparation where needed; exact cumulative fees/amounts, persisted pending state, successful partial relay handling, no ambiguous retry; optional fee ceiling available consistently.
8. Contacts/accounts: create/list/rename/select accounts and add/edit/remove recipients through the wallet engine; confirmation for destructive contact edits and unambiguous @ lookup.
9. Maintenance: deliberate rescan (spent/full as appropriate), password change and safe guided backup/recovery; no competing wallet-file writers. File artifacts are explicit and owner-only, never overwrite silently.
10. Convenience: redacted opt-in incoming-payment notifications while the wallet service is active; deterministic exact CSV/JSON history export to an explicit destination.
11. View-only: create/open view-only wallets, clearly show incoming-only versus spendable evidence; key-image import/export with explicit inputs and correct trust requirements.
12. Offline transactions: prepare unsigned artifacts, describe and review recipient/amount/fee, sign with the offline wallet, then explicitly submit signed artifacts. Never treat file presence or a timeout as broadcast success.
13. Hardware: integrate Monero's supported device workflow (Ledger/Trezor); actual creation/opening/signing delegated to the engine. Present connectivity/locked-device failures clearly. Physical device qualification is a separately recorded limitation when no device is available.
14. Signing/proofs: sign and verify messages, create/check transaction proofs, spend proofs and reserve proofs through typed supported backend calls. Keep proofs/signatures distinct from private keys.
15. UI: consistent mnemonic labels, semantic colors, keyboard/mouse parity, visible controls and focus, editable forms, narrow-terminal fallback, coherent grouping of ordinary versus advanced tools. Retain existing working navigation aliases where unambiguous.
16. Omarchy: extend the installed oliver.monero plugin with privacy, Receive/New address, requests/log/tool shortcuts and notifications; correct desktop branding; install idempotently; full shutdown and idle behavior remain correct.
17. Performance/gauntlet: bounded resource build helper, baseline latency/allocation/syscall evidence before tuning; focused conformance, round-trip, negative/fault and real isolated Monero checks, visual/mouse tests, workspace checks, fresh review, installation and artifact cleanup. Do not claim real funded payment or device acceptance from mocks.

## Delivery waves

- Foundation wave: core checksum/transport/API groundwork and UI mnemonic/color/mouse foundation in disjoint domains. No agent builds.
- Wallet wave: backend workflows and CLI contracts, then UI forms consuming those typed APIs. Central compile and focused regression tests unblock dependents.
- Integration wave: receiving, advanced workflows, logs, widget and installer. Preserve existing owner sessions and service configuration unless the feature installation requires a deliberate update.
- Qualification wave: one builder, isolated scratch backend tests, actual UI interaction/captures, profile and bounded optimizations, feature-scope gauntlet, two fresh review passes, install and cleanup.

The br issues are the execution source of truth. Each issue contains its positive behavior, failure cases, owned files, tests, dependencies and evidence limits. Closure is by the orchestrator after central verification, not by an implementing agent.
