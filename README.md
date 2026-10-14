# fmonero

A Rust terminal wallet and Omarchy companion for Monero. Monero's wallet engine owns cryptography and wallet files; fmonero provides the interface, guarded payment workflows, local status and desktop integration.

Open, create or restore a wallet, make labeled subaddresses and payment requests, track incoming payments, send or sweep, manage accounts and recipients, and inspect live logs. Advanced tools cover encrypted recovery wallets, view-only wallets and exchange artifacts, offline signing, Ledger/Trezor creation, message signatures and payment proofs.

## Install

```bash
scripts/install.sh --dry-run
scripts/install.sh --yes
```

The installer builds one release binary, installs it atomically to `~/.local/bin/fmonero`, creates the **fmonero** desktop entry, and runs idempotent Omarchy setup. Search for **fmonero**, use the **fmonero** menu, or run `fmonero` in a terminal. Monero itself must already be installed; on Omarchy use `omarchy pkg add monero`.

The existing plugin ID remains `oliver.monero`, with fmonero branding. Setup adds the widget and menu entries while preserving unrelated configuration and custom menu labels. Specific desktop destinations open a fresh window on the requested page. An ordinary Open action focuses the app when it is already running. Existing windows retain their loaded binary until reopened after an upgrade.

`scripts/uninstall.sh` removes the app integration. Its optional purge preserves wallets and the chain; pending-payment protection also applies to uninstall.

## Start with a wallet

Run `fmonero` and use **(O)pen**, **(C)reate**, **(R)estore**, or **(N)ew address**. Wallet is the initial screen. Secrets use bounded masked editors or private input pipes; seed restoration never overwrites an existing wallet. Height `0` scans from the beginning when the wallet's birthday is unknown.

Restore distinguishes a wallet ready for use, a wallet created but still needing setup, and an uncertain result. Check the wallet list after an uncertain operation. A ready wallet can still be scanning; balances and receipt status show the current evidence.

```bash
fmonero wallet list
fmonero wallet open savings
fmonero wallet restore recovered --restore-height 0 --yes
fmonero wallet new-address --label 'Invoice 2026-001'
fmonero wallet balance
```

With `--secret-stdin`, restore reads one complete line for the seed, one for the new password, and an optional seed offset only when requested. View-only creation reads the private view key then the new password. Password change reads the old password then the new password. Hardware, recovery backup and native signing read their requested password. These input values belong in protected prompts/pipes. Legacy password-file/environment opening remains compatible with earlier automation.

## The interface

| Page | Keyboard | Main actions |
| --- | --- | --- |
| Node | `1` | Choose local/remote node, inspect network state and transport policy |
| Wallet | `2` | Open/create/restore, labeled addresses, history, accounts and recipients |
| Send | `3` | Exact amount or send-all, priority, optional fee ceiling and final review |
| Logs | `4` | Wallet/node/service/application sources, follow/pause, search and matches |
| Receive | `5` | Selected subaddress, complete request URI/QR and observed receipt status |
| Tools | `6` | Maintenance, exchange, hardware, offline transactions and proofs |

Visible actions have mnemonic labels and mouse targets. Editors own their typing; letters entered into a filter or secret field do not invoke global shortcuts. Escape closes the current editor or review. Long forms/reviews scroll, with their action buttons kept reachable. Color supplements text, selection and status.

Shared privacy hides wallet names, addresses, amounts, notes, logs, errors and request QR contents. Payment review uses deliberate local reveal without changing the global setting. Copying hidden data requires an explicit reveal. Secrets stay masked in both modes. Desktop incoming alerts are opt-in and use generic content.

```bash
fmonero settings set tui.privacy true
fmonero wallet notifications enable
fmonero wallet notifications disable
fmonero app presentation --json
```

The widget offers Receive, New address, Restore, Live logs, Tools, privacy and incoming-alert controls. It checks wallet/node status only while fmonero is active. Closing a terminal leaves an active wallet service scanning; **Save and close fmonero** stops the owned services and closes the app's windows. Unresolved payments prevent shutdown.

```bash
fmonero app status --json
fmonero wallet close
fmonero wallet stop --yes
fmonero app stop --yes
```

## Receiving and payment requests

New address creation selects the actual returned account/subaddress. Requests use exact integer piconero; a generated QR contains the complete URI. Pasted requests are checked for network, checksum, exact amount, repeated or unsupported fields before they fill a Send draft. Parsing a request prepares no payment.

```bash
fmonero wallet request create --account 0 --index 1 --amount 0.01 --description 'Invoice 2026-001' --qr
fmonero wallet request parse 'monero:ADDRESS?tx_amount=0.01'
fmonero wallet validate ADDRESS
```

Offline validation checks Base58, network/form and Keccak checksum. Monero also validates before a payment is prepared. Receipt status distinguishes observed, confirmed, unlocked/unspent and unknown. View-only wallets require key images to establish spent status; observed unlocked funds alone do not prove signing authority.

