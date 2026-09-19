# fmonero — notes for agents working in this repo

Fast Monero node + wallet front end for Omarchy: Rust CLI + engine (`crates/fmonero-core`,
`crates/fmonero`), a FrankenTUI app (`fmonero tui`), a `doctor`, and a thin Quickshell bar widget
(`plugin/oliver.monero`). Owner: Oliver (GitHub org `odcpw`). House precedent:
`~/Projects/odcpw/fproton` (the mature pattern: workspace, CLI envelope, doctor, TUI, plugin,
install script, DESIGN/DECISIONS) and `~/Projects/odcpw/fbittorrent` (`ureq` HTTP client with an
in-process mock server test suite, on-demand systemd user unit for the backend).

## Read first
- `docs/FEASIBILITY.md` — the research statement (2026-09-16) and the architecture decision:
  node via JSON-RPC to a local unrestricted `monerod`; wallet via `monero-wallet-rpc` run as a
  systemd user unit; FrankenTUI `ftui` 0.7; `ureq` 3.4; postcard snapshots. No asupersync, no
  fsqlite, no tokio; stable toolchain.
- `DESIGN.md` — the contract. Its section numbers are cited from code as `(DESIGN.md §n)`.
- `DECISIONS.md` — append-only, newest at the bottom, real transcripts in gate sections.

## Rules that are easy to break
- Money safety: `send`, `sweep` and anything that broadcasts require `--yes`, print the exact
  recipient, amount, fee and priority first, and honour `--dry-run` (wallet-rpc `do_not_relay`).
  Never invent a default recipient or amount.
- Secrets: the wallet password, seed, spend key and view key never appear in argv, logs, JSON
  output, state files, fixtures, DECISIONS.md or test names. Never call `query_key` outside an
  explicit command that requires `--yes` and a TTY. Never read wallet files directly; only
  `monero-wallet-rpc` touches them.
- Loopback only: daemons bind `127.0.0.1`; fmonero never binds a port; RPC URLs are validated as
  loopback unless the user explicitly configured a remote node.
- Never hang: every external process goes through one deadline helper (process group, piped
  stdio, SIGTERM then SIGKILL); every HTTP call has a timeout. `status` never starts a daemon.
- argv only — no shell strings anywhere (Rust or QML). `omarchy-launch-*` helpers and
  `bar.run()` are shell strings: pass constant literals only.
- Core is silent (no print macros in `fmonero-core`); typed `Error`, one variant per exit code,
  every error carries a runnable `hint`. `--json` on every command; non-TTY stdout is JSON.
- No stubs, mocks or placeholders in shipped code. Tests use in-process mock JSON-RPC servers.
- Claim nothing RED in README, `--help` or robot-docs. Append to `DECISIONS.md`; never rewrite
  earlier entries.
- **A sandbox needs `FMONERO_SYSTEMCTL_FIXTURE`, not just `FMONERO_STATE_DIR`.** The state dir moves
  fmonero's files; it does **not** move `systemctl --user`. `units::install` refuses under a state
  dir with no fixture (exit 3 `sandboxed_units`) and `units::start`/`stop` refuse the same way — but
  the refusal is the last line of defence, not the plan. On 2026-09-19 a verification sandbox set the
  state dir and forgot the fixture, and its `node up` started the owner's real unit: a real `monerod`
  ran for thirty seconds and left ~147 MB in `~/.bitmonero` (DECISIONS: V18, round 4's S11). Every
  sandboxed run sets, together: `FMONERO_STATE_DIR`, `FMONERO_CONFIG_DIR`, `FMONERO_CACHE_DIR`,
  `XDG_DATA_HOME`, `FMONERO_SYSTEMD_USER_DIR`, `FMONERO_SYSTEMCTL_FIXTURE` and — if anything could
  fetch a node list — `FMONERO_NODES_URL` pointed at a local `fmonero-mock`.

## State (2026-09-19)
Monero 0.18.5.1 is installed (`extra/monero`). fmonero v0.1 is built, installed to
`~/.local/bin/fmonero`, its two user units are written (disabled until a wallet is opened or the
owner runs `node up`), the bar widget `oliver.monero` is live, and the owner uses **remote node
mode** (`node.mode = remote`, DESIGN.md §2m): no local `monerod`, `monero-wallet-rpc` talks to a
public node. Every command exercised so far is in DECISIONS.md's verification sections; a real
send or sweep of any amount is still RED (never run) and must not be claimed anywhere. Fixtures
are derived from the Monero source at v0.18.5.1 and say so in their headers.