## Payments

```bash
fmonero wallet send @friend 0.1 --max-fee 0.001 --dry-run
fmonero wallet send @friend 0.1 --max-fee 0.001 --yes
fmonero wallet sweep ADDRESS --max-fee 0.001 --yes
```

Monero prepares transactions with `do_not_relay=true`. fmonero validates recipients, amounts and parallel transaction lists, adds exact fees with checked arithmetic, and retains one payment lock through review and relay. Normal payments may split into several transactions. Partial or uncertain relay remains recorded; an acknowledgement or timeout never becomes a claim that nothing was sent. Check actual transfers before clearing uncertainty or attempting another payment.

The CLI requires `--yes` to relay. The TUI requires the exact final review and typed confirmation. The same immutable plan supplies the review and the relay. Amounts accept decimal XMR with at most 12 places or integer piconero with a `p` suffix.

## Accounts, recipients and logs

```bash
fmonero wallet accounts list
fmonero wallet accounts create 'Work'
fmonero wallet accounts rename 1 'Business'
fmonero wallet accounts select 1
fmonero wallet book add ADDRESS 'friend'
fmonero wallet book edit 0 ADDRESS 'updated friend' --yes
fmonero wallet book remove 0 --yes
fmonero wallet logs --source wallet --lines 100
fmonero wallet logs --source wallet-service --grep error
```

Account/contact mutations bind the actual wallet and reviewed row, so a shifted contact index or changed wallet cannot silently redirect an edit. Opening a wallet validates its selected account and resets an invalid selection to account 0. Live log reading starts no service; secret-bearing lines and terminal escape payloads are removed before filtering or display.

## Transport

```bash
fmonero node remote node.example:18081 --yes
fmonero settings set node.tls required
fmonero settings set node.ca_file /absolute/path/ca.pem
fmonero settings set node.proxy 127.0.0.1:9050
```

`node.tls` accepts `auto`, `required` or `disabled`. An explicit HTTPS address requires TLS. A CA file and certificate fingerprint are mutually exclusive; `node.fingerprint` takes a SHA-256 certificate hash. Startup and live daemon configuration use the same validated policy. Proxy changes that conflict with a running engine require a deliberate service restart.

Configured SOCKS proxies govern discovery/probes as well as wallet connections. A probe refuses a certificate pin it cannot enforce; curl's public-key pin is a different concept from Monero's certificate fingerprint. The Rust binary's wallet RPC remains loopback HTTP with kernel connection-owner verification, private Digest credentials, disabled proxy/redirect behavior and bounded zeroizing buffers. Monero handles remote TLS.

## Advanced tools

Use the Tools page for guided forms, prerequisites, immutable review and results. Equivalent CLI entry points include:

```bash
fmonero wallet tools care capabilities
fmonero wallet tools care rescan --yes
fmonero wallet tools care rescan --spent --yes
fmonero wallet tools care password --yes
fmonero wallet tools care backup /absolute/new-recovery-directory --restore-height 0 --yes
fmonero wallet tools care view-only incoming ADDRESS --restore-height 0 --yes
fmonero wallet tools care hardware device-wallet ledger --restore-height 0 --yes
fmonero wallet tools care export-outputs /absolute/outputs.json --yes
fmonero wallet tools care import-outputs /absolute/outputs.json --yes
fmonero wallet tools care export-key-images /absolute/images.json --yes
fmonero wallet tools care import-key-images /absolute/images.json --yes
fmonero wallet tools care export-history /absolute/history.csv --format csv --yes
```

Artifacts have explicit new destinations, private permissions, bounded inputs and atomic no-overwrite publication. History export preserves exact amounts; CSV neutralizes spreadsheet formula text. Recovery backup uses Monero to generate and password-reopen a fresh encrypted wallet with the same primary address. Its coverage is recovery of funds through a rescan; account labels, address-book entries, notes and local history metadata are separate.

Key-image import and spent rescanning require an explicitly trusted daemon. Hardware operations use Monero's device support and report connectivity/locked-device failures. Capability hints describe creation provenance; the backend decides actual support.

```bash
fmonero wallet tools message sign message.txt --output /absolute/message.signature --yes
fmonero wallet tools message verify ADDRESS message.txt /absolute/message.signature
fmonero wallet tools proof transaction TXID ADDRESS --output /absolute/payment.proof --yes
fmonero wallet tools proof check-transaction TXID ADDRESS /absolute/payment.proof
fmonero wallet tools proof spend TXID --output /absolute/spend.proof --yes
fmonero wallet tools proof check-spend TXID /absolute/spend.proof
fmonero wallet tools proof reserve --amount 1 --output /absolute/reserve.proof --yes
fmonero wallet tools proof check-reserve ADDRESS /absolute/reserve.proof
```

Message files preserve their exact UTF-8 bytes. Verification reports invalid signatures separately from unavailable transport and exits nonzero for invalid proofs. Reserve proof creation explicitly chooses an account amount or `--all` scope.

## Offline transactions

```bash
fmonero wallet tools offline prepare ADDRESS 0.1 --output /absolute/unsigned.txset --max-fee 0.001 --yes
fmonero wallet tools offline describe /absolute/unsigned.txset
fmonero wallet tools offline sign /absolute/unsigned.txset --output /absolute/signed.txset
fmonero wallet tools offline sign /absolute/unsigned.txset --output /absolute/signed.txset --yes
fmonero wallet tools offline submit /absolute/signed.txset --max-fee 0.001
fmonero wallet tools offline submit /absolute/signed.txset --max-fee 0.001 --yes
fmonero wallet tools offline pending --json
fmonero wallet tools offline inspect-pending --yes
```

Preparation requires a view-only wallet producing a real unsigned transaction set. Native signing runs offline. Monero itself loads and reviews the immutable artifact; the held process owns the payment lock until sign/submit or decline. Without `--yes`, CLI signing/submission emits the actual review, declines and attempts to reopen the wallet.

Signed submission persists artifact digest, transaction count and unknown transaction IDs before approval. Proven hashes and unresolved counts remain distinct after partial failure. A separate native pending record blocks other mutations. The inspection action reopens only the recorded wallet while retaining that guard; inspect actual transfers before using `offline clear-pending --yes`. Wallet reopen warnings are independent of signing/submission results.

## Verification and development

One shared target directory and `scripts/cargo-serial.sh` serialize builds with two jobs. Development/test profiles disable debug symbols and incremental output. Agents write disjoint source/tests; the orchestrator owns builds, verification and issue closure. The br queue and delivery scope are in `.beads/` and `docs/IMPLEMENTATION.md`.

```bash
scripts/cargo-serial.sh test --workspace --all-targets
scripts/cargo-serial.sh clippy --workspace --all-targets -- -D warnings
scripts/check.sh
python3 tests/native-wallet.py --binary target/release/fmonero --output /tmp/native-wallet.json
python3 tests/native-wallet.py --regtest --binary target/release/fmonero --output /tmp/native-regtest.json
tests/e2e.sh --full
tests/tui_tmux.sh
tests/plugin-smoke.sh --static
node tests/widget-model.mjs
python3 tests/widget-policy.py --binary target/release/fmonero
```

Tests set all configuration, state, cache, data, systemd and node-list paths together. Native tests generate private scratch wallets and credentials; they never use an owner's wallet or funded public network. Protocol fixtures establish parsing and guard behavior. Offline/native and local test-chain evidence remain separate from public-network payment and physical-device qualification.

Current evidence and limits are in [docs/QUALIFICATION.md](docs/QUALIFICATION.md). `DESIGN.md` and `DECISIONS.md` retain historical decisions. The current owner-authorized scope supersedes their earlier cut lists. Core remains silent with typed errors; callers own presentation. `capabilities` and `robot-docs` document the machine interface.

## Current limits

One wallet can be open in each wallet RPC service. Multisig, mining controls, account tags and output freeze/thaw are outside this round. New cryptographic/address protocols depend on the installed Monero engine and explicit frontend support. Hardware qualification needs a connected device; public-network payments require separate funded acceptance. Recovery backup's metadata limits are explicit above. Desktop alerts use at-most-once candidate claims, so a failed desktop delivery can miss an alert and is not automatically retried.

## Credits

The house pattern (workspace shape, CLI envelope, deadline helper, doctor kernel, TUI, bar
widget, install scripts) comes from this author's `fproton` and `fbittorrent`. The panel's
chrome follows Omarchy's own Tailscale plugin. Every Monero fact in `DESIGN.md` carries a
`file:line` into the Monero source at tag v0.18.5.1.

*About Contributions:* Please don't take this the wrong way, but I do not accept outside
contributions for any of my projects. I simply don't have the mental bandwidth to review
anything, and it's my name on the thing, so I'm responsible for any problems it causes; thus,
the risk-reward is highly asymmetric from my perspective. I'd also have to worry about other
"stakeholders," which seems unwise for tools I mostly make for myself for free. Feel free to
submit issues, and even PRs if you want to illustrate a proposed fix, but know I won't merge
them directly. Instead, I'll have Claude or Codex review submissions via `gh` and independently
decide whether and how to address them. Bug reports in particular are welcome. Sorry if this
offends, but I want to avoid wasted time and hurt feelings. I understand this isn't in sync with
the prevailing open-source ethos that seeks community contributions, but it's the only way I can
move at this velocity and keep my sanity.

MIT. Monero is free software from the Monero Project; this is an unofficial front end for its
daemons, and it is not affiliated with them.
