# fmonero — design brief v5.2 (source of truth for every agent working on this repo)

A fast Monero node + wallet front end for Omarchy: a Rust CLI + engine (`crates/fmonero-core`,
`crates/fmonero`), a FrankenTUI app (`fmonero tui`), a `doctor`, and a thin Quickshell bar widget
(`plugin/oliver.monero`). It replaces `monero-gui` (109 QML files, 26 869 lines of QML, a
wallet2 engine in-process and a `monerod` child it babysits by shelling out to `monerod sync_info`
every 2 s — `docs/FEASIBILITY.md` §2) with a process that starts in milliseconds and costs nothing
at idle.

This document is written so that a fresh agent can implement any single phase of §8 from this file
alone. Section numbers are cited from code as `(DESIGN.md §n)`. Where a fact came from the Monero
source it carries a `file:line` from the shallow clone of tag **v0.18.5.1** — the version Arch
ships — under the scratchpad path named in §1.6.

Repo: `~/Projects/odcpw/fmonero` (GitHub `odcpw/fmonero`, pushed later by the owner).
Owner: Oliver, Omarchy 4.0.3, Arch, Hyprland, Quickshell.
Sibling projects whose conventions we copy, and which you must read before writing code:
`~/Projects/odcpw/fproton` (workspace shape, CLI envelope, `proc::run_with_deadline`, doctor,
TUI, plugin, install script) and `~/Projects/odcpw/fbittorrent` (`src/qbt.rs` — the `ureq` client
shape; `src/backend.rs` + `src/setup.rs` — the on-demand systemd user unit; `src/qbt_tests.rs` —
in-process `TcpListener` mock-server tests).

`docs/FEASIBILITY.md` §5 and §6 are the decided architecture. Do not relitigate them.

---

## 0. Scope of v0.1 (decided; do not widen)

**IN.**

- **Node**, via JSON-RPC to a local *unrestricted* `monerod` on `127.0.0.1:18081`:
  `status`, `node up|down|restart|info|log|peers|bans|ban|unban|limit`.
- **Remote (public) node mode** (§2m, v5.2): `settings.node.mode = local | remote` plus
  `fmonero node remote ADDR [--trusted] --yes` and `fmonero node local --yes`. In remote mode
  **only `monero-wallet-rpc` ever talks to the remote node** — fmonero's own HTTP client stays
  loopback-plaintext-only (§2c, unchanged) — and every node command that needs the local daemon
  refuses with exit 4 `reason: remote_node`.
- **Wallet**, via `monero-wallet-rpc` run as a systemd **user** unit on `127.0.0.1:18083`:
  `wallet list|open|close|create|balance|address|new-address|history|send|sweep|`
  `validate|book|refresh|notes|clear-pending|set-daemon`.
- **One open wallet at a time**, one account (`account_index`, default 0) selectable per command.
- **Money safety**: `send`/`sweep` build with `do_not_relay` first, print the *real* fee, and relay
  that exact transaction only after `--yes` (§2a).
- **`settings get|set|list`** — fmonero's own `config.toml`.
- **`setup [--units] [--plugin] [--menu] [--all]`** — writes the two user units, links and enables
  the bar widget, merges the Omarchy menu entries.
- **`doctor [--fix] [--only] [--dry-run] [--save]`, `doctor undo`, `doctor capabilities`** (§7).
- **`tui`** — four screens: Node · Wallet · Send · Log (§7a).
- **Bar widget `oliver.monero`** (§6).
- **`capabilities`, `robot-docs`, `version`**; `scripts/{install,uninstall,check}.sh`;
  `tests/{e2e.sh,plugin-smoke.sh,fixtures/}`; `README.md`; generated `docs/ROBOT.md`.
- **Coexistence with Arch's system `monerod.service`**: detected, used read-only, never fought (§2i).

**CUT from v0.1 (do not build; no stub, no flag, no menu entry, no `--help` line).**

| Cut | Why |
|---|---|
| Multisig (`prepare/make/export/import/sign/submit_multisig`) | Money-critical, multi-party, untestable on one machine. |
| Hardware wallets (Ledger/Trezor) | Needs a device in the loop; `monero-wallet-rpc` support is a separate build path. |
| Mining (`start_mining`, `mining_status`, `generateblocks`) | Not what this tool is for; burns the CPU we exist to save. |
| Tor / i2p (`--proxy`, `--tx-proxy`, `--anonymous-inbound`) | A privacy feature that is worse than useless when half-done. v0.2. |
| Cuprate as a node backend | `sync_info`, `get_connections`, peer list, bans, txpool stats and net stats all return `not_available` (FEASIBILITY §5) — it cannot drive this dashboard. |
| The pure-Rust `monero-wallet` scanner (monero-oxide) | Explicitly "not a wallet": we would own persistence, output selection and reorgs, and it cannot spend under FCMP++ (FEASIBILITY §5, option C). |
| Several wallets open at once | `monero-wallet-rpc` holds exactly one; a second instance means a second port, a second unit and a second lock. |
| `transfer_split` — a send that needs more than one transaction | `transfer` refuses outright when the wallet would need more than one tx, with `-18 TX_TOO_LARGE` and the message "Transaction would be too large.  try /transfer_split." (`src/wallet/wallet_rpc_server.cpp:1275-1281`). v0.1 builds exactly one transaction and, on `-18`, tells the user to sweep or send a smaller amount (§1b.4). This is the ordinary outcome for a wallet holding many small outputs, so it is named here rather than discovered at send time. |
| **Remote node**: pinned TLS, node failover, a remote-node credential, and any claim about a remote node's own sync state | Remote node mode itself is **IN** since v5.2 (§2m); these four parts of it are not. *Pinned TLS* — `--daemon-ssl enabled` with `--daemon-ssl-ca-certificates` or `--daemon-ssl-allowed-fingerprints` (`wallet2.cpp:253-258`) — needs two more settings keys, a file path the doctor must check and a failure mode ("the node's certificate changed") with no safe automatic answer; v0.1 sends `ssl_support: autodetect`, which is opportunistic and **unauthenticated**, and §2m says so in those words rather than implying a private channel. *Failover* (a list of nodes, health-scored) needs a health signal that §1b.9 proves does not exist through `monero-wallet-rpc`. *A remote-node credential* (`username`/`password` on `set_daemon`) would put a third secret in `config.toml` for a facility public nodes do not offer. *Sync state*: no wallet-RPC method returns the daemon's height, target height or `synchronized` — `get_height` is the **wallet's** own (`wallet_rpc_server.cpp:943`) — so fmonero reports `node.reachable: null` in remote mode and never a number it did not receive (§2j). |
| ZMQ (`--zmq-pub`, `json-minimal-chain_main`) | A 1 s poll of three cheap calls is indistinguishable at this scale and needs no second transport. The units are written with `--no-zmq`. |
| Proofs (`get_tx_proof`, `get_reserve_proof`, `check_*`), `query_key`, `export_key_images`, key/seed display, `sign`/`verify`, `describe_transfer`, `sign_transfer`/`submit_transfer` (cold signing), `scan_tx`, `rescan_blockchain`, `rescan_spent`, account tags, freeze/thaw | Each is a separate screen or a separate safety story. `query_key` in particular is out of scope in v0.1: nothing in this codebase may call it. |
| A doctor screen inside the TUI; TUI mouse; desktop notifications wired on | fproton's v0.1 cut list, for the same reasons. |
| **`wallet restore` (seed entry)** | The only path that would carry a **seed** — the secret whose disclosure is total, irreversible loss — through fmonero's prompt, its error messages and its `--json` envelope, for a once-per-wallet operation that `monero-wallet-cli --restore-deterministic-wallet` already does with word-by-word seed validation and a restore-height prompt fmonero does not reproduce. It could never leave RED (§4b): a mock can record the request shape and nothing else. `wallet list` picks the wallet up the moment it exists in `wallet.dir`, and the empty-list output names the one command that creates it (§4.1). |
| **Sweep from the TUI** | CLI only in v0.1. The TUI's Send screen builds exactly one transaction, so its confirm modal never has to render a transaction list and `send::build_sweep` keeps one caller (§7a). `fmonero wallet sweep <addr> --dry-run` is the whole feature and it is AMBER in §4b. |
| CI workflow, CHANGELOG | Added at the first push. |

---

## 1. Facts about this machine (verified 2026-09-16)

### 1.1 Monero is installed; no node runs, and none is wanted

**Changed on 2026-09-17, and it invalidates assertions this document used to make.**
`/usr/bin/monerod`, `/usr/bin/monero-wallet-rpc` and `/usr/bin/monero-wallet-cli` are **present**
(Arch `extra/monero` **0.18.5.1-2**, `monerod --version` reports
`Monero 'Fluorine Fermi' (v0.18.5.1-release)`). `sudo` still needs a password, so no agent installs
or removes anything. The owner has **not** started a local node and does not want one: 120–150 GB
and days of initial sync (§1.3) buy nothing he needs, which is the whole reason §2m exists.

What that changes, and what it does not:

- **`monerod_installed`, `wallet_rpc_installed` and `version_supported` now `pass`** where they
  used to `fail:not_installed`. §4b's GREEN `doctor` row is restated for the machine as it is, and
  `tests/e2e.sh` asserts the new truth rather than the old one. Four of its assertions and one unit
  test failed on this tree **before** any v5.2 code was written, for exactly this reason; the
  transcript is in DECISIONS.md, because a suite that fails on a machine that got *better* is a
  suite that gets weakened (§2j).
- **Nothing that needs a running daemon moved.** No `monerod` is running, there is no
  `~/.bitmonero` chain, and every §4b RED row that needs a live local node stays RED. The §2i
  preflight's `not_installed` answer is still a real code path — it is what a machine without the
  package gets — and it is still tested, now through `units::preflight`'s own unit test rather than
  through this machine's accident.
- Everything that needs a live **remote** daemon is what §4b's new LIVE list covers (§2m): the
  wallet daemon is installed, so `monero-wallet-rpc` against a public node is reachable for the
  first time.
- No README bullet, `--help` line or `robot-docs` sentence may describe a RED row as working.
- Phase 0 fixtures are **derived from the Monero source at v0.18.5.1**, not captured from a run.
  Every derived file starts with
  `# derived from monero v0.18.5.1 source, not captured from a run` followed by `#` provenance
  lines naming the `file:line` each field came from. Consumers skip leading `#` lines. JSON
  fixtures carry the provenance in a top-level `"_provenance"` string that the loader deletes.
- The **mock JSON-RPC server** (§8, Phase 0) serves those fixtures, so a large part of the CLI is
  **AMBER** — exercised end to end against a real socket, with no claim that a real daemon agrees.

### 1.2 The Arch package's layout, which we must not fight

`extra/monero` 0.18.5.1-2 installs `/usr/bin/{monerod,monero-wallet-cli,monero-wallet-rpc}`, a
**system** unit `monerod.service` running as user `monero`, and `/etc/monerod.conf`. Upstream's own
templates are the model for both (`utils/systemd/monerod.service:1-20`, `utils/conf/monerod.conf:1-8`).
If that system unit is enabled or running, its `data-dir` (`/var/lib/monero` in upstream's
template) is **already open in LMDB by another process**. Two `monerod` on one data dir corrupt it.
So: fmonero never starts a second node on a data dir that a running node already serves (§2i), and
`doctor` reports the layout it found.

### 1.3 Disk, RAM, cores

238 GB free on `/home`, 30 GB RAM, 8 cores. A **pruned** mainnet chain is roughly a third to a half
of the full chain, which was about 290 GB in 2026 — call it **100–150 GB, AMBER: an estimate, not
measured on this machine**. `doctor` reports free space against a floor of **120 GB** for a pruned
chain plus 20 GB headroom and says in the detail that the figure is an estimate. A full (unpruned)
node does not fit and `setup` refuses to write a unit without `--prune-blockchain` unless the user
passes `--full-chain --yes`.

### 1.4 Toolchain and the frozen dependency set

Rust 1.98.0 **stable**. `ftui` 0.7.0 and `ftui-extras` 0.7.0 are in the local registry cache
(`~/.cargo/registry/src/index.crates.io-*/ftui-0.7.0`) and are what fproton builds against;
`ftui-extras`'s `charts` feature implies `canvas` (`ftui-extras-0.7.0/Cargo.toml:48-52`).
`ureq` 3.4.0 and 3.4.2 are cached; **`qrcode` and `md-5` are not** and will be fetched on the first
build — that is the only network the build needs. `fsqlite` and `asupersync` require nightly and
are banned (FEASIBILITY §4). Widget and runtime names that exist in 0.7 and that fproton already
imports, so no agent needs to guess: `ftui::{App, Cmd, Event, KeyCode, KeyEvent, KeyEventKind,
Model, Modifiers, Style, Frame, PackedRgba}`, `ftui::layout::{Constraint, Flex, Rect, Sides}`,
`ftui::text::{Line, Span, Text, wrap::WrapMode}`, `ftui::runtime::{Every, StopSignal, SubId,
Subscription}`, `ftui::widgets::{StatefulWidget, StatusItem, StatusLine, Tab, Tabs, TabsState,
Widget, block::Block, borders::BorderType, input::TextInput, paragraph::Paragraph,
progress::{MiniBar, MiniBarColors}, sparkline::Sparkline, table::{Row, Table, TableState},
virtualized::{RenderItem, VirtualizedList, VirtualizedListState}}`. `Table` and `MiniBar` are
**not** re-exported from the `ftui::widgets` root — import from `table::` and `progress::`;
**`VirtualizedList` is** at the root (`ftui-widgets-0.7.0/src/lib.rs:275-278`). Five further symbols
the TUI needs, each re-checked in the local registry on 2026-09-16 so this list stays falsifiable:

| Symbol | Import path | Verified at |
|---|---|---|
| `LogViewer`, `LogViewerState`, `LogWrapMode`, `SearchConfig`, `SearchMode` — the Log screen | `ftui::widgets` (root) | `ftui-widgets-0.7.0/src/lib.rs:269` |
| `Dialog`, `DialogBuilder`, `DialogButton`, `DialogConfig`, `DialogKind`, `DialogResult`, `DialogState` — every modal in §7a | `ftui::widgets::modal` — **not** the widgets root, which re-exports only `Modal`, `ModalConfig`, `ModalState`, `ModalAction`, `ModalPosition`, `BackdropConfig`, `ModalSizeConstraints` | `src/modal/mod.rs:63-66`; root list at `src/lib.rs:307-310` |
| `ProgramSimulator`, `SimulatorError` — the Phase 2 send test | `ftui::runtime::simulator` | `ftui-runtime-0.7.0/src/lib.rs:173` |
| `TextInput::with_mask(char)` — the masked password field (§2b.4) | `ftui::widgets::input` | `ftui-widgets-0.7.0/src/input.rs:217` |
| `Canvas`, `Mode::HalfBlock` — the receive QR | `ftui_extras::canvas` | `ftui-extras-0.7.0/src/canvas.rs:43-50`, `:1051` |

`hyperfine` and `/usr/bin/time` are **both absent** (verified 2026-09-16: neither
`/usr/bin/hyperfine` nor `/usr/bin/time` exists — the `time` package is not installed and `time` is
only a bash keyword, so `/usr/bin/time -f %e` fails with exit 127 and every timing gate that
prescribes it stalls on a missing tool). Measure with a 20-iteration loop around `date +%s%N`,
reporting mean and max, e.g.

```bash
S=$(date +%s%N); for i in $(seq 20); do fmonero status --json >/dev/null; done; E=$(date +%s%N)
echo "$(( (E-S)/20000000 ))ms mean"
```

and say so in DECISIONS.md — or `omarchy pkg add hyperfine` first and say that instead.

### 1.5 Omarchy plugin host (Quickshell) — the facts that bite

Identical to fproton §1.5, re-stated because a plugin agent reads only this file:

- Manifest `barWidget.defaults` / `schema` are **never merged into `settings`**. Enabling writes
  `{"id":"oliver.monero"}` into `~/.config/omarchy/shell.json` and the widget receives only inline
  keys. Every setting needs an in-code fallback duplicating the manifest value verbatim.
- A **symlinked plugin dir loads but does not hot-reload** (the registry watches with
  `inotifywait -r`, which does not descend symlinks). After every QML edit run
  `omarchy-shell shell rescanPlugins`.
- `omarchy-plugin-validate` refuses any symlink: validate the repo directory
  `plugin/oliver.monero`, never the installed link.
- `bar` is a `PluginBarApi` facade: `barForeground` (bar-drawn content), `foreground`, `urgent`,
  `fontFamily`, `barSize`, `requestPopout/releasePopout`, `showTooltip/hideTooltip`, and
  `run(cmd)` — which is `bash -lc <string>`: **never use it**. Always `Quickshell.execDetached(argv)`.
- Shell-string entry points, all three: `omarchy-launch-floating-terminal-with-presentation` joins
  `$*` under `bash -c`; `omarchy-launch-or-focus` ends in `eval exec setsid $LAUNCH_COMMAND`;
  `omarchy-launch-or-focus-tui [--app-id=ID] cmd args…` builds `LAUNCH_COMMAND="omarchy-launch-tui $@"`
  and feeds it to that `eval`. **Only constant literals may reach any of the three** — never an
  address, an amount, a label or anything derived from Status JSON or user input.
  `omarchy-launch-or-focus` matches `\b<pattern>\b` against window class **and** title, so app-ids
  must not be prefixes of one another: use `fmonero` for the TUI and `fmonerodoctor` (no hyphen)
  for the doctor. `omarchy-notification-send [-u low] [-g glyph] headline [body]` is genuinely
  argv-shaped and is the only one that may carry a computed string.
- The shell's PATH includes `~/.local/bin` but **not** `~/.cargo/bin`: install to
  `~/.local/bin/fmonero`.
- Every panel plugin logs a benign `WARN scene: QML IpcHandler … Handler was registered but will
  not be used` at reload; log-based smoke tests must whitelist it. Journal identifier is
  `omarchy-shell`; real failures look like `WARN qml: Plugin widget <id> failed: file:///…:L:C: <msg>`.
- Placement: `omarchy plugin enable oliver.monero` then
  `omarchy bar put oliver.monero --before omarchy.tailscale`.
- Theme colours live at `~/.local/state/omarchy/current/theme/colors.toml`; `src/tui/theme.rs` is
  copied verbatim from fproton and re-reads on mtime change.

### 1.6 Where the Monero source is

```
/tmp/claude-1000/-home-oliver--config-Claude-scratch-workspaces-eff1ae0e-d485-4f42-9ffb-620b5efc18d5-813ee62c-1eed-4c69-bc1f-0b109cdbca45-scratch-2026-09-16-3ded94/4901fef8-2906-4bc0-b226-799715c2e661/scratchpad/monero-src
```

A shallow clone at tag **v0.18.5.1**. Every `file:line` in §1b is relative to it. It is a
scratchpad, not a checkout the repo depends on: §1b is written so that nothing has to be re-read.
**Never read the 2 800-line RPC headers whole**; grep for the struct you need.

---

## 1b. Facts about Monero 0.18.5.1, from source

### 1b.1 Ports, names, units

| Fact | Value | Source |
|---|---|---|
| `CRYPTONOTE_NAME` | `bitmonero` — hence `~/.bitmonero`, `bitmonero.conf`, `bitmonero.log` | `src/cryptonote_config.h:165` |
| Default data dir | `$HOME/.bitmonero` (`testnet`/`stagenet`/`fake` appended for those nets) | `src/common/util.cpp:635-658`, `src/cryptonote_core/cryptonote_core.cpp:100-114` |
| Mainnet P2P / RPC / ZMQ ports | **18080 / 18081 / 18082** | `src/cryptonote_config.h:230-232` |
| Testnet | 28080 / 28081 / 28082 | `src/cryptonote_config.h:273-275` |
| Stagenet | 38080 / 38081 / 38082 | `src/cryptonote_config.h:288-290` |
| Atomic unit | `COIN = 1 000 000 000 000` — **1 XMR = 1e12 piconero**, exactly | `src/cryptonote_config.h:66-67` |
| Unlock: normal tx | `CRYPTONOTE_DEFAULT_TX_SPENDABLE_AGE = 10` confirmations | `src/cryptonote_config.h:49` |
| Unlock: coinbase | `CRYPTONOTE_MINED_MONEY_UNLOCK_WINDOW = 60` | `src/cryptonote_config.h:44` |
| Latest **activated** mainnet hard fork | v16 at height 2 689 608. The table ends there. The row's fourth field, 1 656 629 118 (2022-07-01), is `hardfork_t::time` (`src/hardforks/hardforks.h:34-41`), **not** the date that height was mined (mid-August 2022) — never render it as a block date. | `src/hardforks/hardforks.cpp:75` |
| Upstream systemd template | `User=monero`, `ExecStart=/usr/bin/monerod --config-file /etc/monerod.conf --non-interactive`, `Restart=always`, `WantedBy=multi-user.target` | `utils/systemd/monerod.service:1-20` |
| Upstream conf template | `data-dir=/var/lib/monero`, `log-file=/var/log/monero/monero.log`, `log-level=0` | `utils/conf/monerod.conf:1-8` |

**FCMP++ / Carrot.** Not activated: the mainnet fork table ends at v16 (`hardforks.cpp:75`), so
`hard_fork_info` will report `version: 16` until a v17 fork ships. FEASIBILITY §7 expects that fork
within about a year, with new "Carrot" address forms. **Design rule, enforced in §5b and §8:** the
address prefix table is one `const` array with one row per (network, kind) and the fixtures are one
JSON file with one object per row. Adding Carrot must be *one row plus one fixture object* — no
`if`, no new function, no change to any caller. A unit test asserts the table and the fixture file
have the same row count, so adding one without the other fails the build.

### 1b.2 Node RPC: two transports on one port, and the restricted-RPC gate

`monerod` serves **legacy `POST /<name>`** endpoints and **`POST /json_rpc`** on the same port
(`src/rpc/core_rpc_server.h:100` `BEGIN_URI_MAP2()`, `:143` `BEGIN_JSON_RPC_MAP("/json_rpc")`).
`MAP_*_IF(..., !m_restricted)` is the gate: with `--restricted-rpc` (or `--public-node`) the method
**is not registered at all**.

- A gated-out **json_rpc** method falls through to `END_JSON_RPC_MAP()`, which answers HTTP 200
  with `{"jsonrpc":"2.0","id":…,"error":{"code":-32601,"message":"Method not found"}}`
  (`contrib/epee/include/net/http_server_handlers_map2.h:280-285`).
- A gated-out **legacy** endpoint leaves `handled = false`
  (`http_server_handlers_map2.h:144`) and the epee server answers **HTTP 404**
  (`contrib/epee/include/net/http_protocol_handler.inl:664`).

So: **`sync_info` returning `-32601` is the restricted-RPC probe.** It is the cheapest unambiguous
one, it is what the node dashboard needs anyway, and it costs one call.

| Method | Transport | Restricted? | Source (`core_rpc_server.h`) |
|---|---|---|---|
| `get_info` | both `POST /get_info` and json_rpc `get_info` | open | `:132`, `:167` |
| `get_height` | `POST /get_height` | open | `:101` |
| `get_last_block_header` | json_rpc | open | `:156` |
| `hard_fork_info` | json_rpc | open | `:168` |
| `get_version` | json_rpc | open | `:174` |
| `get_fee_estimate` | json_rpc | open | `:176` |
| `get_transaction_pool_stats` | `POST /get_transaction_pool_stats` | open | `:129` |
| `get_limit` | `POST /get_limit` | open | `:135` |
| **`sync_info`** | json_rpc | **`!m_restricted`** | `:179` |
| **`get_connections`** | json_rpc | **`!m_restricted`** | `:166` |
| **`get_bans` / `set_bans`** | json_rpc | **`!m_restricted`** | `:170`, `:169` |
| **`get_peer_list`** | `POST /get_peer_list` | **`!m_restricted`** | `:121` |
| **`get_net_stats`** | `POST /get_net_stats` | **`!m_restricted`** | `:134` |
| **`set_limit`** | `POST /set_limit` | **`!m_restricted`** | `:136` |
| **`out_peers` / `in_peers`** | `POST /out_peers`, `/in_peers` | **`!m_restricted`** | `:137`, `:138` |
| **`stop_daemon`** | `POST /stop_daemon` | **`!m_restricted`** | `:131` |
| **`set_log_level`** | `POST /set_log_level` | **`!m_restricted`** | `:124` |

This is exactly why FEASIBILITY §5 says "local-first, degrade honestly for remote": against a
restricted node, half the Node screen has no data source, and fmonero must say so rather than
render zeros.

### 1b.3 Node RPC request/response fields we use

All from `src/rpc/core_rpc_server_commands_defs.h`.

- **Every daemon response carries `status` and `untrusted`** (`rpc_response_base`,
  `src/rpc/core_rpc_server_commands_defs.h:101-112`), inherited by the legacy `POST /<name>` body
  and by the `result` object of a json_rpc answer alike. `status` is the daemon's error channel for
  handlers that answer HTTP 200 with **no** JSON-RPC error: `"OK"` (`:78`), `"BUSY"` (`:79`),
  `"PAYMENT REQUIRED"` (`:81`), or free text such as `set_limit`'s `"Invalid parameter"`.
  **`CHECK_CORE_READY()` (`core_rpc_server.cpp:477`) sets `status: "BUSY"` and returns a
  value-initialised response — every numeric field zero, no error object.** `check_core_ready()` is
  just `m_p2p.get_payload_object().is_synchronized()` (`:451-458`), so `"BUSY"` is the answer for the
  **entire initial sync** — the state a fresh node spends days in and the state §2d, §4b's rehearsal
  and §7a's empty states are all written around. Of the calls fmonero makes, **exactly one is gated
  by it: `get_last_block_header` (`:2453`)**; `get_info`, `hard_fork_info`, `get_fee_estimate`,
  `get_transaction_pool_stats`, `sync_info`, `get_connections`, `get_peer_list`, `get_net_stats` and
  `get_limit` are not, which is why `get_info` is the reachability probe. **A `status` other than
  `"OK"` is not a result**: `rpc/jsonrpc.rs` inspects it on every node response *before*
  deserializing the payload, turns `"BUSY"` into `RpcError::Busy` (the caller renders `null`, never
  the zeros the daemon sent) and any other non-`"OK"` value into `Error::Upstream` with the text
  verbatim. Without this rule `node.top_block_age_sec` would render a block roughly 56 years old for
  the whole initial sync, which is precisely the zero-standing-in-for-unknown §5 and §2j forbid —
  and no fixture, mock flag or test in this plan would fail on it, because the mock's
  `get_last_block_header` fixture is a success fixture.
- **`GET_INFO` (`:696`)** — request empty. Response (`:709-745`): `height`, `target_height`,
  `difficulty` (+`wide_difficulty`, `difficulty_top64`), `target`, `tx_count`, `tx_pool_size`,
  `alt_blocks_count`, `outgoing_connections_count`, `incoming_connections_count`,
  `rpc_connections_count`, `white_peerlist_size`, `grey_peerlist_size`, `mainnet`/`testnet`/
  `stagenet`/`nettype`, `top_block_hash`, `cumulative_difficulty`, `block_size_limit`,
  `block_weight_limit`, `block_size_median`, `block_weight_median`, `adjusted_time`, `start_time`,
  `free_space`, `offline`, `bootstrap_daemon_address`, `height_without_bootstrap`,
  `was_bootstrap_ever_used`, `database_size`, `update_available`, `busy_syncing`, `version`,
  `synchronized`, **`restricted`**. Note `target_height` is **0 when synced**, not equal to
  `height` — the sync percentage must special-case it.
  **A restricted node poisons part of this response** rather than omitting it
  (`src/rpc/core_rpc_server.cpp:517` `const bool restricted = m_restricted && ctx;`):
  `start_time: 0` (`:547`), `free_space: 2^64-1` (`:548`), `version: ""` and
  `update_available: false` (`:568-569`), `alt_blocks_count: 0` (`:528`), all four
  connection/peer-list counts `0` (`:529-534`), `height_without_bootstrap: 0` and
  `bootstrap_daemon_address: ""` (`:550-556`), and `database_size` rounded **up** to a 5 GiB
  multiple (`:566-567`). `height`, `target_height`, `synchronized`, `busy_syncing`, `offline`,
  `nettype`, `adjusted_time`, `top_block_hash` and `restricted` itself are real.
  **`tx_pool_size` is a third case — truncated, not poisoned and not whole:** it is
  `m_core.get_pool_transactions_count(!restricted)` (`:527`), so under restriction the
  `include_sensitive` argument is `false` and non-relayed and local transactions are left out. It is
  a lower bound, it is rendered, and the restricted-node warning text says so.
  §5 turns every poisoned field into `null` — a 1970 uptime and a false "no peers" alarm are
  exactly the zeros §1b.2 forbids.
- **`GET_LAST_BLOCK_HEADER` (`:1217`)** — request `fill_pow_hash` (default false; leave it false,
  it is expensive). Response `block_header` = `block_header_response` (`:1166-1215`):
  `major_version`, `minor_version`, `timestamp`, `prev_hash`, `nonce`, `orphan_status`, `height`,
  `depth`, `hash`, `difficulty`, `cumulative_difficulty`, `reward`, `block_size`, `block_weight`,
  `num_txes`, `pow_hash`, `long_term_weight`, `miner_tx_hash`. `timestamp` gives "last block N s ago".
- **`SYNC_INFO` (`:2386`)** — response `height`, `target_height`, `next_needed_pruning_seed`,
  `peers[] { info: connection_info }`, `spans[] { start_block_height, nblocks, connection_id,
  rate, speed, size, remote_address }`, `overview`.
- **`GET_CONNECTIONS` (`:1737`)** — `connections[]` of `connection_info`
  (`src/cryptonote_protocol/cryptonote_protocol_defs.h:47-88`): `incoming`, `localhost`, `local_ip`,
  `ssl`, `address`, `host`, **`ip` (a decimal STRING, `:56`)**, **`port` (a STRING, `:57`)**,
  `rpc_port` (u16), `rpc_credits_per_hash`, **`peer_id` (a STRING, `:61`)**,
  `recv_count`, `recv_idle_time`, `send_count`, `send_idle_time`, `state`, `live_time`,
  `avg_download`, `current_download`, `avg_upload`, `current_upload`, `support_flags`,
  **`connection_id` (a STRING, `:81`)**, `height`, `pruning_seed`, `address_type`.
- **`GET_PEER_LIST` (`:1372`)** — request `public_only` (default **true**), `include_blocked`
  (default false). Response `white_list[]`, `gray_list[]` of `peer` (`:1338-1346`): `id` (u64),
  `host` (string), **`ip` (a u32 NUMBER, `:1341`)**, **`port` (a u16 NUMBER, `:1342`)**, `rpc_port`,
  `rpc_credits_per_hash`, `last_seen` (u64), `pruning_seed`. **`peer.port`/`peer.ip` are numbers
  while `connection_info.port`/`.ip` are strings — the two are not one type**, and §5b's default
  ("`u64` for counts") applied blindly to `Connection` fails to deserialize every row of
  `get_connections` against a real daemon. That is a RED row (§4b), so nothing before Phase 4 would
  find it; §5c therefore also says that `connections[].port` is parsed from the daemon's string and
  **rendered as an integer**, so both arrays of `node peers --json` agree.
- **`GET_BANS` (`:2000`)** — `bans[] { host, ip: u32, seconds }`.
  **`SET_BANS` (`:2035`)** — `bans[] { host, ip: u32, ban: bool, seconds }`. Unban = `ban: false`.
  Send `host` (a string) and leave `ip` 0; the server accepts either.
- **`GET_TRANSACTION_POOL_STATS` (`:1715`)** — `pool_stats` = `txpool_stats` (`:1680-1712`):
  `bytes_total`, `bytes_min`, `bytes_max`, `bytes_med`, `fee_total`, `oldest`, `txs_total`,
  `num_failing`, `num_10m`, `num_not_relayed`, `histo_98pc`, `histo[]`, `num_double_spends`.
- **`GET_BASE_FEE_ESTIMATE`** (json_rpc name **`get_fee_estimate`**, `:2253`) — request
  `grace_blocks`: **always send 10**, the `FEE_ESTIMATE_GRACE_BLOCKS` wallet2 itself uses
  (`src/wallet/wallet2.cpp:123`, applied at `:8597`), so the chips fmonero draws are computed
  exactly as the fee the build will charge; a call with `grace_blocks: 0` returns a different
  `fees[]` and the Send screen would show a fee beside a different fee. Response `fee` (per byte),
  `quantization_mask` (default 1), **`fees[]`** — the per-priority base fees. Use `fees[]` for the
  Send screen's priority chips; it is the node's own answer and needs no local multiplier table.
  Since HF 15 `on_get_base_fee_estimate` sets `res.fee = res.fees[0]`
  (`src/rpc/core_rpc_server.cpp:3011-3012`), so **`fee` is the cheapest tier, not a "normal" one**:
  §5 renders it as `per_byte_pico` beside `fees_pico` and no code may treat it as a default.
- **`GET_VERSION` (`:2173`)** — `version: u32` (packed major<<16|minor), `release: bool`,
  `current_height`, `target_height`, `hard_forks[] { hf_version, height }`.
- **`HARD_FORK_INFO` (`:1961`)** — request `version`; response `version`, `enabled`, `window`,
  `votes`, `threshold`, `voting`, `state`, `earliest_height`. **Two different fork versions arrive
  in one response, and conflating them renders a falsehood.** `on_hard_fork_info` computes
  `uint8_t version = req.version > 0 ? req.version : blockchain.get_next_hard_fork_version();` and
  uses that only for the voting fields, while `res.version` is unconditionally
  `blockchain.get_current_hard_fork_version()`
  (`src/rpc/core_rpc_server.cpp:2738-2741`). So `req.version == 0` means **the *next* fork** for
  `enabled`/`window`/`votes`/`threshold`/`earliest_height`/`voting`, and **the *current* fork** for
  `version` — the opposite of "0 = the current one". fmonero therefore reads only `version` and
  `state` from a `{version: 0}` call, and `node info` (§5c) publishes the voting fields under their
  own `voting_on` object carrying their own version. Without that split, the day a v17 row lands in
  the table `node info` would print `version: 16, enabled: false` side by side, which reads as
  "fork 16 is not enabled" and is exactly the rendered untruth §2j forbids.
- **`GET_NET_STATS` (`:796`)** — `start_time`, `total_packets_in`, `total_bytes_in`,
  `total_packets_out`, `total_bytes_out`. Rates are **our** job: two samples, kept in state.
- **`GET_LIMIT` (`:1855`)** — `limit_up`, `limit_down`.
  **`SET_LIMIT` (`:1879`)** — request `limit_down`, `limit_up`; response echoes both.
  **All limits are kB/s** (comment at `:1882`). `-1` resets to the default, `0` means "unchanged".
- **`OUT_PEERS` (`:1908`) / `IN_PEERS` (`:1935`)** — request `set` (default **true**) and the count;
  response echoes the count. Pass `set: false` to *read* without writing.
- **`STOP_DAEMON` (`:1817`)** — empty both ways.
- **`SET_LOG_LEVEL` (`:1475`)** — `level: i8`, 0–4.
- **`GET_HEIGHT` (`:138`)** — `height`, `hash`. Listed for completeness: fmonero has no consumer for
  it, because `get_info.height` is a hot-path field already (§5), so there is no `NodeClient` method
  (§5b rule 1).

### 1b.4 Wallet RPC

`WALLET_RPC_VERSION` is **1.31** (`src/wallet/wallet_rpc_server_commands_defs.h:49-50`);
`get_version` returns it packed as `major<<16|minor` (`:2554`). All line numbers below are that file.

| Method | Request fields we send | Response fields we read | Line |
|---|---|---|---|
| `get_balance` | `account_index`, `address_indices`, `all_accounts`, `strict` | `balance`, `unlocked_balance`, `multisig_import_needed`, **`blocks_to_unlock`**, **`time_to_unlock`**, `per_subaddress[] { account_index, address_index, address, balance, unlocked_balance, label, num_unspent_outputs, blocks_to_unlock, time_to_unlock }` | `:60`, per-subaddress `:77-99` |
| `get_address` | `account_index`, `address_index[]` | `address` (primary), `addresses[] { address, label, address_index, used }` | `:123` |
| `create_address` | `account_index`, `count` (default 1), `label` | `address`, `address_index`, `addresses[]`, `address_indices[]` | `:208` |
| `label_address` | `index { major, minor }`, `label` | — | `:241` |
| `get_accounts` | `tag`, `strict_balances` | `total_balance`, `total_unlocked_balance`, `subaddress_accounts[] { account_index, base_address, balance, unlocked_balance, label, tag }` | `:263` |
| `get_height` | — | `height` | `:455` |
| `get_transfers` | `in`,`out`,`pending`,`failed`,`pool`, **`filter_by_height` — mandatory for `min_height` to have any effect, see below**, `min_height`, `max_height`, `account_index`, `subaddr_indices`, `all_accounts` | five lists (`in`,`out`,`pending`,`failed`,`pool`) of `transfer_entry`; the `in` list also carries coinbase rows typed `block` | `:1601` |
| `get_transfer_by_txid` | `txid`, `account_index` | `transfer`, `transfers[]` | `:1653` |
| `transfer` | `destinations[] { amount, address }`, `account_index`, `subaddr_indices`, `priority`, `get_tx_key`, **`do_not_relay`**, `get_tx_hex`, **`get_tx_metadata`**. `unlock_time`, `payment_id`, `ring_size` and `outputs` are **never sent** (§2a.8) | `single_transfer_response` (`:567-594`): `tx_hash`, `tx_key`, `amount`, `amounts_by_dest`, **`fee`**, `weight`, `tx_blob`, **`tx_metadata`**, `multisig_txset`, `unsigned_txset`, `spent_key_images` | `:596` |
| `relay_tx` | **`hex`** | `tx_hash` | `:958` |
| `sweep_all` | `address`, `account_index`, **`subaddr_indices_all: true` — always sent, see §2a.10**, `priority`, `get_tx_keys`, `below_amount`, `do_not_relay`, `get_tx_hex`, `get_tx_metadata`. `unlock_time`, `payment_id`, `ring_size` and `outputs` are **never sent** (§2a.8) | `split_transfer_response` (`:633-662`): **lists** — `tx_hash_list`, `fee_list`, `amount_list`, `weight_list`, `tx_metadata_list`, … | `:880` |
| `validate_address` | `address`, **`any_net_type: true`**, `allow_openalias` (**leave false — it does DNS**) | `valid`, `integrated`, `subaddress`, `nettype`, `openalias_address` | `:2576` |
| `refresh` | `start_height` | `blocks_fetched`, `received_money` | `:2054` |
| `auto_refresh` | `enable`, `period` (seconds) | — | `:2079` |
| `open_wallet` | `filename`, `password`, `autosave_current` (default true) | — | `:2208` |
| `close_wallet` | `autosave_current` | — | `:2232` |
| `create_wallet` | `filename`, `password`, `language` (**required, and an unknown value is rejected**: `is_valid_language(req.language)` fails with **`-1 UNKNOWN_ERROR` "Unknown language: <x>"**, `src/wallet/wallet_rpc_server.cpp:3576-3581`, which under the error map below would fall to exit 5 — so fmonero validates the name locally first, against `SEED_LANGUAGES` below, and defaults to `"English"`). **`req.password` is taken verbatim with no emptiness check** (`:3583-3595`, `:3611`) | — | `:2184` |
| `restore_deterministic_wallet` | `restore_height`, `filename`, `seed`, `seed_offset`, `password`, `language`, `autosave_current` | `address`, `seed`, `info`, `was_deprecated` | `:2311` |
| `stop_wallet` | — | — | `:1201` |
| `store` | — | — | `:981` |
| `get_version` | — | `version`, `release` | `:2554` |
| `make_uri` | `uri_spec` (`:1868`): `address`, `payment_id`, `amount`, `tx_description`, `recipient_name` | `uri` | `:1885` |
| `parse_uri` | `uri` | `uri: uri_spec`, `unknown_parameters[]` | `:1903` |
| `get_address_book` | `entries[]` (empty = all) | `entries[] { index, address, description }` | `:1981` |
| `add_address_book` | `address`, `description` | `index` | `:1928` |
| `delete_address_book` | `index` | — | `:2017` |
| `set_daemon` | `address`, `trusted`, `ssl_support` (`disabled`\|`enabled`\|`autodetect`), and `username`/`password` **only in local mode**. `proxy`, `ssl_private_key_path`, `ssl_certificate_path`, `ssl_ca_file`, `ssl_allowed_fingerprints` and `ssl_allow_any_cert` are **never sent** (§0's cut list, §2m) | — (an empty response object, `:2643-2648`) | `:2611-2649`; `trusted` is `KV_SERIALIZE_OPT(…, false)` at `:2631` and `ssl_support` is `KV_SERIALIZE_OPT(…, "autodetect")` at `:2632`, so **an omitted `ssl_support` means autodetect, not plaintext** — see §1b.9 |
| `setup_background_sync` | `background_sync_type` (`off` \| `reuse-wallet-password` \| `custom-background-password`), `wallet_password`, `background_cache_password` | — | `:2725` |
| `start_background_sync` | — | — | `:2749` |
| `stop_background_sync` | `wallet_password`, `seed`, `seed_offset` | — | `:2766` |
| `estimate_tx_size_and_weight` | `n_inputs`, `n_outputs`, `ring_size`, `rct` | `size`, `weight` | `:2694` |
| `get_default_fee_priority` | — | `priority` | `:2790` |
| `set_tx_notes` / `get_tx_notes` | `txids[]` (+ `notes[]`) | `notes[]` | `:1238`, `:1260` |

**The accepted `language` values, so nothing is guessed.** `is_valid_language` returns true when the
string equals **either** the native name **or** the English name of one of the twelve languages in
`get_language_list()` (`src/mnemonics/electrum-words.cpp:437-452`, the predicate at `:494-500`:
`get_english_language_name() == language || get_language_name() == language`). Several native names
are non-ASCII, and `EnglishOld` is a `Language` class but is **not** in that list and is rejected —
so an agent that implements the obvious list (English names only, or native names only) refuses half
the values the daemon accepts, with exit 2, locally: the failure mode this pre-check exists to
avoid, inverted. `address.rs`-style, this is one `const` array in `settings.rs`:

```rust
/// (native, english) — monero v0.18.5.1 electrum-words.cpp:437-452; EITHER spelling is accepted
/// by `is_valid_language` (:494-500). Order as upstream. `EnglishOld` is deliberately absent.
pub const SEED_LANGUAGES: &[(&str, &str)] = &[
    ("Deutsch", "German"), ("English", "English"), ("Español", "Spanish"),
    ("Français", "French"), ("Italiano", "Italian"), ("Nederlands", "Dutch"),
    ("Português", "Portuguese"), ("русский язык", "Russian"), ("日本語", "Japanese"),
    ("简体中文 (中国)", "Chinese (simplified)"), ("Esperanto", "Esperanto"),
    ("Lojban", "Lojban"),
];
```

The match is byte-exact (upstream's is `==`), the default is `"English"`, and an unmatched value
exits **2 `usage`** listing the twelve English names.

Rows in that table are the wallet-RPC surface as it exists, not a list of calls fmonero makes:
`make_uri`, `parse_uri`, `estimate_tx_size_and_weight`, `get_default_fee_priority`, `stop_wallet`,
**`restore_deterministic_wallet`** (cut from v0.1, §0) and the three `*_background_sync`
methods have **no** consumer in §4.1, §5, §7a or §7.1 and
therefore no method in the frozen API (§5b rule 1). They stay here because a later version will want
them and re-deriving the line numbers costs more than a row.

**`transfer_entry` (`:1446-1487`)**, the row behind every history view: `txid`, `payment_id`,
`height`, `timestamp`, `amount`, `amounts[]`, `fee`, `note`, `destinations[]`, **`type`**
(`in`/`out`/`pending`/`failed`/`pool`/`block`), `unlock_time`, **`locked`**, `subaddr_index`,
`subaddr_indices[]`, `address`, `double_spend_seen`, **`confirmations`**,
`suggested_confirmations_threshold`.

**`subaddr_index` is a JSON object, not a number.** It is a `cryptonote::subaddress_index`
(`:1460`), KV-serialized as `{"major": N, "minor": M}`
(`src/cryptonote_basic/subaddress_index.h:38-55`); `subaddr_indices[]` (`:1461`) is a list of those,
and `label_address`'s request `index` is the same type. `major` is the account index, `minor` the
subaddress index. A serde field typed `u32` against that key fails to deserialize every history row,
which is why §5b's `Transfer` carries a `SubaddrIndex { major, minor }`.

**`type` has SIX values, not five.** `fill_transfer_entry` types an incoming row
`pd.m_coinbase ? "block" : "in"` (`src/wallet/wallet_rpc_server.cpp:501`) and pushes it into the
**`in`** list, so `"block"` is an ordinary member of `get_transfers`' `in` array for any wallet
holding a mined or p2pool output — and p2pool pays miners directly in the coinbase. A five-variant
`TransferKind` makes serde fail on the *whole* `get_transfers` response, so `wallet history` exits 1
and the §2a.4 resolve step cannot complete, which under §2a.4's own rule leaves the pending record
in place and blocks every later send. §5b's `TransferKind` therefore carries `Block`, and Phase 0's
`get_transfers.json` fixture carries a `block` row.

**`destinations[]` (`:1456`, KV-serialized at `:1476`)** is a list of `transfer_destination`
(`{amount, address}`) and is empty for `in` rows. It is the one fact a user checks after a send, so
§5b's `Transfer` carries it and §7a's detail pane renders it.

**`confirmations`, `locked` and `suggested_confirmations_threshold` are derived per response, not
stored, and they are derived against the WALLET's height — never the node's.** `wallet-rpc`
recomputes them on every call from `m_wallet->get_blockchain_current_height()`:
`set_confirmations(entry, m_wallet->get_blockchain_current_height(), …)`
(`src/wallet/wallet_rpc_server.cpp:162`, called at `:505`, `:535`, `:566`, `:589`) and
`entry.locked = !m_wallet->is_transfer_unlocked(...)` (`:498`, `:517`; unconditionally `true` for
pending and pool rows, `:551`, `:581`). A cached copy is wrong one block later, which is why §5b's
history snapshot stores neither. **Anything fmonero recomputes must use the same clock**: a wallet
that is behind the node — the normal state for hours after `wallet open`, which §2d devotes a whole
bullet to — would otherwise be told a payment is more settled than its own chain can see, which is a
wrong statement about money in exactly the state this design calls common.

The two formulas, so that nothing has to be re-derived (`wh` = the wallet's current height):

- **`confirmations`** (`:162-167`): `0` when `height >= wh`, and `0` for a `pending`/`pool` row with
  `height == 0`; otherwise `wh - height`.
- **`locked`** = `!is_transfer_unlocked`, and `is_transfer_unlocked` is a **conjunction of two
  independent conditions** (`src/wallet/wallet2.cpp:7308-7317`), not a maximum:
  - *age*: `height + CRYPTONOTE_DEFAULT_TX_SPENDABLE_AGE (10) <= wh` (`:7313-7314`);
  - *spend time* (`is_tx_spendtime_unlocked`, `:7319-7344`): `unlock_time == 0` ⇒ unlocked;
    else if `unlock_time < CRYPTONOTE_MAX_BLOCK_NUMBER` (**500 000 000**,
    `src/cryptonote_config.h:40`) it is an **absolute block index** and the condition is
    `wh - 1 + CRYPTONOTE_LOCKED_TX_ALLOWED_DELTA_BLOCKS (1) >= unlock_time`, i.e. `wh >= unlock_time`;
    else it is a **unix timestamp** and the condition is `adjusted_now + leeway >= unlock_time`,
    where `adjusted_now` is `get_daemon_adjusted_time()` with **local time as the fallback**
    (`wallet2.cpp:7331-7333` — and local time is what fmonero uses, since `wallet history` contacts
    no node, §4.1) and `leeway` is **60 s for a row below the V2 height** (mainnet
    1 009 827, testnet 624 634, stagenet 32 000) **and 120 s above it** (`wallet2.cpp:7336-7337`,
    `src/cryptonote_config.h:88-90`). Every row fmonero will ever show on mainnet is above that
    height, so fmonero uses **120 s** and says so. §5b's code form of this formula says
    `adjusted_now`, not `now_unix`, so the two statements of one formula in this document cannot
    differ; the doctor's `node_clock` check (§7.1) is what makes the local clock trustworthy, and
    the column is labelled `locked (est.)`.

  A **coinbase (`type: "block"`) row needs no special age rule**: upstream's
  `is_transfer_unlocked` uses `CRYPTONOTE_DEFAULT_TX_SPENDABLE_AGE (10)` unconditionally
  (`wallet2.cpp:7308-7317`), and the 60-block mined-money window arrives through `unlock_time`,
  which a miner transaction sets to an absolute height — already handled by the height branch above.
  A second, coinbase-specific `60` would double-count it.

  `unlock_time` is therefore **never a delta added to `height`**. Treating it as one marks every
  timestamp-unlocked incoming transfer — which fmonero cannot prevent, since it only controls what
  it sends — locked until the year 55 000.

**`get_transfers`' height filter is inert unless `filter_by_height: true` is sent.**
`on_get_transfers` reads `min_height`/`max_height` **only** inside `if (req.filter_by_height)`
(`src/wallet/wallet_rpc_server.cpp:2890-2895`), and `KV_SERIALIZE(filter_by_height)`
(`wallet_rpc_server_commands_defs.h:1611`, `:1624`) is not an `_OPT`, so an omitted key
value-initialises the flag to `false` and the bounds silently become
`0 .. CRYPTONOTE_MAX_BLOCK_NUMBER`. A request that sets only `min_height` therefore gets the **whole
history**, correct-looking either way — the difference between the §2l (d) budget and a full
download, and a `--since HEIGHT` that quietly returns rows below the height the user asked for. A
fixture mock cannot catch it (it serves the same file regardless), so the flag is part of the
contract: `TransferQuery` carries an explicit `filter_by_height: bool` (§5b) and
`history::refresh` always sends it `true` alongside `min_height`. `max_height` is left unset — its
`KV_SERIALIZE_OPT` default is already `CRYPTONOTE_MAX_BLOCK_NUMBER` (`:1626`).

**The height filter applies only to `in` and `out`.** `min_height`/`max_height` are
read once (`:2890-2895`) and used in the `req.in` (`:2905`) and `req.out` (`:2915`) branches only;
`pending`/`failed` (`:2925-2937`) and `pool` (`:2939-2952`) are **always returned in full**, and the
`pool` branch runs `update_pool_state`/`process_pool_state` — a daemon round trip. An incremental
top-up therefore replaces those three lists wholesale and never caches them (§5b `history`).

**The fact the whole send flow rests on:** `relay_tx`'s `hex` is **not** a raw transaction blob. It
is hex of a **serialized `wallet2::pending_tx`** — exactly what `transfer` returns in
`tx_metadata` when `get_tx_metadata: true` (`src/wallet/wallet_rpc_server.cpp:1914-1943`,
where the hex is parsed into `tools::wallet2::pending_tx`). `tx_blob` is the wire transaction and
is **not** what `relay_tx` wants. `relay_tx` is also refused in restricted wallet mode
(`wallet_rpc_server.cpp:1905-1910`), which is one more reason `--restricted-rpc` is never set on
our wallet unit.

**Fee priorities. `0` is the *lowest* tier, not a "default" that means normal.** Since HF 15
(`HF_VERSION_2021_SCALING`, `src/cryptonote_config.h:194`; mainnet has been at v16 since 2022,
`src/hardforks/hardforks.cpp:75`) `wallet2::get_base_fee(priority)` takes the 2021-scaling branch
(`src/wallet/wallet2.cpp:8584-8609`): it clamps `0` → `1` and `> 4` → `4`, decrements, and returns
`fees[priority]` straight from the daemon's `get_fee_estimate.fees[]` — so **`priority: 0` behaves
exactly as priority 1, the cheapest level**. `on_transfer` first runs `adjust_priority`, which raises
0 to 2 only when `m_default_priority == 0`, `auto_low_priority()` and `estimate_backlog` together
report a backlogged pool (`wallet2.cpp:8688-8708`). The `{1, 5, 25, 1000}` multiplier table
(`wallet2.cpp:8518-8527`) is the **pre-2021** path (`get_fee_multiplier`) and does not apply on
mainnet; it is where the old "0 resolves to 2" reading came from.

fmonero therefore accepts `--priority 0..4`, labels them
`0 = lowest (an alias of 1) · 1 low · 2 normal · 3 high · 4 highest`, **defaults
`wallet.priority` to `2`** (§4) rather than to 0, and prints the node's own `fees[]` entry the chosen
level maps to next to the chips rather than reimplementing any multiplier table. The node's `fees[]`
is the authority; its four entries are the four levels, in order.

**Wallet RPC error codes** (`src/wallet/wallet_rpc_server_error_codes.h`), the ones fmonero maps:

| Code | Name | fmonero exit |
|---|---|---|
| -2 | `WRONG_ADDRESS` | 15 `invalid_address` |
| -4 | `GENERIC_TRANSFER_ERROR` | 16 `tx_rejected` |
| -5 | `WRONG_PAYMENT_ID` — including "Standalone payment IDs are obsolete" (`wallet_rpc_server.cpp:1106-1107`) | 2 `usage` — hint: use an integrated address |
| -8 | `WRONG_TXID`. In `get_transfer_by_txid` the message **"Transaction not found."** (`wallet_rpc_server.cpp:3045-3046`) is the normal "not in this wallet yet" answer and becomes `Ok(None)`, never an error (§5b). The "Transaction ID has invalid format/size" variants (`:2971-2972`, `:2982-2983`) are caller errors. | `Ok(None)` / 2 `usage` |
| -12 | `WRONG_INDEX` | 2 `usage` |
| -13 | `NOT_OPEN` | 9 `wallet_locked` |
| -14 / -15 | `ACCOUNT_INDEX_OUT_OF_BOUNDS`, `ADDRESS_INDEX_OUT_OF_BOUNDS` | 2 `usage` — `--account`/`--index` names a subaddress the wallet does not have |
| -16 | `TX_NOT_POSSIBLE` | 16 `tx_rejected` |
| -17 | `NOT_ENOUGH_MONEY` | 14 `insufficient_funds` |
| -18 | `TX_TOO_LARGE` — "this send needs more than one transaction" (`wallet_rpc_server.cpp:1275-1281`), the ordinary outcome on a wallet of many small outputs | 16 `tx_rejected`, with its **own** hint: `fmonero wallet sweep <same address> --below <amount> --dry-run   # v0.1 builds one transaction; this send needs more` |
| -19 | `NOT_ENOUGH_OUTS_TO_MIX` | 16 `tx_rejected` |
| -20 | `ZERO_DESTINATION`, -46 `ZERO_AMOUNT` | 2 `usage` |
| -21 | `WALLET_ALREADY_EXISTS` | 3 `safety` |
| -22 | `INVALID_PASSWORD` — **`open_wallet` never returns it.** `on_open_wallet` lets `handle_rpc_exception` set the right code and then overwrites it: `er.code = WALLET_RPC_ERROR_CODE_UNKNOWN_ERROR; er.message = "Failed to open wallet : " + er.message;` (`wallet_rpc_server.cpp:3705-3712`). So a wrong wallet password arrives as **-1**. -22 reaches fmonero only from `change_wallet_password`, which v0.1 does not call. | 9 `wallet_locked` |
| -1 | `UNKNOWN_ERROR` **from `open_wallet`**. That call can also answer **`-23 NO_WALLET_DIR`** (`wallet_rpc_server.cpp:3651-3656`) and, in restricted wallet mode, **`-7 DENIED`** (`:3645-3650`), both *before* it reaches the `-1` overwrite; both are mapped elsewhere in this table and **keep their own exits** (4 `backend`). Every *other* failure of the call collapses into -1, because `on_open_wallet` overwrites whatever `handle_rpc_exception` set (`:3705-3712`). fmonero pre-checks that `<wallet.dir>/<NAME>.keys` exists (exit **6 `not_found`** if not), so a -1 that survives that check unambiguously means a bad password or unreadable keys. Mapping *everything* from that call to exit 9 would tell a user with a misconfigured unit to "open the wallet". | 9 `wallet_locked`, message verbatim |
| -23 | `NO_WALLET_DIR` | 4 `backend` |
| -26 | `BAD_HEX`, -27 `BAD_TX_METADATA` | 1 `failure` |
| -29 | `WATCH_ONLY` | 3 `safety` |
| -37 | `NOT_ENOUGH_UNLOCKED_MONEY` | 14 `insufficient_funds` |
| -38 | `NO_DAEMON_CONNECTION` | 4 `backend` |
| -7 | `DENIED`, -48 `DISABLED` | 4 `backend` |
| -50 | `NONZERO_UNLOCK_TIME` (`wallet_rpc_server_error_codes.h:83`; raised at `wallet_rpc_server.cpp:1249`, `:1309`, `:1762`, `:1828` **before** any other validation) | 1 `failure` — fmonero never sends the field, so this means our request builder is wrong (§2a.8) |
| -51/-52 | `IS_BACKGROUND_WALLET`, `IS_BACKGROUND_SYNCING` | 9 `wallet_locked` |
| 0 | *no name — epee's catch-all.* `MAP_JON_RPC_WE_IF` wraps every handler in `try { res = callback_f(…); } catch (const std::exception &e) { MERROR(…) }` over a value-initialised `fail_resp` (`contrib/epee/include/net/http_server_handlers_map2.h:213-232`, the `AUTO_VAL_INIT` at `:217`, the catch at `:223`), so an exception the handler did not catch itself is serialized as `{"error":{"code":0,"message":""}}`. **`on_get_transfers` (`wallet_rpc_server.cpp:2880-2955`) and `on_get_transfer_by_txid` (`:2957-3048`) have no try/catch of their own**, and both call `update_pool_state` (`:2942`, `:3025`), which throws whenever wallet-rpc cannot reach the daemon (`THROW_ON_RPC_RESPONSE_ERROR` in `update_pool_state_by_pool_query`, `src/wallet/wallet2.cpp:3776-3794`). So this is the ordinary answer to a §2a.4 resolve step with the node down, not a corner case. | 5 `upstream`, `reason: rpc_error`, with a **synthesised** message: `"monero-wallet-rpc failed without a message (usually: it cannot reach the daemon)"` and `hint: fmonero status --json`. A verbatim empty `error` string would break §4's envelope and is the §2j failure — an error report that reports nothing. |
| anything else | — | 5 `upstream`, message verbatim |

### 1b.5 `monero-wallet-rpc` command line

All from `src/wallet/wallet_rpc_server.cpp` unless noted.

- **`--rpc-bind-port` is required and has no default** — the descriptor is
  `arg_descriptor<std::string, true>`, where `true` is *required* (`:139`). There is no port to
  guess; fmonero picks one and writes it into the unit (§3, §5b).
- `--disable-rpc-login` (`:140`) and `--rpc-login user[:pass]` (`src/rpc/rpc_args.cpp:98`) are
  **mutually exclusive**; passing both is a fatal error (`:341-349`).
- **If neither is given, `monero-wallet-rpc` generates its own credential**: username `monero`
  (`:150`) and a base64'd 128 random bits, written as `<user>:<pass>` to
  `monero-wallet-rpc.<bind_port>.login` **relative to the process working directory** (`:351-380`).
  It is created through `tools::private_file`, which opens with `S_IRUSR`, raises the mode to
  `S_IRUSR|S_IWUSR` only to get a write handle and then restores it
  (`src/common/util.cpp:215-224`) — so the file settles at **0400** — and whose destructor
  **deletes it on exit** (`src/common/util.cpp:263`). `drop_and_recreate` (`:246`) removes a stale
  one at startup, so a lingering file means the previous run was `SIGKILL`ed. This is the single
  most useful fact in this section; §2c builds on it.
- `--restricted-rpc` (`:141`) turns the wallet view-only and blocks `relay_tx`. Never set it.
- **A wallet `filename` may not contain `/`.** `on_open_wallet`, `on_create_wallet`,
  `on_restore_deterministic_wallet` and `on_generate_from_keys` each run
  `strchr(req.filename.c_str(), '/')` and answer `-1 UNKNOWN_ERROR` "Invalid filename"
  (`:3571`, `:3670`, `:3900`, `:4076`); the wallet file is always `<wallet-dir>/<filename>`.
  fmonero rejects such a name locally with exit 2 rather than round-tripping it.
- `--wallet-dir DIR` (`:142`) and `--wallet-file FILE` are **incompatible** — the check is at
  `:318-324` and nowhere else (`:4903` is `if (!wallet_dir.empty())`, and `:4905` is `wal = NULL;`);
  `--password-file` is **not allowed with `--wallet-dir`** (`:4897-4901`). Exactly one of
  `--wallet-file` / `--generate-from-json` / `--wallet-dir` must be given (`:4909-4913`); the usage
  line is at `:5083`. With `--wallet-dir` in force the wallet file is **always**
  `m_wallet_dir + "/" + req.filename` (`:3685` for `open_wallet`, `:3574`, `:3903`, `:4079` for the
  others) — the daemon resolves it, fmonero does not, which is why `settings.wallet.dir` and the
  unit's `--wallet-dir` must name the same directory byte for byte (§4.2).
- `--daemon-address <host>:<port>` (`src/wallet/wallet2.cpp:244`), `--trusted-daemon` /
  `--untrusted-daemon` (`:247`, `:248`), `--password` (`:249` — **never used by fmonero**, it would
  put the wallet password in argv), `--password-file` (`:250`), `--offline` (`:279`).
- `--rpc-bind-ip` defaults to **`127.0.0.1`** (`src/rpc/rpc_args.cpp:92`); binding elsewhere needs
  `--confirm-external-bind` (`:99`). `--rpc-access-control-origins` defaults to empty (`:100`), so
  no CORS headers are sent. `--rpc-ssl` defaults to `autodetect` (`:101`).
- `--log-file` (`src/wallet/wallet_args.cpp:114`), `--log-level` (`:110`), `--max-concurrency`
  (`:113`), `--config-file` (`:115`).
- `--non-interactive` and `--detach`/`--pidfile` come from the daemonizer
  (`src/daemonizer/posix_daemonizer.inl:42-53`). Under systemd we use `--non-interactive` and
  **never** `--detach`.

### 1b.6 `monerod` command line

| Flag | Default / note | Source |
|---|---|---|
| `--data-dir` | `$HOME/.bitmonero` | `src/cryptonote_core/cryptonote_core.cpp:100` |
| `--config-file` | `<data-dir>/bitmonero.conf`; **argv wins over the file**, because `po::store` is called on argv first and boost never overwrites an already-set value (`src/daemon/main.cpp:215-241`). A missing *default* config file is fine; a missing *named* one is fatal (`:243-247`). | `src/daemon/command_line_args.h:40-53` |
| `--prune-blockchain` | false | `src/cryptonote_core/cryptonote_core.cpp:193` |
| `--rpc-bind-ip` / `--rpc-bind-port` | `127.0.0.1` / 18081 | `src/rpc/rpc_args.cpp:92`, `src/rpc/core_rpc_server.cpp:3717` |
| `--restricted-rpc` | false | `src/rpc/core_rpc_server.cpp:3737` |
| `--rpc-restricted-bind-port` | unset; when set, that port serves the restricted view | `src/rpc/core_rpc_server.cpp:3731` |
| `--rpc-login user[:pass]` | unset. **`monerod` does *not* auto-generate one** — `http_login` is populated only when `rpc_config->login` is set (`src/rpc/core_rpc_server.cpp:349-350`). | `src/rpc/rpc_args.cpp:98` |
| `--public-node` | false; "restricted RPC mode, view-only commands" and advertises over P2P | `src/daemon/command_line_args.h:109-113` |
| `--zmq-pub ADDR` / `--no-zmq` | ZMQ pub off unless asked / disable the ZMQ RPC server | `src/daemon/command_line_args.h:134-137`, `:151-154` |
| `--out-peers` / `--in-peers` | both `-1` (= auto) | `src/p2p/net_node.cpp:168-169` |
| `--limit-rate-up` / `--limit-rate-down` / `--limit-rate` | kB/s; `-1` for `--limit-rate` | `src/p2p/net_node.cpp:172-174` |
| `--no-sync` | false — the "simple mode" trick monero-gui uses | `src/p2p/net_node.cpp:161` |
| `--offline` | false | `src/cryptonote_core/cryptonote_core.cpp:116` |
| `--log-file` / `--log-level` | `<data-dir>/bitmonero.log` / "" | `src/daemon/command_line_args.h:55-68`, `:80-84` |
| `--max-concurrency` | 0 (= all cores) | `src/daemon/command_line_args.h:93-97` |
| `--db-sync-mode` | `fast:async:250000000bytes` | `src/blockchain_db/blockchain_db.cpp:153-157` |
| `--check-updates` | `notify` | `src/cryptonote_core/cryptonote_core.cpp:173` |
| `--bootstrap-daemon-address` | unset; `auto` discovers public nodes | `src/rpc/core_rpc_server.cpp:3743` |
| `--non-interactive` / `--detach` / `--pidfile` | daemonizer | `src/daemonizer/posix_daemonizer.inl:42-53` |
| `--sync-pruned-blocks` | false | `src/cryptonote_core/cryptonote_core.cpp:129` |

### 1b.7 HTTP authentication is MD5 Digest, and only MD5

`digest_algorithms` is a one-element tuple containing `md5_`
(`contrib/epee/src/http_auth.cpp:155`), whose advertised name is `"MD5"` (`:108-110`) and whose
implementation is `EVP_md5()` (`:144`). There is no SHA-256 variant. Four facts decide how
`rpc/digest.rs` must be written (§2c):

1. **A 401 carries two `WWW-Authenticate` headers, not one.** `add_challenge` loops
   `for (unsigned i = 0; i < 2; ++i)` and emits one per algorithm name — `algorithm=MD5` and
   `algorithm=MD5-sess` (`:665-691`). Both are accepted by the server, and `-sess` is a *different*
   HA1 (`key = md5(key:nonce:cnonce)`, `:548-551`), so a client that reads "the" header and lands on
   the second one fails every request. A client must iterate all of them and take `MD5`.
2. **`qop=auth` is unconditional.** The challenge is built from
   `fvalue = "Digest qop=\"auth\""` (`:671`), so the RFC 2069 no-`qop` branch (`:250-268`) is
   server-side dead code against `monerod`/`monero-wallet-rpc`. fmonero does not implement it (§2h).
3. **`nc` is per nonce, not per client.** The server zeroes its own counter whenever it issues a new
   nonce (`user->counter = 0;` immediately before `create_digest_response`, `:749`), increments
   it before each verify (`++(user->counter)`, `:734`), and then requires
   `parsed->counter() == user.counter` (`:335-345`). So the first request on a *new* nonce must
   carry `nc=00000001`; a monotonic client counter that is not reset on a nonce change mismatches
   forever, and each mismatch costs a `stale` 401 plus a retry.
4. **The server never sends `opaque`.** The challenge builder emits only `qop`, `algorithm`,
   `realm`, `nonce` and `stale` (`:676-683`); `opaque` appears only on the client side and only when
   non-empty (`:246-247`).

The session that holds the counter and the nonce is `m_auth`, a member of the per-connection HTTP
protocol handler (`contrib/epee/include/net/http_protocol_handler.h:186`, `:223`) — **it is
per connection**, so a second pooled connection starts with no nonce and answers 401 once.

### 1b.8 Address forms

Prefixes (`src/cryptonote_config.h:227-229` mainnet, `:270-272` testnet, `:285-287` stagenet):

| Network | standard | integrated | subaddress |
|---|---|---|---|
| mainnet | 18 | 19 | 42 |
| testnet | 53 | 54 | 63 |
| stagenet | 24 | 25 | 36 |

Lengths follow from Monero's base58: blocks of 8 bytes encode to 11 characters, and a trailing
partial block of 5 bytes encodes to 7 (`encoded_block_sizes[] = {0,2,3,5,6,7,9,10,11}`,
`src/common/base58.cpp:48`; alphabet at `:46`). A standard/subaddress payload is 1 prefix byte + 32 public spend +
32 public view + 4 checksum = **69 bytes → 8×11 + 7 = 95 characters**. An integrated address adds
an 8-byte payment id = **77 bytes → 9×11 + 7 = 106 characters**.

**The leading character is a *set*, not a character.** It is the base58 digit of the whole first
8-byte block — the prefix varint plus the first seven bytes of the public spend key — so for some
prefixes the block value straddles a digit boundary. Dividing each prefix's block range
`[p<<56, (p<<56)|0x00FF_FFFF_FFFF_FFFF]` by `58^10` over Monero's alphabet
(`123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz`, `base58.cpp:46`) gives:

| Network | standard | integrated | subaddress |
|---|---|---|---|
| mainnet | `4` | `4` | `8` |
| testnet | **`9` or `A`** | `A` | `B` |
| stagenet | `5` | `5` | `7` |

Testnet standard (prefix 53) spans two digits, so a hard leading-character check would reject
roughly one testnet address in five. `AddrForm::leading` is therefore a `&'static [char]` and
`address::parse` uses it only as a display hint and a fast reject **when the set has one member**;
length and base58 decodability decide, the decoded prefix varint names the form, and the authority
before any send is always `validate_address` (§2a.6).

### 1b.9 What `set_daemon` actually does, and what a remote daemon changes

Every fact in §2m rests on this subsection. All of it was read in the v0.18.5.1 tree of §1.6;
nothing here is inferred from documentation.

**1. `set_daemon` never contacts the daemon.** `on_set_daemon`
(`src/wallet/wallet_rpc_server.cpp:4684-4765`) validates its arguments and then calls
`m_wallet->set_daemon(...)` (`:4758`), which is
`return m_http_client->set_server(address, get_daemon_login(), std::move(ssl_options));`
(`src/wallet/wallet2.cpp:1409`) — it disconnects any live connection (`:1386-1387`), stores the
address, the login and `m_trusted_daemon` (`:1392-1394`), invalidates the node-RPC proxy cache
(`:1395-1405`) and returns. `abstract_http_client::set_server` is a **parse**
(`contrib/epee/src/abstract_http_client.cpp:134-148`): it runs `parse_url`
(`contrib/epee/src/net_parse_helpers.cpp:174`, the bracketed-IPv6 form at `:135`), fills a missing
port with 80 for `http` and 443 for `https` (`:139-144`), and hands **host and port only** to the
real `set_server` (`:146`). No socket is opened. So:

- a successful `set_daemon` proves **nothing** about reachability — which is why §2m's send gate is
  a `refresh` and not a `set_daemon`;
- **`-38 NO_DAEMON_CONNECTION` "Unable to set daemon"** (`wallet_rpc_server.cpp:4758-4763`) means
  the address did not *parse*, never that the node is down. `set_server` fails only when
  `parse_port` fails (`net_parse_helpers.cpp:96-99`, `try_lexical_convert` plus `<= 65535`) — and
  `parse_url` itself `return true`s even on a regex miss (`:189`). §4.2's v5 parenthetical said
  "raised when wallet2 cannot reach the address"; that was wrong and v5.2 corrects it.
- **the scheme is discarded.** `https://` does not enable TLS and `http://` does not disable it;
  only `ssl_support` does. The scheme's *whole* effect is the default port at `:139-144`, and
  neither 80 nor 443 is a Monero RPC port — which is why §2m makes `:port` mandatory in
  `node.remote` rather than guessing 18081.
- **everything after the host:port is discarded**: `parse_url` puts it in `content.uri`
  (`net_parse_helpers.cpp:165-169`, `:204`) and `set_server` never reads that field. A path silently
  does nothing, so §2m refuses one rather than accepting it and lying.

**2. `ssl_support` has exactly three values, and `autodetect` is opportunistic and unauthenticated.**
`ssl_support_from_string` accepts `enabled`, `disabled` and `autodetect` and nothing else
(`contrib/epee/src/net_ssl.cpp:654-665`); a fourth value is `-38` "Invalid ssl support mode"
(`wallet_rpc_server.cpp:4732-4737`). `on_set_daemon` starts from
`ssl_options_t ssl_options = e_ssl_support_enabled;` (`:4726`) and that constructor sets
`verification = system_ca` for anything but `disabled`
(`contrib/epee/include/net/net_ssl.h:87-93`), so `autodetect` with no CA file and no fingerprints
means **support = autodetect, verification = system_ca**. Two consequences, both load-bearing for
what §2m and README may claim:

- epee's own note: "If `this->support == autodetect && this->verification != none`, then the
  handshake will **not fail** when peer verification fails … a warning is logged instead of
  failure" (`net_ssl.h:121-124`). A bad or forged certificate is a log line, not a refusal.
- a failed handshake **downgrades the connection to plaintext, permanently for that client
  object**: `try_connect` returns `CONNECT_NO_SSL` (`contrib/epee/include/net/net_helper.h:179-198`)
  and `connect` then sets `m_ssl_options.support = e_ssl_support_disabled` and reconnects
  (`:225-233`).

So autodetect means "use TLS if the server speaks it, in plaintext otherwise, and do not check who
the server is". It is the only value v0.1 can use: `--daemon-ssl enabled` **throws at process
start** unless a CA file, a fingerprint list or `--daemon-ssl-allow-any-cert` is given
(`wallet2.cpp:416-432`), and §0 cuts those three keys. §2m states this rather than implying a
private channel, and README repeats it.

**3. The command-line flags, all registered on `monero-wallet-rpc`.**
`wallet_rpc_server.cpp:5060` calls `tools::wallet2::init_options(desc_params)`, which adds
(`wallet2.cpp:1308-1339`) exactly the descriptors at `wallet2.cpp:244-259`:
`--daemon-address` (`:244`), `--trusted-daemon` (`:247`), `--untrusted-daemon` (`:248`),
`--daemon-login` (`:252`), **`--daemon-ssl enabled|disabled|autodetect`, default `autodetect`**
(`:253`), `--daemon-ssl-private-key` (`:254`), `--daemon-ssl-certificate` (`:255`),
`--daemon-ssl-ca-certificates` (`:256`), `--daemon-ssl-allowed-fingerprints` (`:257`),
`--daemon-ssl-allow-any-cert` (`:258`), `--daemon-ssl-allow-chained` (`:259`). There is no
`--daemon-ssl-support` and no `--untrusted`; the two names above are the ones §4.2 renders.

**`--trusted-daemon` and `--untrusted-daemon` are not symmetric, and neither is the default.**
Giving both is a fatal error (`wallet2.cpp:449-450`). Giving *neither* does **not** mean untrusted:
`make_basic` falls through to `trusted_daemon = tools::is_local_address(daemon_address)`
(`:451-462`), so a loopback address is silently trusted and a public one is silently untrusted.
§4.2 therefore renders one of the two flags **explicitly in both modes** — an inherited default that
depends on a string parse is exactly the implicit configuration §2j objects to, and it is the flag
that decides item 4.

**4. What `trusted` changes.** It is one bool, `m_trusted_daemon` (`wallet2.h:1090-1091`,
`wallet2.cpp:1394`), and it changes **five things in `wallet2` and five more in `monero-wallet-rpc`
itself**. The five in `wallet2`, two of them privacy:

- **`select_available_outputs_from_histogram` sends the wallet's own amounts to the daemon when
  trusted**: `if (is_trusted_daemon()) req_t.amounts = get_unspent_amounts_vector(false);`
  (`wallet2.cpp:12281-12282`). Against an untrusted daemon it asks for the whole histogram instead.
  This is the single strongest reason a public node is never marked trusted: the request itself
  names what the wallet holds.
- **`import_key_images` checks spent status against the daemon only when trusted**
  (`wallet2.cpp:12170`).

Two are error reporting — with an untrusted daemon the node's own text is replaced:
`get_rpc_status` returns `"<error>"` for anything but `OK`/`BUSY`/`PAYMENT REQUIRED`
(`wallet2.cpp:15977-15986`), and `get_daemon_blockchain_height` / `_target_height` report
`"daemon error"` instead of the daemon's string (`:13529-13536`, `:13556-13563`). fmonero must not
present `<error>` as an upstream message: §1b.4's "message verbatim" mapping still applies, and the
message it will see in remote mode is frequently that placeholder.

One is correctness: **`scan_tx` refuses to re-process unrequested transactions through an untrusted
daemon** (`wallet2.cpp:1929-1932`) — irrelevant to v0.1, which cuts `scan_tx` (§0), and listed so
that nobody restores it thinking the flag is cosmetic.

`refresh` also takes the bool (`on_refresh` passes `m_wallet->is_trusted_daemon()`,
`wallet_rpc_server.cpp:3386`) and uses it only to pick the short-chain-history granularity on the
first refresh (`wallet2.cpp:4123`, `:4132`): untrusted costs a coarser first probe, nothing more.

**And `monero-wallet-rpc` gates five handlers of its own on `is_trusted_daemon()`, which an
enumeration that stops at `wallet2` misses.** Three refuse outright with
`-1 UNKNOWN_ERROR` "This command requires a trusted daemon.": `import_key_images`
(`wallet_rpc_server.cpp:3151`), `rescan_spent` (`:3456`) and `start_mining` (`:3484`).
`import_multisig`'s post-import `rescan_spent` is *skipped* rather than refused (`:4432`), and the
background-mining check returns early (`:438`). All five are cut by §0 in v0.1 — which is why
nothing breaks today — and all five are where a v0.2 that restores one of them will meet the flag:
at the wallet-RPC layer, not in `wallet2`. That is the same reason the `scan_tx` note above exists,
and it is why the count is stated as five-and-five rather than as "five".

**5. No wallet-RPC method reports the daemon's state.** `on_getheight` answers
`m_wallet->get_blockchain_current_height()` (`wallet_rpc_server.cpp:938-951`, the assignment at
`:943`) — the **wallet's** height. There is no `get_daemon_height`, no daemon `synchronized`, no
daemon version and no peer count in `wallet_rpc_server_commands_defs.h`. A remote node's own sync
state is therefore unobservable from fmonero in remote mode, and §5 renders `null` rather than a
guess.

**6. What a `refresh` proves, and what it costs.** `COMMAND_RPC_REFRESH`
(`wallet_rpc_server_commands_defs.h:2054-2077`) takes `start_height` (`KV_SERIALIZE_OPT`, default 0)
and returns `blocks_fetched` and `received_money`; `on_refresh` (`wallet_rpc_server.cpp:3375-3395`)
calls `m_wallet->refresh(...)` and maps a failure through `handle_rpc_exception`, whose first catch
turns `tools::error::no_connection_to_daemon` into **`-38 NO_DAEMON_CONNECTION`**
(`:3777-3781`). So a `refresh` that returns proves (a) the daemon answered and (b) the wallet holds
every block that daemon offered — which is exactly the pair §2m's send gate needs, and is strictly
more than `set_daemon` can prove. It does **not** prove that the daemon's tip is the network's tip.

---

## 2. Non-negotiables

Each of these is a testable statement, and the test that proves it is named. Phase 3 runs them all.

### 2a. Money

1. **`send` and `sweep` require `--yes`.** Without it, on a TTY or not, they print the exact
   recipient, amount, fee, priority and account and exit **3 `safety`**. There is no prompt-based
   confirmation in the CLI; the TUI's modal (§7a) is the interactive path.

   **Order of refusal, which decides the exit code.** Printing the fee needs a build, a build needs
   the wallet daemon, and the daemon may be down — so "refused without `--yes`" and "refused because
   nothing answered" are two different answers to one command and the order has to be stated, or the
   CLI, the TUI and the §8 gate commands will each encode a different one. `send` and `sweep` run,
   in this order:

   1. argument and amount parsing (§2a.5) — exit **2 `usage`**;
   2. `address::parse` (§1b.8) — exit **15 `invalid_address`**; **skipped when `dest` begins with
      `@`**, which is resolved at step 5a and parsed there;
   3. the §2a.11 `send.lock` acquisition — exit **3 `safety`**, `reason: send_in_progress`;
   4. the §2a.4 `pending_send` entry check — exit **3 `safety`**, `reason: pending_send`;
   5. the wallet-daemon reachability probe — exit **4 `backend`**;
   5a. **`@label` resolution (§4)**: one `get_address_book` on the open wallet, matched
       case-insensitively against `description`; zero matches exit **6 `not_found`**, more than one
       exits **7 `ambiguous`** listing them; the resolved address then runs through
       `address::parse`. The resolved **address** — never the label — is what `Plan.dest`, every
       line `send::relay` announces, the `pending.json` record and §5c's `dest` key carry. The
       TUI's `@` picker yields a literal address and never reaches this step.
   6. `validate_address` and the nettype check (§2a.6) — exit **15**;
   7. **the daemon-currency gate, which is one step with two implementations (v5.2)** —
      `send::require_node_current`, switching on the `NodeGate` the caller passed (§5b):
      - `NodeGate::Local(n)` — one `get_info`; `synchronized == false` exits
        **10 `node_syncing`**, `reason: node_not_synchronized`;
      - `NodeGate::Unreachable` — exit **4 `backend`**, `reason: unreachable` (unchanged: the
        caller has already proved there is no local node);
      - `NodeGate::Remote` — **one bounded `refresh { start_height: 0 }`** under
        `timeouts.refresh_sec`, which must succeed (§2m). `-38 NO_DAEMON_CONNECTION` or a
        transport failure exits **4 `backend`**, `reason: unreachable`; a call killed at the
        deadline exits **10 `node_syncing`**, `reason: wallet_catching_up`, and arms **no**
        §2d breaker;
   8. the build (`do_not_relay`);
   9. the `--max-fee` check, when given (§2a.2) — exit **3 `safety`**, `reason: fee_above_max`;
   10. the `--yes` check — exit **3 `safety`**, `reason: needs_yes`, now able to print the real fee.

   So a no-`--yes` send with the wallet daemon down exits **4, not 3**: §2a.1's "print the fee
   first" is reachable only once a fee exists. §4b's GREEN row and the Phase 1b gate assert exactly
   that.

   **Step 7 is the only step remote mode changes, and it stays step 7.** It runs after
   `validate_address`, so a bad address is still exit 15 in milliseconds rather than after a
   120 s network wait, and before the build, so the fee is always computed against a wallet that
   has just been proved current. It runs for `--dry-run` too: a fee quoted from a stale wallet is
   a fee the `--yes` run will not reproduce, which is the §2a.2 failure. It runs **inside** the
   §2a.11 `send.lock` (taken at step 3), so a second sender meets `send_in_progress` for the
   duration of the refresh — the same property the local build window already has, with a
   different bound.

   **Client construction is never step 0.** `WalletClient::from_ctx` succeeds even when
   `monero-wallet-rpc`'s login file is absent — it returns a client with `auth: None` (§2c) — so
   steps 1–4 always run in their stated order and the absence of a wallet daemon surfaces at step 5
   as exit **4**, with `reason: unreachable` for a refused connect and `reason: auth_refused` for a
   401 from a daemon whose credential we could not read. A constructor that failed instead would
   decide the exit code of the money path from outside the refusal order, turning a bad address
   (15) and an unresolved `pending_send` (3) into 4.
2. **The fee the user sees is the real fee.** `--dry-run`, the no-`--yes` refusal and the TUI's
   confirm modal all obtain it the same way: one `transfer` call with
   `do_not_relay: true, get_tx_metadata: true, get_tx_key: false`, whose `fee` is the fee of the
   transaction that was actually built. fmonero never estimates a fee itself, never multiplies
   `get_fee_estimate.fee` by anything, and never shows a fee it did not receive from a built tx.

   **What this does not claim.** It is true *within one invocation* and false across the flow this
   tool's own hint prescribes. The no-`--yes` refusal's transaction is **discarded** (§2a.3), so the
   `--yes` invocation the hint suggests builds a *new* one, whose fee may differ — different inputs,
   or a different `adjust_priority` outcome after a pool backlog (`wallet2.cpp:8688-8708`). The user
   would otherwise confirm fee A and pay fee B, with fee B appearing only in the JSON payload after
   the money has moved, which is the §2j failure on a money surface. So:
   - `send::relay` emits the built recipient, amount, fee, total, priority and account as one line
     through its `announce` sink **immediately before the first `relay_tx`**, on every `--yes` run,
     so the number that is paid is always displayed. The CLI's sink writes that line to stderr and
     the TUI's appends it to the log pane; core itself stays silent (§2g). stdout stays the JSON
     envelope (§4).
   - **`--max-fee AMOUNT`** (§4.1, on `wallet send` and `wallet sweep`) refuses with exit
     **3 `safety`**, `reason: fee_above_max`, when the built total fee exceeds it. It is the only
     bound that survives the rebuild, and it lives on `SendRequest`/`SweepRequest` so the CLI and
     the TUI cannot encode it differently: `send::build` raises it **immediately after the build,
     before the `--yes` check** (so a no-`--yes` run with `--max-fee` says so rather than printing a
     fee it would have refused), and `send::relay` re-checks it as the enforcing gate. It costs ~10
     lines.
   - The single invocation in which the printed fee *is* the paid fee is `--dry-run` followed by
     nothing, and the TUI, whose modal relays the same `Plan` it displayed.

   *Test:* the mock returns a fee above `--max-fee`; the command exits 3 with `reason: fee_above_max`
   and the mock records zero `relay_tx`.
3. **Never build twice, and the sequence lives in one place.** With `--yes`, the same process that
   built the transaction relays it: `relay_tx { hex: <the tx_metadata from that very response> }`.
   There is no second `transfer` call anywhere in the send path. Without `--yes` the built
   transaction is **discarded**, which is safe precisely because `do_not_relay` never commits the
   pending tx, so no output is marked spent and nothing leaks. The TUI holds the metadata in memory
   between the form and the modal and relays that, so it genuinely builds once per send.
   **`fmonero_core::send::build` and `fmonero_core::send::relay` (§5b) are the only two functions in
   the codebase that may call `WalletClient::build_transfer`, `build_sweep_all` or `relay_tx`, and
   the only two writers of the `pending.json` record are `send::relay` — which creates it and marks
   entries relayed — and `send::build`, which only ever *clears* a fully-resolved one as §2a.4's
   resolve step (`state::clear_pending_send`, never a create, extend or mark). `wallet clear-pending`
   is the third writer, and it is the user's explicit command, not a code path.** The CLI and the TUI are two
   independently written callers; an invariant that lives in both of them is an invariant that holds
   in one. Putting it in core makes §2a.1-4 provable by one unit test against the mock instead of
   two, and makes the grep below non-vacuous.
4. **A crash can never double-send, and the record clears itself on the happy path.** `sweep_all`
   returns a *list* (`split_transfer_response`, §1b.4) and `relay_tx` is called once per entry, so
   the record is a list too:

   ```jsonc
   // ~/.local/state/fmonero/pending.json, mode 0600 — its OWN file, not part of state.json
   {
     kind: "send" | "sweep", account_index, dest, built_at,
     txs: [ { txid, amount_pico, fee_pico, relayed: false, relayed_at: null }, … ]
   }
   ```

   **The record lives in its own file, and that is not cosmetic.** `state.json` holds the 60 s cold
   cache, the `get_net_stats` sample pair, the stall breakers and `last_error`, and **`status`
   rewrites it on every invocation** — from the bar widget every 3–8 s and from the TUI every 1 s
   (§6.3, §7a). A JSON document has no field-level write: every writer loads the whole file,
   mutates and renames a new one over it. `rename(2)` makes a *reader* safe; it does not make two
   read-modify-write cycles safe. A `status` that loaded state at T, a `send::relay` that writes the
   pending record at T+5 ms and the `status` rename at T+15 ms lose the record entirely, and the
   next `send` finds no interlock — the one lost update that can double-spend. So
   `paths.pending_file()` is `state_dir/pending.json`, its only writers are `send::relay`,
   `state::mark_relayed`, `state::clear_pending_send` (called from `send::build`'s resolve step and
   from `wallet clear-pending`), and its only other reader is `status`
   (read-only) and this entry check. §5's locking rule covers the rest of `state.json`.
   `rename(2)` makes a *reader* safe and a separate file makes `status` safe; neither makes **two
   senders** safe, which is what §2a.11's lock is for.

   **Write order.** The whole record — every txid from the build, in relay order — is written and
   fsynced **once, before the first `relay_tx`**. Every txid is already final: the transactions are
   fully built, so their hashes are known from the `do_not_relay` response. After each `relay_tx`
   returns, that entry's `relayed` flips to `true` and the file is rewritten and fsynced before the
   next call. The tx metadata blob is **never** written to disk — it carries the transaction secret
   key, and crash safety needs only the txid.

   After the **last** successful `relay_tx`, and before the resolve step, `send::relay` issues one
   wallet **`store`**. `monero-wallet-rpc` writes the wallet file only on a graceful `stop()`
   (`src/wallet/wallet_rpc_server.cpp:294-302`, which calls `m_wallet->store()`), on this explicit
   call (`:2060-2077`), or on the open/create paths' `autosave_current`; its auto-refresh idle
   handler **never** stores (`:210-285` — it refreshes and nothing else). So without it a SIGKILL,
   an OOM kill or a power cut after a relay loses the wallet's own record of the outgoing transfer
   and its tx key. Funds are not lost — a rescan re-derives the spend — but the resolve step then
   answers "not found" for a transaction that really was broadcast, leaves the record in place, and
   blocks every later send until the user runs `clear-pending` by hand: the habit §2a.4 exists to
   prevent. One extra RPC on the send path buys durability for the record the whole interlock reads.

   **Entry check, which is what makes the interlock survivable.** On entry, `send` and `sweep`
   inspect the record:
   - any entry with `relayed == false` ⇒ exit **3 `safety`**, because that send may be in flight;
   - every entry `relayed == true` ⇒ *resolve*: one `get_transfer_by_txid { txid, account_index }`
     per entry. An answer of **any** type — `in`, `out`, `pending`, `pool` **or `failed`** — counts
     as found. `on_get_transfer_by_txid` searches `get_unconfirmed_payments_out`
     (`src/wallet/wallet_rpc_server.cpp:3014-3022`), the same source that feeds both the `pending`
     and the `failed` lists of `on_get_transfers`, and `fill_transfer_entry` types those rows
     `is_failed ? "failed" : "pending"` (`:561`). A `failed` row means the wallet saw the
     transaction and it did not make it, which is not a double-spend risk; omitting that type from
     the accepted set would leave the record in place forever and force the user to type
     `clear-pending` out of habit, which is the one thing §2a.4 exists to prevent. All found ⇒ clear
     the record and proceed. Any entry answered as **not found** (`-8` "Transaction not found.",
     §1b.4) ⇒ leave the record and exit 3 — a positive "not found" is the only answer allowed to
     block.
     **Any other failure of a resolve call ⇒ leave the record and do not block**: the wallet
     unreachable, `-38 NO_DAEMON_CONNECTION`, or epee's `code: 0` catch-all (§1b.4, the ordinary
     answer when wallet-rpc cannot reach the daemon). None of them is evidence of a second spend,
     and every entry's `relay_tx` already returned. Blocking on them would arm the interlock
     exactly when the node hiccups after a send — the state in which a user most wants the next
     command to work.

   The resolve step also runs immediately after the last successful `relay_tx` of a send, so a
   normal successful send leaves nothing behind. `relay_tx` calls `commit_tx` internally
   (`src/wallet/wallet_rpc_server.cpp:1903-1960`, `m_wallet->commit_tx(ptx)`), so the wallet registers the transaction as
   pending at once and the resolve finds it on the first try. `wallet clear-pending --yes` therefore
   stays what it is meant to be — the manual escape hatch after a crash — instead of something the
   user learns to type after every send, which would disarm the interlock by habit.

   **Partial sweep.** A `relay_tx` that fails at entry *k* of *N* aborts the loop, drops the
   metadata of entries *k..N* (never written to disk), leaves the record in place and exits
   **16 `tx_rejected`** naming "k-1 of N broadcast" with their txids and
   `hint: fmonero wallet history --limit <N>   # then: fmonero wallet clear-pending --yes`.

   `status` surfaces the record as `pending` (§5) and raises `send_may_be_in_flight` while **any**
   entry is `relayed: false`. The blocking hint is
   `fmonero wallet history --limit 5   # confirm whether <txid> landed, then: fmonero wallet clear-pending --yes`.
5. **Amounts are integers.** `Piconero(u64)` end to end. No `f64` touches a monetary value
   anywhere, in Rust or in QML — the plugin formats amounts from the pre-rendered `*_xmr` **string**
   in Status JSON and never divides. Parsing rejects more than 12 decimal places, a bare `.`, a
   leading `+`, thousands separators, and anything that would overflow `u64` — every amount in
   both RPCs is a `uint64_t` of piconero (§1b.3, §1b.4), so a value that does not fit in `u64` is
   not a value Monero could represent either.
6. **Recipients are validated twice.** `address::parse` structurally (§1b.8) at parse time, then
   `validate_address { address, any_net_type: true, allow_openalias: false }` on the open wallet
   immediately before building — including for a recipient resolved from the address book.
   `any_net_type: true` is deliberate and load-bearing: with it `false`, `on_validate_address`
   only tries the wallet's own nettype (`src/wallet/wallet_rpc_server.cpp:4644`), so a foreign
   address comes back `valid: false, nettype: ""` and the user is told "invalid address" when the
   truth is "that is a stagenet address" — and with no wallet open it returns `-13` outright
   (`:4641`). A recipient whose returned `nettype` differs from the wallet's exits
   **15 `invalid_address`** with
   `error: "<addr> is a <nettype> address; this wallet is <wallet nettype>"`.
   `allow_openalias` is always `false`: it does DNS.

   **The wallet's nettype is read from the wallet itself, never from a setting.** No frozen API
   returns it otherwise — `WalletClient` has no such method, `Status` carries only `node.nettype`
   (the *node's*, from `get_info`), and `settings.node.network` is a setting that can disagree with
   the running daemon's `--stagenet`/`--testnet` flag after a hand-edited config, so a Phase 1b
   agent would pick one of three and a settings/unit skew would silently change which addresses the
   send path accepts. It is `address::parse(get_address{account_index}.address)?.net` — the open
   wallet's own primary address, already a hot-path call (§5) and authoritative about the network
   the daemon was started on. `wallet_session::open` records it alongside the wallet name
   (`state::set_last_open(name, net)`, `state::last_open() -> Option<(String, NetType)>`, §5b), so
   `send::build` compares against it without a second RPC. A recorded nettype that differs from
   `settings.node.network` raises the `nettype_mismatch` warning in `status` (§5) and the doctor's
   `wallet_nettype_matches` check (§7.1). It is a warning, not a refusal: the send path is already
   safe because it follows the wallet, and the mismatch is a configuration fact the user fixes.
7. **No default recipient, no default amount, ever.** `sweep` requires an explicit destination.
8. **`SendRequest` and `SweepRequest` have no `unlock_time`, no `payment_id`, no `ring_size`, no
   `outputs` and no `subtract_fee_from_outputs` field, and the serializer emits none of them.**
   `subtract_fee_from_outputs` (`wallet_rpc_server_commands_defs.h:603`, serialized at `:617`) makes
   the recipient pay the fee out of the amount, so the number fmonero printed would not be the
   number that arrives; it value-initializes empty and fmonero exposes no flag for it. Omission is
   the intended encoding: `KV_SERIALIZE_N` ignores the serializer's return value
   (`contrib/epee/include/serialization/keyvalue_serialization.h:78-79`), so a missing key is simply
   not loaded, and `struct_init` value-initializes the request — hence `unlock_time` is 0 and
   `adjust_mixin(0)` picks the consensus ring size. **`transfer` has no `outputs` field at all** in
   0.18.5.1 (`COMMAND_RPC_TRANSFER::request_t`, `:596-627`); `sweep_all`'s does and defaults to 1
   (`:906 KV_SERIALIZE_OPT(outputs, (uint64_t)1)`, with `:942` the same for `sweep_single`), which is
   the value fmonero wants. It is also
   the only safe encoding: `on_transfer` rejects **any** non-zero `unlock_time` with
   `-50 NONZERO_UNLOCK_TIME` before validating anything else (`wallet_rpc_server.cpp:1249`), and any
   non-empty standalone `payment_id` with `-5` "Standalone payment IDs are obsolete" (`:1106-1107`).
   A payment id reaches Monero only inside an integrated address, which `validate_address` reports
   as `integrated`. fmonero exposes no flag for either.
9. **The relayed txid must be the txid we showed.** `relay_tx`'s `tx_hash` is compared against the
   `BuiltTx.txid` recorded in `pending_send`. On a mismatch the entry is **not** marked relayed, the
   command exits **1 `failure`**, `reason: txid_mismatch` (§4), printing both hashes, with
   `hint: fmonero wallet history --limit 5   # two transaction ids differ; check which one landed`,
   and `send_may_be_in_flight` is raised. The response already carries the check for free, and the
   whole §2a.3/§2a.4 chain rests on one transaction's identity holding across build → record →
   relay.

   **What this check proves, and what it does not.** `relay_tx`'s `tx_hash` is computed by
   wallet-rpc *locally*, from the metadata blob fmonero just sent —
   `res.tx_hash = pod_to_hex(get_transaction_hash(ptx.tx))` (`wallet_rpc_server.cpp:1963`) — with
   the same function `transfer` used to fill its own `tx_hash` (`:1216-1221`). A mismatch therefore
   means fmonero relayed the wrong entry of a sweep's `tx_metadata_list`: a real bug, and the only
   bug this check can catch. It is **not** evidence that the daemon accepted the transaction. That
   evidence is `relay_tx` returning at all — it calls `commit_tx`, which throws and becomes
   `-4 GENERIC_TRANSFER_ERROR` on daemon rejection (`:1952-1961`) — and, after the fact, the §2a.4
   resolve step. §2j does not let a check be described as stronger than its mechanism.

10. **`sweep` always sends `subaddr_indices_all: true`, and there is no field to turn it off.**
    With `subaddr_indices_all` absent (it defaults to `false`,
    `wallet_rpc_server_commands_defs.h:903`) and `subaddr_indices` empty, `on_sweep_all` hands an
    **empty set** to `create_transactions_all` (`wallet_rpc_server.cpp:1786-1795`), and
    `create_transactions_all` then picks **one subaddress at random**: "in case subaddress index
    wasn't specified, choose non-empty subaddress randomly (with index=0 being chosen last)"
    (`src/wallet/wallet2.cpp:11846-11853`). A command named `sweep`, documented as the escape hatch
    for `-18 TX_TOO_LARGE` and as the way to empty an account, would move the funds of one randomly
    chosen subaddress, report success, and leave the rest — discoverable only from the balance
    afterwards, and invisible to any fixture-driven mock. With `subaddr_indices_all: true` the
    server enumerates every subaddress of the account (`:1787-1791`), the set is non-empty and the
    random branch is unreachable. `SweepRequest` (§5b) carries no subaddress selector precisely so
    that this cannot be switched off.

11. **Only one build→relay sequence may be in flight, and `do_not_relay` does not give this for
    free.** §2a.4's record is written before the first `relay_tx`, so it covers the *relay* window
    only — and a build reserves nothing: `on_transfer` calls `commit_tx` **only when `do_not_relay`
    is false** (`src/wallet/wallet_rpc_server.cpp:1213-1214`), and `commit_tx` is what marks the
    selected outputs spent (`wallet2::commit_tx`, `set_spent(idx, 0)` for each
    `ptx.selected_transfers`, `src/wallet/wallet2.cpp:7628`). Two fmonero processes — a CLI
    `wallet send --yes` while the TUI holds a built `Plan` across its confirm modal, or one agent
    invoking twice — therefore each select from the same unspent set with nothing on disk to stop
    them. Both then relay: the second is either rejected by the daemon as a double spend
    (`-4 GENERIC_TRANSFER_ERROR`, exit 16) or, if the selections happened to be disjoint, **both
    land and the user has spent twice what they confirmed**. Worse, both write the whole
    `pending.json` record, the second clobbering the first, so if one process dies mid-relay the
    other's txids are gone and nothing blocks the next send — the guarantee of §2a.4 defeated by
    concurrency rather than by crash.

    So `send::build` and `send::build_sweep` take an exclusive
    `flock(LOCK_EX|LOCK_NB)` on **`state_dir/send.lock`** (`O_CREAT|O_RDWR`, 0600) **before the
    §2a.4 entry check**, and the returned `Plan` **owns the open file descriptor** (`Plan._lock`,
    §5b). The lock is released only when the `Plan` is dropped — after `send::relay` finishes, after
    a `--dry-run` or a no-`--yes` discard, or when the process dies, which is what makes it
    crash-safe with no marker to clean up. The entry check, the build, the record write, every
    `relay_tx`, the `store` and the resolve step are therefore one critical section, and the TUI's
    confirm modal deliberately holds it for as long as the human takes.

    A lock already held means another fmonero process is between a build and a relay: exit
    **3 `safety`**, `reason: send_in_progress`,
    `error: "another fmonero send is building or relaying"`,
    `hint: fmonero status --json   # wait for pending to clear, then retry`. **`flock` does not
    name the holder** and the lock file stays empty, so the message carries no pid; the holder is
    identified through `fmonero status --json`'s `pending` record, which is what the hint points
    at. (`fcntl(F_GETLK)` would name one, but §2i's `flock`-vs-`fcntl` choice is load-bearing and
    the two are independent namespaces — a probe in the other family proves nothing about this
    lock.) This lock is distinct
    from §5's `state.json` `flock` and from the `pending.json` record, which covers the *post*-relay
    window.

*Tests:* `send` and `sweep` without `--yes` exit 3 and print a fee obtained from a mocked
`do_not_relay` response (mock e2e); a unit test asserts the send path issues exactly one `transfer`
and one `relay_tx` against the in-process mock and that the `relay_tx.hex` is byte-identical to the
`transfer` response's `tx_metadata`; a unit test writes a `pending_send` whose single entry is
`relayed: false` and asserts the next `send` exits 3; **a unit test writes a fully-relayed record
whose txid the mock reports through `get_transfer_by_txid`, runs `send`, and asserts exit 0 and an
empty record**; a unit test builds a two-tx sweep, kills the process between the first and the
second `relay_tx`, and asserts the reloaded record shows exactly one `relayed: true` and one
`relayed: false`; the mock's request log shows neither `unlock_time` nor `payment_id` in any
recorded `transfer` or `sweep_all` body (§2a.8); the mock returns a `tx_hash` that differs from the
built txid and the test asserts exit 1 with the entry still `relayed: false` (§2a.9);
**the mock answers `get_transfer_by_txid` with a `failed`-type row and the test asserts the next
`send` exits 0 with an empty record** (§2a.4);
**the mock's request log shows `"subaddr_indices_all": true` in every recorded `sweep_all` body, and
a unit test asserts `TransferRequest`'s sweep constructor sets it unconditionally** (§2a.10);
**a concurrency test runs `status` in a loop in one process while `send --yes` relays in another
against the mock, and asserts `pending.json` is present and complete after every iteration**
(§2a.4) — which requires the mock's thread-per-connection accept loop (§8 Phase 0), since fmonero
holds its socket for the life of the process (§2c);
**a second concurrency test starts two `send --yes` processes against one mock started with
`--delay-ms 1500`**, so the first process is provably still holding `send.lock` when the second
reaches it (against an instant mock the first can finish build → relay → resolve before the second
reaches `flock`, both legitimately succeed, and the only test of the one interlock that can
double-spend becomes a coin flip that gets marked known-flaky and then deleted), **and asserts
exactly one `transfer` in the mock's request log, one exit 0, one exit 3 with
`reason: send_in_progress`, and that `pending.json` afterwards holds exactly the successful
process's txids** (§2a.11);
**`--force` is proved *not* to widen**, one case per sentence of §4's `--force` paragraph: with a
`pending_send` whose entry is `relayed: false`, `wallet send … --yes --force` exits 3 and the mock
records zero requests; with the `system_unit` layout fixture, `node down --yes --force` exits 3 and
the mock records zero requests; with `node.url` set to `http://10.0.0.5:18081`, `status --force`
exits 2;
**the three remote-mode step-7 cases** (§2m): with `node.mode = remote`, `wallet send --dry-run`
against `fmonero-mock --role wallet` records exactly one `refresh` **before** the `transfer` in the
request log; `--fail refresh=-38:'no connection to daemon'` makes it exit 4 `reason: unreachable`
with zero `transfer` calls; and `--delay-ms 3000` with `--timeout 1` makes it exit 10
`reason: wallet_catching_up` with zero `transfer` calls and **no breaker armed** (asserted by a
second invocation in the same state dir reaching the mock);
`grep -rn 'relay_tx\|set_pending_send\|build_transfer\|build_sweep_all' crates/fmonero/src` finds
no hit outside the doctor's read-only probes (§2a.3);
`grep -rn 'as f64' crates/*/src` finds no line in an amount path.

### 2b. Secrets

1. **The wallet password never reaches argv.** Not fmonero's argv, not `monero-wallet-rpc`'s
   (`--password` exists — §1b.5 — and is banned). Three sources, consulted in this order, first
   hit wins: (1) `--password-file PATH` — read, one trailing newline stripped, and refused with
   exit 3 if the file has any group or other permission bit; (2) `$FMONERO_WALLET_PASSWORD`;
   (3) a TTY prompt with echo off (`termios` through `libc`; `SIGINT` restores the terminal). With
   no TTY and neither of the first two, `wallet open` exits **9 `wallet_locked`** with
   `hint: FMONERO_WALLET_PASSWORD=… fmonero wallet open <name>   # or --password-file`, so an agent
   is never left waiting on a prompt that cannot appear. `--password-file` is an argument to
   *fmonero* and is never forwarded to `monero-wallet-rpc`, which would reject it anyway alongside
   `--wallet-dir` (§1b.5).

   **`proc::run_with_deadline` removes `FMONERO_WALLET_PASSWORD` from every child's environment**
   (`Command::env_remove`). fmonero spawns `systemctl`, `omarchy` and the doctor's version probes;
   none of them needs it, `/proc/<pid>/environ` is readable by root, and a child is free to log its
   own environment. A documented secret handed to processes that have no use for it is a leak with
   no upside. *Test:* a child that prints its environment shows no `FMONERO_WALLET_PASSWORD`.
2. **Secrets never appear in output or on disk.** The password, a seed, a spend key, a view key and
   a transaction secret key never appear in `--json`, in `-v` output, in `state.json`, in a
   fixture, in a doctor probe, in a log line or in DECISIONS.md. **No seed ever enters fmonero's
   process at all**: `wallet restore` is cut from v0.1 (§0) precisely so that there is no seed input
   path to get wrong. `create_wallet` does **not** print the
   generated seed: fmonero tells the user to run `monero-wallet-cli --wallet-file … seed` to see
   it, because a seed printed to a terminal ends up in scrollback and in shell history tooling.

   **Not printing the seed makes an unconfirmed password an irreversible loss path, so `create`
   closes it.** A wallet whose password the user does not have is a wallet that can never be opened
   and whose seed can never be read — reading the seed from `monero-wallet-cli` needs that same
   password, and `on_create_wallet` takes `req.password` verbatim with no emptiness check
   (`wallet_rpc_server.cpp:3583-3595`, `:3611`), so an empty prompt silently creates an unencrypted
   wallet. `wallet create` therefore prompts for the password **twice on a TTY and compares**,
   refuses an empty one without `--allow-empty-password` (§4.1), and ends by printing, before
   anything can be received:

   ```
   Back up your seed BEFORE funding this wallet:
     monero-wallet-cli --wallet-file <wallet.dir>/<name> --offline
     # then type: seed
   ```

   `--json` carries the same string under `seed_backup_hint`. **`--offline` is not decoration**
   (`src/wallet/wallet2.cpp:279`): fmonero's own node unit requires `rpc-login` (§2c) and
   `monero-wallet-cli` has not been given that credential, so without `--offline` it tries the
   daemon, gets a 401 and stalls or warns before the user ever sees the seed. Showing a seed needs
   no daemon.
3. **`query_key` is out of scope.** `grep -rn 'query_key' crates/` must be empty.
4. **The TUI prompts in a modal** with a `TextInput` in masked mode; the password lives in one
   `String` in the modal's state, is moved into the `open_wallet` request, and the modal's buffer
   is overwritten with zeroes on close.
5. **The bar widget never opens a wallet.** It has no password field and no `wallet open` argv. It
   shows a balance only when `status.wallet.open` is already true.
6. **Wallet files are only ever touched by `monero-wallet-rpc`.** fmonero reads the wallet
   *directory* to list `*.keys` basenames (`wallet list`) and does nothing else with those bytes.
7. **Everything fmonero writes is owner-only.** Every file created under `state_dir`, `cache_dir`
   and `config_dir` is created mode **0600** and every directory **0700**, at the target mode —
   open with `O_EXCL` at that mode, write, `rename(2)`; never `chmod` after the fact, which leaves a
   window. This covers `state.json` (which holds the recipient address, the amounts and the open
   wallet's name), `cache_dir/history.bin` (the whole transfer history with addresses and notes),
   `doctor/runs/**` and `backups/**`. §2c's threat model already includes other local users; the
   files that record who was paid what are not an exception, and on a machine where `$HOME` is 0755
   the default would be world-readable.
8. **Doctor probes are redacted at capture.** `probes.rs` rewrites any captured line matching
   `rpc-login=`, `--rpc-login`, `--daemon-login`, `--password`, `login` or `password` to
   `<key>=<redacted>` **before** the bytes reach `runs/<id>/probes/`. A `backups/` copy keeps the
   file's own bytes verbatim — `undo` needs them — and is written 0600.
   **`probes.rs` never captures the *contents* of `paths.monerod_conf()` or
   `paths.wallet_login_file(port)` — only their modes.** Key-name redaction cannot fire on a login
   file whose single line is `monero:<base64>`: there is no key to match. The only safe rule is not
   to read it into a probe at all, and nothing needs to — `wallet_rpc_auth` (§7.1) needs a stat and
   an authenticated `get_version`, never the bytes.

*Tests:* (1) a **key-name** grep over the full `--json` and `-v` output of every command run against
the mock, failing on the keys `seed`, `spend_key`, `view_key`, `tx_key`, `tx_metadata`, `password`,
`login`; (2) a **known-value** grep — the test's own wallet password, the exact contents of the
mock's login file, and the `rpc-login=` token written by `setup --units --dry-run` must appear in
none of: stdout, stderr, `state.json`, `history.bin`, any file under `doctor/runs/`, any fixture,
DECISIONS.md; (3) `grep -rn 'query_key' crates/` is empty, a Phase 3 gate. The old
"any 64-hex-character run" shape test is **deliberately not used**: fmonero's own honest output is
full of 64-hex runs — `top_block_hash`, `pending.txs[].txid`, every `wallet history` row, the
argument to `wallet notes get TXID` — so it would fail on correct output and be weakened or deleted
the first time it ran. A fixed-value grep cannot be vacuous and cannot fire on a txid. (4) the
rendered wallet unit in `setup --units --dry-run`'s output contains `StateDirectoryMode=0700`
(§4.2) — asserted on that text so it needs no systemd run — and
`systemd-analyze --user verify ~/.config/systemd/user/fmonero-wallet-rpc.service` is clean.

### 2c. Loopback, and the authentication decision

Both units bind `127.0.0.1` (the default, §1b.5/§1b.6) and fmonero never binds a port itself. The
`ureq` agent is built with `default-features = false, features = ["json"]`, so **the binary
contains no TLS stack at all** — "loopback plaintext only" is a compile-time property, not a
runtime check. **Both `NodeClient::new` and `WalletClient::new`** reject any URL whose scheme is not `http`, and
accept as host only a **literal** address in `127.0.0.0/8` or `::1`, through one shared
`rpc::require_loopback(url)` (§5b) that both constructors call as their first statement;
`settings set node.url` **and `settings set wallet.url`** validate through that same function
(exit 2, `reason: non_loopback_url`). The wallet client is the one that carries a credential and
moves money — it sends `open_wallet` with the wallet password's HA1, and `transfer`/`relay_tx`
bodies — so exempting it would leave the rule true only where it costs nothing, and the
compile-time "no TLS stack" property above does not help: plaintext `http://` to an arbitrary host
is exactly what `ureq` with no TLS still does.
The name `localhost` is accepted only after resolution and only
when *every* returned address is loopback. (`localhost` is a name, not an address: an edited
`/etc/hosts` or a name-service module can point it anywhere, which would turn the one property this
paragraph claims back into an assumption.) Anything else exits 2 with the hint
`fmonero node remote <host>:<port> --yes   # a remote node is reached by monero-wallet-rpc, not by fmonero`
(v5.2: the old hint named `wallet set-daemon`, a one-off the next `wallet open` undoes — §2m).

**Remote node mode (§2m) does not weaken one word of this paragraph, and that is the point.**
`settings.node.remote` is **not a URL any fmonero client ever sees**: it is rendered into the
wallet unit's `--daemon-address` and sent as `set_daemon`'s `address`, both of which are consumed by
`monero-wallet-rpc`. `rpc::require_loopback` is unchanged, both constructors still call it first,
and `settings set node.url` / `settings set wallet.url` still validate through it — `node.url` stays
loopback-only **and is unused in remote mode** rather than being relaxed, because a key that is
loopback-only in one mode and free in another is a key whose validator every later agent has to
re-derive. In remote mode `NodeClient::from_ctx` does not construct a client at all: it returns
`Error::Backend{reason:"remote_node"}` (§5b), so there is no code path in which fmonero's own
`ureq` agent is handed a public address. *Test:* a unit test sets `node.mode = remote` and
`node.remote = node.example.com:18089` and asserts `NodeClient::from_ctx` returns exactly that
error — the enforceable form of "fmonero never speaks to a remote node", which a grep cannot state.
The two compile-time greps (iv) and (v) below are unchanged and still pass: remote mode adds no
crate, no TLS backend and no socket.

**Decision: Option 2 (Digest authentication), for both daemons.** The rationale that settles it is
not "defence in depth" but a specific attack that loopback binding does not stop: any web page the
user visits can `fetch("http://127.0.0.1:18083/json_rpc", {method:"POST", body:…})`. With
`--rpc-access-control-origins` empty (§1b.5) the page cannot *read* the reply — but a
`sweep_all` does not need a readable reply. Digest auth stops the request from being *executed* at
all: the unauthenticated POST gets a 401 and the handler never runs, and a browser cannot complete
a Digest handshake it has no credentials for. That is the difference between an annoyance and a
money-losing bug, so Option 1 is rejected.

Option 2 is also nearly free here, because of §1b.5:

- **Wallet unit: pass neither `--disable-rpc-login` nor `--rpc-login`.** `monero-wallet-rpc`
  generates its own 128-bit credential and writes `monero-wallet-rpc.<port>.login` into its
  working directory, and deletes it when it exits. The file ends up **mode 0400** — owner read
  only: `private_file::create` opens it with `S_IRUSR`, raises it to `S_IRUSR|S_IWUSR` only long
  enough to obtain a write handle, then restores the original mode
  (`src/common/util.cpp:215-224`). fmonero's doctor therefore checks "no group and no other bits",
  not a literal mode, so it stays right if upstream changes the dance. The unit sets
  `WorkingDirectory=%S/fmonero/run`, so the file lands at
  `~/.local/state/fmonero/run/monero-wallet-rpc.18083.login`. **fmonero never generates, writes or
  rotates this secret** — it reads `user:pass` from that file at client-construction time and keeps
  it in memory. **An absent file is not a construction failure**: `WalletClient::from_ctx` returns
  `Ok` with `auth: None`, so the local checks of §2a.1 steps 1–4 still run in their stated order and
  no constructor decides a money-path exit code. The absence surfaces at the first request — a
  refused connect is exit 4 `reason: unreachable`, and a 401 from a daemon whose credential we could
  not read is exit 4 `reason: auth_refused`, the same class of answer ("the wallet daemon is not
  usable") with a discriminator that says which.

  **How a test supplies the credential.** This rule is the single precondition standing between the
  design and roughly half of §4b, and it is invisible from any one section — §2c states it, §4b
  states the scenarios, §8 states the gates, and nothing joins them. `WalletClient::from_ctx` reads
  `paths.wallet_login_file(settings.wallet.port)`, and `Paths::run_dir` follows `FMONERO_STATE_DIR`
  (§4.2), so **every test that drives the binary starts the mock with `--auth fmonero:<pass>` and
  writes that same `fmonero:<pass>` to `$FMONERO_STATE_DIR/run/monero-wallet-rpc.<port>.login` at
  mode 0600 *before* the first fmonero invocation.** That is one helper — `mock_wallet_up()` in
  `tests/e2e.sh` — and the same three lines in the in-process test helper. There is **no
  unauthenticated wallet path that works**: with no login file the client sends unauthenticated
  requests, a daemon that requires Digest answers 401 twice and the command exits 4
  `reason: auth_refused` (§2a.1), so a test that omits the file is testing exit 4, not the command
  it named. (The §4b GREEN row for `wallet open` with no unit is still exit 4 `reason: unreachable`:
  nothing is listening at all.) The §4.2 escape ("a test never installs a unit: it points `WalletClient::new` at
  `fmonero-mock --auth` instead") covers in-process unit tests only — not `tests/e2e.sh`, not the
  Phase 2 doctor gate, and not the TUI's `ProgramSimulator` send test, all of which go through a
  `Context`.
- **Node unit: `rpc-login=fmonero:<32 hex chars>` inside `~/.config/fmonero/monerod.conf`**, mode
  0600, referenced by `--config-file`. It is not in argv — `/proc/<pid>/cmdline` is world-readable
  — and `monerod` does not auto-generate one (§1b.6). The 16 random bytes come from
  `/dev/urandom`. This credential is an access token for a local daemon, in the same class as
  fbittorrent's WebUI password; it is explicitly **not** covered by the "no secrets on disk" rule
  of §2b, which is about wallet key material, and §2b's output-grep test therefore excludes
  `~/.config/fmonero/monerod.conf` from what it reads but asserts the token never appears in
  `--json`, `-v` or a doctor probe.
- **`crates/fmonero-core/src/rpc/digest.rs`, ~130 lines, `md-5 = "0.10"`**, shared by both clients,
  written against the four facts of §1b.7:
  - A 401 carries **two** `WWW-Authenticate: Digest …` headers. Iterate all of them and take the one
    whose `algorithm` is exactly `MD5` (case-insensitive) or absent; never `MD5-sess`, whose HA1 is
    a different formula (`http_auth.cpp:548-551`).
  - `HA1 = md5(user:realm:pass)`, `HA2 = md5(method:uri)` where `uri` is the request path exactly as
    sent (`/json_rpc`, `/get_info`, …), `response = md5(HA1:nonce:nc:cnonce:auth:HA2)` under
    `qop=auth`. A fresh 128-bit `cnonce` per request; `nc` as 8 lowercase hex digits, unquoted;
    `cnonce` quoted.
  - **`nc` resets to `00000001` every time the stored nonce changes**, because the server zeroes its
    own counter whenever it issues a nonce and then requires equality (`http_auth.cpp:734`, `:749`,
    `:335-345`). A monotonic counter that is not reset costs a `stale` 401 plus a retry on every
    request — two round-trips forever, silently doubling the §2l budget.
  - `Session` carries no `opaque`: the server never sends one (`:676-683`).
  - The **RFC 2069 no-`qop` branch is not implemented** (§2h): the server advertises
    `qop="auth"` unconditionally (`:671`), so that branch could never execute.
  - The `Session` lives behind a `Mutex` inside the client, and the whole sequence — authorize,
    send, and on any 401 re-read the challenge and retry exactly once — holds that lock, so requests
    from the TUI's several threads (§7a's poll subscription, action subscription and history task)
    cannot race the counter. The agent is built with an **idle connection pool of one**, because
    epee's auth session is per connection (`http_protocol_handler.h:186`) and a second connection
    would start from no nonce. Any 401 is re-challenged, with or without `stale=true`; a second 401
    in a row is exit 4 `backend`.
- **A node with no credentials still works**, unauthenticated, because Arch's system unit has none.
  The client's `auth: Option<Login>` is `None` there. The doctor reports it as a `warn`, not a
  `fail`, with the fix being `fmonero setup --units` (which only ever writes fmonero's own units).

*Tests:* the mock server (§8) implements the same **two-header** Digest challenge and asserts
(i) the client picks the `MD5` challenge and never `MD5-sess`, (ii) the second request on one nonce
carries `nc=00000002`, and (iii) after the mock rotates the nonce the next request carries
`nc=00000001` again; a unit test asserts `NodeClient::new("https://example.com")`,
`NodeClient::new("http://10.0.0.5:18081")` and a `localhost` that resolves off-loopback all fail
with exit 2, **and the same three cases against `WalletClient::new`**, plus
`fmonero settings set wallet.url http://10.0.0.5:18083` exiting 2 with `reason: non_loopback_url`; `grep -rn 'rpc-login' crates/*/src` shows it only in the config-file writer, never in
an argv array. **The two compile-time claims of this section get greps in `scripts/check.sh`, each
with a self-test fixture proving it fires, as §2f's has** — an unenforced compile-time property is
one `Cargo.toml` edit away from being false, and §3.1 already names an escape hatch that would take
it silently:
(iv) `cargo tree -p fmonero -e normal --prefix none` has no line matching
`^(rustls|native-tls|openssl|openssl-sys|hyper-tls|tokio)` — the "no TLS stack in the binary" half;
if §3.1's `rustls` fallback is ever taken, this grep is relaxed **in the same commit** that records
the reason in DECISIONS.md;
(v) `grep -rn 'TcpListener\|UdpSocket\|\.bind(' crates/fmonero-core/src crates/fmonero/src` is empty
(`crates/fmonero-mock` and `#[cfg(test)]` helpers are excluded by path) — the "fmonero never binds a
port" half.

### 2d. Never hang

- **One helper.** `fmonero_core::proc::run_with_deadline(argv, timeout, cancel) -> Result<Output,
  Error>`, copied verbatim from `fproton/crates/fproton-core/src/proc.rs`: own process group, all
  three stdio `Stdio::piped()`, SIGTERM then SIGKILL after 2 s, a 25 ms poll of the deadline and
  the `CancelToken`, and a bounded 200 ms drain after reaping so a `setsid` descendant holding the
  pipe cannot stall the reader. A killed child is `Output { timed_out: true }` — data, not a panic.
  A cancelled one is `Err(Error::Interrupted)`.
- **Every HTTP call has a deadline.** The `ureq::Agent` is built once per client with
  `.timeout_global(Some(d)).http_status_as_error(false)` — fbittorrent's shape
  (`fbittorrent/src/qbt.rs:249-252`) — so there is no code path that can call `ureq` without one.
  Defaults, all overridable by `--timeout` and `settings set timeouts.*`: node probe **3 s**,
  wallet probe **5 s**, `refresh` **120 s**, `transfer`/`sweep` build **120 s**, `relay_tx` **30 s**,
  `open_wallet` **300 s** (opening a large wallet is slow), `systemctl` **10 s**,
  unit-start health probe **30 s**. *Test:* `scripts/check.sh` runs
  `grep -rn 'ureq::' crates/*/src` and fails on any hit outside
  `crates/fmonero-core/src/rpc/mod.rs`, with the same self-test fixture §2f's grep uses — the
  `Agent` is constructed in exactly one function and `timeout_global` is set there, and §2c already
  argues that an unenforced structural claim is one edit away from false. Without the grep, "no code
  path can call `ureq` without a deadline" rests on a single e2e route test.
- **`poll_inflight` guard.** One `AtomicBool` per poller. The TUI's 1 s `Every` and the plugin's
  timer both skip a tick while the previous call is in flight, so `refresh` calls can never pile up
  (FEASIBILITY §7 names this as the wallet-rpc quirk to design around).
- **A 60 s breaker after a deadline kill.** When a call made by a **command that needed that
  daemon** is killed at its deadline, state records `{node|wallet}.stall = { at, backoff_until }`.
  **`status` never arms a breaker — it only reports one.** `monero-wallet-rpc` serves RPC on
  **one** thread (`epee::http_server_impl_base<wallet_rpc_server, connection_context>::run(1, true)`,
  `src/wallet/wallet_rpc_server.cpp:291-292`, under upstream's own comment "DO NOT START THIS SERVER
  IN MORE THEN 1 THREADS WITHOUT REFACTORING"), so every probe issued while another caller holds a
  long call — `open_wallet` (300 s), a `transfer`/`sweep_all` build (120 s), `refresh` (120 s) — is
  killed at its 5 s deadline **by construction**, not exceptionally. The plugin polls `status` every
  3–8 s and the TUI every second, so letting those probes arm the breaker would raise
  `wallet_stalled` at severity *error* during every successful send and every wallet open and refuse
  wallet commands for 60 s afterwards; the `blocks_behind > 100` exemption below does not rescue it,
  because a wallet blocked inside `open_wallet` answers nothing and `blocks_behind` is then `null`.
  A breaker that fires on correct behaviour is the breaker an agent disables.
  **`send::relay` likewise never consults a breaker**: once a plan is built the §2a.11 `send.lock`
  is held and the transaction exists, so exiting 5 there would abandon a built transaction rather
  than protect anything. During the backoff every command that
  needs that daemon exits **5 `upstream`** immediately with the doctor hint, rather than queueing
  another 120 s wait. **`--force`** (a global flag, §4) on the affected command overrides it for
  that invocation; it bypasses nothing else. `status` is never blocked by a breaker: it reports it.
- **The wallet daemon blocks itself while it syncs, and that is not a stall.**
  `monero-wallet-rpc` refreshes on its own idle handler every `DEFAULT_AUTO_REFRESH_PERIOD = 20 s`
  (`src/wallet/wallet_rpc_server.cpp:63`) on the same thread that serves RPC, and while the wallet
  is behind the tip it deliberately keeps blasting through block chunks, yielding only ~200-300 ms
  per period — upstream's own comment is that otherwise "other calls to the RPC will hang"
  (`:211-280`). Against a 5 s wallet probe, the hot-path calls will routinely be killed at their
  deadline for hours after `wallet open` on a wallet with a restore height. Therefore:
  (1) immediately after a successful `open_wallet`, fmonero issues `auto_refresh { enable: true,
  period: 20 }` — the upstream default `DEFAULT_AUTO_REFRESH_PERIOD`
  (`src/wallet/wallet_rpc_server.cpp:63`) — so the setting is **explicit rather than inherited**,
  and it is deliberately **not raised**. The throttle works the opposite way round from the obvious
  reading: while the wallet is behind the tip it keeps blasting through chunks and yields the RPC
  thread for only `auto_refresh_evaluation_ms + 100 ms` (200–300 ms) once per *whole*
  `auto_refresh_period` of uninterrupted syncing (`:215`, `:257-280`, the yield computed at
  `:270-276`). A **larger** period therefore means **longer** blocking bursts and fewer of them, so
  the period is a floor on responsiveness, not a ceiling on a stall — `period: 30` would have made
  the very problem this bullet names measurably worse. The real mitigation is (2);
  (2) a wallet call killed at its deadline while `status.wallet.blocks_behind > 100` is classified
  **`wallet_syncing`, not a stall** — it raises the `wallet_behind` warning and does **not** arm the
  breaker. Only a timeout with the wallet within 100 blocks of the node arms it.
  **In remote mode no wallet call killed at its deadline ever arms the breaker** (v5.2).
  `blocks_behind` — the one discriminator this bullet has between "catching up" and "stuck" — is
  `node.height - wallet.height`, and in remote mode `node.height` is `null` because no wallet-RPC
  method reports the daemon's height (§1b.9 item 5). The rule above already chooses "do not arm"
  for the other case where `blocks_behind` is `null` (a wallet blocked inside `open_wallet`), and
  the alternative here is a breaker that fires on the ordinary first sync against a public node and
  then refuses every wallet command for 60 s — the breaker an agent disables. The §2m send gate's
  own deadline miss is exit 10 `wallet_catching_up`, which is the same instruction ("wait, then
  retry") with no 60 s penalty attached.
  **100 blocks ≈ 3.3 hours** at Monero's 120 s target block time (§1b.1) and comfortably exceeds the
  10-block spendable age (§1b.4), so it separates "still catching up" from "stuck" without firing on
  a wallet that is merely a few minutes behind. It is **one constant, `WALLET_BEHIND_BLOCKS`**, used
  by this classification, by the `wallet_behind` warning (§5), and by `wallet_daemon_link` and
  `wallet_daemon_auth` (§7.1) — four uses, and changing it must change all four together, which a
  literal `100` in each place would not. `status` degrades
  the affected wallet fields to `null`, sets `staleness.wallet_probe_ms` to the deadline, and still
  exits 0 (§2e).
- **`open_wallet`'s 300 s deadline is the one long one**, and it runs as a `Subscription` in the
  TUI and with a progress line on stderr in the CLI, never silently.
- **No command waits without a bound.** `node up` without `--wait` is bounded by 30 s; `node up
  --wait` by `--wait-timeout`, **default 1 800 s**, and the only way to ask for an unbounded poll is
  `--wait-timeout 0`, which is refused (exit 2) when stdout is not a TTY because a non-interactive
  caller has nothing that can interrupt it (§4.1). `node log --follow` is refused outside the TUI.
  A command that can block for days is a command an agent cannot call, and §4b's own rehearsal
  script runs `node up --wait` on a chain that takes hours.

*Test:* an e2e case points the node URL at a mock route that sleeps 10 s with `--timeout 2` and
asserts exit 5 and wall clock between 1.5 s and 5 s; a second invocation in the same state dir
asserts the fast breaker path; a third with `--force` asserts the call is attempted; a fourth points
the wallet URL at a slow route while the mock reports a large `blocks_behind` and asserts that no
breaker is armed and `status` still exits 0 — asserted on a foreground `wallet balance` as well,
since `status` never arms a breaker at all and the classification would otherwise be untested; a fifth runs
`node up --wait --wait-timeout 2` against a mock whose `get_info` never reports `synchronized` and
asserts **exit 8** with wall clock under 4 s.

### 2e. `status` never starts anything

`fmonero status` issues HTTP GET/POSTs to whatever is already listening and runs at most one
`systemctl --user is-active` per unit. It never runs `systemctl start`, never spawns `monerod` or
`monero-wallet-rpc`, and never calls `open_wallet`, `refresh` or any other wallet mutator. It *does*
write `state.json` — the 60 s cold cache, the `get_net_stats` sample pair and `staleness` — which is
not a contradiction but a distinction worth stating precisely, because the plugin calls `status`
every 3 s: see §5 for the atomic-write rule that makes concurrent `status` invocations safe. A daemon that is down is
`reachable: false`, not an error: `status` exits **0** with warnings. *Test:* an e2e case runs
`status --json` with both units stopped, asserts exit 0, and asserts `systemctl --user is-active`
still reports them inactive afterwards.

### 2f. argv only

No shell strings anywhere, in Rust or QML. Every external command is an argv array. User input is
validated before it reaches argv; in practice almost nothing does, because the RPC bodies are JSON.
The only argv fmonero builds from data are `systemctl --user <verb> <unit>` where the unit name is
one of two constants. *Test:* `scripts/check.sh` greps for `"(sh|bash)"\s*,\s*"-c"` and
`Command::new\("(sh|bash)"\)` across `crates/*/src`, with the same three-case self-test fproton's
`check.sh` uses so the grep cannot be vacuous; `tests/plugin-smoke.sh` greps the QML for
`bar.run(` and `execDetached(["bash"`.

### 2g. Silent core, typed error

No `println!`, `eprintln!`, `print!` or `dbg!` anywhere in `crates/fmonero-core/src`. All output is
produced by `crates/fmonero/src/output.rs`. Every core function returns `Result<_, Error>` where
`Error` has one variant per exit code (§4a) and `exit_code()`, `code_name()`, `hint()`. `anyhow` is
a dependency of the **binary only**. *Test:* the grep is a `check.sh` step; `fmonero-core`'s
`Cargo.toml` has no `anyhow`.

### 2h. No stubs

No function in shipped code returns fake data, a hard-coded example, or `todo!()`. A capability
that is not implemented is not in the CLI surface at all. The one sanctioned "not implemented" is
`Error::unimplemented(what)` (copied from fproton's `error.rs:129-134`), which exits 1 and names
the gap — and by the Phase 3 gate no shipped path constructs it. *Test:*
`grep -rn 'todo!\|unimplemented!\|FIXME\|XXX\|placeholder' crates/*/src` is empty, and the
`mock-code-finder` skill is run once in Phase 4.

### 2i. Coexistence with Arch's node

fmonero **never starts a second `monerod` on a data dir that is already served**. Before any
`node up`, and as a doctor check, it determines the layout:

| Layout | How it is detected | What `node up` does |
|---|---|---|
| `user_unit` | `settings.node.mode == local` **and** `~/.config/systemd/user/fmonero-monerod.service` exists. **Ownership is decided by configuration alone; who currently holds the data dir is a separate question, answered below.** | the preflight and the single-writer precondition below, then `systemctl --user start fmonero-monerod.service`, then poll `get_info` until it answers or 30 s |
| `system_unit` | `settings.node.mode == local` **and** (`systemctl is-active monerod.service` (no `--user`) is `active`, or `/etc/monerod.conf` exists and `systemctl is-enabled monerod.service` succeeds) | exits **3 `safety`**: `hint: sudo systemctl start monerod   # fmonero does not use sudo`. Status, info, peers, bans and the wallet all work against it, subject to §1b.2 if it is restricted. |
| `remote` | **`settings.node.mode == remote`** (v5.2) — a settings fact and nothing else | exits **4 `backend`**, `reason: remote_node`, `hint: fmonero node local --yes`. So does every other node command that needs the local daemon (§2m, §4.1). |
| `none` | `settings.node.mode == local` and nothing found | exits **4 `backend`**: `hint: fmonero setup --units --yes` — the hint is a command an agent runs, and `fmonero setup --units` alone exits 3 `needs_yes` |

**`Remote` is tested first, and it is keyed on `node.mode` alone** (v5.2). The v5.1 rule —
"`settings.node.url` is not loopback" — was **unreachable through the CLI**: `settings set node.url`
validates through `rpc::require_loopback` (§2c) and refuses every value that would have produced it,
so the branch could be reached only by hand-editing `config.toml`, and where it *was* reached every
node client failed to construct and every node command exited 2. A layout variant that the tool
cannot enter and that does nothing useful when entered is a variant that documents a feature
nobody has. It is replaced, not deleted: `node.mode = remote` is the same variant with a reachable
key, a specified refusal (exit 4 `remote_node`, not exit 2 `usage` — the arguments are fine, the
backend is elsewhere) and a way back (`fmonero node local --yes`). `detect_layout` therefore reads
`ctx.settings.node.mode` first and returns `Layout::Remote` before it stats a unit file or spawns a
`systemctl`, which also makes remote mode's `status` cheaper than local mode's rather than more
expensive.

**The single-writer probe is a precondition of `node up`, not a term in `detect_layout`.** It cannot
distinguish fmonero's own running `monerod` from a foreign one — the fcntl probe reports whoever
holds `lmdb/lock.mdb`, and the `/proc` scan matches any `monerod` on that data dir, ours included.
Folding it into layout detection would mean that the moment `node up` succeeds, the layout stops
being `UserUnit`, `may_mutate_node` turns false, and `node down`, `node restart`, `node ban`,
`node unban`, `node limit --up`, `set_log_level` and the TUI's `u`/`d`/`L` all refuse — about
fmonero's own node, in exactly the state where the user needs them. So:

- **`node up`** runs `data_dir_in_use(ctx, settings.node.data_dir)` and classifies the holder: no
  holder ⇒ start; holder PID equals our unit's `MainPID` (`systemctl --user show -p MainPID <unit>`)
  ⇒ **exit 0, already running** (idempotent, no second start); any other PID ⇒ **exit 3 `safety`**
  with `error: "monerod pid <N> already has <data-dir> open"` and
  `hint: fmonero status --json   # a monerod is already serving this data dir`.

**Two questions come before any `systemctl` call, and both answer within milliseconds** (v5.1).
`units::preflight(ctx, unit, binary)` asks them, and `node up`, `node restart`, `wallet open` and
`wallet create` all go through it:

1. **is the unit file in `paths.systemd_user_dir`?** Else exit **4 `backend`**,
   `reason: unit_missing`, `hint: fmonero setup --units --yes`.
2. **is the daemon binary (`monerod`, `monero-wallet-rpc`) on `PATH`?** Else exit **4 `backend`**,
   `reason: not_installed`, `hint: omarchy pkg add monero` — the same `proc::which` probe the
   doctor's `monerod_installed` already makes.

Without (2), a machine `setup --units` has configured and `omarchy pkg add monero` has not reached
answers `node up` with **exit 8** after thirty seconds — §4a's "nothing is broken", whose hint points
at a `status` document that will say `reachable: false` without saying why — and leaves the unit
restarting every five seconds on `203/EXEC`, which is systemd saying the binary is not there. §4a
assigns that condition exit **4**. This is the state `scripts/install.sh --yes` leaves on a machine
with no Monero, so it is the first thing an owner meets; it was finding 1 of the Phase 4
verification, and §4.2 spends a paragraph on the identical failure shape one layer down (a missing
`wallet.dir` ancestor). `node restart` asks **before** its own `down`, because a restart that stops
the node and then reports "monerod is not installed" has left the machine worse than it found it.

**Where each question lives, and why the wallet path differs.** For `node up`/`node restart` the
preflight runs immediately after the layout gate. For `wallet open`/`wallet create` it runs inside
`units::ensure_running`, **after** the probe and immediately before `units::start` — because
`ensure_running` probes first precisely so that every AMBER wallet row of §4b works against
`fmonero-mock` with no unit installed (§5b), and a preflight ahead of the probe would refuse all of
them. A daemon that is already answering needs neither a unit file nor a binary on this machine's
`PATH`.

**(2) is skipped under `Systemctl::Fixture`.** The §2i seam already stands in for the machine's
systemd; a fixture that reports a unit as present and active is standing in for its daemon too, and
`fmonero-mock` is what answers the RPC in every one of those rows. The seam is honoured only under
`FMONERO_STATE_DIR`, so no real invocation can reach the exemption, and `units`' own unit test drives
`preflight` directly with an explicit binary name — so the check is not exempt from being tested, and
`tests/e2e.sh` proves the real answer with no fixture set at all.

**A unit fmonero started that never answered is stopped again** (v5.1). `units::ensure_running` and
`node_session::up` both poll to a deadline; on expiry, and only when *they* issued the start, they
run `systemctl --user stop <unit>` followed by `systemctl --user reset-failed <unit>` and say so in
the error. Without it every such failure leaves a `Restart=on-failure` loop behind a command that has
already failed — which is what the Phase 4 verifier's runs left on this machine, twice, and had to
undo by hand. `reset-failed` is part of it because the units now set their own start limit (§4.2): a
unit that has hit it refuses the *next* start until the failed state is cleared, and an owner who has
just installed Monero must not meet that.
- **`node down` / `node restart`** require only `Layout::UserUnit`; they never consult
  `data_dir_in_use`.
- The doctor's `node_data_dir_single_writer` uses it as a report-only check.

"Served by another process" is decided without `lsof` or `fuser` (neither is guaranteed installed),
by two independent probes; if either says yes, `node up` refuses:

1. Read `/proc/*/cmdline` for a `monerod` whose `--data-dir` (or whose `--config-file`'s
   `data-dir=`) resolves to the same canonical path.
2. Test the **POSIX record lock on `<data-dir>/lmdb/lock.mdb`**. monerod's LMDB environment lives in
   `<data-dir>/lmdb/` (`src/cryptonote_core/cryptonote_core.cpp:520`, `folder /= db->get_db_name()`
   with `get_db_name()` returning `"lmdb"`, `src/blockchain_db/lmdb/db_lmdb.cpp:1748`), and its lock
   file is `lock.mdb` (`src/cryptonote_config.h:167`). LMDB locks the environment with **POSIX
   record locks** (`external/db_drivers/liblmdb/mdb.c:5083-5099`)
   — `struct flock { l_type: F_WRLCK, l_whence: SEEK_SET, l_start: 0, l_len: 1 }` through
   `fcntl(F_SETLK)`, never `flock`. (The nearby comment at `mdb.c:4700-4701` about "the flock()
   issues noted under Caveats in lmdb.h" explains why the lockfile gets `FD_CLOEXEC`; it is **not**
   the justification for the lock family, and must not be quoted as one.) On Linux `flock()`
   and `fcntl()` locks are independent namespaces, so an `flock(LOCK_EX|LOCK_NB)` probe **succeeds
   while monerod holds the environment** — it would report "free" exactly when it must say "served",
   and this is the only guard standing between fmonero and the LMDB corruption of §1.2. The probe
   is: open `<data-dir>/lmdb/lock.mdb` read-only, then
   `fcntl(fd, F_GETLK, &{ l_type: F_WRLCK, l_whence: SEEK_SET, l_start: 0, l_len: 1 })`; if
   `l_type != F_UNLCK` the environment is open and `l_pid` names the holder. A missing `lock.mdb`
   means the directory was never opened.

   **The lock a running `monerod` holds is SHARED, not exclusive, and the query type is therefore
   load-bearing.** `mdb_env_excl_lock` takes an `F_WRLCK` only while the environment is being set
   up; `mdb_env_share_locks` — "Downgrade the exclusive lock on the region back to shared",
   `external/db_drivers/liblmdb/mdb.c:5022-5057`, which sets `lock_info.l_type = F_RDLCK` and
   `fcntl(F_SETLK)`s it onto the same byte — replaces it, and that **read** lock is held for the life
   of the process. The query must therefore be made with **`l_type: F_WRLCK`**: a write-lock query
   conflicts with a held read lock, a read-lock query does **not**. An implementation that probes
   with `F_RDLCK` — the natural reading of "is this file locked?" — reports a live monerod's data
   dir as **free**, and `node up` starts a second writer on the LMDB.

**Mutating node calls are gated on layout, and the gate is a type rather than a convention.** The
five mutators — `stop_daemon`, `set_bans`, `set_limit`, `set_peers_count` and `set_log_level` —
**do not exist on `NodeClient` at all**. They exist only on `NodeMutators`, and the only way to
obtain one is `NodeClient::mutators(&ctx)`, which returns `Error::Safety` unless
`units::detect_layout(ctx) == Layout::UserUnit` (§5b). There are eight or more call sites across
three phases and three agents — `node down`, `node restart`, `node ban`, `node unban`,
`node limit` with args, the TUI's `u`/`d`/`L` and ban actions, and the plugin's `node up`/`node down`
argv — and §2a.3 already rejects "every caller performs the check" for the send path in this
document's own words: *an invariant that lives in both of them is an invariant that holds in one*.
Making the gate structural means a Phase 2 agent who forgets it gets a **compile error** instead of
a silently working `node down` against Arch's system daemon, reachable from a right-click on the bar
widget (§6.4). Under `system_unit` — and under `remote` — constructing the mutators
fails **3 `safety`** *before any RPC is issued*, with
`hint: sudo systemctl <verb> monerod   # fmonero does not mutate a node it does not own`.
Read-only calls (`get_info`, block headers, `sync_info`, `get_connections`, `get_peer_list`,
`get_bans`, `get_limit`, `peers_count()` — which always sends `set: false` — txpool and net stats)
stay on `NodeClient` and run against
any layout. Without this, `node down` — specified as "`POST /stop_daemon` if reachable" — would
successfully shut down Arch's system daemon over RPC, and `node ban`/`node limit` would silently
reconfigure it; §0's "detected, used read-only, never fought" would be an aspiration with an active
counter-example, reachable from a right-click on the bar widget (§6.4). `--force` does **not**
override this; there is no override. The TUI disables `u`, `d`, `L` and the ban actions with the
same text (§7a).

**The test seam, frozen now because §2k freezes the API at the 1a gate.** `detect_layout` and
`data_dir_in_use` read the real `/proc` and shell out to the real `systemctl`, and no agent on this
machine can create `/etc/monerod.conf` or a system unit (§1.1). So `Paths` carries
`proc_root: PathBuf` (`/proc`, overridden by `FMONERO_PROC_ROOT`), **`systemd_user_dir: PathBuf`
(`~/.config/systemd/user`, overridden by `FMONERO_SYSTEMD_USER_DIR` — v5.1, §4.2)** and `Context` carries
`systemctl: Systemctl` — `Real`, or `Fixture(PathBuf)` from `FMONERO_SYSTEMCTL_FIXTURE`, a file of
`<unit> <is-active> <is-enabled> <exists> <main-pid>` lines (`-` for an absent pid). Under
`Fixture`, `units::state`, `units::main_pid` **and `detect_layout`'s unit-file-exists test** all
read that file instead of stat'ing `paths.systemd_user_dir` or spawning `systemctl`, and
`units::{start, stop}` and the daemon-reload are **no-ops that rewrite the `<is-active>` column of
the named line** and return `Ok`. Without those four, nothing can reach `Layout::UserUnit` in a test
at all — `units::install` refuses to run under `FMONERO_STATE_DIR` (§4.2), so no test may write a
real unit — and §2d's `node up --wait --wait-timeout 2` case, the `user_unit` + write-lock case
below and §4b's AMBER `node up`/`node down` rows would have no way to run. **All three overrides are
honoured only when `FMONERO_STATE_DIR` is also set**, the same guard `units::install` already uses
to refuse running under a test state dir.

`FMONERO_SYSTEMD_USER_DIR` is the third, and it is the one that makes the *absence* of a unit
testable: `Systemctl::Fixture` answers `is-active`/`is-enabled`/`exists`/`MainPID` and nothing more,
while `detect_layout`, `units::preflight`, `units::install` and six §7.1 doctor checks all read the
**directory**. v5 left it out; the Phase 2 doctor added it inside `probes.rs` as a recorded
deviation, so the doctor and the rest of the code could disagree about which directory a test was
looking at; and the Phase 4 verifier's one real `scripts/install.sh --yes` then made ten assertions
of this project's own suite fail on their own premise, because `tests/e2e.sh` could sandbox state,
config and cache but not this. It is one field, honoured in `Paths::discover` under the same guard
as the other two, and `probes.rs` reads the field.

*Tests:* a doctor fixture supplies a synthetic `/proc` tree plus a systemctl fixture and asserts the
`node_layout` verdict for all four cases; **a unit test holds an `fcntl` READ lock (`F_RDLCK`,
`l_start: 0, l_len: 1`) on a temporary `lock.mdb` from a forked child and asserts `data_dir_in_use`
returns `Some(child_pid)`** — this is the lock a real monerod holds (`mdb.c:5050`), and it is the
only case that distinguishes a correct `F_GETLK{F_WRLCK}` probe from an `F_GETLK{F_RDLCK}` one,
which would report a live data dir as free; plus a write-lock case asserting the same, and a
negative case asserting that an
`flock`-only holder is **not** detected (documenting why `flock` was rejected); an e2e case with the
layout fixture set to `system_unit` asserts that `node down`, `node restart`, `node ban 1.2.3.4`,
`node unban 1.2.3.4`, `node limit --up 1024` and the TUI's log-level action each exit 3 and that the
mock recorded **zero** requests; **and the case that would have caught the layout/occupancy
conflation** — a unit test holds an `fcntl` write lock on a temporary `lock.mdb`, sets the layout
fixture to `user_unit`, and asserts that `node down`, `node ban` and `node limit --up 1024` are
**allowed** (they reach the mock) while `node up` exits 3 naming the PID.

### 2j. Honest

README, `--help`, `robot-docs` and `capabilities.build_note` claim only what §4b marks GREEN or
AMBER, and say which. A line describing a RED row as working is a Phase 3 blocker. Fixtures say in
their header whether they were captured or derived. `DECISIONS.md` is appended to, never rewritten.

### 2k. Ownership

Touch only the files §8 assigns you. Never `cargo clean`. **Nobody edits a `Cargo.toml` after the
Phase 1a gate** — a later agent missing a crate stops and reports it to the orchestrator.

### 2l. Fast

Three separate claims, because only two of them can be measured on this machine and conflating them
would put an invented number in DECISIONS.md (§2j):

| Claim | Budget | Class | Measured where |
|---|---|---|---|
| (a) `fmonero status --json` with **both daemons absent** | **60 ms** | GREEN | Phase 1b gate |
| (b) `fmonero status --json` against `fmonero-mock` on loopback | **80 ms** | AMBER — it measures our HTTP, JSON and rendering path, not a real daemon | Phase 1b gate |
| (c) `fmonero status --json` warm with real `monerod` + `monero-wallet-rpc` | **80 ms target** | **RED** — a target, not a measurement; must not be claimed anywhere until the owner runs the §4b list | §4b RED list |
| (d) `fmonero wallet history --limit 50 --json --cached` (cache hit, zero RPC) | **50 ms** | AMBER | Phase 1b gate |

The `status` figure covers, with a wallet open, five loopback RPCs (node `get_info` and
`get_last_block_header`; wallet `get_height`, `get_balance`, `get_address` — §5) **plus two
`systemctl --user is-active` process spawns**, which are the expensive part. Without `--cached`,
`wallet history` tops up from the wallet daemon first and has **no** budget: it inherits
`timeouts.wallet_sec`. Measured as §1.4 says and pasted into DECISIONS.md — a miss is recorded with
the real number, not hidden.

**Claim (a)'s budget is set above its own measured floor, not below it.** Two
`systemctl --user is-active` spawns measure **~19 ms for the pair** on this machine (20-iteration
loop, 371 ms total, 2026-09-16), and they dominate; 30 ms left ~11 ms for process start, settings
load, two refused loopback connects, a state read-modify-write under `flock` and JSON rendering,
which is probably unreachable — and a GREEN budget set below its floor is a number that gets quietly
relaxed on first contact, the §2j failure in the section that exists to make speed claims honest. If
a future `status` drops to one spawn or caches unit state, revisit it.

### 2m. Remote node mode (v5.2)

**Why.** v0.1 assumed one shape: fmonero owns a local `monerod`, and everything — the dashboard,
the peers table, the send gate, the doctor — reads it directly. That shape costs 120–150 GB and
days of initial sync (§1.3), and the owner of this machine has Monero installed and does not want
either. A wallet does not need a local chain: `monero-wallet-rpc` will talk to any node, and
**that** is the one process in this design that was always allowed to leave loopback. So remote
mode is not a new transport — it is the decision to stop pretending the local node exists, stated
once, in settings, with every consumer reading the same key.

Three things make it a *mode* rather than the one-off `wallet set-daemon` already in §4.1. First,
`wallet_session::open` issues `set_daemon` after **every** `open_wallet` (§4.2), so a one-off is
undone the next time a wallet is opened — a setting the tool silently reverts is worse than no
setting. Second, the wallet **unit** carries `--daemon-address` and one of
`--trusted-daemon`/`--untrusted-daemon`, and a `monero-wallet-rpc` started before any `wallet open`
auto-refreshes against whatever the unit said (§1b.9 item 3), so the unit and the call must agree or
the wallet syncs from a node `status` does not name. Third, every *other* component — the node
commands, `status`, the doctor, the TUI, the bar widget — has to change behaviour together, and a
flag on one command cannot coordinate that.

#### 2m.1 The mode model

Three settings keys, and nothing else is consulted anywhere:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `node.mode` | `local` \| `remote` | `local` | Which daemon `monero-wallet-rpc` is pointed at. The **only** input to `units::detect_layout`'s `Remote` branch (§2i). |
| `node.remote` | address string | `""` | The daemon address, in the grammar of §2m.2. Read by `units::render_wallet_unit`, `wallet_session::open`, `status`, `node info` and the doctor — all through the one accessor `units::daemon_target` (§5b). |
| `node.remote_trusted` | bool | `false` | Sent as `set_daemon`'s `trusted` and rendered as `--trusted-daemon`/`--untrusted-daemon`. **A public node is never trusted**; this key exists only for a remote node the owner runs (§2m.6). |

**The invariant, enforced by `Settings::set` and relied on everywhere:
`node.mode == remote` implies `node.remote` is a valid address.** `settings set node.mode remote`
with `node.remote` empty exits **2**, `reason: remote_unset`,
`error: "node.mode = remote needs node.remote to be set first"`,
`hint: fmonero node remote <host>:18089 --yes`; and `settings set node.remote ""` while the mode is
`remote` exits **2** with the same reason,
`error: "node.remote is in use by node.mode = remote"`,
`hint: fmonero node local --yes   # then clear node.remote`. Without the invariant every consumer
needs an "empty address in remote mode" branch, and the honest behaviour of that branch (refuse
everything) is a fourth layout nobody specified.

**`settings.node.url` stays loopback-only and is simply unused in remote mode.** It is not relaxed
and not cleared: `node local --yes` must find it, and §2c's argument for the loopback rule does not
become weaker because a second key exists. Likewise `node.remote` and `node.remote_trusted` are
**left untouched by `node local`** — they are inert in local mode, and discarding an address the
user will want back is a discourtesy with no benefit.

**`settings set` never spawns systemd and never re-renders a unit**, in this or any other key. So
`settings set node.remote <other>` on a configured machine leaves the wallet unit naming the old
address; the doctor's `wallet_daemon_matches_unit` (§7.1) reports exactly that, and
`fmonero node remote <other> --yes` is the command that does the whole job. That division — a dumb
key writer and a convenience command that composes — is why §2m.5 exists at all.

#### 2m.2 The address grammar, and why each refusal

```
remote-address = [ scheme "://" ] host ":" port
scheme         = "http" / "https"          ; case-insensitive, stored lower-case
host           = IPv4-literal / "[" IPv6-literal "]" / dns-name
dns-name       = label *( "." label )      ; label = 1*63( ALPHA / DIGIT / "-" ), not "-"-edged
port           = 1*5DIGIT                  ; 1..65535
```

Every refusal is exit **2 `usage`**, `reason: bad_remote_address`, and every one carries the same
hint: `fmonero node remote <host>:18089 --yes   # host:port, no path, no credential`.

| Input | `error` | Why it is refused rather than accepted |
|---|---|---|
| `""` | `a remote node address is required` | — |
| `node.example.com` | `the port is required — monero-wallet-rpc fills a missing port with 80 or 443, never 18081` | `abstract_http_client.cpp:139-144` (§1b.9 item 1). Guessing 18081 here would make fmonero's rendering disagree with what `monero-wallet-rpc` does with the identical string in `--daemon-address`; refusing makes the two agree by construction. |
| `http://node.example.com:18089/json_rpc` | `a remote node address is host:port with no path — monero-wallet-rpc discards everything after the port` | `parse_url` puts it in `content.uri` and `set_server` never reads that field (`net_parse_helpers.cpp:165-169`, `abstract_http_client.cpp:146`). Accepting a path would silently do nothing, which is the §2j failure in its purest form. |
| `user:pass@node:18089` | `a remote node address may not carry a username or password` | §0 cuts a remote-node credential. `set_daemon`'s `username`/`password` are sent **only** in local mode, from `monerod.conf` (§4.2). |
| `ftp://node:18089`, `tcp://…` | `only http:// and https:// are understood` | **With the port mandatory the scheme has no effect at all**, so an accepted `ftp://` would be silently discarded rather than honoured — the same §2j failure as the path row. `parse_url`'s regex captures an explicit port in group 6 whatever the scheme (`net_parse_helpers.cpp:181`, `:199-203`) and `set_server` consults its 80/443 table only `if (parsed.port == 0)` (`abstract_http_client.cpp:139-144`), after which only host and port are passed on (`:146`) — §1b.9 item 1's "the scheme is discarded". fmonero refuses a scheme it would only be pretending to understand. |
| `node.example.com:0`, `:70000` | `port must be 1..65535` | `parse_port` (`net_parse_helpers.cpp:96-99`). |
| `[::1:18089` | `an IPv6 address must be bracketed: [2001:db8::1]:18089` | `parse_url_ipv6` (`net_parse_helpers.cpp:135-172`) requires the brackets; without them the last colon is read as the port separator. |

**The scheme is stored but is almost decoration.** `set_server` discards it (§1b.9 item 1): TLS is
decided by `ssl_support` alone. fmonero keeps the user's spelling so that `status`, `node info` and
the wallet unit all show back what was typed, and §2m.6 says in as many words that typing `https://`
does not by itself make the connection private.

**A loopback `node.remote` is allowed.** It is what anyone testing the mode types, it is harmless,
and refusing it would need a second copy of `require_loopback`'s logic inverted. `fmonero node
local --yes` remains the supported way to use fmonero's own node, and §7.1's
`remote_node_untrusted` treats loopback as private.

#### 2m.3 What each component does in remote mode

**`monero-wallet-rpc` unit (§4.2).** `--daemon-address <node.remote>`, exactly one of
`--untrusted-daemon` (default) or `--trusted-daemon` (iff `node.remote_trusted`), and
`--daemon-ssl autodetect` (or `disabled` for a loopback `node.remote`, per the rule below). **No `--daemon-login`**, ever (§4.2's existing rule; `/proc/<pid>/cmdline`
is world-readable) and no local credential of any kind — fmonero's `rpc-login` belongs to fmonero's
own node and means nothing to a stranger's. In **local** mode the same render now emits
`--daemon-ssl disabled` alongside `--trusted-daemon`, which is new in v5.2 and is a correction
rather than a feature: the flag's default is `autodetect` (`wallet2.cpp:253`), so until now our own
unit asked `monero-wallet-rpc` to attempt a TLS handshake against `127.0.0.1:18081` on every
reconnect and fall back (§1b.9 item 2) — a wasted round trip against a daemon we started ourselves
in plaintext. Both modes now render the flag explicitly, and the unit and `set_daemon` cannot
disagree because both read `units::daemon_target` (§5b).

**`wallet_session::open` (§4.2, §5b).** The four-step sequence is unchanged in shape; only the
`set_daemon` arguments move, and they come from `units::daemon_target(ctx)`:

| | local | remote |
|---|---|---|
| `address` | `host_port(settings.node.url)` | `settings.node.remote`, verbatim |
| `trusted` | `true` | `settings.node.remote_trusted` |
| `username`/`password` | `Login::from_monerod_conf`, omitted when absent | **omitted, always** |
| `ssl_support` | `"disabled"` | `"autodetect"` |

**`ssl_support` is derived from the *address*, not from the mode**, and the table above is that
rule's outcome rather than a separate one: `Disabled` when the host is a loopback literal, else
`Autodetect`. In local mode `node.url` is loopback-validated (§2c) so it is always `Disabled`; in
remote mode it is `Autodetect` unless the user pointed `node.remote` at loopback, which §2m.2
allows. Keying it on the mode instead would send `autodetect` to `127.0.0.1` for anyone testing the
mode locally — the wasted handshake this very paragraph corrects in the other direction — and would
leave `wallet set-daemon`, whose ADDRESS is neither mode's key, with a rule of its own. One
derivation, in `units::daemon_target` and in the address `wallet set-daemon` was given (§5b).

**The `-38` exemption is narrowed, because v5 attributed it to the wrong cause.** §4.2 said a `-38`
from `set_daemon` is "raised when wallet2 cannot reach the address"; §1b.9 item 1 shows that
`set_daemon` opens no socket and that `-38` there means the address did not **parse**. In *local*
mode the swallow survives — `node.url` is loopback-validated, the wallet really is open either way,
and `status` reports the truth — but the comment that justifies it now says what it is. In *remote*
mode it is **not** swallowed: the address came from the user through §2m.2's grammar minutes ago, so
a `-38` means fmonero's grammar and epee's parser disagree, and reporting a successful `wallet open`
for a wallet whose daemon was rejected is exactly the silence this design forbids. It exits
**4 `backend`**, `reason: remote_address_rejected`, message verbatim,
`hint: fmonero node remote <addr> --yes`.

**`wallet create` takes the new wallet's restore height from the remote node, before fmonero's
`set_daemon` ever runs.** `on_create_wallet` builds the wallet2 from the *process's* own
`--daemon-address` (`tools::wallet2::make_new(vm2, …)`, `wallet_rpc_server.cpp:3596`), then issues
`wal->invoke_http_json("/getheight", hreq, hres)` and `wal->set_refresh_from_block_height(hres.height)`
with `hres.height` initialised to 0 and left at 0 when the call fails (`:3606-3609`). So in remote
mode the **public node** decides where the new wallet starts scanning, over the address in the
*unit's* argv — which is a third argument for `wallet_daemon_matches_unit` (§7.1), and the hardest
one, because it is the only one that bites before any fmonero call. Two failure shapes, both new in
remote mode and neither recoverable in v0.1:

- **an unreachable node at create time** silently yields restore height **0**, so the very next
  command of §4b's LIVE rehearsal — `fmonero wallet refresh` under `timeouts.refresh_sec` (120 s) —
  scans from genesis and can only ever exit **10 `wallet_catching_up`**. The remedy is to create
  the wallet again once the node answers;
- **a node that reports a height above the true tip** sets a restore height past every block that
  will ever pay the wallet, so received funds are invisible — and §0 cuts both `rescan_blockchain`
  and `scan_tx`, so **fmonero ships no command that recovers from it**. `monero-wallet-cli
  --wallet-file <wallet.dir>/<name>` then `rescan_bc` is the way out, and `wallet create`'s
  `seed_backup_hint` already names that binary.

This is why §4b's LIVE rehearsal asserts a plausible restore height — a `wallet refresh` whose
`blocks_fetched` is not a genesis scan — **before** it records the `wallet refresh` row GREEN.

**`send::build` step 7 (§2a.1).** The `Option<&NodeClient>` argument becomes
`NodeGate { Local(&NodeClient), Unreachable, Remote }` (§5b). Why an enum rather than a documented
second meaning for `None`: `None` already means "the caller proved there is no node ⇒ refuse", and
remote mode means the opposite — proceed, by a different gate. Two opposite behaviours behind one
`None` is the arrangement §2a.3 rejects, on the one code path where getting it wrong either refuses
every remote send or accepts every local send against a node that is down. Three variants make the
caller name the situation and the compiler enumerate the cases.

In the `Remote` variant the gate is **one bounded `refresh { start_height: 0 }`** under
`timeouts.refresh_sec`, cancellable, issued through `wallet_session::refresh` so the
`state::note_refresh` bookkeeping has one home (§5b). It must succeed. What that buys, precisely:
a `refresh` that returns proves the remote daemon answered **and** that the wallet holds every block
that daemon offered (§1b.9 item 6) — which is strictly more than the local gate's
`get_info.synchronized` proves about our own node, since that one is the node's opinion of itself.

**And here is what it does not buy, stated because a send is the one place a half-truth costs
money.** *A remote node's own sync state cannot be observed through `monero-wallet-rpc`.* There is
no method that returns the daemon's height, target height, `synchronized` flag or version:
`get_height` is the wallet's own (`wallet_rpc_server.cpp:943`), and the request definitions contain
nothing else (§1b.9 item 5). So a remote node that is itself hours behind — or lying — answers the
refresh, the wallet reaches *that* node's tip, and the gate passes. The transaction is then built
against a stale tip, and the outcome is a **relay rejection**: `relay_tx` calls `commit_tx`, which
throws on daemon rejection and becomes `-4 GENERIC_TRANSFER_ERROR` (`wallet_rpc_server.cpp:1952-1961`,
§2a.9), i.e. **exit 16 `tx_rejected`, with the funds unaffected** — no output was marked spent,
because `commit_tx` runs only on a successful relay (§2a.11). It is a failed command, never a loss.
Neither §5, `node info`, the TUI nor README may state or imply otherwise, and `node.reachable` is
`null` in remote mode for exactly this reason: `false` would be a claim and `true` would be a
guess.

**`status` (§5).** No `NodeClient` is constructed. The wallet hot path is unchanged
(`get_height`, plus `get_balance` and `get_address` when open) and the wallet cold path is
unchanged; every node call — hot and cold — is skipped. Both `systemctl --user is-active` spawns
stay, because `units.node` is still a real field a user switching back needs and because one §2l
budget is better than two. Remote mode therefore only ever makes **fewer** calls than local mode, so
the §2l (a)/(b)/(d) budgets stand unaltered.

**Node commands.** `up`, `down`, `restart`, `peers`, `bans`, `ban`, `unban`, `limit` and `log` all
need the local daemon and all exit **4 `backend`**, `reason: remote_node`,
`error: "fmonero node <verb> needs the local monerod; node.mode is remote (<addr>)"`,
`hint: fmonero node local --yes`. The gate has **one** implementation and it is structural:
`NodeClient::from_ctx` returns that error when `node.mode == remote`, and every one of those
commands except `up`/`down`/`restart` reaches the daemon only through it. Those three go through
`node_session`, whose existing `Layout::Remote` arm changes from `Error::usage` to the same
`Error::backend(...).with_reason("remote_node")` — exit 2 was wrong there: the arguments are
correct, the backend is somewhere else, and an agent's reflex on exit 2 (re-read `--help`) leads
nowhere. `node info` is the exception and exits **0**: **in remote mode it takes a settings-only
path and never calls `NodeClient::from_ctx` at all**, printing the address, the trusted flag and the
mode from `units::daemon_target` with the four measured sub-objects `null` (§5c). That is a
deliberate branch in one command, not a hole in the gate: "which node am I using" is the question
remote mode makes people ask, and a command that answers it by refusing is a command people stop
running. One `if`, against nine commands that need none.

**Doctor (§7.1).** The `node` group reports `skip:remote_node` except **two** checks: `node_layout`,
which passes and names the address, and the new `remote_node_untrusted`, which reports
`skip:local_mode` in local mode and does its work here — it is the one check in this document that
is meaningful *only* in remote mode, so skipping it with the rest of its group would disable the
trusted-public-node warning in the only mode it can fire and leave the doctor reporting a clean
`node` group on a machine that has marked a stranger's node trusted.
`wallet_daemon_auth`, `wallet_daemon_link`, `node_auth`, `disk_space` and
`prune_configured` skip for the same reason; `monerod_installed` becomes a `warn` rather than a
`fail`, because a missing `monerod` is not a fault on a machine configured for a remote node and a
check that fails on a correct machine is the check an agent relaxes (§7). Two checks are new:
`wallet_daemon_matches_unit` and `remote_node_untrusted`.

**TUI (§7a) and the bar widget (§6).** The Node screen renders the address, the trusted flag, the
wallet's height, the last refresh and one line of explanation, and `u`/`d`/`L` toast the command
that would restore them. The bar icon gains a remote case and the panel a `remote: <addr>` line in
place of the sync gauge. Neither invents a sync percentage.

#### 2m.4 What stays exactly as it is: §2c

Restating it here because §2m is the section a future agent reads when they want to "just add a
little TLS": **fmonero itself makes no remote call, in any mode.** No TLS crate enters
`Cargo.toml` — `ureq` keeps `default-features = false, features = ["json"]` and §2c's
`cargo tree` grep is unchanged. `rpc::require_loopback` is unchanged and still the first statement
of both constructors. `settings set node.url` and `settings set wallet.url` still refuse a
non-loopback value. Nothing binds a port. The only process that dials a stranger is
`monero-wallet-rpc`, which upstream wrote for the purpose, and the only thing fmonero hands it is a
string. §2k's manifest freeze holds: remote mode adds **no dependency**.

#### 2m.5 The two convenience commands

`fmonero node remote ADDR [--trusted] [--dry-run] --yes` and `fmonero node local [--dry-run] --yes`
exist because the job is the nine steps below, which must happen together and in that order, and
because the alternative is a README paragraph telling the user to type four commands of which the
third is `systemctl`. Both are in §4.1 with §5c payloads; both are **idempotent** — run twice, the second run
reports every action `unchanged`, restarts nothing and exits 0.

`node remote ADDR` does, in this order:

1. **parse ADDR** against §2m.2 — exit 2 `bad_remote_address` before anything is touched;
2. **`--dry-run`** — print the plan and the unit diff and exit **0**, writing nothing. It comes
   *before* the `--yes` gate and does not need `--yes`, exactly as `setup --dry-run` does not
   (§4b's GREEN row): "show me what this would do" is the safe half of the command, and a
   `--dry-run` that exits 3 teaches the user to type `--yes` in order to see a preview, which is
   the habituation §2a.4 argues against. `--dry-run` still beats `--yes` when both are given (§4);
3. **take the §2a.11 `send.lock`** — `flock(LOCK_EX|LOCK_NB)` on `state_dir/send.lock`, held to the
   end of the command — else exit **3 `safety`**, `reason: send_in_progress`,
   `error: "another fmonero send is building or relaying"`,
   `hint: fmonero status --json   # wait for pending to clear, then retry`;
4. **the §2a.4 `pending_send` entry check** — any entry with `relayed == false` exits
   **3 `safety`**, `reason: pending_send`, with §2a.4's hint
   (`fmonero wallet history --limit 5   # confirm whether <txid> landed, then: fmonero wallet clear-pending --yes`).
   **No resolve step runs here**: resolving belongs to `send::build` and needs the wallet daemon
   this command is about to restart;
5. **`--yes` gate** — without it, print what would change and exit **3 `safety`**,
   `reason: needs_yes`, hint = the user's own command with `--yes` appended verbatim. It needs
   `--yes` because it changes which node a subsequent send is built against, which §4.1's own rule
   reserves `--yes` for;
6. **write the three keys in one `Settings::save`** — `node.remote`, `node.remote_trusted`,
   `node.mode` — so a crash cannot leave `mode == remote` with no address. §2m.1's invariant is a
   property of the file, not of the order of three writes;
7. **`units::install(ctx, dry_run)`** — re-render both units and `monerod.conf` and compare byte for
   byte. The node unit and `monerod.conf` are rendered in remote mode too: they are unused, but
   `node local --yes` must find them and the doctor's `units_present` compares them byte for byte
   in every mode;
8. **restart the wallet unit iff it was `active` before step 7** (`units::state` is read *before*
   the render), because a running `monero-wallet-rpc` holds `--daemon-address` from its argv;
   payload `wallet_unit_restarted`;
9. **if a wallet was open, say so.** The restart closes it, so `state::clear_last_open` runs and the
   payload carries `wallet_must_reopen: "<name>"` with `hint: fmonero wallet open <name>`. Reporting
   success while `status` still claimed a wallet was open would be a stale fact about money.

**Steps 3 and 4 are the money interlock, and they are here because this is the one command whose
whole purpose is to change the node a send is resolved against.** Without them a restart landing
between `send::relay`'s record fsync and its last `relay_tx` kills the command mid-money (the record
survives — that is what §2a.4 buys — but the remaining relays of a sweep do not); and, worse, after
the switch §2a.4's resolve step runs `get_transfer_by_txid` against a **different** node, which need
not hold that transaction in its pool, answers "not found" for a transaction that really was
broadcast, leaves the record in place and blocks every later send until the user types
`clear-pending` — the habit §2a.4 exists to prevent. Every other path that can disturb money already
carries both guards (§2a.1 steps 3 and 4), and §2a.3's argument is that an invariant which lives in
some callers and not others holds in none. The cost is one `flock` and one file read.

**`--dry-run` is exempt from steps 3 and 4, deliberately and in as many words.** It exits at step 2,
before either guard: it writes nothing, restarts nothing and can abort no relay, so blocking a
preview on a send in flight would refuse the safe half of the command for a risk it does not carry —
the same argument step 2 already makes against putting `--dry-run` behind `--yes`. A `--dry-run`
during a send prints the plan; the `--yes` that follows meets `send_in_progress` at step 3, which is
the refusal that matters.

`node local` is the same nine steps with `node.mode = local` and ADDR absent; `node.remote` and
`node.remote_trusted` are left where they are (§2m.1).

Neither command starts a daemon, and neither runs `node up`: §2i's single-writer precondition is
the only thing allowed to start `monerod`, and folding a start into a mode switch would route around
it. `node local --yes` therefore ends by saying `fmonero node up` is the next command when the node
unit is inactive.

#### 2m.6 Privacy, trust, and what README must say

A public node is a stranger who sees, for every wallet the user opens:

- **the IP address and the timing** of every request, including the burst that accompanies a send;
- **which blocks the wallet asks for**, which bounds the wallet's restore height and therefore its
  age;
- with `trusted` set, **the amounts the wallet holds**, because
  `select_available_outputs_from_histogram` sends `get_unspent_amounts_vector` in the request itself
  (`wallet2.cpp:12281-12282`, §1b.9 item 4);
- **the decoys of every transaction it builds**, because the node supplies the material they are
  chosen from: the RCT output distribution arrives over `get_output_distribution`
  (`wallet2.cpp:9258-9261`, sanity-checked only for monotonicity at `:9269` and a suspicious count
  at `:9271`), `gamma_picker` is seeded from exactly those daemon-supplied `rct_offsets` (`:9402`),
  and the ring members themselves come from `/get_outs.bin` (`:9736`). A node that skews the
  distribution or the outputs it returns controls which decoys the wallet picks and can make the
  true spend identifiable. **`trusted: false` does not reduce this** — it is the one item on this
  list the trust flag does not touch — which is why it is stated separately from the bullet above.
  It is a **privacy** loss, not a fund loss.

And it can lie: about the chain tip, about the fee estimate, and about whether a transaction was
relayed. Monero's own design absorbs part of that — the wallet verifies what it scans — but the
three lies do not have one consequence and must not be given one:

- **a withheld tip** makes a send build against a stale chain, which §2m.3 spells out as a relay
  rejection: exit 16, funds unaffected, because `commit_tx` marks outputs spent only after a
  successful relay (`wallet2.cpp:7628`, §2a.11);
- **an inflated fee estimate costs real money, and nothing detects it.** `get_base_fee(priority)`
  returns the daemon's own `get_dynamic_base_fee_estimate_2021_scaling` answer verbatim
  (`wallet2.cpp:8584-8608`, the call at `:8597`) and every build path uses it — `transfer`
  (`:11160`), `sweep_all` (`:11807`), `sweep_single`'s path (`:11924`) — while `adjust_priority`
  additionally reads the daemon's pool backlog (`:8688`). A node returning a hugely inflated
  fee-per-byte produces a transaction that relays **successfully** and pays the excess to a miner.
  This is a failed *wallet*, not a failed command. The bound that answers it already exists:
  **`--max-fee AMOUNT`** (§2a.2, on `wallet send` and `wallet sweep`, raised by `send::build`
  immediately after the build and re-checked by `send::relay` as the enforcing gate) — the only fee
  bound that survives the build-discard-rebuild flow. README's public-node section names it;
- **a false "relayed"** is covered by §2a.4's record and the resolve step, and by §2a.9's txid
  comparison, neither of which the node can forge into a second spend.

So, three rules with no exceptions:

1. **`node.remote_trusted` defaults to `false` and a public node never gets `true`.** It is there for
   the case it is honest in: a node the owner runs, on a LAN or a private overlay. The doctor's
   `remote_node_untrusted` (§7.1) warns whenever it is `true` for a host that is not a literal
   address in `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `100.64.0.0/10`
   (CGNAT, which is where Tailscale lives), `169.254.0.0/16`, `::1`, `fc00::/7` or `fe80::/10`. **A
   DNS name is never treated as private**, because classifying one means resolving it — a network
   call inside a pure detector (§7's axiom) whose answer a name server can change between runs.
2. **`--daemon-ssl autodetect` is not a private channel and must never be described as one.** It
   tries TLS, falls back to plaintext on failure — permanently, for that connection
   (`net_helper.h:225-233`) — and does **not** fail on a certificate it cannot verify
   (`net_ssl.h:121-124`). It is the only value v0.1 can send, because `enabled` throws at process
   start without a CA file, a fingerprint list or `--daemon-ssl-allow-any-cert`
   (`wallet2.cpp:416-432`), all three of which §0 cuts. README says "opportunistic, unauthenticated"
   in those words.
3. **Tor is not in v0.1** (§0's existing row) and remote mode does not sneak it in. `set_daemon`'s
   `proxy` field is never sent and `--proxy` is never rendered. A user who wants the node not to see
   their IP wants v0.2, and telling them otherwise is the dishonesty §2j is about.

Nothing here verifies that the remote node is on `settings.node.network`. It cannot: nettype is not
observable through wallet-RPC either. The symptom of a mismatch is a `refresh` that errors or a
wallet that never advances, `node info`'s detail says so, and the existing `wallet_nettype_matches`
check (§7.1) still compares the **wallet's** own nettype against the setting — which is the half
that can be checked.

#### 2m.7 The README section, specified so it cannot drift

README (§8's docs agent owns the file; this is its brief) gains one section, **"Public node"**,
between "Install" and "Status". It contains exactly five things and claims nothing beyond §4b:

1. **The three-command path**, runnable and in this order:

   ```bash
   fmonero node remote <host>:18089 --yes    # a public node; never --trusted
   fmonero wallet open <name>                # or: fmonero wallet create <name> --yes
   fmonero wallet refresh                    # the first contact with the node
   ```

   and `fmonero node local --yes` as the way back, named in the same block. One sentence beside the
   `wallet create` line, because it is a money-visibility fact and not a footnote: **in remote mode
   the node decides the new wallet's restore height** (§2m.3), so create the wallet while the node
   is answering — a create against an unreachable node scans from genesis, and one against a node
   reporting a height above the tip never sees its funds, with `monero-wallet-cli`'s `rescan_bc` the
   only way back in v0.1.
2. **What the node learns**: this machine's IP address and the timing of every request, including
   the burst that accompanies a send, and which blocks the wallet asks for — which bounds the
   wallet's age. In those terms, not as "some metadata".
3. **What the node can do**, with the three outcomes kept apart because they are not one outcome.
   It can lie about the chain tip: a send built against a stale tip is **rejected on relay** —
   exit 16, funds unaffected (§2m.3) — never lost. It can lie about the **fee estimate**, and that
   one **does** cost money: the wallet takes the node's fee-per-byte verbatim, the transaction
   relays successfully and the excess goes to a miner, so README names **`--max-fee AMOUNT`** as
   the bound (§2a.2) in the same breath. And it **supplies the decoys**: the output distribution
   and the ring members every transaction's decoys are chosen from come from the node
   (`wallet2.cpp:9258-9261`, `:9402`, `:9736`), so a hostile one can degrade the ring signature's
   plausible deniability — a privacy loss, not a fund loss, and one that **`trusted: false` does
   not reduce**. Saying only the reassuring half of any of the three would be an assurance the code
   does not provide; saying only the frightening half would be scarier than the truth.
4. **Never mark a public node trusted**, with the one-line reason from §1b.9 item 4 (a trusted
   daemon receives the wallet's own unspent amounts inside a request) and the note that
   `--trusted` exists for a node the reader runs themselves.
5. **Two limits, stated plainly**: TLS here is **opportunistic and unauthenticated** —
   `--daemon-ssl autodetect` tries TLS, falls back to plaintext, and does not verify the
   certificate (§1b.9 item 2) — and **Tor is not in v0.1** (§0). Neither is described as a
   planned feature with a date.

Its Status section gains the LIVE rows of §4b marked as what they are: real contact with a real
public node, on an empty wallet, with the send path still RED. The same five points, in the same
words, are `robot-docs`' `remote node` section (§5b's `agent` module) — one text, two renderings,
because a privacy claim that differs between the human and the agent document is the drift §2j
exists to prevent.

---

## 3. Repo layout (Cargo workspace, resolver 2)

```
Cargo.toml                      workspace; resolver 2; ALL deps pinned here; frozen after Phase 1a
crates/fmonero-core/            the engine: silent library, no terminal I/O
  src/lib.rs                    pub mod …; pub use Context, Status, Error, CancelToken, Piconero, Address
  src/error.rs                  Error → exit_code / code_name / hint; EXIT_CODES table (§4a)
  src/context.rs                Context: Settings + Paths + Timeouts + json + verbose. Clone + Send + Sync + 'static
  src/paths.rs                  XDG paths; FMONERO_STATE_DIR / FMONERO_CONFIG_DIR / FMONERO_CACHE_DIR overrides
  src/settings.rs               ~/.config/fmonero/config.toml + the settings key table (§4)
  src/state.rs                  state.json (stalls, last_error, net samples, cold cache, last_open) under one
                                flock; pending.json (§2a.4) is its OWN 0600 file; send.lock (§2a.11) is a
                                third file, an empty 0600 lock held from build to relay
  src/proc.rs                   run_with_deadline + CancelToken (verbatim from fproton)
  src/rpc/mod.rs                shared: Agent construction, Login, RpcError, retry-once-on-401
  src/rpc/digest.rs             MD5 Digest client (§2c), ~110 lines
  src/rpc/jsonrpc.rs            JSON-RPC 2.0 envelope + the legacy POST /name form; inspects the
                                daemon's `status` field before deserializing (BUSY ⇒ RpcError::Busy,
                                §1b.3)
  src/rpc/node.rs               NodeClient (§5b) — every method of §1b.2/§1b.3
  src/rpc/wallet.rs             WalletClient (§5b) — every method of §1b.4
  src/units.rs                  systemd user units: render, write, reload, start/stop, is_active, layout detection (§2i)
  src/amount.rs                 Piconero: parse/format, no floats (§2a.5)
  src/address.rs                Address: structural parse, the PREFIXES table (§1b.8)
  src/send.rs                   the ONLY send orchestrator: send::build + send::relay (§2a.3, §5b)
  src/session.rs                wallet_session::{open,close,refresh}, node_session::{up,down,restart}
                                and node_mode::{set_remote,set_local} (v5.2, §2m.5) — the one
                                implementation of each sequence, shared by the CLI and the TUI (§5b)
  src/log.rs                    bounded tail of a daemon log file (§5b log::tail)
  src/history.rs                postcard snapshot of transfer_entry rows, incremental fetch
  src/status.rs                 Status (§5) from node + wallet + units + state
  src/agent.rs                  pub const ROBOT_GUIDE: &str + capabilities_json(ctx)
crates/fmonero/                 the binary
  src/main.rs                   clap; the whole §4 surface; dispatch; exit codes
  src/output.rs                 human tables vs JSON; colour policy
  src/setup.rs                  setup: units, plugin symlink + enable + bar put, menu merge
  src/prompt.rs                 TTY password/seed prompt with echo off (§2b.1)
  src/doctor/{mod,checks,probes,mutate,report}.rs      §7
  src/tui/{mod,theme,screens,send,subs}.rs             §7a
  assets/menu.jsonc             merged by `setup --menu`
crates/fmonero-mock/            test-only; publish = false; never installed (§8 Phase 0)
  src/main.rs                   thin main() over serve(routes); full argv in §8 Phase 0
plugin/oliver.monero/           manifest.json, Panel.qml, Service.qml, MoneroIcon.qml, Model.js, README.md
scripts/{install,uninstall,check}.sh
tests/e2e.sh                    --fast (default, < 20 s, GREEN + mock-AMBER) · --full (< 90 s, adds deadline paths)
tests/plugin-smoke.sh           §6.8
tests/fixtures/                 §8 Phase 0; README.md states captured vs derived per file
docs/ROBOT.md                   generated by `fmonero robot-docs > docs/ROBOT.md` in tests/e2e.sh; never hand-edited
DESIGN.md (this) · DECISIONS.md · CLAUDE.md · README.md · LICENSE (MIT)
```

### 3.1 The frozen dependency set

Phase 1a writes exactly this into the workspace `Cargo.toml` and then nobody touches a manifest
again (§2k). Every version is the one already used by fproton or fbittorrent unless a reason is
given.

```toml
[workspace]
resolver = "2"
members = ["crates/fmonero-core", "crates/fmonero", "crates/fmonero-mock"]

[workspace.dependencies]
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
anyhow      = "1"                                    # binary only (§2g)
thiserror   = "2"
clap        = { version = "4.6", features = ["derive"] }
dirs        = "6"
toml        = "1.1"
libc        = "0.2"                                  # isatty + termios for the password prompt
ureq        = { version = "3.4", default-features = false, features = ["json"] }
ftui        = { version = "0.7", features = ["crossterm"] }
ftui-extras = { version = "0.7", features = ["canvas", "charts"] }
unicode-width = "0.2"
postcard    = { version = "1.1", features = ["use-std"] }
sha2        = "0.10"                                 # doctor backup hashes; history cache wallet fingerprint
jiff        = "0.2"                                  # RFC 3339 with a local offset; std has none
qrcode      = { version = "0.14", default-features = false }   # receive QR on a Canvas; no image/svg deps
md-5        = "0.10"                                 # §2c Digest; RustCrypto, digest 0.10 is already cached

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true
```

**Crate membership**, so the 1a agent does not have to infer which crate takes which dependency:
`fmonero-core` takes `serde`, `serde_json`, `thiserror`, `dirs`, `toml`, `libc`, `ureq`, `postcard`,
`sha2`, `jiff`, `md-5`. `fmonero` (the binary) takes those plus `anyhow`, `clap`, `ftui`,
`ftui-extras`, `unicode-width`, `qrcode`. `fmonero-mock` takes `serde_json` only (§8 Phase 0).
`anyhow` is a dependency of the binary and of nothing else (§2g), and `ureq` is core-only — the
binary never speaks HTTP.

Why each non-obvious choice:

- **`ureq` with `default-features = false`.** The default features are `rustls` + `gzip`. We speak
  plaintext HTTP to loopback only (§2c), so dropping them removes the whole TLS tree and turns the
  loopback rule into a property of the binary. *If `ureq` 3.4 refuses to build with no TLS backend,
  Phase 1a adds `rustls` back and records the reason in DECISIONS.md — that is the one Cargo.toml
  change the freeze permits, and only at the 1a gate.*
- **`ftui-extras` with `canvas` + `charts`.** `charts` implies `canvas`
  (`ftui-extras-0.7.0/Cargo.toml:48-52`). `canvas` draws the receive QR in half-block mode;
  `charts` draws the node's net-rate sparkline and the sync gauge. `forms` and `validation` are
  **not** enabled: the Send screen's three fields are simpler with plain `TextInput`s and one
  validator function than with a form engine, and every extra feature is compile time.
- **`qrcode` with `default-features = false`.** Its default features pull image/SVG renderers we do
  not want; we only need the module matrix, which we paint onto a `Canvas` ourselves.
- **`md-5`.** Only because epee's Digest is MD5-only (§1b.7). It is used for HTTP authentication and
  nothing else; no fmonero security property depends on MD5's collision resistance.
- **`postcard` for the history cache, not `serde_json`** (which is already a dependency, so this
  choice has to earn itself). A self-describing format re-parses every row of `transfer_entry` — a
  few thousand on a used wallet — on every `wallet history`; postcard deserializes the whole
  snapshot in one pass with no field-name matching, which is what makes the §2l 50 ms `--cached`
  budget reachable at all. Its *lack* of self-description is precisely why the header carries
  `format_version`, bumped on any change to a serialized struct (§5b).
- **`sha2`.** Shared by the doctor's backup hashes and the history cache's wallet fingerprint, so it
  adds no dependency for the second use. The fingerprint only has to notice that this is *a
  different wallet*; it is not resisting an adversary, and the address it hashes is public (§2b).
- **No `tiny-keccak`.** Address checksums would need Keccak-256; §2a.6 gets a stronger guarantee
  from `validate_address` for free, so the dependency buys nothing.
- **No `regex`.** The one pattern we need (the log viewer's search) is `ftui`'s own `LogViewer`
  filter plus `str::contains`.

---

## 4. CLI contract

Global flags, all `global = true`: `--json`, `--timeout <SECONDS>`, `-v` (repeatable),
`--no-color`, `--yes`, `--dry-run`, `--force`.
**`--timeout <SECONDS>`** lowers every deadline to `min(configured, SECONDS)` and **never raises
one**; `0` is rejected (exit 2). It applies to the six settings-backed deadlines **and** to
`unit_probe` and `relay`. The direction is stated because it is the one global flag every §4b and §8
gate command depends on, and "clamps" admits two readings. Because it is a ceiling, `--timeout 3` on
`wallet open` (300 s) or on a `transfer` build (120 s) **will** kill a real call — which is why
§4b's AMBER rows run it only against `fmonero-mock`, whose answers are immediate, and why §2d's
deadline test pairs `--timeout 2` with a deliberately slow mock route.
**`--force`** ignores an armed §2d breaker for this invocation and tries the daemon anyway. That is
all it does: it does not imply `--yes`, and it never bypasses a `pending_send` (§2a.4), the §2i
layout gate, a loopback check, or any other money check. It appears in `capabilities`, in every
`node_stalled`/`wallet_stalled` hint, and on every node/wallet command a breaker can block.
**`--dry-run` is implemented by exactly six commands** — `wallet send`, `wallet sweep`, `setup`,
`doctor` and, since v5.2, **`node remote`** and **`node local`** (§2m.5 step 2, where it precedes
the `--yes` gate and does not need it, as `setup --dry-run` does not). On any other command it exits **2 `usage`** with
`error: "--dry-run is not supported by this command"` and `hint: fmonero <cmd> --help`. It is never
silently ignored: a flag that means "change nothing", accepted globally and ignored on a mutating
command, is worse than a flag that does not exist — a user who has learned from the paragraph below
that `--dry-run` beats `--yes` on the send path would otherwise find `fmonero node down --dry-run
--yes` stopping the daemon and `fmonero wallet book remove 3 --dry-run --yes` deleting the entry.
`capabilities` lists the commands that accept it, from the same table the CLI branches on, so the
count in this paragraph and the count in the payload cannot drift. *Test:* `fmonero node down
--dry-run` exits 2 and the mock records zero requests — the neighbouring `node` subcommand that does
**not** accept it, which is the case a blanket "node commands take `--dry-run`" would break.
Bare `fmonero` on a TTY opens the TUI; without a TTY it prints `status --json`.

**Envelope.** One JSON object per invocation on stdout. **A non-TTY stdout implies `--json` for
every command that has a JSON payload.** The three exceptions are named here so the rule is not
quietly false: `robot-docs` always emits plain text (it is a handbook, not a record), `--help`
always emits plain text, and `tui` exits **2** with `hint: fmonero status --json` when stdout is not
a TTY. `wallet address --qr` is not an exception — the QR is simply skipped and the payload is the
normal JSON. Every command says which it is in `capabilities.commands[].output_format`, which is
`json` everywhere but those three.
Colour is off when stdout is not a TTY, with `--no-color`, or when `NO_COLOR` or
`CI` is set. stdout is data; progress and diagnostics go to stderr. Success carries
`{"ok":true,"schema":1,…}`; failure carries
`{"ok":false,"schema":1,"code":N,"code_name":"…","error":"…","hint":"<runnable command>","reason":"…"}`
and exits with the same code. Every human-mode error ends with a `hint:` line. `capabilities` and
`robot-docs` are always machine output.

**`reason` is a stable machine-readable discriminator *within* a code**, listed by `capabilities`,
because exit 3 alone covers eight unrelated conditions and an agent's reflex on exit 3 —
append `--yes` and retry — is right for one of them and pointless or dangerous for the rest. For
exit 2 the values are `bad_args` · `unknown_key` · `bad_amount` · `non_loopback_url` ·
`unsupported_flag` · **`bad_remote_address`** · **`remote_unset`** (§2m); for exit 3, `needs_yes` ·
`pending_send` · `send_in_progress` · `fee_above_max` ·
`foreign_node` · `data_dir_in_use` · `wallet_exists` · `full_chain_refused`; for exit 4,
`not_installed` · `unit_missing` · `unreachable` · `auth_refused` · **`remote_node`** ·
**`remote_address_rejected`** (§2m); for exit 5, `stalled` ·
`rpc_error` · `restricted` · `killed_at_deadline`; for exit 9, `not_open` · `bad_password` ·
`background_sync`; **for exit 10, `node_not_synchronized` · `wallet_catching_up`** (§2m.3 — exit 10
carried no `reason` before v5.2, and the two situations take different actions: the first waits for
*our* node, the second waits for a refresh against someone else's);
for exit 16, `too_large` · `not_possible` · `relay_refused`; **for exit 1,
`txid_mismatch`** (§2a.9 — the one exit-1 path where retrying is the wrong reflex, and
`capabilities.reasons` is generated from `Error::REASONS`, so a documented pair that cannot be
constructed would be a drift this table exists to prevent);
`null` for every other code. Two of the exit-4 values are answered **before any `systemctl`
call and within milliseconds** (§2i, v5.1): `unit_missing` when the unit file is not in
`paths.systemd_user_dir`, and `not_installed` when the daemon binary is not on `PATH` — for
`node up`, `node restart`, `wallet open` and `wallet create`. **An agent branches on
`code_name` + `reason` and never on prose.** It is the difference between "append `--yes`" and "a
transaction may be in flight". Exit 5 is the case that makes this load-bearing off the money path:
a §2d breaker can hold it armed for up to 60 s, and `stalled` means *wait or pass `--force`* while
`rpc_error` means *the daemon answered and refused* — indistinguishable from prose, and the wrong
reflex (retry immediately) re-arms nothing and gets nowhere. Exit 9 likewise separates "open a
wallet" from "ask the user for the password again", which retrying the same password never fixes.

**The list above is generated, not transcribed.** `Error::REASONS` (§5b) is the single table;
`capabilities.reasons` is printed from it, so the documented set and the constructible set cannot
drift.

**Amount syntax** (§2a.5). Either a decimal XMR string — `0`, `1`, `0.5`, `1.234567891234`, at most
12 decimals, no separators, no exponent — or an integer followed by `p` for raw piconero
(`250000000000p`). **`all` is not an amount anywhere in the CLI**: `wallet sweep` takes no amount
argument — it always sweeps the account, bounded by the optional `--below` — so there is nothing for
`all` to mean, and `Piconero::parse` rejects it with exit 2 and the hint
`fmonero wallet sweep <address> --dry-run   # to move everything`. (A documented token that no
parser accepts and no argument position exists for is a token somebody implements anyway, on the
sweep path, where the difference between "some" and "all" is the account balance.) The TUI's Send
screen refuses it inline with the same text (§7a). Output always carries both: an integer
`*_pico` and a pre-rendered `*_xmr` string (12 decimals, trailing zeros trimmed to a minimum of 4).

**`send`/`sweep` have exactly three outcomes, and `--dry-run` always beats `--yes`.**
`--dry-run` (with or without `--yes`) builds with `do_not_relay`, prints recipient, amount, fee,
total, priority and account, relays nothing and exits **0**. `--yes` without `--dry-run` builds and
relays. Neither flag: build, print, relay nothing, exit **3 `safety`**, and the hint is the user's
own command with `--yes` appended **verbatim**
(`fmonero wallet send <to> <amount> --account N --priority P --yes`), not a generic `… --yes`.
This interaction is spelled out because the two plausible readings of `--dry-run --yes` differ by
whether money moves. *Test:* `send --dry-run --yes` against the mock records exactly one `transfer`
and zero `relay_tx`.

**Recipient syntax.** Either a Monero address, or `@label` — an address-book entry, matched
case-insensitively against `description` from `get_address_book`; zero matches exit 6, more than
one exits 7 listing them. A bare word that is not `@`-prefixed is never guessed at as a label.
**The resolution has one owner and one position**: `send::build` performs it at step 5a of §2a.1's
refusal order, on the open wallet, and replaces `dest` with the resolved address before
`address::parse` runs (which is why step 2 skips a `@`-prefixed `dest` instead of rejecting it as
exit 15). `main.rs` never resolves a label — it is frozen at the 1a gate and the call needs an open
wallet.

### 4.1 The table

`M` = mutating · `Y` = needs `--yes` · `W` = needs an open wallet · `N` = needs a reachable node.

| Command | Args | Calls | M | Y | W | N |
|---|---|---|---|---|---|---|
| *(bare)* | — | TUI on a TTY, else `status --json` | – | – | – | – |
| `status` | — | §5's hot call set (three RPCs with the wallet closed, five with it open) plus `systemctl --user is-active` ×2 and state. **Never starts anything and calls no wallet mutator** (§2e); it does write `state.json` atomically (§5). | no | – | no | no |
| `node up` | `[--wait] [--wait-timeout SECONDS]` | **`node_session::up` (§5b)** — the one implementation, shared with the TUI's `u` key: §2i layout, then **§2i's preflight — the unit file (exit 4 `unit_missing`) and `monerod` on `PATH` (exit 4 `not_installed`), both answered in milliseconds and both before any `systemctl` call** —, then the §2i single-writer precondition (no holder ⇒ start; our own unit's `MainPID` ⇒ **exit 0, already running**; any other PID ⇒ exit 3 naming it), then `systemctl --user start fmonero-monerod.service`. Returns as soon as one `get_info` answers, waiting at most 30 s (exit 8 if it never does). **`--wait` additionally polls until `get_info.synchronized` is true**, bounded by `--wait-timeout`, **default 1 800 s**; when it elapses the command exits **8 `timeout`** with `error: "still syncing at <pct>% after <n>s"` and `hint: fmonero node up --wait --wait-timeout 86400   # or: fmonero status --json`. `--wait-timeout 0` is the explicit opt-in to "poll until interrupted" and is refused (exit 2) when stdout is not a TTY, because a non-interactive caller has nothing that can interrupt it (§2d). There is no unbounded default: a command that can block for days is a command an agent cannot call. Without `--wait` a still-syncing node is exit 0 plus the `node_syncing` warning. Needs layout `user_unit` (§2i). **A unit this command started that never answered is stopped again and the error says so** (§2i, v5.1): a failed `node up` never leaves a `Restart=on-failure` loop behind it. **In remote mode: exit 4 `backend`, `reason: remote_node`, hint `fmonero node local --yes`, before any RPC or `systemctl` spawn (§2m.3, §4).** | yes | no | – | – |
| `node down` | — | **`node_session::down` (§5b)**: `POST /stop_daemon` if reachable, then `systemctl --user stop`; the RPC call first so LMDB closes cleanly. **Layout `user_unit` only** — exits 3 before any RPC otherwise (§2i). **In remote mode: exit 4 `backend`, `reason: remote_node`, hint `fmonero node local --yes`, before any RPC or `systemctl` spawn (§2m.3, §4).** | yes | **yes** | – | – |
| `node restart` | — | **`node_session::restart` (§5b)**: `down` then `up`. **Layout `user_unit` only** (§2i), and §2i's preflight runs **before** the `down` — a restart that stops the node and then reports "monerod is not installed" has left the machine worse than it found it (v5.1). **In remote mode: exit 4 `backend`, `reason: remote_node`, hint `fmonero node local --yes`, before any RPC or `systemctl` spawn (§2m.3, §4).** | yes | **yes** | – | – |
| `node info` | — | `get_info` + `get_last_block_header` + `hard_fork_info` + `get_version` + `get_fee_estimate` + `get_transaction_pool_stats`; `sync_info`/`get_net_stats`/`get_limit` when unrestricted. **In remote mode it is the one node command that still exits 0** and issues nothing: it prints the address, the trusted flag and the mode from settings, with `txpool`, `sync`, `hard_fork` and `rpc` all `null` and the `node` object carrying §5's remote-mode shape — the same key set as local mode, so one parser serves both (§5c). Its detail names what cannot be known: a remote node's own height and sync state are not observable through `monero-wallet-rpc` (§1b.9 item 5), and nothing verifies that it is on `settings.node.network` (§2m.6). | no | – | – | yes (no in remote mode) |
| `node log` | `[--lines N] [--follow] [--grep TEXT]` | `log::tail` (§5b) over `settings.node.log_file` (default `~/.bitmonero/bitmonero.log`), a bounded read; a missing file is exit 0 with "no log yet"; `--follow` only from the TUI's Log screen, refused (exit 2) in the CLI where a pipe would hang. **In remote mode: exit 4 `backend`, `reason: remote_node`, hint `fmonero node local --yes`, before any RPC or `systemctl` spawn (§2m.3, §4).** — the log it tails belongs to the local daemon, and in remote mode there is none; "no log yet" would be a true sentence pointing at the wrong thing | no | – | – | no |
| `node peers` | `[--white\|--gray] [--limit N]` | `get_connections` (live) and `POST /get_peer_list` (`public_only:true`). Restricted node → exit 5 with the restricted hint. **In remote mode: exit 4 `backend`, `reason: remote_node`, hint `fmonero node local --yes`, before any RPC or `systemctl` spawn (§2m.3, §4).** | no | – | – | yes |
| `node bans` | — | `get_bans`. **In remote mode: exit 4 `backend`, `reason: remote_node`, hint `fmonero node local --yes`, before any RPC or `systemctl` spawn (§2m.3, §4).** | no | – | – | yes |
| `node ban` | `HOST [--seconds N]` (default 86400) | `set_bans [{host, ban:true, seconds}]`. **Layout `user_unit` only** (§2i). **In remote mode: exit 4 `backend`, `reason: remote_node`, hint `fmonero node local --yes`, before any RPC or `systemctl` spawn (§2m.3, §4).** | yes | – | – | yes |
| `node unban` | `HOST` | `set_bans [{host, ban:false, seconds:0}]`. **Layout `user_unit` only** (§2i). **In remote mode: exit 4 `backend`, `reason: remote_node`, hint `fmonero node local --yes`, before any RPC or `systemctl` spawn (§2m.3, §4).** | yes | – | – | yes |
| `node limit` | `[--up KBPS] [--down KBPS] [--out-peers N] [--in-peers N]` | no args → `NodeClient::{get_limit, peers_count}`, both read-only (any layout); with args → `NodeMutators::{set_limit, set_peers_count}`, which exist only on a type the **layout `user_unit`** gate hands out (§2i). `-1` restores the default. **In remote mode: exit 4 `backend`, `reason: remote_node`, hint `fmonero node local --yes`, before any RPC or `systemctl` spawn (§2m.3, §4).** | with args | – | – | yes |
| `node remote` | `ADDR [--trusted] [--yes] [--dry-run]` | **`node_mode::set_remote` (§5b, §2m.5)** — the persistent form of `wallet set-daemon`, in nine steps: parse ADDR against §2m.2's grammar (exit 2 `bad_remote_address`) → `--dry-run` prints the plan and the unit diff and exits 0, **without needing `--yes`** and **before the two money guards below**, exactly as `setup --dry-run` does → the §2a.11 `send.lock` (exit 3 `send_in_progress`) → the §2a.4 `pending_send` entry check (exit 3 `pending_send`) — the two guards every other money-adjacent path carries, here because this is the command that changes which node a pending send would be *resolved* against (§2m.5) → the `--yes` gate (exit 3 `needs_yes`, hint = the user's own command with `--yes` appended verbatim) → **one** `Settings::save` writing `node.remote`, `node.remote_trusted` and `node.mode = remote` together, so a crash cannot leave the §2m.1 invariant broken → `units::install` re-rendering both units and `monerod.conf` → `systemctl --user restart <wallet.unit>` **iff `units::state` said it was active before the render** (a running `monero-wallet-rpc` holds `--daemon-address` in its argv) → when a wallet was open, `state::clear_last_open` and `wallet_must_reopen: "<name>"` in the payload with `hint: fmonero wallet open <name>`. Starts no daemon and never runs `node up`: §2i's single-writer precondition is the only thing allowed to start `monerod`. Idempotent — a second run reports every action `unchanged`, restarts nothing, exits 0. Payload shape: §5c | yes | **yes** | no | no |
| `node local` | `[--yes] [--dry-run]` | **`node_mode::set_local` (§5b, §2m.5)** — the same nine steps with `node.mode = local` and no ADDR. `node.remote` and `node.remote_trusted` are **left as they are** (§2m.1): they are inert in local mode and the address is the one thing the user would have to retype. Ends by naming `fmonero node up` when the node unit is inactive. Payload shape: §5c | yes | **yes** | no | no |
| `wallet list` | — | `wallet_dir::list` (§5b): `*.keys` basenames in `settings.wallet.dir`, sorted; no RPC, no wallet bytes read (§2b.6). A NAME containing `/` is exit 2 (§1b.5). An empty list (and a `-23 NO_WALLET_DIR`) prints, besides `fmonero wallet create <name> --yes`, the one command that brings an existing seed back: `hint: monero-wallet-cli --restore-deterministic-wallet --wallet-file <wallet.dir>/<name>   # then: fmonero wallet open <name>` (§0 cuts `wallet restore`). | no | – | no | no |
| `wallet open` | `NAME [--password-file P]` | `wallet_session::open` (§5b) — the single implementation of the §4.2 sequence, shared with the TUI's open modal: `units::ensure_running` — which probes first, and only when the probe fails runs §2i's preflight (exit 4 `unit_missing` / `not_installed`) and then the start, so an answering daemon needs no unit and no binary while an absent one is refused in milliseconds rather than at a 30 s deadline (v5.1) —, then `open_wallet{filename, password}` (§2b.1), **then** exactly one `set_daemon{address, trusted, ssl_support, username, password}` whose four arguments come from `units::daemon_target(ctx)` and therefore differ by mode (§2m.3 — in remote mode the two credential fields are omitted, always), then `auto_refresh{enable:true, period:20}` (§2d). The order is not free: `on_set_daemon` answers `-13 NOT_OPEN` with no wallet loaded (`wallet_rpc_server.cpp:4692`), so issuing it first would fail silently and leave the wallet unable to reach fmonero's own authenticated node. Records `state::set_last_open(name, net)` — the wallet's own nettype, read from its primary address, being the §2a.6 send-path authority — so `status` can name it. A NAME containing `/` is exit 2 (§1b.5); a missing `<wallet.dir>/<NAME>.keys` is exit **6 `not_found`**, checked locally because wallet-rpc collapses that case into the same `-1` a wrong password gives (§1b.4). | yes | no | – | no |
| `wallet close` | — | `wallet_session::close` (§5b): `close_wallet{autosave_current:true}` then `state::clear_last_open` | yes | no | yes | no |
| `wallet create` | `NAME [--language L] [--allow-empty-password]` | `units::ensure_running(wallet.unit)` — the same first step as `wallet open`, including §2i's preflight (v5.1), because the units are written **disabled** and started on demand (§4.2), so without it `wallet create` on a freshly `setup`-ed machine exits 4 `backend` and §4b's RED rehearsal stops at its sixth line — then `create_wallet`. **`create_wallet` leaves the new wallet open** — `on_create_wallet` ends `m_wallet = wal.release()` (`wallet_rpc_server.cpp:3639`) — so `wallet create` must finish the same post-open steps `wallet_session::open` performs, or the wallet it just made is open with no daemon credential and no recorded name: `status` would report `wallet.open: true` with `wallet.name: null` (§5 sources that from `state::last_open`), every wallet→daemon call would 401 (§4.2), and the §2a send path would accept a nameless wallet. Those steps are exactly the last three of `wallet_session::open`, called on the wallet `create_wallet` left open — no new API: one `set_daemon` from `units::daemon_target(ctx)`, carrying `Login::from_monerod_conf` in local mode and no credential at all in remote mode (§2m.3), then `auto_refresh{enable:true, period:20}` (§2d), then `state::set_last_open(name, net)` with the nettype read from the new wallet's own primary address (§2a.6). `--language` is validated locally against `SEED_LANGUAGES` (§1b.4) and defaults to `"English"` (the field is required and an unknown value comes back as a bare `-1`). Refuses if the file exists (exit 3, `reason: wallet_exists`). The password is read as §2b.1 says and, **on a TTY, prompted twice and compared** — a mismatch exits 2 without creating anything; an **empty** password is refused with exit 2 unless `--allow-empty-password` is passed, because an unencrypted wallet is a deliberate choice and never a typo; with no TTY the password must come from `--password-file` or `$FMONERO_WALLET_PASSWORD`, where there is nothing to mistype. Does not print the seed, and ends with the `seed_backup_hint` of §2b.2. **In remote mode the new wallet's restore height is decided by the remote node**, not by fmonero: `on_create_wallet` reads `/getheight` over the address in the *unit's* argv and calls `set_refresh_from_block_height` before fmonero's own `set_daemon` runs (`wallet_rpc_server.cpp:3596`, `:3606-3609`), with the height left at **0** when that call fails. An unreachable node therefore produces a genesis-scanning wallet whose first `refresh` cannot finish inside `timeouts.refresh_sec`, and a node reporting a height above the tip produces a wallet that will never see its funds — with no in-tool recovery, since §0 cuts `rescan_blockchain` and `scan_tx` (§2m.3). | yes | **yes** | leaves one open | no |
| `wallet balance` | `[--account N] [--subaddresses]` | `get_balance{account_index, all_accounts:false}` | no | – | yes | no |
| `wallet address` | `[--account N] [--index N] [--qr]` | `get_address`; `--qr` renders the primary/selected address as a half-block QR on stdout (skipped when not a TTY) | no | – | yes | no |
| `wallet new-address` | `[--account N] [--label TEXT]` | `create_address` | yes | no | yes | no |
| `wallet history` | `[--account N] [--limit N] [--in] [--out] [--pending] [--failed] [--pool] [--since HEIGHT] [--cached]` | postcard cache of confirmed `in`/`out` rows, topped up by one `get_transfers` (§5b `history`). **`--in` selects `in` *and* `block` (coinbase) rows** — they arrive in the same list and there is no separate request flag (§1b.4) — and the cache may hold confirmed `block` rows on the same terms as `in`. **`--since HEIGHT` sends `filter_by_height: true`**; without that flag the bound is silently ignored and the daemon returns the whole history (§1b.4). Without `--cached` it also issues one wallet `get_height` — already needed for the top-up — and renders `confirmations`/`locked` through `history::confirmations` / `history::locked` (§5b, never recomputed in `output.rs`) against that **wallet** height, which is the clock wallet-rpc itself uses (§1b.4). **With `--cached` it issues nothing at all**: the cache is served alone, `confirmations` and `locked` are **`null`** (human output shows `—` in those columns plus a footer `cached: confirmations unknown (no daemon call)`), and `cache_age_sec`/`high_water` carry what is known. Zero RPC and a live confirmation count are mutually exclusive; saying which one `--cached` gives up, and showing `—` rather than `0`, is what keeps §5's "never a zero standing in for unknown" true on a money table. `--cached` is the invocation the §2l 50 ms budget measures. Raises `history_cache_stale` when the high-water height is more than 1 000 blocks behind. **The `N` column is `no` except with `--pool`**: `on_get_transfers` reads `min_height`/`max_height` locally, but its `pool` branch runs `update_pool_state`/`process_pool_state` — a daemon round trip (`wallet_rpc_server.cpp:2939-2950`). `history::query` therefore sends `pool` **only when the caller asked for pool rows**, never unconditionally; `pending` and `failed` are local (`get_unconfirmed_payments_out`) and stay on. A `get_transfers` that fails **because** that branch could not reach the daemon (epee's `code: 0`, §1b.4) is retried **exactly once without `pool`**, and the command returns the rows it does have with `pool_unavailable: true` in the payload (§5c) plus a footer line in human mode — not a failure of a read-only command, and not an empty `error` string. | no | – | yes | only with `--pool` |
| `wallet send` | `TO AMOUNT [--account N] [--priority 0-4] [--note TEXT] [--max-fee AMOUNT] [--yes] [--dry-run]` | `send::build` then, with `--yes` and without `--dry-run`, `send::relay` (§5b) — the CLI calls nothing else on the money path. **`main.rs` chooses the `NodeGate` (§5b) from `settings.node.mode` and the node probe**: `Remote` in remote mode, else `Local(&client)` or `Unreachable`; step 7 does the rest (§2a.1, §2m.3). `--note` and `--max-fee` travel on `SendRequest` and are applied by `send::relay`, not by the caller (§2a.2, §5b). Payload shape: §5c | yes | **yes** | yes | yes |
| `wallet sweep` | `TO [--account N] [--priority 0-4] [--below AMOUNT] [--max-fee AMOUNT] [--yes] [--dry-run]` | same two calls with `is_sweep: true`; the build response is a **list**, so the confirmation prints every tx and the total fee, and `send::relay` calls `relay_tx` once per `tx_metadata_list` entry in order under the §2a.4 record. Payload shape: §5c | yes | **yes** | yes | yes |
| `wallet clear-pending` | `[--yes]` | clears the `pending.json` record after the user has checked (§2a.4). Prints the record and what to check first. | yes | **yes** | no | no |
| `wallet validate` | `ADDRESS` | `address::parse` then `validate_address{any_net_type:true, allow_openalias:false}`; prints `valid/integrated/subaddress/nettype`. `any_net_type:true` removes the `-13 NOT_OPEN` precondition, so this works with or without an open wallet (§2a.6); it falls back to the structural verdict alone, clearly labelled, only when the wallet daemon is unreachable. | no | – | no | no |
| `wallet book` | `list` · `add ADDRESS DESC` · `remove INDEX` | `get_address_book` / `add_address_book` / `delete_address_book` | on add/remove | on remove | yes | no |
| `wallet refresh` | `[--start-height H]` | **`wallet_session::refresh` (§5b)** — `refresh` under `timeouts.refresh_sec` (120 s) followed by `state::note_refresh`, so `status`'s `wallet.last_refresh_at` (§5) has one writer shared with the TUI's `R` and with `send`'s remote gate (§2m.3). Works in both modes; in remote mode it is the command that proves the remote node answers, which is why it is the first thing §4b's LIVE list runs | yes | no | yes | yes |
| `wallet notes` | `get TXID` · `set TXID TEXT` | `get_tx_notes` / `set_tx_notes` | on set | no | yes | no |
| `wallet set-daemon` | `ADDRESS [--trusted]` | One `set_daemon` on the open wallet, with `ssl_support` chosen the way §2m.3's table chooses it for the address's shape: `"disabled"` for a loopback address, `"autodetect"` otherwise. **It is a one-off and the payload says so** (`persistent: false`, §5c): `wallet_session::open` issues its own `set_daemon` after every `open_wallet` (§4.2), so the next `wallet open` replaces whatever this set. The persistent form is `fmonero node remote <addr> --yes` (§2m.5), which this command's hint names. ADDRESS is validated against §2m.2's grammar — the same one `node.remote` uses, so the two commands cannot accept different strings | yes | **yes** | yes | no |
| `settings list` · `get K` · `set K V` | | `~/.config/fmonero/config.toml` | on set | no | no | no |
| `setup` | `[--units] [--plugin] [--menu] [--all] [--yes] [--dry-run]` | §4.2 | yes | **yes** | no | no |
| `doctor` | `[--fix] [--only GROUP] [--dry-run] [--yes] [--save]`, `doctor undo RUN\|latest`, `doctor capabilities` | §7. `--fix` alone performs every fixer in v0.1 (classes A and B); `--yes` is accepted and ignored. | with `--fix` | – | no | no |
| `tui` | `[--node\|--wallet\|--send\|--log]` | §7a | – | – | – | – |
| `capabilities` | — | `agent::capabilities_json` | no | – | no | no |
| `robot-docs` | `[SECTION]` | `agent::ROBOT_GUIDE` | no | – | no | no |
| `version` | — | | no | – | no | no |

**Every node command that needs the local `monerod` refuses in remote mode, in one place.**
With `settings.node.mode == remote` (§2m), `node up`, `node down`, `node restart`, `node peers`,
`node bans`, `node ban`, `node unban`, `node limit` and `node log` exit **4 `backend`**,
`reason: remote_node`,
`error: "fmonero node <verb> needs the local monerod; node.mode is remote (<addr>)"`,
`hint: fmonero node local --yes`, **before any RPC is issued and before any `systemctl` spawn**.
The refusal is structural rather than per-command: `NodeClient::from_ctx` returns that error in
remote mode (§5b) and the six read/mutate commands reach the daemon only through it, while
`node up|down|restart` meet the same error in `node_session`'s `Layout::Remote` arm (§2i). `node
info` is the one node command that still exits 0 — it reports the address, the trusted flag and the
mode (§5c) — and `status` is not a node command and never refuses (§2e). *Test:* one e2e case sets
`node.mode = remote` and asserts all nine exit 4 with `reason: remote_node` and that the mock
recorded **zero** requests, the same shape §2i's `system_unit` case already uses.

**`node ban`, `node unban` and `node limit` do *not* take `--yes`.** They are local, reversible (the
inverse is the adjacent subcommand, or `-1`) and move no money, and the §2i `NodeMutators` type gate
already stops them reaching a node fmonero does not own. A confirmation flag is worth something only
when it is rare: demanding it here teaches `--yes` as a reflex on commands where it costs nothing,
which is the habituation §2a.4 argues against for `clear-pending`. `--yes` is reserved for the
operations where a mistake costs money or a resync — `wallet send`, `wallet sweep`, `wallet create`,
`wallet clear-pending`, `wallet set-daemon`, `node down`, `node restart`, **`node remote`**,
**`node local`**, `setup`. The two mode commands are on that list because they decide which node the
next send is built against, because they restart a daemon that may be holding an open wallet, and
because they change the node an **already-relayed** send would be resolved against (§2a.4) — which
is why they also take the `send.lock` and the `pending_send` check before they write anything
(§2m.5 steps 3 and 4), the same two guards §2a.1 puts ahead of every build.

**Settings keys** (`settings list` prints this table; `set` validates against it):
`node.network` (`mainnet` \| `stagenet` \| `testnet`, default `mainnet`) ·
**`node.mode`** (`local` \| `remote`, default `local` — §2m; `remote` is refused while
`node.remote` is empty, exit 2 `reason: remote_unset`) ·
**`node.remote`** (default `""`; the §2m.2 grammar `[http[s]://]host:port`, port mandatory, no
path, no userinfo — exit 2 `reason: bad_remote_address`; clearing it while `node.mode` is `remote`
is exit 2 `reason: remote_unset`) ·
**`node.remote_trusted`** (`true` \| `false`, default `false` — §2m.6; `true` for a non-private
host is accepted and raises the doctor's `remote_node_untrusted` warning, because a remote node the
owner runs on a LAN or a private overlay is a legitimate case that no local check can distinguish
from a public one by address alone) ·
`node.url` (default `http://127.0.0.1:18081`, loopback-checked per §2c; **unused in remote mode**
and deliberately not relaxed there, §2m.1) ·
`node.unit` (`fmonero-monerod.service`) · `node.data_dir` (`~/.bitmonero`) ·
`node.log_file` (`~/.bitmonero/bitmonero.log`) · `node.prune` (`true`) ·
`wallet.url` (default `http://127.0.0.1:18083`, **loopback-checked per §2c**) ·
`wallet.unit` (`fmonero-wallet-rpc.service`) ·
`wallet.dir` (`~/.local/share/fmonero/wallets/<network>`) · `wallet.port` (`18083`) ·
`wallet.account` (`0`) · `wallet.priority` (`2` — *normal*; `0` is accepted but is an alias for the
lowest tier, not a "wallet default", §1b.4) ·
`timeouts.node_sec` (3) · `timeouts.wallet_sec` (5) · `timeouts.refresh_sec` (120) ·
`timeouts.transfer_sec` (120) · `timeouts.open_sec` (300) · `timeouts.systemctl_sec` (10) ·
`tui.poll_ms` (1000) · `tui.tick_ms` (120).
**18083** is the wallet port because 18082 is `monerod`'s ZMQ port (§1b.1) and a fixed,
documented port is what lets the unit, the doctor, the panel and `robot-docs` agree without
discovery; `wallet.port` changes it and `setup --units` rewrites the unit accordingly.

**An unknown key is redirected, never merely refused.** Twenty-three dotted keys is exactly the namespace
an agent half-remembers (`node.log` for `node.log_file`, `wallet.priority` for
`wallet.default_priority`, `timeouts.node` for `timeouts.node_sec`), and `Settings::KEYS` already
holds the data to suggest from. `settings set` on an unknown key names the nearest key from `KEYS`
by case-insensitive Levenshtein distance ≤ 3 and makes its hint that exact runnable command —
`fmonero settings set wallet.priority 2   # did you mean this?` — falling back to
`fmonero settings list` when nothing is near.

**`node.network` exists so that the first real send need not be mainnet money.** §4b's RED list
tells the owner to rehearse the send path before using it in anger, and a rehearsal the tool cannot
perform is the one dishonesty that costs money rather than credibility (§2j). It is one key and
three template substitutions, using the ports already established in §1b.1:

| `node.network` | node P2P/RPC | wallet RPC | `monerod.conf` extra | wallet argv extra | `wallet.dir` |
|---|---|---|---|---|---|
| `mainnet` | 18080 / 18081 | 18083 | — | — | `…/wallets/mainnet` |
| `stagenet` | 38080 / 38081 | 38083 | `stagenet=1` | `--stagenet` | `…/wallets/stagenet` |
| `testnet` | 28080 / 28081 | 28083 | `testnet=1` | `--testnet` | `…/wallets/testnet` |

Exactly one network is configured at a time — switching means changing the key and re-running
`setup --units`, which rewrites both units, `monerod.conf`, `node.url` and `wallet.url`. The unit
*names* do not change, so nothing else in this document has to branch. `node.data_dir` keeps
Monero's own convention of appending the net name (§1b.1).

**`node.network` and `node.mode` are independent, and remote mode cannot check the pairing.** The
network still decides the wallet unit's `--stagenet`/`--testnet` flag, `wallet.dir` and
`wallet.port`; `node.remote` must simply be a node on that network, and nothing in wallet-RPC
reports a daemon's nettype (§1b.9 item 5), so fmonero cannot verify it. The symptom of a mismatch is
a `refresh` that errors or a wallet that never advances; `node info`'s detail says so (§2m.6).

### 4.2 `setup`

Idempotent; running it twice leaves the same machine. `--dry-run` prints a unified diff and changes
nothing.

- `--units` writes `~/.config/systemd/user/fmonero-monerod.service` and
  `fmonero-wallet-rpc.service` plus `~/.config/fmonero/monerod.conf` (mode 0600, §2c), runs
  `systemctl --user daemon-reload`, and **disables** both units (on-demand only, fbittorrent's
  pattern: `fbittorrent/src/setup.rs:58-66`). All three files are rendered for
  `settings.node.network` (§4), and it rewrites the derived settings keys for that network:
  `node.url`, `wallet.url` **and `wallet.port`** (18083 | 38083 | 28083), so that the unit's
  `--rpc-bind-port`, `wallet.url` and `Paths::wallet_login_file(port)` cannot disagree.
  `wallet.port` is not optional in that list: `monero-wallet-rpc` names its login file after its
  **bind port** (`wallet_rpc_server.cpp:351-380`), so a `node.network stagenet` switch that left
  `wallet.port` at 18083 would have the daemon writing `monero-wallet-rpc.38083.login` while
  `WalletClient::from_ctx` reads `…18083.login`, finds nothing, and — by §2c's own rule — reports
  "the wallet daemon is not running". The symptom points away from the cause, which is the failure
  mode `node.network` exists to avoid.
  It also **creates `settings.wallet.dir` and every missing parent at mode 0700** before writing the
  wallet unit. `monero-wallet-rpc` creates only the *leaf*, with one `mkdir(dir, 0700)`, and treats
  any failure but `EEXIST` as fatal: `init()` returns false and the daemon exits
  (`src/wallet/wallet_rpc_server.cpp:325-340`). With `wallet.dir` at
  `~/.local/share/fmonero/wallets/<network>` the parent does not exist on a fresh machine, so
  without this step the first `systemctl --user start fmonero-wallet-rpc.service` fails with ENOENT,
  `Restart=on-failure` retries it every 5 s into systemd's start limit, and every fmonero command
  reports "the wallet daemon is not running" — the symptom pointing away from the cause again, on
  the very first run.
  It refuses (exit 3) when §2i finds a `system_unit` layout or when
  free space is under the §1.3 floor without `--full-chain --yes`.
  `--rotate-rpc-login` mints a fresh node credential; see below.
- `--plugin` symlinks `plugin/oliver.monero` → `~/.config/omarchy/plugins/oliver.monero`, then
  `omarchy plugin enable oliver.monero` and `omarchy bar put oliver.monero --before omarchy.tailscale`,
  **each skipped when already done, and the pre-check is named** (v5.1): both are read from
  `~/.config/omarchy/shell.json`, the file Omarchy itself writes — §1.5: "Enabling writes
  `{"id":"oliver.monero"}` into `~/.config/omarchy/shell.json`" — and for a bar widget the two
  questions are one fact, because `omarchy plugin enable` puts the id into a section of
  `bar.layout` and `omarchy bar put` "leaves a widget that is already on the bar where it is"
  (`/usr/share/omarchy/bin/omarchy-bar`). An id already in that document reports
  `FileStatus::Unchanged` with **no process spawned**; an unreadable or unrecognised document is
  "cannot tell" and the command runs as before, because skipping a step that was never taken is the
  one wrong answer here. The file rather than `omarchy plugin list`, for two reasons: the listing is
  an IPC call that needs a **running** shell, and `setup` must answer this on a machine whose shell
  has not started; and it would cost a process spawn before the two it exists to avoid. Without the
  pre-check both steps reported `written` on **every** run — the machine was unharmed, since both
  commands are idempotent, but §8's idempotence assertion reads the report, and that is what it
  read (finding 2 of the Phase 4 verification).
- `--menu` merges `crates/fmonero/assets/menu.jsonc` into
  `~/.config/omarchy/extensions/omarchy-menu.jsonc` **textually**, exactly as fproton does: copy
  the file verbatim to `~/.local/state/fmonero/backups/omarchy-menu.jsonc.<UTC basic>`; strip
  comments into a scratch copy only, and parse *that* to learn which ids exist; insert the single
  line for each absent id immediately before the file's final `}`, preceded on first insertion by a
  `// fmonero` marker; leave existing ids byte-for-byte alone. A serde round-trip would delete every
  comment in a user-owned file, so there is none.
  Template: `"monero": {"icon":"󰗠","label":"Monero","aliases":["monero","xmr","wallet"]}` ·
  `"monero.open": {"icon":"󰆍","label":"Open","action":"omarchy-launch-or-focus-tui --app-id=fmonero fmonero tui"}` ·
  `"monero.send": {"icon":"󰧧","label":"Send","action":"omarchy-launch-or-focus-tui --app-id=fmonero fmonero tui --send"}` ·
  `"monero.node": {"icon":"󰒋","label":"Node","action":"omarchy-launch-or-focus-tui --app-id=fmonero fmonero tui --node"}` ·
  `"monero.doctor": {"icon":"󰩺","label":"Doctor","action":"omarchy-launch-or-focus-tui --app-id=fmonerodoctor fmonero doctor"}`.

**The two units.** Written by `setup --units`, and reproduced here so a doctor or plugin agent can
check them without running `setup`:

```ini
# ~/.config/systemd/user/fmonero-monerod.service
[Unit]
Description=Monero daemon (fmonero)
After=network-online.target
Wants=network-online.target
# v5.1: RestartSec=5 can never reach systemd's own 10 s / 5-start default, so a 203/EXEC
# (the answer when monerod is absent) would restart every five seconds forever.
StartLimitIntervalSec=60
StartLimitBurst=3

[Service]
Type=simple
# The ABSOLUTE, XDG-resolved `paths.monerod_conf()` — NEVER `%h/.config/...` (v5.1; see below).
ExecStart=/usr/bin/monerod --config-file /home/<user>/.config/fmonero/monerod.conf --non-interactive
Restart=on-failure
RestartSec=5
# monerod is I/O-heavy and must never compete with the desktop
Nice=5
IOSchedulingClass=best-effort
IOSchedulingPriority=6

[Install]
WantedBy=default.target
```

```ini
# ~/.config/fmonero/monerod.conf   (mode 0600 — it carries rpc-login, §2c;
#  everything `setup` PRINTS redacts that value — see below)
# NOTE: this file is read by monerod through boost::program_options (--config-file,
# src/daemon/command_line_args.h:40-53, applied in src/daemon/main.cpp:215-241), NOT by systemd.
# systemd specifiers (%h, %S) are NOT expanded here: every path must be ABSOLUTE.
data-dir=/home/<user>/.bitmonero
prune-blockchain=1
rpc-bind-ip=127.0.0.1
rpc-bind-port=18081
rpc-login=fmonero:<32 hex chars from /dev/urandom>
no-zmq=1
# The update check is a DNS TXT lookup, so we turn it off — which makes node.update_available (§5)
# permanently false under OUR unit, by design: core::check_updates() returns before m_update_available
# is ever set when check_updates_level == UPDATES_DISABLED (cryptonote_core.cpp:1653-1654, :1663, :1669)
# and get_info reports m_core.is_update_available() verbatim (core_rpc_server.cpp:568).
check-updates=disabled
log-file=/home/<user>/.bitmonero/bitmonero.log
log-level=0
max-log-files=3
out-peers=16
in-peers=32
# stagenet=1 / testnet=1 and the 38081 / 28081 port when settings.node.network says so (§4)
```

**Everything `setup` prints redacts the credential.** The `rpc-login=` value is replaced by
`<redacted>` in the human diff, in `FileAction.diff` under `--json`, and in any `-v` line. The
byte-for-byte comparison that decides `Written`/`Unchanged` runs on the **real** rendered bytes
inside `units::install`, so idempotence and the empty second-run diff are unaffected; only the
displayed diff is redacted. Without this the node credential is printed to stdout by §8's own
Phase 1b gate command (`fmonero setup --units --dry-run | grep -n '%[hSt]'`), becomes a key of the
`setup` JSON payload, and is pasted into DECISIONS.md — which §2b's known-value grep then fails on,
on correct behaviour, and §2c's claim that the token never appears in `--json`, `-v` or a doctor
probe would be false.

**`out-peers=16` / `in-peers=32` narrow monerod's `-1` auto default (§1b.6) deliberately**: the
desktop is the primary workload, connection count is the main driver of monerod's CPU and socket use
at idle, and 16 outgoing is comfortably above the ~8 needed to keep a sync saturated. A later agent
that "tunes" these back to the upstream default is undoing the thing this project exists to protect.
**`--max-concurrency 4` on the wallet unit** (below) halves upstream's all-cores default (§1b.5) for
the same reason: wallet scanning is the one fmonero workload that will happily eat all 8 cores while
the user is doing something else.

`units::render_node_conf` substitutes `ctx.paths.home` and `ctx.settings.node.data_dir` as absolute
paths. **No systemd specifier may reach `monerod.conf`**: boost does no specifier expansion, so a
`%h` here would make `monerod` create a directory literally named `%h` under its working directory
and resync the chain from zero — a failure no mock can catch.
Its guard is therefore a **permanent unit test**, `units::render_node_conf_has_no_systemd_specifiers`,
which calls `units::render_node_conf` directly and asserts the returned string contains no `%h`,
`%S` or `%t`. The guard is about the **conf**, and it stays exactly that: `%S` still appears in the
wallet unit's `StateDirectory=`, `WorkingDirectory=` and `--log-file`, where systemd expands it and
nothing is compared against a setting. The node unit carries **no specifier at all** (v5.1): its
one path, `--config-file`, is now compared against `paths.monerod_conf()`. The Phase 1b gate runs that test **and** greps the rendered `monerod.conf` hunk of
`setup --units --dry-run`'s output. Grepping the file itself after a `--dry-run` would be vacuous:
`--dry-run` changes nothing, so the file does not exist, `grep` exits 2 with an error and produces
no output — the gate would pass whether or not a `%h` was emitted, and a gate that cannot fail is
worse than no gate, because this section cites it as the mitigation and a later agent reads that as
coverage.

**The `rpc-login` token is generated once.** `setup --units` reads
`~/.config/fmonero/monerod.conf` first: if it exists and contains a syntactically valid
`rpc-login=fmonero:<32 hex>`, that exact line is carried into the rendered file and no new
randomness is drawn. Only an absent, unreadable or malformed line draws 16 fresh bytes from
`/dev/urandom`. This is what makes `setup` idempotent against the byte-for-byte `units_present`
check, what keeps `--dry-run`'s diff empty on a second run, and what stops a second `setup` from
locking out a **running** `monerod` whose `NodeClient` would then get 401 → exit 4 and present as
"the node is unreachable". `fmonero setup --units --rotate-rpc-login --yes` is the explicit way to
mint a new one; it prints that the node must be restarted.

```ini
# ~/.config/systemd/user/fmonero-wallet-rpc.service
[Unit]
Description=Monero wallet RPC (fmonero)
After=fmonero-monerod.service
# v5.1, for the same reason as the node unit: three tries in a minute, then stop.
StartLimitIntervalSec=60
StartLimitBurst=3

[Service]
Type=simple
StateDirectory=fmonero/run
StateDirectoryMode=0700
WorkingDirectory=%S/fmonero/run
ExecStart=/usr/bin/monero-wallet-rpc \
  --rpc-bind-ip 127.0.0.1 \
  --rpc-bind-port 18083 \
  --wallet-dir /home/<user>/.local/share/fmonero/wallets/mainnet \
  --daemon-address 127.0.0.1:18081 \
  --trusted-daemon \
  --daemon-ssl disabled \
  --non-interactive \
  --log-file %S/fmonero/run/monero-wallet-rpc.log \
  --log-level 0 \
  --max-concurrency 4
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

**The three daemon flags are rendered from `units::daemon_target(ctx)` and differ by mode**
(v5.2, §2m.3). The unit above is the **local** form; in remote mode the same three lines are

```ini
  --daemon-address node.example.com:18089 \
  --untrusted-daemon \
  --daemon-ssl autodetect \
```

with `--trusted-daemon` instead of `--untrusted-daemon` iff `settings.node.remote_trusted`, and
`--daemon-address` carrying `settings.node.remote` **verbatim, scheme included**. Three points that
are contract rather than taste:

- **exactly one of the two trust flags is always rendered, in both modes.** Giving both is a fatal
  error (`wallet2.cpp:449-450`) and giving *neither* is not "untrusted" — `make_basic` falls through
  to `trusted_daemon = tools::is_local_address(daemon_address)` (`:451-462`), so the flag's absence
  means "whatever the address parse decided", which is precisely the implicit configuration §2j
  objects to on the one setting that decides whether the wallet's own amounts travel in a request
  (§1b.9 item 4).
- **`--daemon-ssl` is rendered in both modes**, from `units::daemon_target`'s address-derived rule
  (§2m.3): `disabled` for a loopback literal host — which local mode's `node.url` always is (§2c) —
  and `autodetect` otherwise. Its
  default is `autodetect` (`wallet2.cpp:253`), so until v5.2 our own unit asked
  `monero-wallet-rpc` to attempt a TLS handshake against `127.0.0.1:18081` on every reconnect and
  fall back to plaintext (§1b.9 item 2) — a wasted round trip against a daemon we started
  ourselves in the clear. `enabled` is not an option in either mode: it throws at process start
  without a CA file, a fingerprint list or `--daemon-ssl-allow-any-cert` (`wallet2.cpp:416-432`),
  and §0 cuts all three.
- **no `--daemon-login` in either mode**, and in remote mode no credential at all: fmonero's
  `rpc-login` belongs to fmonero's own node.

These three lines change the rendered bytes, so the doctor's `units_present` reports `warn:unit_drift`
on a unit written by a pre-v5.2 build and its class-B fixer rewrites it — the same upgrade path
v5.1's two unit changes used, and again no migration code (§7.1).

**`%h` is deliberately not used for `--wallet-dir`.** `settings.wallet.dir` is an absolute path
once `Settings` has loaded it (a `~` in `config.toml` is expanded there, once, and the default is
derived from `paths.wallet_dir` so that `setup --units`' own `want_dir` and the setting cannot
disagree), and the doctor's
`wallet_dir_matches_unit` compares the unit's argument against that path **literally** — so a unit
carrying `%h/...` would make the check report `fail` on a perfectly correct machine, its class-B
fixer rewrite the unit to bytes systemd expands identically, and the next run fail again. A check
that fails on a correct machine is the check an agent relaxes. `%S` stays in
`StateDirectory=`/`WorkingDirectory=`/`--log-file`, which nothing compares against a setting.

**`%h` is not used for the node unit's `--config-file` either** (v5.1, closing `secrets-5`; v5
kept the literal here on the grounds that it was "likewise uncompared", and that was the defect).
`--config-file` is rendered as the absolute `paths.monerod_conf()`, exactly as `--wallet-dir` is
rendered from `settings.wallet.dir`. systemd expands `%h` to `$HOME`, and **there is no specifier
for `XDG_CONFIG_HOME`**: with that variable set to anything but `~/.config`, `setup --units` writes
`monerod.conf` where `Paths` says and the unit names a file that is not there. `monerod` treats a
missing *named* config file as fatal (§1b.6, `src/daemon/main.cpp:243-247`), so it exits
`203`-class immediately, `Restart=on-failure` retries it, and `node up` answers "did not answer
`get_info` within 30 s" — exit 8, "nothing is broken", with the symptom pointing away from the
cause. Two consequences follow, and both are part of the contract:

- the doctor's `units_present`, which compares the installed unit against `units::render_node_unit`
  **byte for byte**, reports `warn:unit_drift` on a unit written by a pre-v5.1 build, and its
  class-B fixer rewrites it — which is the intended upgrade path and needs no migration code;
- the §7.1 fixtures under `tests/fixtures/doctor/{units_present,unit_no_password_in_argv*}/input/`
  carry the absolute form (`/home/user/.config/fmonero/monerod.conf`, matching their
  `_settings` home), and say in their provenance header why.

**`StartLimitIntervalSec=60` / `StartLimitBurst=3` on both units** (v5.1) exist because this
document twice describes a failing unit as retrying "into systemd's start limit" and that was not
true: the manager defaults are `DefaultStartLimitIntervalSec=10s` and `DefaultStartLimitBurst=5`,
and a unit with `RestartSec=5` gets two starts per interval — so the burst of five is never reached
and a `203/EXEC` loops forever. The Phase 4 verifier met exactly that and had to stop both units by
hand. Three tries in a minute, then `failed`, is the behaviour the prose already claimed; the
milliseconds-fast refusal of §2i's preflight is what keeps a user from meeting it at all.

**`--wallet-dir` is rendered from `settings.wallet.dir` and therefore always carries the network
segment**, never from a template constant. It is the daemon that resolves
`<wallet-dir>/<filename>` (`wallet_rpc_server.cpp:3685`), and a wallet name may not contain `/`
(`:3571`, `:3670`, `:3900`, `:4076`), so a one-level mismatch is **unbridgeable at call time**: a
unit saying `…/wallets` while `settings.wallet.dir` says `…/wallets/mainnet` would have
`wallet create` writing into one directory while `wallet list`, the TUI's open modal and the
doctor's `wallet_dir_exists` all read the other, and §4b's stagenet rehearsal would fail at the
`wallet open` step. It would also put a mainnet and a stagenet wallet of the same name in one
directory, which is exactly what that rehearsal sequence produces. The doctor's
`wallet_dir_matches_unit` check (§7.1) asserts the two agree byte for byte.

**`--log-level 0` is a secrets control, not a tuning knob.** Above 0, epee logs HTTP request and
response headers (`contrib/epee/include/net/http_protocol_handler.inl:457`, `:638`, at
`LOG_PRINT_L3`) and `wallet2` logs its own internals, into a file that lives in the state dir §2b.7
explicitly *exempts* from the 0600 rule, in a directory that also holds the daemon's credential —
and that log records the open wallet's name, its heights and its refresh activity. §2b's known-value
grep reads stdout, stderr, `state.json`, `history.bin`, doctor runs, fixtures and DECISIONS.md, and
never reads that log, so nothing would notice the level changing. The doctor's
`unit_no_password_in_argv` therefore also asserts both units' `--log-level` is `0` (§7.1), and §4b's
rehearsal greps the log for the test password.

**`StateDirectoryMode=0700` is not optional either.** systemd's default for `StateDirectory=` is
**0755**, and this directory holds `monero-wallet-rpc`'s credential file and its log — a log that
records the open wallet's name, its heights and its refresh activity — in a tree §2b.7 declares
owner-only, on a machine whose threat model (§2c) explicitly includes other local users.

**`Wants=fmonero-monerod.service` is deliberately absent.** `After=` orders but never starts;
`Wants=` would make `systemctl --user start fmonero-wallet-rpc.service` — which `wallet open` does
whenever the unit is down — start a `monerod` **without** the §2i single-writer check, and two
`monerod` on one data dir corrupt the LMDB (§1.2). Every node start goes through `node up`, which
runs `detect_layout` and `data_dir_in_use` first. The window is real even though `setup --units`
refuses under a `system_unit` layout: the user can enable Arch's `monerod.service` at any time after
setup, and `~/.bitmonero` is also the default data dir of a hand-run `monerod`.
`monero-wallet-rpc` starting before the node is harmless — it retries the daemon connection.

`%S` is `$XDG_STATE_HOME` (`~/.local/state`), so `StateDirectory=fmonero/run` creates
`~/.local/state/fmonero/run` before the daemon starts and `WorkingDirectory` puts the login file
there (§2c). `Paths::run_dir` follows `FMONERO_STATE_DIR` when it is set, so a test's `run_dir` and
the unit's working directory diverge — which is fine, because a test never installs a unit: it
points `WalletClient::new` at `fmonero-mock --auth` instead. `units::install` refuses to run at all
when `FMONERO_STATE_DIR` is set, so that divergence can never produce a half-written unit.

**And `Paths::systemd_user_dir` follows `FMONERO_SYSTEMD_USER_DIR` under the same guard** (v5.1,
§2i's seam list). Because `install` refuses to write a unit under a test state dir, the *absence* of
a unit is the only unit state a test can produce — and until v5.1 it could not produce even that,
since `Paths::discover` read the real `XDG_CONFIG_HOME`. The Phase 4 verifier's one real
`scripts/install.sh --yes` therefore broke ten assertions of this project's own suite, each of which
had silently been asserting "nothing is installed on this machine". A suite a correct installer
breaks is a suite that gets weakened, so the seam is the fix and the assertions are unchanged.

Note what is **absent** from that argv and why: no `--disable-rpc-login` and no `--rpc-login`, so
the daemon writes its own credential into `WorkingDirectory` (§2c); no `--password`, ever (§2b.1);
no `--wallet-file` and no `--password-file`, because they are incompatible with `--wallet-dir`
(§1b.5); no `--restricted-rpc`, because it would block `relay_tx` (§1b.4); no `--detach`, because
systemd owns the process; and **no `--daemon-login`**, because `/proc/<pid>/cmdline` is
world-readable (§2c).

**That last omission has a consequence that must be handled, or the wallet can never sync.** §2c
gives fmonero's own node unit `rpc-login=fmonero:<32 hex>`, and `wallet2` takes the daemon
credential only from `--daemon-login` (`src/wallet/wallet2.cpp:252`) or from `set_daemon`'s
`username`/`password` fields (`wallet_rpc_server_commands_defs.h:2611-2626`). With neither, every
wallet→daemon call gets a 401 and the wallet cannot refresh, build a transfer or relay one.
So: **every successful `open_wallet` is followed by exactly one** `set_daemon`, whose four
arguments come from `units::daemon_target(ctx)` (§5b) — the same accessor
`units::render_wallet_unit` reads, so the unit and the call cannot disagree. In **local** mode that
is `{ address: host_port(settings.node.url), trusted: true, ssl_support: "disabled" (the
address-derived value of §2m.3), username, password }`, with the credential read from `~/.config/fmonero/monerod.conf` through
`Login::from_monerod_conf` — the same 0600 file the node client reads; when that file has no
`rpc-login=` (the Arch `system_unit` layout, §2i), the login is `None` and the two fields are
omitted. In **remote** mode it is `{ address: settings.node.remote, trusted:
settings.node.remote_trusted, ssl_support: "autodetect" }` — `"disabled"` for a loopback
`node.remote` — with **no** `username` and **no** `password`, ever (§2m.3).

**It cannot be issued before `open_wallet`.** `on_set_daemon` begins
`if (!m_wallet) return not_open(er);` (`src/wallet/wallet_rpc_server.cpp:4692`), i.e. it answers
`-13 NOT_OPEN` with no wallet loaded. A "start unit → `set_daemon` → `open_wallet`" order would fail
that call every time, never install the credential, and present as "the node is unreachable" —
defeating the entire reason this paragraph exists. **A `-38 NO_DAEMON_CONNECTION` from `set_daemon` does not mean what v5 said it meant**, and v5.2
corrects it (§1b.9 item 1): `set_daemon` opens no socket — it is
`m_http_client->set_server(...)` (`wallet2.cpp:1409`), a URL parse
(`abstract_http_client.cpp:134-148`) — so `-38` "Unable to set daemon" (`:4758-4763`) means the
address did not **parse**. In **local** mode it still does not fail `wallet open`: the wallet is
open, `node.url` is loopback-validated so the case is nearly unreachable, and `status` reports the
truth either way, so fmonero raises the `node_down`/`wallet_down` warning and exits 0 — the
behaviour is unchanged and only its justification is now the right one. In **remote** mode it is
**not** swallowed: the address passed §2m.2's grammar minutes earlier, so a `-38` means fmonero's
grammar and epee's parser disagree, and a `wallet open` reported successful for a wallet whose
daemon was rejected is the silence this document forbids. It exits **4 `backend`**,
`reason: remote_address_rejected`, message verbatim,
`hint: fmonero node remote <addr> --yes`. The whole four-step sequence lives in one place — `wallet_session::open`
(§5b) — because the CLI subcommand and the TUI's open modal are two independently written callers
of it, and §2a.3 already settles what happens to an invariant that lives in two. In local mode `--daemon-address` points at the
*unrestricted* local node and `--trusted-daemon` is therefore correct; if §2i found the Arch system
unit instead, `setup --units` writes the same address (it is the same port) and the doctor checks
whether that node is restricted. In remote mode neither holds, which is why the flag is
`--untrusted-daemon` unless the owner said otherwise (§2m.6).

### 4a. Exit codes

0–8 are fproton's and fbittorrent's, unchanged, so an agent that knows one knows all three.
Printed verbatim by `capabilities` and `robot-docs`.

| Code | `code_name` | Meaning | Typical hint |
|---|---|---|---|
| 0 | `ok` | success | — |
| 1 | `failure` | unexpected (bug, I/O) | `fmonero doctor` |
| 2 | `usage` | bad arguments, unknown settings key, non-loopback URL, bad amount | `fmonero <cmd> --help` |
| 3 | `safety` | refused without `--yes`; refused because a `pending_send` exists; refused to start a second node. The `reason` key (§4) says which. | `… --yes` |
| 4 | `backend` | `monerod`/`monero-wallet-rpc` not installed, unit missing, daemon unreachable, auth refused, **or a node command that needs the local daemon while `node.mode` is `remote`** (`reason: remote_node`, §2m) | `omarchy pkg add monero` |
| 5 | `upstream` | a daemon errored, or was killed at its deadline (a stall lands here and arms the breaker, §2d) | `fmonero doctor` |
| 6 | `not_found` | unknown wallet name, address-book label, txid, settings key | `fmonero wallet list` |
| 7 | `ambiguous` | an address-book label matched several entries | `fmonero wallet book list` |
| 8 | `timeout` | `node up` never saw one `get_info` answer within 30 s, or `node up --wait` hit `--wait-timeout` (default 1 800 s) before `synchronized` (§4.1); nothing is broken | `fmonero status --json` |
| 9 | `wallet_locked` | no wallet open, wrong password (which arrives as `-1`, not `-22` — §1b.4), or a background-sync wallet | `fmonero wallet open <name>` |
| 10 | `node_syncing` | the daemon the wallet builds against is not current enough for this operation: locally `get_info.synchronized` is false (`reason: node_not_synchronized`), remotely the §2m.3 bounded `refresh` was killed at `timeouts.refresh_sec` (`reason: wallet_catching_up`) | `fmonero status --json` · `fmonero wallet refresh` |
| 11 | `doctor` | the doctor found failures | `fmonero doctor --fix` |
| 12 | `doctor_partial` | a fixer failed, or fixed only part | `fmonero doctor` |
| 13 | `doctor_refused` | unsafe precondition or undo integrity failure | see detail |
| 14 | `insufficient_funds` | `NOT_ENOUGH_MONEY` / `NOT_ENOUGH_UNLOCKED_MONEY` (§1b.4) | `fmonero wallet balance` |
| 15 | `invalid_address` | structural failure, or `validate_address` said no, or the wrong nettype | `fmonero wallet validate <addr>` |
| 16 | `tx_rejected` | the transaction could not be built or was refused on relay | `fmonero node info` |
| 130 | `interrupted` | SIGINT, or a cancelled TUI action | — |

**10 `node_syncing` is narrow on purpose.** It is returned by exactly one *step*: step 7 of `send`
and `sweep` (§2a.1), because a transaction built against a chain tip we do not have is a transaction
that may be rejected. v5.2 gives that step a second implementation for remote mode (§2m.3) and the
code stays the same, because the instruction to the caller is identical — *wait, then retry* — while
the two situations differ in what one waits for; `reason` separates them (§4), which is what keeps
the code narrow without keeping it ambiguous. Everything else — including `wallet balance`
against a wallet that is behind — returns its real answer with exit 0 and a `wallet_behind` or
`node_syncing` **warning**, because a syncing node is the normal state for hours and an exit code
that means "wait" is an error an agent cannot act on.

### 4b. Verification matrix

Columns: `command | scenario | class | how verified | evidence | claimed in README?`. Phase 4 fills
the evidence column with real output and pastes the table into DECISIONS.md.

**The unit of this matrix is a (command, scenario) pair, not a command.** A command may appear in
several lists under different scenarios — `wallet send` is GREEN for its argument checks, AMBER for
the mocked build-and-relay, and RED for a real broadcast. The gate is: **every command in §4.1
appears in at least one list**, and README's "Status" section mirrors the *lowest* class any of its
scenarios reaches. There are **four** lists since v5.2: GREEN, AMBER, **LIVE** — real daemon
contact, recorded GREEN, which remote node mode makes possible for the first time (§2m) — and RED. A command in no list is a Phase 1b gate failure. (The old "exactly one list"
rule was false against this section's own content — `node up`, `wallet open` and `wallet send` each
appeared in two or three — and a gate its own document fails is a gate an agent quietly drops.)

- **GREEN — verifiable end to end on this machine today, with no Monero installed:**
  `status` (both daemons absent: exit 0, `reachable:false`, warnings), `wallet list` (empty dir),
  `wallet validate` (structural path, wallet daemon unreachable), `settings list|get|set`,
  `capabilities`, `robot-docs`, `version`, `setup --dry-run`,
  **`doctor`** — which **exits 11**, not 0. Restated on 2026-09-17 for the machine as it now is
  (§1.1): `monerod_installed`, `wallet_rpc_installed` and `version_supported` **pass** and name
  `/usr/bin/monerod`'s real version; the one `fail` is **`node_reachable`** (`node_unreachable`),
  because the package is installed and nothing is running; every other `node`/`wallet`/`storage`
  check is a `skip:<reason>` or a `warn` that names one, **never a bare `skip`**; and the `install`
  and `security` groups carry real verdicts. `tests/e2e.sh` asserts that set. The previous wording
  — "exactly `monerod_installed` and `wallet_rpc_installed` at `fail:not_installed`" — was the
  assertion the install broke, and it is replaced rather than relaxed: the shape being asserted is
  still "exit 11 with a named failing set and no bare skips" —, `doctor capabilities`,
  **`doctor undo latest`** (round trip under `FMONERO_STATE_DIR`: corrupt a class-A file, `--fix`,
  `undo latest`, byte-compare) and `doctor undo` against a run whose backup hash mismatches
  (exit 13),
  `tui` (opens, renders every screen in its empty state, quits cleanly), `node up` with no units
  (exit 4 `unit_missing`), `wallet open` with no unit (exit 4 `unit_missing`),
  **`node up`, `node restart` and `wallet open` with the units installed and no Monero binary
  (exit 4 `not_installed`, in milliseconds — §2i's preflight, v5.1)**, `wallet send` with no wallet daemon (**exit 4
  `reason: unreachable`**, at step 5 and before any build — steps 1–4 of §2a.1's refusal order run
  first, which they can because `WalletClient::from_ctx` succeeds with no login file (§2c), so a bad
  address is still exit 15 and an unresolved `pending_send` is still exit 3, but a valid address
  with the daemon down is exit 4, *not* the no-`--yes` 3),
  `wallet send --dry-run --yes` with no daemon (same, and proof the flag pair cannot relay),
  `fmonero node down --dry-run` (exit 2 — `--dry-run` is implemented by six commands and this is
  not one of them, §4),
  `wallet clear-pending` (pure state), `node log` with no log file (exit 0, "no log yet"), bare
  `fmonero` on a pipe (= `status --json`) and on a pty (opens the TUI), every `--help`, every usage
  error (exit 2), amount and address parsing, and the §2l (a) timing.
  **Remote node mode's local half (v5.2, §2m)**, all of which is settings and rendering and needs no
  daemon of any kind: `settings set node.mode remote` with `node.remote` empty (exit 2
  `reason: remote_unset`) and `settings set node.remote ""` while the mode is remote (same reason);
  every §2m.2 refusal row — `node remote node.example.com` (no port), `… http://n:18089/json_rpc`
  (a path), `… user:pass@n:18089` (userinfo), `… ftp://n:18089`, `… n:70000`, `… [::1:18089` —
  each exit 2 `reason: bad_remote_address`; `node remote <addr>` without `--yes` (exit 3
  `needs_yes`, hint = the command with `--yes` appended); `node remote <addr> --dry-run` (exit 0 **without** `--yes`, `config.toml` and both units
  byte-identical afterwards) and `node remote <addr> --dry-run --yes` (the same, proving
  `--dry-run` beats `--yes` here as on the send path);
  **the two money guards of §2m.5 steps 3 and 4, which need no daemon at all**: with a
  `pending.json` whose single entry is `relayed: false`, `node remote <addr> --yes` and
  `node local --yes` each exit **3 `safety`**, `reason: pending_send`, leaving `config.toml` and
  both units byte-identical, while `node remote <addr> --dry-run` on the same state still exits 0
  (the documented exemption); and with `send.lock` held by another process, `node remote <addr>
  --yes` exits 3 `reason: send_in_progress`; with `node.mode = remote`,
  **all nine** of `node up|down|restart|peers|bans|ban|unban|limit|log` exiting 4
  `reason: remote_node` with the mock recording zero requests; `node info` in remote mode (exit 0,
  the §5c key set with four `null`s); `status --json` in remote mode (exit 0, `node.layout ==
  "remote"`, `node.reachable == null`, `node.trusted == false`, the `remote_node` warning);
  `doctor --json` in remote mode (the node/storage skips, `node_layout: pass`,
  `monerod_installed: warn`, exit 11 only if something else fails); and
  `setup --units --dry-run` in remote mode rendering `--daemon-address <node.remote>`,
  `--untrusted-daemon` and `--daemon-ssl autodetect` in the wallet-unit hunk.

- **AMBER — verified against `crates/fmonero-mock` serving the derived fixtures, which proves our
  wire format and our parsing but proves nothing about a real daemon:**
  `node info`, `node peers`, `node bans`, `node ban`, `node unban`, `node limit`,
  `wallet open|close|balance|address|new-address|history|refresh|book|notes|set-daemon`,
  `wallet send --dry-run`, `wallet send` **without** `--yes` (builds with `do_not_relay`, prints
  the mock's real fee, relays nothing, exits 3 — §2a.1), `wallet send --yes` (the mock asserts
  exactly one `transfer` and one `relay_tx` with identical metadata, §2a.3),
  `wallet sweep --dry-run`, `status` with both daemons
  "up", the restricted-node degradation (`--restricted` makes the mock answer `-32601`, and
  `--scenario restricted` makes `get_info` return the poisoned fields of §1b.3; we assert the
  warning, the `null`s and the empty panes), Digest authentication including the `nc` reset and
  increment (`--auth`), the §1b.4 error map (`--fail <method>=<code>:<msg>`, one case per mapped
  code), the §2d deadline, breaker and `--force` paths, the §2d wallet-syncing classification,
  `wallet sweep --yes` (asserts one `relay_tx` per entry of `tx_metadata_list`, in order, and the
  §2a.4 record after a mid-sweep kill), **`wallet sweep` in any form asserted to carry
  `subaddr_indices_all: true` in the recorded body (§2a.10)**, `node log` against a synthetic log
  fixture,
  `wallet create` (the mock accepts and records the call; **no wallet file is ever
  created**, so this proves the request shape, the double prompt and the empty-password refusal
  only), the §2l (b) and (d) timings, and —
  against the §2i `/proc` + `systemctl` fixtures, which prove our *detection logic* and not that a
  real Arch install is detected — `node up` with the Arch system unit present (exit 3) and the
  §2i mutating-call gate (`node down|restart|ban|unban|limit` exit 3 with zero requests recorded),
  **and the `"BUSY"` degradation** — `--scenario syncing` makes `get_last_block_header` answer
  `status: "BUSY"` with an all-zero header and we assert `status --json` reports
  `top_block_age_sec: null` and still exits 0 (§1b.3).
  **Remote node mode's wire half (v5.2, §2m)**, against `fmonero-mock --role wallet` plus the §2i
  `systemctl` fixture: `node remote <addr> --yes` end to end (the three keys written in one save,
  both units re-rendered, the wallet unit's `<is-active>` column flipped by the fixture's restart,
  `wallet_must_reopen` carrying the name that was open and `state::last_open` cleared afterwards),
  and the same command run **twice** asserting the second reports every action `unchanged`,
  `wallet_unit_restarted: false` and exit 0; `node local --yes` returning the machine to the local
  render; `wallet open` in remote mode, asserted **on the mock's request log**, issuing exactly one
  `set_daemon` whose body has `address == node.remote`, `trusted == false`,
  `ssl_support == "autodetect"` and **no `username` and no `password` keys at all**; and the three
  step-7 cases of §2a's test list — one `refresh` before the `transfer`, `-38` → exit 4
  `unreachable`, a deadline miss → exit 10 `wallet_catching_up` with no breaker armed.
  Every AMBER row runs in its own `FMONERO_STATE_DIR` with `--timeout 3`, and every row that reaches
  the wallet starts the mock with `--auth fmonero:<pass>` **and writes the matching
  `$FMONERO_STATE_DIR/run/monero-wallet-rpc.<port>.login` first** (§2c) — without it every wallet row
  exits 4 `auth_refused` instead of testing what it names.

- **LIVE — the rows remote node mode moves out of RED, verified against a REAL public node with a
  real `monero-wallet-rpc` and a scratch wallet (v5.2).** Monero 0.18.5.1 is installed on this
  machine, the owner runs no local node, and §2m makes a public one usable — so for the first time
  a row can leave RED **without** a local chain. The pair verified live, in this order, each
  recorded GREEN with real output pasted into DECISIONS.md:
  `wallet create <scratch> --yes` · `wallet open <scratch>` · `wallet refresh` ·
  `wallet balance` · `wallet address` · `wallet history --limit 5` — **on an empty wallet**, so no
  amount and no counterparty address is ever recorded (§2b.2's rule applied to the evidence, not
  only to the code).
  **The `wallet refresh` row is recorded GREEN only after the restore height is checked** (§2m.3):
  in remote mode the *node* set it, over the unit's argv and before any fmonero call, so the
  rehearsal asserts `blocks_fetched` is a plausible top-up and not a genesis scan — a run that
  scanned from 0 would be `wallet create` silently recording restore height 0 against a node that
  did not answer, which is a RED row wearing a GREEN one's output.
  Each is a (command, scenario) pair distinct from its local-mode row: the same
  commands stay **RED** against a local `monerod`, which still does not exist on this machine, and
  README's Status section mirrors the lowest class each command reaches. The scratch wallet's
  password comes from `$FMONERO_WALLET_PASSWORD`, the wallet lives under `wallet.dir` and is
  deleted afterwards, and the run also greps
  `~/.local/state/fmonero/run/monero-wallet-rpc.log` for that password expecting 0 hits (§4.2's
  `--log-level 0`). Rehearsal:

  ```bash
  fmonero node remote <host>:18089 --yes      # a public node; NOT --trusted (§2m.6)
  fmonero wallet create scratch --yes         # prompts twice; refuses an empty password
  fmonero wallet open scratch
  fmonero wallet refresh --json               # the first real contact with a stranger's node;
                                              # blocks_fetched must be a top-up, NOT a genesis scan
                                              # — the node set the restore height (§2m.3)
  fmonero wallet balance --json               # expect 0; an empty wallet is the point
  fmonero wallet address --json
  fmonero wallet history --limit 5 --json     # expect an empty transfers array
  fmonero status --json | jq '.node'          # layout "remote", reachable null, trusted false
  fmonero doctor --json | jq '.summary'
  fmonero node local --yes                    # leave the machine as it was found
  ```

- **RED — cannot be verified at all today; must not be claimed working anywhere:**
  a real `node up`, `node down` and `node restart` against `monerod` (they need a unit and a real
  process, which no mock supplies), real sync progress, real peers, a **real send or sweep of any
  amount** (in either mode — remote mode changes what a send is *gated* on, never what a broadcast
  proves, §2m.3), a real balance / `refresh` / `open_wallet` **against a local node**, a real
  `setup`/`setup --units`/`--plugin`/`--menu` run (the one real `setup` belongs to the
  Phase 4 verifier, through `scripts/install.sh --yes` — **done**, twice, and again after the v5.1
  fix-up), `setup --units`' refusal under the §2i `system_unit` layout (which needs Arch's own
  `monerod.service` on the machine), **the §1.3 disk-space floor as a NUMBER** — the refusal it
  drives is GREEN since v5.1, because free space is a local `statvfs` measurement and a test may
  point `node.data_dir` at a filesystem under the floor; what stays RED is the 120 GB figure
  itself, which §1.3 states as an estimate —, the §2l (c) warm timing, and every doctor check in
  the `node`, `wallet` and `storage` groups against a live daemon. (**Background sync is not on this list**: §0 cuts the three
  `*_background_sync` methods from v0.1 and §1b.4 lists them as having no consumer. A cut feature
  inside the honesty instrument implies a path that merely has not been tested yet.)
  **The exact commands the owner runs to turn these GREEN**, in order. The rehearsal runs on
  **stagenet**, which `node.network` (§4) makes real: stagenet coins are free from a faucet, so the
  send path is exercised for the first time with nothing at stake. It needs a local node, so it is
  separate from and additional to the LIVE list above — remote mode turns the six read-only wallet
  rows green without a chain, and nothing but this rehearsal turns the **send** row green.

  ```bash
  omarchy pkg add monero                        # installs monerod + monero-wallet-rpc
  scripts/install.sh --yes                      # builds, installs to ~/.local/bin, setup --all
  fmonero settings set node.network stagenet    # ports 38080/38081/38083, wallets/stagenet
  fmonero setup --units --yes                   # writes both units + monerod.conf (0600), stagenet
  fmonero node up --wait --wait-timeout 86400   # expect: exit 0 once synchronized, or exit 8
  fmonero wallet create stagetest --yes         # wallet-rpc creates it under wallet.dir;
                                                # prompts twice, refuses an empty password
  monero-wallet-cli --wallet-file ~/.local/share/fmonero/wallets/stagenet/stagetest --offline
                                                # then type: seed — WRITE IT DOWN BEFORE FUNDING
  fmonero wallet open stagetest                 # prompts for the password, no echo
  grep -c "$YOUR_TEST_PASSWORD" \
    ~/.local/state/fmonero/run/monero-wallet-rpc.log          # expect: 0  (§4.2 --log-level 0)
  fmonero wallet address --qr                   # fund this from a stagenet faucet
  fmonero wallet history --limit 5 --json
  fmonero wallet send <addr> 0.01 --dry-run     # expect: a real fee, nothing relayed
  fmonero wallet send <addr> 0.01 --yes         # THE row that matters; then check the txid in history
  fmonero wallet send <addr> 0.01 --yes         # run it TWICE: proves the §2a.4 record self-clears
  fmonero node restart --yes                    # RED row: a real restart against monerod
  fmonero node down --yes                       # RED row: stop_daemon then systemctl --user stop
  fmonero node up --wait --wait-timeout 3600    # RED row: a real start
  S=$(date +%s%N); for i in $(seq 20); do fmonero status --json >/dev/null; done; E=$(date +%s%N)
  echo "warm status mean: $(( (E-S)/20000000 ))ms"   # the §2l (c) figure; paste into DECISIONS.md
  fmonero doctor --json | jq '.summary'
  tests/e2e.sh --full
  # then, only once every row above passed, repeat on mainnet:
  fmonero settings set node.network mainnet && fmonero setup --units --yes
  ```

  Mechanically enforced: Phase 3 greps README, every `--help` string and `robot-docs` for any
  sentence claiming a successful send, a successful sync or a live peer list, and fails on a hit
  that is not adjacent to the word "unverified".

---

## 5. Status JSON (schema 1) — the contract for the plugin, the TUI and agents

**Shape rules.** Every key is always present. Unknown is `null` — never an absent key, never `{}`,
never a zero standing in for unknown. **A daemon `status` other than `"OK"` is not a result** and is
never rendered: `rpc/jsonrpc.rs` turns `"BUSY"` into `RpcError::Busy` before deserializing, so the
value-initialised zeros a `CHECK_CORE_READY()`-gated call returns during the whole initial sync
become `null` here (§1b.3). Timestamps are RFC 3339 with a local offset (jiff). Durations
are integers named `*_sec`. Every monetary value appears twice: `*_pico` (integer) and `*_xmr`
(pre-rendered string, §4). **`status` starts nothing and calls no wallet mutator (§2e).** It *does*
write `~/.local/state/fmonero/state.json` — the 60 s cold cache, the `get_net_stats` sample pair and
`staleness` — through a write to `.tmp.<pid>` at mode 0600 followed by `rename(2)`, so the plugin's
3 s polling can never observe a half-written file, and a read-only state dir degrades to `null`
rates rather than an error. (An implementer told "status never mutates" writes no cache, and then
`rx_bps` is permanently null and the cold path does not exist.)

**Every read-modify-write of a state file holds an exclusive lock for its whole duration.**
`state::with_state(ctx, |s| …)` opens `state_dir/.lock` `O_CREAT|O_RDWR` at 0600, takes
`flock(LOCK_EX)` with a 2 s deadline, loads, mutates, writes `.tmp.<pid>` at 0600, fsyncs, renames
and releases. On a lock it cannot take within the deadline the write is **skipped** — a lock fmonero
cannot take never blocks `status`, which is a read-only document with a stale cache at worst.
`rename(2)` alone makes a *reader* safe; it does not make two read-modify-write cycles safe, and
`status` runs from the plugin every 3 s and from the TUI every second. The money record is not
covered by this rule because it is not in this file at all: `pending.json` (§2a.4) is its own 0600
file with three writers, none of which is `status`.

The call set is fixed and bounded:

- **Hot path, every invocation — three calls with the wallet closed, five with it open:** node
  `get_info` and `get_last_block_header`; wallet `get_height`, which is simultaneously the
  reachability probe *and* the open/closed probe (an answer ⇒ reachable and open; `-13 NOT_OPEN` ⇒
  reachable and closed; a transport error ⇒ unreachable), and, only when open, `get_balance` and
  `get_address`. `wallet.authenticated` is true when that `get_height` completed a Digest handshake.
  With the wallet closed there is nothing to spend two guaranteed `-13`s on. Each call is answered
  from loopback in single-digit milliseconds; together with two `systemctl --user is-active` spawns
  they are the §2l (b)/(c) budget.
- **Cold path, at most once per 60 s and cached in state:** node `hard_fork_info{version:0}`,
  `get_fee_estimate`, `POST /get_net_stats`, `POST /get_limit`, and wallet `get_accounts` and
  `get_version` (which is the only source of `wallet.rpc_version` — null until the first cold pass).
  Their fields are served from the cache in between, and `staleness.node_extras_age_sec` says how
  old it is. A 60 s window is also what makes `net.rx_bps`/`tx_bps` meaningful: they are the delta
  between two `get_net_stats` samples, and a 3 s window on a node's lifetime counters is noise.
- **Never:** `sync_info`, `get_connections`, `get_peer_list`, `get_transfers`, `refresh`. Those
  belong to `node info`, `node peers` and `wallet history`. `get_transfer_by_txid` is not here
  either: the §2a.4 resolve step belongs to `send`/`sweep`, not to a read-only document.
- **In remote mode, every node call above — hot and cold — is skipped and no `NodeClient` is
  constructed at all** (§2m.3). The wallet halves of both paths are unchanged, and both
  `systemctl --user is-active` spawns stay: `units.node` is a real field a user switching back
  needs, and one §2l budget that holds in both modes is worth more than a mode-dependent pair.
  Remote mode therefore only ever makes **fewer** calls than local mode, so (a), (b) and (d) stand.

```json
{
  "ok": true, "schema": 1, "generated_at": "2026-09-16T14:02:11+02:00",
  "node": {
    "layout": "user_unit",
    "url": "http://127.0.0.1:18081",
    "trusted": true,
    "reachable": true,
    "restricted": false,
    "authenticated": true,
    "nettype": "mainnet",
    "height": 3541122,
    "target_height": 0,
    "sync_percent": 100.0,
    "synchronized": true,
    "busy_syncing": false,
    "offline": false,
    "top_block_age_sec": 47,
    "outgoing_connections_count": 16,
    "incoming_connections_count": 3,
    "white_peerlist_size": 1000,
    "grey_peerlist_size": 5000,
    "tx_pool_size": 14,
    "alt_blocks_count": 0,
    "database_size": 118235000000,
    "database_size_rounded": false,
    "free_space": 96000000000,
    "hard_fork_version": 16,
    "version": "0.18.5.1-release",
    "update_available": false,
    "uptime_sec": 41233,
    "net": {"total_bytes_in": 0, "total_bytes_out": 0, "rx_bps": null, "tx_bps": null,
            "limit_up_kbps": 2048, "limit_down_kbps": 8192},
    "fee": {"per_byte_pico": 20000, "fees_pico": [20000, 80000, 320000, 4000000],
            "quantization_mask": 10000}
  },
  "wallet": {
    "unit_active": true,
    "reachable": true,
    "authenticated": true,
    "url": "http://127.0.0.1:18083",
    "open": true,
    "name": "main",
    "account_index": 0,
    "primary_address": "4…",
    "height": 3541122,
    "blocks_behind": 0,
    "balance_pico": 1234567891234, "balance_xmr": "1.234567891234",
    "unlocked_pico": 1234567891234, "unlocked_xmr": "1.234567891234",
    "blocks_to_unlock": 0,
    "time_to_unlock_sec": 0,
    "subaddress_count": 3,
    "accounts": 1,
    "rpc_version": "1.31",
    "last_refresh_at": "2026-09-17T09:41:02+02:00"
  },
  "units": {
    "node":   {"name": "fmonero-monerod.service",    "exists": true, "active": true,  "enabled": false},
    "wallet": {"name": "fmonero-wallet-rpc.service", "exists": true, "active": true,  "enabled": false}
  },
  "pending": null,
  "warnings": [],
  "last_error": null,
  "staleness": {"node_probe_ms": 6, "wallet_probe_ms": 11, "node_extras_age_sec": 41,
                "history_cache_age_sec": 12}
}
```

Field-by-field, with the RPC each one comes from and what makes it null:

| Field | Type | Source | Null when |
|---|---|---|---|
| `node.layout` | `"user_unit"\|"system_unit"\|"remote"\|"none"` | §2i detection — **configuration, not occupancy**: who holds the data dir is `node up`'s precondition, not a term here. `"remote"` is keyed on `settings.node.mode` alone (v5.2, §2m.1) and is the discriminator every consumer branches on | never |
| `node.url` | string | **the address in force** (v5.2): `settings.node.url` in local mode, `settings.node.remote` — verbatim, scheme included — in remote mode, both through `units::daemon_target` (§5b). It means "where the daemon is", which is what every consumer already reads it as; a loopback URL nothing contacts would be the §2j failure in the one field people copy | never |
| `node.trusted` | bool | **new in v5.2.** `true` in local mode (the unit renders `--trusted-daemon` and `set_daemon` sends `trusted: true`); `settings.node.remote_trusted` in remote mode. It is in `status` rather than only in settings because it changes what wallet2 puts *in a request* — `select_available_outputs_from_histogram` sends the wallet's own unspent amounts when trusted (`wallet2.cpp:12281-12282`, §1b.9 item 4) — so it is a privacy fact about the running system, not a preference | never |
| `node.reachable` | bool \| null | any answer at all from `get_info`. **`null` in remote mode** (v5.2): fmonero constructs no node client, and no wallet-RPC method reports the daemon's state (§1b.9 item 5), so `false` would be a claim and `true` a guess | remote mode |
| `node.restricted` | bool \| null | `get_info.restricted` — a field of the hot-path call, so `status` never needs the `sync_info` probe; `node info` and the doctor cross-check it with `is_restricted()` (§1b.2) | node unreachable |
| `node.authenticated` | bool \| null | `NodeClient::authenticated()` (§5b) — whether the hot-path call went through a Digest handshake (`true`) or was accepted with no challenge at all (`false`). What the wire did, never what `monerod.conf` holds (§2j) | node unreachable |
| `node.height`, `target_height`, `synchronized`, `busy_syncing`, `offline`, `tx_pool_size`, `alt_blocks_count`, `*_connections_count`, `*_peerlist_size`, `database_size`, `free_space`, `version`, `nettype` | | `get_info` (§1b.3) | node unreachable, **or remote mode** — where there is no `get_info` at all and every one of them, plus `restricted`, `authenticated`, `sync_percent`, `top_block_age_sec`, `hard_fork_version`, `update_available`, `uptime_sec`, `net.*` and `fee.*`, is `null` (§2m.3) |
| `node.update_available` | bool \| null | `get_info.update_available`, and **always `false` under fmonero's own unit**, which sets `check-updates=disabled` (§4.2) because the check is a DNS lookup: `core::check_updates()` returns before `m_update_available` is ever set (`cryptonote_core.cpp:1653-1654`, `:1663`, `:1669`) and `get_info` reports `is_update_available()` verbatim (`core_rpc_server.cpp:568`). The field — and the `node_update_available` warning — are therefore meaningful **only** under the `system_unit` layout. Naming that here is §2j: a field that can never be true under the default layout is the quiet version of claiming something works. | node unreachable |
| `node.sync_percent` | f64 \| null | `100.0` when `synchronized`; else `height / max(target_height, height) * 100`, because `target_height` is **0 when synced** (§1b.3) | node unreachable |
| `node.top_block_age_sec` | int \| null | `now - get_last_block_header.block_header.timestamp` | node unreachable, **or the node is not synchronized** — `get_last_block_header` is the one call fmonero makes that `CHECK_CORE_READY()` gates, and it answers `status: "BUSY"` with an all-zero header (§1b.3), which would render a 56-year-old block for the whole initial sync |
| `node.hard_fork_version` | int \| null | `hard_fork_info{version:0}.version` = the node's **current** fork version. The sibling voting fields of that same response describe a *different* version (§1b.3) and are not surfaced in `status`; `node info` publishes them under `hard_fork.voting_on`. | node unreachable |
| `node.uptime_sec` | int \| null | `now - get_info.start_time` | node unreachable |
| `node.net.total_bytes_*` | int \| null | `POST /get_net_stats` | restricted node |
| `node.net.rx_bps`, `tx_bps` | int \| null | two samples of the above; the previous one lives in state | fewer than two samples, or restricted |
| `node.net.limit_*_kbps` | int \| null | `POST /get_limit` (open even when restricted, §1b.2) | node unreachable |
| `node.fee.*` | | `get_fee_estimate` | node unreachable |
| `wallet.unit_active` | bool | `systemctl --user is-active` | never |
| `wallet.url` | string | `settings.wallet.url` | never |
| `wallet.reachable` | bool | the hot-path `get_height` produced **any** JSON-RPC answer, `-13` included | never (false, not null) |
| `wallet.authenticated` | bool \| null | `WalletClient::authenticated()` (§5b) — whether the hot-path `get_height` completed a Digest handshake (§2c) | wallet unreachable |
| `wallet.account_index` | int | `settings.wallet.account`, overridden by `--account` | never |
| `wallet.open` | bool | `get_height` succeeded vs `-13 NOT_OPEN` (§1b.4) | never |
| `wallet.name` | string \| null | `state::last_open(ctx)?.0` (§5b — the record is `(name, NetType)`, the nettype being the §2a.6 send-path authority), written by `wallet_session::open` and cleared by `wallet_session::close` and by any hot-path `get_height` that answers `-13 NOT_OPEN` | no wallet open |
| `wallet.primary_address` | string \| null | `get_address{account_index}.address`. Public by construction; not a secret (§2b). | no wallet open |
| `wallet.last_refresh_at` | string \| null | **new in v5.2.** RFC 3339 for the last `refresh` **fmonero itself issued** — written by `wallet_session::refresh` (§5b), the single wrapper the CLI's `wallet refresh`, the §2m.3 send gate and the TUI's `R` all go through; read back from `state.json`. The qualifier is the whole point: `monero-wallet-rpc`'s own 20 s auto-refresh (§2d) is invisible to fmonero, so a field named "last refresh" without it would be a claim about the daemon's behaviour we cannot make (§2j). It matters most in remote mode, where `blocks_behind` is `null` and this is the only freshness signal left | fmonero has issued none since the state file was created |
| `wallet.height`, `blocks_behind` | int \| null | `height` is the wallet's own `get_height`; **`blocks_behind` is `node.height - wallet.height`, clamped at 0** — *positive means the wallet is behind*, which is the sign every consumer assumes (§5's `wallet_behind`, §2d's `WALLET_BEHIND_BLOCKS` classification, §7.1's `wallet_daemon_link` and `wallet_daemon_auth`). The two heights are the same convention — wallet `get_height` returns `get_blockchain_current_height()` (`wallet_rpc_server.cpp:938-944`) and `get_info.height` is top+1 (`core_rpc_server.cpp:521`) — so the subtraction is well defined in either direction; it is `Option<i64>` rather than `u64` only so that a wallet transiently ahead of a restarting node reads as a small negative rather than wrapping. Written the other way round the `wallet_behind` warning and §2d's syncing exemption could never fire. **In remote mode `blocks_behind` is always `null`**, because `node.height` is — which is why §2d exempts remote-mode deadline kills from the breaker and why `wallet.last_refresh_at` exists (§2m.3). | wallet closed, node unreachable, **or remote mode** |
| `wallet.balance_*`, `unlocked_*`, `blocks_to_unlock`, `time_to_unlock_sec` | | `get_balance{account_index}` (§1b.4) | no wallet open |
| `wallet.subaddress_count` | int \| null | **`get_address{account_index}.addresses.len()`** — every generated subaddress of the account (§1b.4 `:123-160`). **Not** `get_balance.per_subaddress.len()`, which `wallet2::balance_per_subaddress` builds only from subaddresses holding unspent, unfrozen outputs (`src/wallet/wallet2.cpp:7037-7060`): a wallet with 20 generated subaddresses and funds on one would report 1, and a freshly restored wallet would report 0. `get_address` is already a hot-path call, so this costs nothing. | no wallet open |
| `wallet.accounts` | int \| null | `get_accounts.subaddress_accounts.len()`, cached 60 s in state | no wallet open |
| `wallet.rpc_version` | string \| null | wallet `get_version`, **cold path** | no cold pass yet, or wallet unreachable |
| `units.*` | | `systemctl --user is-active` / `is-enabled` / file exists | never |
| `pending` | object \| null | `pending.json` (§2a.4), rendered by `PendingStatus::from_record` (§5b): `{kind, dest, account_index, built_at, elapsed_sec, relayed_count, total_count, total_amount_pico, total_amount_xmr, total_fee_pico, total_fee_xmr, txs: [{txid, amount_pico, amount_xmr, fee_pico, fee_xmr, relayed}]}`. **The four totals are summed in Rust with `Piconero::checked_add` and pre-rendered**, because a sweep's record holds several transactions, §6.5's pending row asks for a total, and the plugin may not do arithmetic on money (§2a.5, §6.5) — without them the QML's only route to that number is `parseFloat` over `txs[]`, which §6.7's grep then fails on. The on-disk record and this document are **different types**: the record stays minimal (§2a.4's "the tx metadata is NEVER part of it") while the document carries the derived counts and the rendered `*_xmr` strings the plugin and the TUI display. An array of objects, never parallel arrays — the plugin, the TUI and agents all index it, and two arrays that must stay in lockstep are two arrays that will not. | no unresolved record |
| `last_error` | object \| null | `{at, code, code_name, text, hint}`; any successful mutating command clears it | never errored since |
| `staleness.node_probe_ms`, `wallet_probe_ms` | ints | measured | never |
| `staleness.node_extras_age_sec` | int \| null | age of the 60 s cold cache | before the first cold pass |
| `staleness.history_cache_age_sec` | int \| null | `history::header().built_at` | no cache on disk |

**Restricted nodes poison part of `get_info`, and §2j forbids rendering the poison.** When
`get_info.restricted == true`, `monerod` returns `start_time: 0`, `free_space: 2^64-1`,
`version: ""`, `update_available: false`, `alt_blocks_count: 0`, all four connection/peer-list
counts as 0, and `database_size` rounded up to a 5 GiB multiple (§1b.3, with the `file:line`s).
`status` therefore sets `node.uptime_sec`, `node.free_space`, `node.version`,
`node.update_available`, `node.alt_blocks_count`, `node.outgoing_connections_count`,
`node.incoming_connections_count`, `node.white_peerlist_size` and `node.grey_peerlist_size` to
**`null`** whenever `restricted` is true, and `node.database_size` carries a sibling flag
`database_size_rounded: true`. `height`, `target_height`, `synchronized`, `busy_syncing`, `offline`
and `nettype` are real on a restricted node and stay populated. `tx_pool_size` is rendered but is a
**lower bound** under restriction — `get_pool_transactions_count(!restricted)` drops non-relayed and
local transactions (`core_rpc_server.cpp:527`, §1b.3) — and the `node_restricted` warning text says
so. (`adjusted_time` is also real; `status` does not surface it and `NodeStatus` has no such field —
the doctor's `node_clock` check reads it from its own probe.)
Without this rule, the Arch `system_unit` layout — the one the owner is most likely to meet first —
shows a 56-year uptime, 16 exabytes free and a permanent false "no peers" alarm.

**`warnings`** is an array of `{id, severity: "info"\|"warn"\|"error", text, hint}`. The ids are
stable, listed by `capabilities`, and exhaustive for `status`:

| id | severity | raised when |
|---|---|---|
| `monero_not_installed` | error | in **local** mode, `monerod` is absent; in **remote** mode, `monero-wallet-rpc` is absent — remote mode needs no `monerod`, and raising an error about a binary the configuration does not use is the alarm a user learns to ignore (§2m.3). The text names which binary it looked for |
| `remote_node` | info | **new in v5.2.** `node.mode == remote`. Text: the address, that the node learns this machine's IP and the timing of every request, and that its own sync state cannot be seen from here (§2m.6). `hint: fmonero node local --yes` |
| `remote_node_trusted` | warn | **new in v5.2.** `node.mode == remote`, `node.remote_trusted` is true, and the host is not a private literal (§7.1's `remote_node_untrusted` list). A trusted daemon receives the wallet's own unspent amounts inside a request (`wallet2.cpp:12281-12282`). `hint: fmonero node remote <addr> --yes   # re-set it without --trusted` |
| `node_down` | warn | `layout != "none"` and `node.reachable == false`. **`null` is not `false`**: in remote mode `node.reachable` is `null` and this warning — like `node_restricted`, `node_syncing`, `node_offline`, `node_no_peers`, `node_low_disk`, `node_update_available`, `auth_missing` and `node_stalled` — is **never raised**, because every one of them tests a field that has no source there (§2m.3) |
| `node_restricted` | warn | `node.restricted == true` — half the Node screen has no source (§1b.2); the text also says that `tx_pool_size` is a lower bound on such a node (§1b.3) |
| `node_syncing` | info | `synchronized == false`; text carries `sync_percent` |
| `node_offline` | warn | `get_info.offline == true` |
| `node_no_peers` | warn | `outgoing_connections_count == 0` and not `offline` and **`restricted != true`** (a restricted node reports 0 by construction) |
| `node_low_disk` | warn | `free_space != null` and under the §1.3 floor; the text says the floor is an estimate |
| `node_update_available` | info | `get_info.update_available` — reachable only under the `system_unit` layout, since our own conf disables the check (§4.2) |
| `foreign_node_running` | info | `layout == "system_unit"` — we are a guest on someone else's node (§2i) |
| `wallet_down` | warn | the unit is inactive and a command needs it |
| `wallet_closed` | info | reachable but no wallet open |
| `wallet_behind` | warn | `blocks_behind > 100` |
| `wallet_locked_funds` | info | `blocks_to_unlock > 0` |
| `send_may_be_in_flight` | error | **any** entry of `pending.txs` has `relayed == false` (§2a.4) |
| `node_stalled` / `wallet_stalled` | error | a §2d breaker is armed; text names the seconds left |
| `auth_missing` | warn | the node accepted us with no credential (§2c) |
| `nettype_mismatch` | warn | a wallet is open and its recorded nettype (§2a.6, from its own primary address) differs from `settings.node.network`; the send path follows the **wallet**, never the setting |
| `history_cache_stale` | info | the postcard cache's high-water height is more than 1 000 blocks behind |

`doctor` raises one further id, `plugin_not_linked` (the bar widget is not linked). It never appears
in `status`, which is why it is not in the table above.

## 5b. Core API freeze

Frozen at the Phase 1a gate. Phase 2 agents compile against exactly these signatures before Phase
1b has filled in a single body.

**How the freeze is amended, since v5.2 amends it.** A signature below changes only when a brief
revision says it does, only for a reason stated in the section that needs it, and **every change is
listed in §5b.1 and recorded in `DECISIONS.md` in the same commit that makes it**. The freeze is
not a promise that nothing ever changes — it is a promise that nothing changes *silently*, because
Phase 2's four agents compile against this text in four separate worktrees. Remote node mode
(§2m) is the first such revision; §5b.1 is its complete list, and an agent that finds a fifth
change it needs stops and reports it rather than making it. Later phases may **add** fields to `TuiArgs`/`DoctorArgs`/`SetupArgs`
— which is why those structs live in the Phase 2 modules and are only re-exported into `main.rs`'s
clap tree — but may not change any signature below or touch `main.rs`. Every function returns `Result<_, Error>` —
never `anyhow` — so `main.rs` maps a failure to its §4a code without downcasting.

**Two rules that make the freeze usable rather than merely binding:**

1. **A method enters this API only when something consumes it** — a §4.1 command, a §5 status field,
   a §7a screen or a §7.1 check. A frozen method with no caller is code Phase 1b must write and test
   and no later phase may delete (§2k). A Phase 2 agent that finds a gap stops and reports it to the
   orchestrator rather than an earlier agent pre-building one.
2. **A type named here is defined here.** "Compile against exactly these signatures" is not possible
   against a type that only has a name, and `SendRequest` is on the money path. The types that cross
   a phase boundary are spelled out below. **Every other type named in return position** —
   `GetInfo`, `BlockHeader`, `HardForkInfo`, `NodeVersion`, `FeeEstimate`, `TxpoolStats`, `SyncInfo`,
   `Connection`, `PeerLists`, `Ban`, `BanRequest`, `NetStats`, `Limits`, `AccountRow`, `NewAddress`,
   `AddressCheck`, `Refreshed`, and the `state.json` sub-structs `Stall { at: i64, backoff_until:
   i64 }`, `NetSample { at: i64, bytes_in: u64, bytes_out: u64 }` and `ColdCache` (the 60 s cold
   set of §5, plus its `built_at: i64`) — is a
   `#[derive(Clone, Debug, Serialize, Deserialize)]` struct whose fields are exactly those listed
   for it in §1b.3/§1b.4, snake_case, `u64` for counts and heights, `Piconero` for amounts,
   `Option<T>` only where the RPC marks the field optional — with the two exceptions §1b.3 names,
   `connection_info.ip`/`.port`/`.peer_id`/`.connection_id`, which are **strings** upstream while
   `peer.ip`/`.port` are numbers. `TransferQuery` is spelled out rather than defaulted, because one
   of its fields changes an unbounded query into a bounded one:
   `TransferQuery { account_index: u32, r#in: bool, out: bool, pending: bool, failed: bool,
   pool: bool, filter_by_height: bool, min_height: u64, subaddr_indices: Vec<u32> }` — `filter_by_height`
   is a field of the frozen type and not a serializer detail, because omitting it silently returns
   the whole history (§1b.4). Phase 1a writes them and they freeze at
   the 1a gate, whose closing check is `cargo doc -p fmonero-core` listing every type named here.
3. **Every enum that appears in JSON carries its wire spelling.** `Layout`, `Severity`,
   `PendingKind`, `TransferKind`, `Network`, **`NodeMode`**, **`SslSupport`** (v5.2), `NetType`,
   `AddrKind`, `units::FileStatus` and the
   doctor's `Verdict` status each carry `#[serde(rename_all = "snake_case")]` — lowercase for the
   single-word ones — so `Layout::UserUnit` serializes as `"user_unit"`, `Severity::Warn` as
   `"warn"` and `TransferKind::In` as `"in"`. A plain `#[derive(Serialize)]` would emit
   `"UserUnit"`, `"Warn"` and `"In"`, which is a JSON document no section of this design describes,
   and every one of these types is frozen at the 1a gate and compiled against by Phase 2 — so the
   wrong spelling would ship into the plugin's parser, the doctor's output and the e2e assertions
   before anyone noticed. "Compile against exactly these signatures" makes the derive attributes
   part of the contract. **A round-trip unit test in Phase 1a asserts every variant against the
   literal string §5/§5c/§7 promises** — except the doctor's `Verdict` status, whose type lives in
   `crates/fmonero/src/doctor/checks.rs` and therefore belongs to the Phase 2 doctor gate (§8).

```rust
// ---- fmonero-core -------------------------------------------------------------
#[derive(Clone, Debug, Serialize)]
pub struct Context { pub settings: Settings, pub paths: Paths, pub timeouts: Timeouts,
                     pub json: bool, pub verbose: u8, pub force: bool,
                     pub systemctl: Systemctl }
/// §2i test seam. `Fixture` is honoured ONLY when FMONERO_STATE_DIR is also set.
#[derive(Clone, Debug, Serialize)]
pub enum Systemctl { Real, Fixture(PathBuf) }   // FMONERO_SYSTEMCTL_FIXTURE
impl Context {
    pub fn load() -> Result<Context, Error>;
    #[must_use] pub fn with_flags(self, json: bool, verbose: u8, timeout: Option<u64>,
                                  force: bool) -> Context;
    pub fn for_test(dir: &std::path::Path) -> Result<Context, Error>;
}

#[derive(Clone, Debug, Serialize)]
pub struct Paths { pub state_dir: PathBuf, pub config_dir: PathBuf, pub cache_dir: PathBuf,
                   pub wallet_dir: PathBuf, pub run_dir: PathBuf, pub bitmonero_dir: PathBuf,
                   pub systemd_user_dir: PathBuf, pub omarchy_config_dir: PathBuf,
                   pub local_bin: PathBuf, pub home: PathBuf,
                   /// "/proc", or FMONERO_PROC_ROOT when FMONERO_STATE_DIR is also set (§2i).
                   pub proc_root: PathBuf }
// `systemd_user_dir` is `~/.config/systemd/user`, or FMONERO_SYSTEMD_USER_DIR when
// FMONERO_STATE_DIR is also set (§2i, §4.2 — v5.1). Same field, same guard as `proc_root`.
// `Context::for_test(dir)` additionally puts `systemd_user_dir`, `local_bin` and
// `settings.wallet.dir` under `dir`: they pointed at the real machine until v5.1, and the one
// real `scripts/install.sh --yes` then made five unit tests fail on their own premise.
impl Paths {
    /// FMONERO_STATE_DIR / FMONERO_CONFIG_DIR / FMONERO_CACHE_DIR override the XDG locations.
    pub fn discover() -> Result<Paths, Error>;
    /// Creates every directory 0700 and is the only place they are created (§2b.7).
    pub fn ensure_dirs(&self) -> Result<(), Error>;
    pub fn config_file(&self) -> PathBuf;        // config.toml
    pub fn state_file(&self) -> PathBuf;         // state.json
    /// state_dir/pending.json — the §2a.4 money record, in its OWN file so that `status`'s
    /// whole-document rewrite (3 s from the plugin, 1 s from the TUI) can never lose it.
    pub fn pending_file(&self) -> PathBuf;
    pub fn state_lock(&self) -> PathBuf;         // state_dir/.lock, 0600 (§5)
    pub fn monerod_conf(&self) -> PathBuf;       // config_dir/monerod.conf, 0600 (§2c)
    pub fn history_cache(&self) -> PathBuf;      // cache_dir/history.bin
    pub fn wallet_login_file(&self, port: u16) -> PathBuf;  // run_dir/monero-wallet-rpc.<port>.login
    pub fn backups_dir(&self) -> PathBuf;
    pub fn doctor_runs_dir(&self) -> PathBuf;
    pub fn doctor_latest(&self) -> PathBuf;
    pub fn omarchy_menu(&self) -> PathBuf;
    pub fn plugin_link(&self) -> PathBuf;
    /// `~/.config/omarchy/shell.json` — Omarchy's own record of which plugins are enabled and
    /// what the bar carries (§1.5). Read by `setup --plugin`'s pre-check (§4.2, v5.1).
    pub fn omarchy_shell_json(&self) -> PathBuf;
}

// ---- the types that cross a phase boundary (rule 2 above) -----------------------
#[derive(Clone, Debug, Serialize, Deserialize)]
/// The resolved deadlines of §2d. The first six come from `TimeoutSettings` (the §4 keys) and
/// are clamped by `--timeout`; `relay` (30 s) and `unit_probe` (30 s) are fixed constants with
/// no settings key, because neither is a knob a user has a reason to turn.
pub struct Timeouts { pub node: Duration, pub wallet: Duration, pub refresh: Duration,
                      pub transfer: Duration, pub open: Duration, pub systemctl: Duration,
                      pub unit_probe: Duration, pub relay: Duration }

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network { Mainnet, Stagenet, Testnet }

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// v5.2. The wire spellings are epee's three and no others (`net_ssl.cpp:654-665`); they are also
/// the three values of `--daemon-ssl` (`wallet2.cpp:253`), so one type serves the unit render and
/// the `set_daemon` body. `Enabled` is never constructed by fmonero in v0.1: it throws at process
/// start without a CA file, a fingerprint list or `--daemon-ssl-allow-any-cert`
/// (`wallet2.cpp:416-432`), all three of which §0 cuts. It is a variant so that adding pinned TLS
/// in v0.2 is a settings key rather than a new type.
pub enum SslSupport { Disabled, Enabled, Autodetect }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings { pub node: NodeSettings, pub wallet: WalletSettings,
                      pub timeouts: TimeoutSettings, pub tui: TuiSettings }
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// v5.2, §2m.1. The ONLY input to `units::detect_layout`'s `Remote` branch.
pub enum NodeMode { Local, Remote }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeSettings { pub network: Network, pub mode: NodeMode, pub url: String,
                          /// §2m.2's grammar; empty iff `mode == Local`. NEVER handed to
                          /// `NodeClient`/`WalletClient` — it is rendered into the wallet unit's
                          /// `--daemon-address` and sent as `set_daemon`'s `address` (§2c).
                          pub remote: String, pub remote_trusted: bool,
                          pub unit: String,
                          pub data_dir: PathBuf, pub log_file: PathBuf, pub prune: bool }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletSettings { pub url: String, pub unit: String, pub dir: PathBuf,
                            pub port: u16, pub account: u32, pub priority: u32 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimeoutSettings { pub node_sec: u64, pub wallet_sec: u64, pub refresh_sec: u64,
                             pub transfer_sec: u64, pub open_sec: u64, pub systemctl_sec: u64 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TuiSettings { pub poll_ms: u64, pub tick_ms: u64 }

impl Settings {
    /// (key, default, one-line doc) — the §4 table, and the thing `settings list` prints.
    pub const KEYS: &'static [(&'static str, &'static str, &'static str)];
    pub fn get(&self, key: &str) -> Result<String, Error>;          // NotFound on unknown key
    /// On an unknown key the error names the nearest key from `KEYS` by case-insensitive
    /// Levenshtein distance ≤ 3 and its hint is that exact runnable command —
    /// `fmonero settings set wallet.priority 2   # did you mean this?` — falling back to
    /// `fmonero settings list` when nothing is near. Twenty-three dotted keys is exactly the namespace
    /// an agent half-remembers, and `KEYS` already holds the data to suggest from (§4).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), Error>;   // Usage on a bad value
    pub fn save(&self, ctx: &Context) -> Result<(), Error>;         // 0600, tmp + rename
    /// (key, why) for every key that is present but invalid — the doctor's settings_valid (§7.1).
    pub fn invalid_keys(&self) -> Vec<(String, String)>;
    /// **v5.2.** §2m.2's grammar, in one place: returns the normalized address (scheme
    /// lower-cased, kept; host and port unchanged) or `Error::Usage{reason:"bad_remote_address"}`
    /// with the row's exact message and the one hint §2m.2 prescribes. Called by
    /// `Settings::set("node.remote", …)`, by `node_mode::set_remote` and by `wallet set-daemon`,
    /// so the three cannot accept different strings.
    pub fn parse_remote_address(s: &str) -> Result<String, Error>;
}

/// The §5 document, field-for-field. Every `Option` is a documented `null` of that section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status { pub ok: bool, pub schema: u32, pub generated_at: String,
                    pub node: NodeStatus, pub wallet: WalletStatus, pub units: UnitsStatus,
                    pub pending: Option<PendingStatus>, pub warnings: Vec<Warning>,
                    pub last_error: Option<LastError>, pub staleness: Staleness }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeStatus {
    pub layout: units::Layout,
    /// The address in force: `settings.node.url` locally, `settings.node.remote` remotely (§5).
    pub url: String,
    /// v5.2: `true` locally, `settings.node.remote_trusted` remotely (§5, §1b.9 item 4).
    pub trusted: bool,
    /// v5.2: `Option`, because remote mode has no source for it and `false` would be a claim (§5).
    pub reachable: Option<bool>,
    pub restricted: Option<bool>, pub authenticated: Option<bool>, pub nettype: Option<String>,
    pub height: Option<u64>, pub target_height: Option<u64>, pub sync_percent: Option<f64>,
    pub synchronized: Option<bool>, pub busy_syncing: Option<bool>, pub offline: Option<bool>,
    pub top_block_age_sec: Option<i64>,
    pub outgoing_connections_count: Option<u64>, pub incoming_connections_count: Option<u64>,
    pub white_peerlist_size: Option<u64>, pub grey_peerlist_size: Option<u64>,
    pub tx_pool_size: Option<u64>, pub alt_blocks_count: Option<u64>,
    pub database_size: Option<u64>, pub database_size_rounded: bool, pub free_space: Option<u64>,
    pub hard_fork_version: Option<u8>, pub version: Option<String>,
    pub update_available: Option<bool>, pub uptime_sec: Option<i64>,
    pub net: NetStatus, pub fee: FeeStatus }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetStatus { pub total_bytes_in: Option<u64>, pub total_bytes_out: Option<u64>,
                       pub rx_bps: Option<u64>, pub tx_bps: Option<u64>,
                       pub limit_up_kbps: Option<i64>, pub limit_down_kbps: Option<i64> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeeStatus { pub per_byte_pico: Option<Piconero>, pub fees_pico: Option<Vec<Piconero>>,
                       pub quantization_mask: Option<u64> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletStatus {
    pub unit_active: bool, pub reachable: bool, pub authenticated: Option<bool>, pub url: String,
    pub open: bool, pub name: Option<String>, pub account_index: u32,
    pub primary_address: Option<String>, pub height: Option<u64>, pub blocks_behind: Option<i64>,
    pub balance_pico: Option<Piconero>, pub balance_xmr: Option<String>,
    pub unlocked_pico: Option<Piconero>, pub unlocked_xmr: Option<String>,
    pub blocks_to_unlock: Option<u64>, pub time_to_unlock_sec: Option<u64>,
    pub subaddress_count: Option<u32>, pub accounts: Option<u32>,
    pub rpc_version: Option<String>,
    /// v5.2: RFC 3339 for the last refresh **fmonero** issued (§5). Not the daemon's own 20 s
    /// auto-refresh, which fmonero cannot observe.
    pub last_refresh_at: Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnitsStatus { pub node: units::UnitState, pub wallet: units::UnitState }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Warning { pub id: String, pub severity: Severity, pub text: String, pub hint: String }
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity { Info, Warn, Error }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LastError { pub at: String, pub code: i32, pub code_name: String,
                       pub text: String, pub hint: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Staleness { pub node_probe_ms: u64, pub wallet_probe_ms: u64,
                       pub node_extras_age_sec: Option<u64>,
                       pub history_cache_age_sec: Option<u64> }

/// §2a.4. `txs` is one entry per built transaction, in relay order. This is the ON-DISK record,
/// serialized to `paths.pending_file()` (pending.json, 0600) and nowhere else; the tx metadata is
/// NEVER part of it. It is never serialized into `Status` — `PendingStatus` is (see below).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingSend { pub kind: PendingKind, pub account_index: u32, pub dest: String,
                         pub built_at: i64, pub txs: Vec<PendingTx> }
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingKind { Send, Sweep }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingTx { pub txid: String, pub amount: Piconero, pub fee: Piconero,
                       pub relayed: bool, pub relayed_at: Option<i64> }

/// The §5 `pending` document, field for field — the thing the plugin's `Model.pendingLabel`, the
/// TUI's banner and `wallet clear-pending`'s payload all parse. Derived from `PendingSend` by
/// `status()`; keeping the two apart is what lets the on-disk record stay minimal (§2a.4) while the
/// document carries derived counts and pre-rendered `*_xmr` strings (§5's shape rules).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingStatus { pub kind: PendingKind, pub dest: String, pub account_index: u32,
                           pub built_at: i64, pub elapsed_sec: i64,
                           pub relayed_count: usize, pub total_count: usize,
                           /// Summed in Rust with `Piconero::checked_add` and pre-rendered here,
                           /// because a sweep's record holds several transactions and the plugin
                           /// may not do arithmetic on money (§2a.5) — §6.5's pending row asks for
                           /// a total, and without these fields the QML would have to
                           /// `parseFloat` its way to one.
                           pub total_amount_pico: Piconero, pub total_amount_xmr: String,
                           pub total_fee_pico: Piconero, pub total_fee_xmr: String,
                           pub txs: Vec<PendingTxStatus> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingTxStatus { pub txid: String, pub amount_pico: Piconero, pub amount_xmr: String,
                             pub fee_pico: Piconero, pub fee_xmr: String, pub relayed: bool }
impl PendingStatus { pub fn from_record(p: &PendingSend, now: i64) -> PendingStatus; }

/// What the CLI and the TUI hand to `send::build`. No `unlock_time`, no `payment_id`,
/// no `ring_size`, no `outputs` — by §2a.8 those fields do not exist here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendRequest { pub dest: String, pub amount: Piconero, pub account_index: u32,
                         pub priority: u32, pub note: Option<String>,
                         /// §2a.2: refuse the relay when the BUILT total fee exceeds this. The one
                         /// fee bound that survives the build-discard-rebuild flow, and it is
                         /// enforced in `send::relay`, never by a caller.
                         pub max_fee: Option<Piconero> }
/// No subaddress selector, deliberately: the wire request always carries
/// `subaddr_indices_all: true` (§2a.10) and there is no field that could disable it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SweepRequest { pub dest: String, pub account_index: u32, pub priority: u32,
                          pub below: Option<Piconero>, pub max_fee: Option<Piconero> }
/// The wire shape of `transfer` / `sweep_all`, built only inside send.rs from the two above.
#[derive(Clone, Debug, Serialize)] pub struct TransferRequest { /* private fields */ }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Balance { pub balance: Piconero, pub unlocked: Piconero,
                     pub blocks_to_unlock: u64, pub time_to_unlock_sec: u64,
                     pub per_subaddress: Vec<SubaddressBalance> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubaddressBalance { pub address_index: u32, pub address: String, pub label: String,
                               pub balance: Piconero, pub unlocked: Piconero,
                               pub num_unspent_outputs: u64, pub blocks_to_unlock: u64 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddressSet { pub primary: String, pub addresses: Vec<SubaddressRow> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubaddressRow { pub address: String, pub label: String,
                           pub address_index: u32, pub used: bool }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookEntry { pub index: u64, pub address: String, pub description: String }

#[derive(Clone, Debug, Default)]
pub struct HistoryQuery { pub account_index: u32, pub limit: usize,
                          pub kinds: Vec<TransferKind>, pub since_height: Option<u64>,
                          /// `--cached`: serve the snapshot alone, issue no RPC (§2l (d)).
                          pub cached: bool,
                          /// The WALLET's current height (wallet `get_height`), which is the clock
                          /// wallet-rpc itself recomputes `confirmations` and `locked` against
                          /// (§1b.4, `wallet_rpc_server.cpp:162`, `:505`) — NOT the node's. A
                          /// wallet behind the node is the normal state for hours after
                          /// `wallet open` (§2d), and against the node's height fmonero would
                          /// report a payment as more settled than the wallet can see.
                          /// **Written by `history::query` from its own `get_height`, never by a
                          /// caller** — it is ignored on input, which is what makes handing it the
                          /// node's height unrepresentable. Stays `None` with `--cached` (no RPC at
                          /// all, §4.1), in which case `confirmations` and `locked` render `null`.
                          /// Callers read it back after `query` to render those two columns.
                          pub wallet_height: Option<u64>,
                          /// The clock the timestamp branch of `locked` uses. **Local time**,
                          /// which is upstream's own fallback when no daemon answers
                          /// (`get_daemon_adjusted_time()` with `catch(...) { time(NULL) }`,
                          /// `wallet2.cpp:7331-7333`) — `wallet history` contacts no node (§4.1),
                          /// so there is no `adjusted_time` to be had and inventing an RPC for it
                          /// would break that row's `N: no`. The doctor's `node_clock` check
                          /// (§7.1) is what makes local time trustworthy here, and the column is
                          /// labelled `locked (est.)` for exactly this reason.
                          /// Written by `history::query` alongside `wallet_height` and ignored on
                          /// input; `None` with `--cached`, in which case `locked` is `null` like
                          /// `confirmations`.
                          pub adjusted_now: Option<i64> }

/// One history row. `confirmations` and `locked` are NOT fields: wallet-rpc recomputes them per
/// response from the wallet's current height (§1b.4), so they are derived at render time from
/// `height`, `unlock_time` and `q.wallet_height`. Storing them would show stale numbers about money.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transfer { pub txid: String, pub kind: TransferKind, pub height: u64,
                      pub timestamp: i64, pub amount: Piconero, pub fee: Piconero,
                      pub note: String, pub address: String,
                      /// `transfer_entry.subaddr_index` is the JSON OBJECT `{major, minor}`
                      /// (§1b.4, `subaddress_index.h:38-55`), never a scalar. `major` IS the
                      /// account index, so there is no separate `account_index` field: a u32 here
                      /// would fail to deserialize every history row.
                      pub subaddr_index: SubaddrIndex,
                      /// `transfer_entry.destinations[]` (§1b.4 `:1456`); empty for `in` rows.
                      /// The §7a detail pane renders it — for an outgoing transfer it is the one
                      /// fact a user checks after a send.
                      pub destinations: Vec<Destination>,
                      pub unlock_time: u64,
                      pub payment_id: String, pub double_spend_seen: bool }
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SubaddrIndex { pub major: u32, pub minor: u32 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Destination { pub address: String, pub amount: Piconero }
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// `Block` is a COINBASE incoming row. `fill_transfer_entry` types it
/// `pd.m_coinbase ? "block" : "in"` (`wallet_rpc_server.cpp:501`) and returns it inside the `in`
/// list, so a five-variant enum fails to deserialize the WHOLE `get_transfers` response for any
/// wallet holding a mined or p2pool output — and with it `wallet history` and the §2a.4 resolve
/// step. It needs no special unlock rule: upstream's `is_transfer_unlocked` uses the same
/// 10-block age for every row, and a coinbase's 60-block window arrives inside `unlock_time`
/// as an absolute height, which the height branch of `locked` already handles (§1b.4).
pub enum TransferKind { In, Out, Pending, Failed, Pool, Block }

pub enum Error {   // one variant per §4a code; ALL carry
                   // { msg: String, hint: Option<String>, reason: Option<&'static str> }
    Failure, Usage, Safety, Backend, Upstream, NotFound, Ambiguous, Timeout,
    WalletLocked, NodeSyncing, Doctor, DoctorPartial, DoctorRefused,
    InsufficientFunds, InvalidAddress, TxRejected, Interrupted,
}
impl Error {
    // one constructor per variant, lowercase: msg only; reason/hint default to None
    pub fn failure(msg: impl Into<String>) -> Error;    // … one per variant, same shape …
    pub fn unimplemented(what: &str) -> Error;          // §2h, exits 1
    pub fn exit_code(&self) -> i32;
    pub fn code_name(&self) -> &'static str;
    pub fn hint(&self) -> String;                       // always a runnable command
    /// The §4 machine-readable discriminator WITHIN a code. `None` for codes that have none.
    /// `capabilities.reasons` is generated from `REASONS` below, so the documented list and the
    /// constructible set cannot drift.
    pub fn reason(&self) -> Option<&'static str>;
    #[must_use] pub fn with_hint(self, hint: impl Into<String>) -> Error;
    #[must_use] pub fn with_reason(self, reason: &'static str) -> Error;
    pub fn is_interrupted(&self) -> bool;
    /// Maps a wallet-RPC error code (§1b.4) to the right variant, message verbatim.
    pub fn from_wallet_rpc(code: i32, message: &str) -> Error;
    /// Maps a node JSON-RPC error; -32601 becomes Upstream with the restricted-RPC hint.
    pub fn from_node_rpc(code: i32, message: &str) -> Error;
}
/// (exit code, reason) — the §4 table, printed by `capabilities`.
pub const REASONS: &[(i32, &str)];
pub const EXIT_CODES: &[(i32, &str, &str)];             // printed by capabilities + robot-docs
// `reason` is a field of the type, not of `main.rs`, because §4 makes it a MANDATORY key of every
// failure envelope and §5c repeats it for every command. An `Error` that cannot carry it leaves
// `main.rs` — which owns the exit mapping, and freezes at the 1a gate (§2k) — with two options:
// change a frozen signature, or drop the key. Dropping it silently turns exit 3 back into eight
// indistinguishable conditions on the money path. The constructors are frozen for the same reason:
// "compile against exactly these signatures" is false if the doctor and TUI agents have to guess
// between `Error::Usage { msg, hint }` and `Error::usage("…")`.

#[derive(Clone, Debug, Default)] pub struct CancelToken(Arc<AtomicBool>);
impl CancelToken { pub fn new() -> Self; pub fn cancel(&self); pub fn is_cancelled(&self) -> bool; }

pub mod proc {
    pub struct Output { pub code: Option<i32>, pub signal: Option<i32>,
                        pub stdout: Vec<u8>, pub stderr: Vec<u8>, pub timed_out: bool }
    impl Output { pub fn success(&self) -> bool; pub fn stdout_string(&self) -> String;
                  pub fn stderr_string(&self) -> String; pub fn stderr_tail(&self) -> String; }
    pub fn which(program: &str) -> Option<PathBuf>;
    pub fn run_with_deadline(argv: &[&str], timeout: Duration, cancel: Option<&CancelToken>)
        -> Result<Output, Error>;
}

// ---- amounts (§2a.5) ----------------------------------------------------------
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Piconero(pub u64);
impl Piconero {
    pub const PER_XMR: u64 = 1_000_000_000_000;         // cryptonote_config.h:67
    /// "0.5" | "1.234567891234" | "250000000000p". Rejects >12 decimals, separators,
    /// exponents, a bare ".", a leading "+", and anything that overflows u64.
    pub fn parse(s: &str) -> Result<Piconero, Error>;   // Error::Usage
    /// 12 decimals, trailing zeros trimmed to a minimum of 4. **Exact, never rounded or
    /// truncated**: `Piconero::parse(x.to_xmr_string()) == x` for every `u64`. Examples:
    /// `1_234_567_891_234` → `"1.234567891234"`; `500_000_000_000` → `"0.5000"` (trimmed to the
    /// four-decimal minimum); `0` → `"0.0000"`. The round trip is a Phase 1a gate assertion,
    /// because §5's worked example is the concrete artefact an implementer copies and a truncating
    /// formatter would make the `*_xmr` string disagree with the `*_pico` integer beside it —
    /// which §5 pairs precisely so the two can be cross-checked.
    pub fn to_xmr_string(self) -> String;
    pub fn checked_add(self, other: Piconero) -> Result<Piconero, Error>;
}
impl std::fmt::Display for Piconero {}                  // = to_xmr_string

// ---- addresses (§1b.8, §2a.6) -------------------------------------------------
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")] pub enum NetType { Mainnet, Testnet, Stagenet }
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")] pub enum AddrKind { Standard, Integrated, Subaddress }
pub struct AddrForm { pub net: NetType, pub kind: AddrKind, pub prefix: u64,
                      /// The base58 digits the first 8-byte block can start with. Usually one;
                      /// testnet standard (prefix 53) is '9' or 'A' (§1b.8).
                      pub leading: &'static [char], pub len: usize }
/// The ONLY place a form is described. Adding Carrot is one row here plus one object in
/// tests/fixtures/address-forms.json; a unit test asserts the two have equal length (§1b.1).
pub const PREFIXES: &[AddrForm];
#[derive(Clone, Debug, Serialize)]
pub struct Address { pub text: String, pub net: NetType, pub kind: AddrKind }
/// Structural only. **Length and base58 decodability decide**; the decoded prefix varint names
/// the (net, kind). `leading` is a display and fast-reject hint and is used to reject only when
/// the set has exactly one member — the leading character is the base58 digit of the whole first
/// 8-byte block, not of the prefix (§1b.8).
/// NOT authoritative — every send also calls WalletClient::validate_address (§2a.6).
pub fn parse(s: &str) -> Result<Address, Error>;        // Error::InvalidAddress

// ---- RPC ----------------------------------------------------------------------
pub mod rpc {
    /// The §2c loopback rule, in one place: `Ok(())` only for scheme `http` with a LITERAL host in
    /// `127.0.0.0/8` or `::1`, or a `localhost` whose every resolved address is loopback.
    /// `NodeClient::new` and `WalletClient::new` each call it as their first statement, and
    /// `settings set node.url` / `settings set wallet.url` validate through it —
    /// `Error::Usage{reason:"non_loopback_url"}`. One function rather than a rule restated per
    /// client: the wallet client is the one that carries a credential and moves money.
    pub fn require_loopback(url: &str) -> Result<(), Error>;

    #[derive(Clone)] pub struct Login { pub user: String, pub pass: String }
    impl Login {
        /// Reads `user:pass` from monero-wallet-rpc's own 0600 login file (§2c).
        /// Ok(None) when the file is absent, which means "the daemon is not running".
        pub fn from_wallet_login_file(path: &Path) -> Result<Option<Login>, Error>;
        /// Reads `rpc-login=` from a monerod config file. Ok(None) when absent.
        pub fn from_monerod_conf(path: &Path) -> Result<Option<Login>, Error>;
    }
    pub mod digest {
        /// No `opaque`: epee's server never sends one (§1b.7). `nc` belongs to `nonce` and is
        /// reset to 1 whenever `nonce` changes. Lives behind the client's Mutex (§2c).
        pub struct Session { /* realm, nonce, qop, nc */ }
        /// Takes EVERY `WWW-Authenticate` header of the 401, in order, and selects the one whose
        /// algorithm is `MD5` or absent; `MD5-sess` is never selected (§1b.7).
        pub fn challenge(www_authenticate: &[&str]) -> Result<Session, Error>;
        pub fn authorization(s: &mut Session, login: &Login, method: &str, uri: &str) -> String;
    }
}

pub struct NodeClient { /* agent, base, auth, timeout */ }
impl NodeClient {
    /// Rejects any non-http scheme and any non-loopback host with Error::Usage, through
    /// `rpc::require_loopback` — the same function `WalletClient::new` calls (§2c).
    pub fn new(url: &str, auth: Option<Login>, timeout: Duration) -> Result<NodeClient, Error>;
    /// **The remote-mode gate, in one place** (v5.2, §2m.3): returns
    /// `Error::Backend{reason:"remote_node"}` with `hint: fmonero node local --yes` when
    /// `ctx.settings.node.mode == NodeMode::Remote`, before constructing anything. `node info`,
    /// `node peers`, `node bans`, `node ban`, `node unban` and `node limit` reach the daemon only
    /// through this constructor, so none of them needs a check of its own; `node up|down|restart`
    /// meet the same error in `node_session` (§2i). A per-command check would be the arrangement
    /// §2a.3 rejects, on nine commands across three phases.
    pub fn from_ctx(ctx: &Context) -> Result<NodeClient, Error>;
    pub fn get_info(&self) -> Result<GetInfo, Error>;
    pub fn get_last_block_header(&self) -> Result<BlockHeader, Error>;
    pub fn hard_fork_info(&self, version: u8) -> Result<HardForkInfo, Error>;
    pub fn get_version(&self) -> Result<NodeVersion, Error>;
    pub fn get_fee_estimate(&self, grace_blocks: u64) -> Result<FeeEstimate, Error>;
    pub fn get_txpool_stats(&self) -> Result<TxpoolStats, Error>;
    pub fn sync_info(&self) -> Result<SyncInfo, Error>;          // Upstream(-32601) if restricted
    pub fn get_connections(&self) -> Result<Vec<Connection>, Error>;
    pub fn get_peer_list(&self, public_only: bool) -> Result<PeerLists, Error>;
    pub fn get_bans(&self) -> Result<Vec<Ban>, Error>;
    pub fn get_net_stats(&self) -> Result<NetStats, Error>;
    pub fn get_limit(&self) -> Result<Limits, Error>;
    /// Read-only: always sends `set: false` (§1b.3). The writing form lives on NodeMutators.
    pub fn peers_count(&self) -> Result<(u32, u32), Error>;      // out_peers + in_peers
    /// One call, classifying the answer: Ok(false) = unrestricted, Ok(true) = -32601 (§1b.2).
    pub fn is_restricted(&self) -> Result<bool, Error>;
    /// Whether the last COMPLETED request went through a Digest handshake (`Some(true)`), was
    /// accepted with no challenge at all (`Some(false)`), or has not been attempted (`None`).
    /// Feeds §5's `node.authenticated`, the `auth_missing` warning and the doctor's `node_auth`
    /// (§7.1). **It reports what the wire did, never what a config file says**: inferring it from
    /// `Login::from_monerod_conf` returning `Some` would report `authenticated: true` against a
    /// node that never challenged us, which is the false claim §2j forbids. Without this method
    /// those three consumers have no producer and a Phase 1b agent must either add an unfrozen
    /// public method or reach into the client's private `auth` field from `crates/fmonero/src/doctor`.
    pub fn authenticated(&self) -> Option<bool>;
    /// **The §2i layout gate, as a type rather than as a comment.** Returns `Error::Safety` with
    /// the `sudo systemctl …` hint unless `units::detect_layout(ctx) == Layout::UserUnit`.
    /// There is no `--force` override. The five mutators exist ONLY on `NodeMutators`, so a caller
    /// that did not pass this gate has no way to name them and gets a compile error rather than a
    /// working `node down` against Arch's system daemon (§2i).
    pub fn mutators<'a>(&'a self, ctx: &Context) -> Result<NodeMutators<'a>, Error>;
}

pub struct NodeMutators<'a> { /* &'a NodeClient */ }
impl<'a> NodeMutators<'a> {
    pub fn stop_daemon(&self) -> Result<(), Error>;
    pub fn set_log_level(&self, level: i8) -> Result<(), Error>;
    pub fn set_bans(&self, bans: &[BanRequest]) -> Result<(), Error>;
    pub fn set_limit(&self, down_kbps: i64, up_kbps: i64) -> Result<Limits, Error>;
    pub fn set_peers_count(&self, out: Option<u32>, inc: Option<u32>) -> Result<(u32, u32), Error>;
}

pub struct WalletClient { /* agent, base, auth, timeout */ }
impl WalletClient {
    /// Rejects any non-http scheme and any non-loopback host with Error::Usage, through the same
    /// `rpc::require_loopback` `NodeClient::new` calls (§2c).
    pub fn new(url: &str, auth: Option<Login>, timeout: Duration) -> Result<WalletClient, Error>;
    /// Reads `user:pass` from the login file (§2c). **An absent file is `Ok` with `auth: None`,
    /// never an error**: steps 1–4 of §2a.1's refusal order must still run in order, so no
    /// constructor may decide a money-path exit code. The absence surfaces at the first request as
    /// exit 4 — `reason: unreachable` for a refused connect, `auth_refused` for a 401.
    pub fn from_ctx(ctx: &Context) -> Result<WalletClient, Error>;
    /// As `NodeClient::authenticated`: what the wire did on the last completed request. Feeds §5's
    /// `wallet.authenticated`, §5c's `wallet set-daemon.authenticated` and the doctor (§7.1).
    pub fn authenticated(&self) -> Option<bool>;
    pub fn get_version(&self) -> Result<(u16, u16, bool), Error>;    // (major, minor, release)
    pub fn get_height(&self) -> Result<u64, Error>;                  // WalletLocked if -13
    /// Maps ANY `-1` to `Error::WalletLocked` (exit 9): `on_open_wallet` overwrites the mapped
    /// code, so a wrong password never arrives as -22 (§1b.4). The caller pre-checks that the
    /// `.keys` file exists (exit 6), which is what makes that mapping unambiguous.
    /// Called only from `wallet_session::open`.
    pub fn open_wallet(&self, name: &str, password: &str) -> Result<(), Error>;
    pub fn close_wallet(&self) -> Result<(), Error>;
    pub fn create_wallet(&self, name: &str, password: &str, language: &str) -> Result<(), Error>;
    pub fn get_balance(&self, account: u32) -> Result<Balance, Error>;
    pub fn get_accounts(&self) -> Result<Vec<AccountRow>, Error>;
    pub fn get_address(&self, account: u32) -> Result<AddressSet, Error>;
    pub fn create_address(&self, account: u32, label: &str) -> Result<NewAddress, Error>;
    pub fn label_address(&self, account: u32, index: u32, label: &str) -> Result<(), Error>;
    pub fn get_transfers(&self, q: &TransferQuery) -> Result<Vec<Transfer>, Error>;
    /// `Ok(None)` when wallet-rpc answers `-8` with "Transaction not found." — the normal
    /// "not in this wallet yet" answer, and the whole basis of the §2a.4 resolve step.
    /// `Err(Usage)` for the malformed-txid variants of `-8` (§1b.4).
    pub fn get_transfer_by_txid(&self, txid: &str, account: u32)
        -> Result<Option<Transfer>, Error>;
    /// ALWAYS called with do_not_relay=true and get_tx_metadata=true (§2a.2); the type
    /// makes the other combination unrepresentable. **Callable only from `send::build`** (§2a.3).
    pub fn build_transfer(&self, req: &TransferRequest, cancel: &CancelToken)
        -> Result<BuiltTx, Error>;
    pub fn build_sweep_all(&self, req: &TransferRequest, cancel: &CancelToken)
        -> Result<Vec<BuiltTx>, Error>;
    /// -> the server's `tx_hash`, which the caller compares with the built txid (§2a.9).
    /// **Callable only from `send::relay`** (§2a.3).
    pub fn relay_tx(&self, metadata_hex: &str) -> Result<String, Error>;
    /// Persists the wallet file. Its ONLY caller is `send::relay`, after the last successful
    /// `relay_tx` (§2a.4): wallet-rpc writes the wallet only on a graceful `stop()`
    /// (`wallet_rpc_server.cpp:294-302`) or on this call (`:2060-2077`) — the auto-refresh idle
    /// handler never stores (`:210-285`) — so without it a SIGKILL after a relay loses the
    /// wallet's record of the transfer and the resolve step then blocks the next send.
    pub fn store(&self) -> Result<(), Error>;
    /// Always `{ any_net_type: true, allow_openalias: false }` (§2a.6), so the answer names the
    /// address's real network and the call does not need an open wallet.
    pub fn validate_address(&self, address: &str) -> Result<AddressCheck, Error>;
    pub fn refresh(&self, start_height: u64, cancel: &CancelToken) -> Result<Refreshed, Error>;
    pub fn auto_refresh(&self, enable: bool, period_sec: u32) -> Result<(), Error>;
    pub fn get_address_book(&self) -> Result<Vec<BookEntry>, Error>;
    pub fn add_address_book(&self, address: &str, description: &str) -> Result<u64, Error>;
    pub fn delete_address_book(&self, index: u64) -> Result<(), Error>;
    pub fn set_tx_notes(&self, txids: &[String], notes: &[String]) -> Result<(), Error>;
    pub fn get_tx_notes(&self, txids: &[String]) -> Result<Vec<String>, Error>;
    /// `login` carries the node's `rpc-login` credential (§4.2) and is `None` in remote mode,
    /// always (§2m.3). Without it a wallet pointed at fmonero's own authenticated node gets a 401
    /// on every daemon call and can never sync.
    /// **Requires an already-open wallet**: `on_set_daemon` answers `-13 NOT_OPEN` with no wallet
    /// loaded (`wallet_rpc_server.cpp:4692`), so call it immediately AFTER `open_wallet`, never
    /// before. Called only from `wallet_session::open` and from `wallet set-daemon`.
    /// `ssl` is a typed enum rather than a string because epee accepts exactly three spellings and
    /// answers `-38 "Invalid ssl support mode"` for a fourth (`net_ssl.cpp:654-665`,
    /// `wallet_rpc_server.cpp:4732-4737`) — a typo would be a runtime failure on the send path's
    /// own precondition. It is `Disabled` locally and `Autodetect` remotely; **an omitted key
    /// would mean `Autodetect`, not plaintext** (`KV_SERIALIZE_OPT`, §1b.4), so it is always sent.
    /// Never sends `proxy`, `ssl_ca_file`, `ssl_allowed_fingerprints` or `ssl_allow_any_cert` (§0).
    pub fn set_daemon(&self, address: &str, trusted: bool, login: Option<&Login>,
                      ssl: SslSupport) -> Result<(), Error>;
}

/// The ONE implementation of the `wallet open` / `wallet close` sequences (§4.1, §4.2). Called by
/// the CLI subcommand AND by the TUI's open modal; neither reimplements the order. Without this
/// module the four-step open sequence would exist twice, written by two agents in two phases —
/// the arrangement §2a.3 rejects for the send path, applied here to the step whose omission means
/// the wallet can never reach the node.
pub mod wallet_session {
    /// `units::ensure_running`, then `open_wallet`, then exactly one `set_daemon` whose four
    /// arguments come from **`units::daemon_target(ctx)`** (v5.2, §2m.3) and therefore differ by
    /// mode: in **local** mode the credential is `Login::from_monerod_conf` (both fields omitted
    /// when there is none — the §2i `system_unit` layout) and `ssl` is `Disabled`; in **remote**
    /// mode `login` is `None` **always** (§2m.3) and `ssl` is derived from the address, not from
    /// the mode. Then `auto_refresh { enable: true, period: 20 }` (§2d).
    /// **A `-38` from `set_daemon` is handled by mode, and the two answers are opposite** (§4.2,
    /// §2m.3): in **local** mode it is a warning, not a failure — the wallet is open, `node.url`
    /// is loopback-validated so the case is nearly unreachable, and `status` reports the truth
    /// either way; in **remote** mode it is **not** swallowed — the address passed §2m.2's grammar
    /// minutes earlier, so a `-38` means fmonero's grammar and epee's parser disagree, and
    /// reporting a successful `wallet open` for a wallet whose daemon was rejected is the silence
    /// §2j forbids. It exits **4 `backend`**, `reason: remote_address_rejected`, message verbatim,
    /// `hint: fmonero node remote <addr> --yes`. **`open` can therefore fail for a reason it could
    /// not before v5.2**, which is why §5b.1 lists it (#10).
    /// Records `state::set_last_open(name, net)`, where `net` is
    /// `address::parse(get_address{account}.address)?.net` — the open wallet's OWN nettype, which
    /// is the §2a.6 send-path authority and cannot drift from the daemon the way
    /// `settings.node.network` can. `password` is moved in and zeroed before return.
    pub fn open(ctx: &Context, w: &WalletClient, name: &str, password: String,
                cancel: &CancelToken) -> Result<Opened, Error>;
    pub struct Opened { pub unit_started: bool, pub name: String, pub net: NetType }
    /// `close_wallet { autosave_current: true }` then `state::clear_last_open`.
    pub fn close(ctx: &Context, w: &WalletClient) -> Result<(), Error>;
    /// **v5.2.** `WalletClient::refresh` followed by `state::note_refresh` — the one place the
    /// "last refresh fmonero issued" fact is written (§5's `wallet.last_refresh_at`). Three
    /// independently written callers — the CLI's `wallet refresh`, `send`'s §2m.3 remote gate and
    /// the TUI's `R` — and §2a.3 already settles what happens to a step that lives in three.
    pub fn refresh(ctx: &Context, w: &WalletClient, start_height: u64, cancel: &CancelToken)
        -> Result<Refreshed, Error>;
}

/// The two convenience commands of §2m.5, in core because they write settings, render units and
/// call `systemctl` — three core responsibilities — and because `main.rs` is frozen at the 1a gate.
/// The TUI does not call them: it toasts the command (§7a).
pub mod node_mode {
    /// What a mode switch did. IS §5c's `node remote` / `node local` payload, verbatim.
    #[derive(Clone, Debug, Serialize)]
    pub struct ModeChange { pub mode: NodeMode, pub address: String, pub trusted: bool,
                            pub ssl_support: SslSupport,
                            pub actions: Vec<units::FileAction>,
                            pub wallet_unit_restarted: bool,
                            /// The wallet that was open and must be reopened, or `None` (§2m.5).
                            pub wallet_must_reopen: Option<String>,
                            pub changed: bool, pub dry_run: bool }
    /// §2m.5's nine steps. `addr` is validated by `Settings::parse_remote_address` first, so an
    /// unparseable one costs nothing. **Then, unless `dry_run`, the two money guards §2a.1 puts
    /// ahead of every build**: the §2a.11 `send.lock` (`Error::Safety{reason:"send_in_progress"}`)
    /// and the §2a.4 `pending_send` entry check (`Error::Safety{reason:"pending_send"}`) — this is
    /// the command that changes which node a relayed-but-unresolved send would be resolved
    /// against, and §2a.3 does not allow an invariant to live in some callers only. The lock is
    /// held for the rest of the call. Writes the three keys in ONE `Settings::save`.
    pub fn set_remote(ctx: &Context, addr: &str, trusted: bool, dry_run: bool,
                      cancel: &CancelToken) -> Result<ModeChange, Error>;
    pub fn set_local(ctx: &Context, dry_run: bool, cancel: &CancelToken)
        -> Result<ModeChange, Error>;
}

/// The ONE implementation of `node up` / `node down` / `node restart` (§4.1, §2i). Called by the
/// CLI subcommands AND by the TUI's `u`/`d` actions; neither reimplements the order. `units`
/// exposes only the ingredients (`detect_layout`, `data_dir_in_use`, `main_pid`, `start`, `stop`),
/// and without this module the composition — above all the §2i single-writer precondition, the one
/// guard against two `monerod` on one LMDB (§1.2) — would exist twice, written by two agents in
/// two phases, and the TUI's copy is the one nobody reviews. It is also the precondition an agent
/// is likeliest to skip, because §2i attributes it to a *command* rather than to `units::start`.
pub mod node_session {
    pub struct Started { pub already_running: bool, pub waited_sec: u64,
                         pub reachable: bool, pub synchronized: Option<bool> }
    /// `units::detect_layout` (UserUnit, else `Error::Safety` with §2i's hint) →
    /// `units::data_dir_in_use`: no holder ⇒ start; holder == `units::main_pid(unit)` ⇒
    /// `Ok(already_running: true)` with no second start; any other pid ⇒
    /// `Error::Safety{reason:"data_dir_in_use"}` naming it → `units::start` → poll `get_info` to
    /// `timeouts.unit_probe` (`Error::Timeout`, exit 8) → when `wait` is `Some`, poll
    /// `get_info.synchronized` to that duration (`Error::Timeout`, exit 8).
    pub fn up(ctx: &Context, wait: Option<Duration>, cancel: &CancelToken)
        -> Result<Started, Error>;
    /// `NodeClient::mutators(ctx)?.stop_daemon()` if reachable — so LMDB closes cleanly — then
    /// `units::stop`. Layout `UserUnit` only; the mutators type IS the gate (§2i).
    pub fn down(ctx: &Context, cancel: &CancelToken) -> Result<(), Error>;
    pub fn restart(ctx: &Context, cancel: &CancelToken) -> Result<Started, Error>;
}

// ---- the send orchestrator (§2a.3) --------------------------------------------
/// The ONLY two functions in the codebase that may call `WalletClient::build_transfer`,
/// `build_sweep_all` or `relay_tx`, and `relay` is the only writer of the `pending.json` record.
/// The CLI (`wallet send`, `wallet sweep`) and the TUI (§7a) are two independently written
/// callers; every §2a guarantee lives here so that one unit test proves it for both.
pub mod send {
    /// Runs §2a.1's refusal order, which is the order the exit code depends on: amount rules
    /// (§2a.5) → `address::parse` (skipped for a `@`-prefixed `dest`) → the §2a.11 `send.lock` →
    /// the §2a.4 entry check and resolve step → the wallet-daemon
    /// probe → **`@label` resolution through one `get_address_book`, which replaces `dest` with the
    /// resolved address and then parses it (step 5a: exit 6 on no match, 7 on several; the TUI's
    /// `@` picker yields a literal address and never reaches this path)** →
    /// `validate_address` with the nettype check (§2a.6) → `Error::NodeSyncing` (exit 10)
    /// when `get_info.synchronized` is false → build. Then builds
    /// **once** with `do_not_relay: true, get_tx_metadata: true`, and raises
    /// `Error::Safety{reason:"fee_above_max"}` when `req.max_fee` is `Some` and the built total fee
    /// exceeds it (§2a.2), before returning. Never relays.
    /// The ONLY state it writes is the §2a.4 resolve step's clear: when every entry of an existing
    /// record answers found it calls `state::clear_pending_send` and proceeds; it never creates,
    /// extends or marks an entry. (Leaving the clear unassigned would put it in the CLI and again
    /// in the TUI — two independently written callers of one invariant, §2a.3.)
    /// **v5.2: the node argument is a three-variant enum, not an `Option`** (§2m.3). `None`
    /// already meant "the caller proved there is no node ⇒ refuse", and remote mode means the
    /// opposite — proceed, by a different gate. Two opposite behaviours behind one `None` is the
    /// arrangement §2a.3 rejects, on the one path where getting it wrong either refuses every
    /// remote send or accepts every local send against a node that is down.
    pub fn build(ctx: &Context, w: &WalletClient, gate: NodeGate<'_>,
                 req: &SendRequest, cancel: &CancelToken) -> Result<Plan, Error>;
    /// Step 7 of §2a.1's refusal order, as data. `Remote` carries no client because fmonero
    /// constructs none in remote mode (§2c).
    pub enum NodeGate<'a> {
        /// Local mode, node answering: one `get_info`; not `synchronized` ⇒ exit 10
        /// `reason: node_not_synchronized`.
        Local(&'a NodeClient),
        /// Local mode, node proved unreachable by the caller ⇒ exit 4 `reason: unreachable`.
        Unreachable,
        /// Remote mode: one bounded `refresh` through `wallet_session::refresh` under
        /// `timeouts.refresh_sec`, which must succeed. `-38`/transport ⇒ exit 4
        /// `reason: unreachable`; killed at the deadline ⇒ exit 10 `reason: wallet_catching_up`,
        /// **no §2d breaker armed** (§2m.3, §2d).
        Remote,
    }
    /// The one implementation of step 7, switching on the gate. Called by `build` and
    /// `build_sweep` and by nothing else.
    fn require_node_current(ctx: &Context, w: &WalletClient, gate: NodeGate<'_>,
                            cancel: &CancelToken) -> Result<(), Error>;
    /// Sweep's counterpart; the resulting `Plan` may hold several transactions. Its wire request
    /// always carries `subaddr_indices_all: true` (§2a.10). One caller only: `wallet sweep` —
    /// the TUI has no sweep path in v0.1 (§0, §7a).
    pub fn build_sweep(ctx: &Context, w: &WalletClient, gate: NodeGate<'_>,
                       req: &SweepRequest, cancel: &CancelToken) -> Result<Plan, Error>;
    /// Re-checks `Error::Safety{reason:"fee_above_max"}` when `plan.max_fee` is `Some` and
    /// `plan.total_fee` exceeds it (§2a.2 — `build` raised it first; this is the enforcing gate,
    /// so a caller that builds a plan by other means still cannot relay past it),
    /// formats the built recipient/amount/fee/total/priority/account as one line and hands it to
    /// `announce` **before** the first `relay_tx` — **core stays silent (§2g)**: `main.rs` passes
    /// `&|s| eprintln!("{s}")` and the TUI passes a closure that appends to its log pane. A
    /// `println!` here would sit in `crates/fmonero-core/src` and fail the Phase 3 grep, and §2a.2
    /// deliberately puts the announcement in `relay` rather than in each caller.
    /// Then writes and fsyncs the whole `pending.json` record BEFORE the first
    /// `relay_tx`, relays every transaction of the plan in order, compares each returned `tx_hash`
    /// with the built txid (§2a.9), marks each entry relayed and re-fsyncs, issues one wallet
    /// `store` after the last relay (§2a.4), runs the resolve step, and finally — when `plan.note`
    /// is `Some` — one `set_tx_notes { txids, notes }` over every relayed txid.
    /// **A failure of that `set_tx_notes` is a warning, never a failure of the send**: the money has
    /// moved, and an exit code that says otherwise sends the user to re-run the command. It goes
    /// through the same `announce` sink and into `last_error`, and the command still exits 0.
    /// Returns the txids in relay order. The `Plan` is consumed, so its `send.lock` is released here.
    pub fn relay(ctx: &Context, w: &WalletClient, plan: Plan, announce: &dyn Fn(&str),
                 cancel: &CancelToken) -> Result<Vec<String>, Error>;
    /// A built, unbroadcast plan. Holds `BuiltTx`, so it is never serialized (see below).
    pub struct Plan { pub txs: Vec<BuiltTx>, pub total_amount: Piconero,
                      pub total_fee: Piconero, pub total_weight: u64,
                      pub dest: String, pub account_index: u32,
                      pub priority: u32, pub is_sweep: bool,
                      /// Carried from SendRequest so that `relay` — not each caller — applies it:
                      /// the CLI and the TUI both offer a note field, and a field that lives in the
                      /// request type but is applied by whichever caller remembers is the §2a.3
                      /// arrangement with its worst failure mode (a successful broadcast reported
                      /// as an error) left undefined.
                      pub note: Option<String>,
                      pub max_fee: Option<Piconero>,
                      /// The §2a.11 `send.lock` file descriptor. Private, never serialized; the
                      /// lock lives exactly as long as the plan does, so a discarded plan, a
                      /// `--dry-run` and a crash all release it with nothing to clean up.
                      _lock: std::fs::File }
    impl Plan { pub fn total(&self) -> Result<Piconero, Error>; }   // amount + fee, checked
}

/// A transaction that exists but has NOT been broadcast. `metadata` is the hex `relay_tx`
/// wants (§1b.4). It is never serialized, never logged, never written to disk (§2a.4).
pub struct BuiltTx { pub txid: String, pub amount: Piconero, pub fee: Piconero,
                     pub weight: u64, pub metadata: String }
impl std::fmt::Debug for BuiltTx {}   // hand-written: prints metadata as "<redacted>"

// ---- units (§2i, §4.2) --------------------------------------------------------
pub mod units {
    #[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Layout { UserUnit, SystemUnit, Remote, None }
    #[derive(Clone, Debug, Serialize)]
    pub struct UnitState { pub name: String, pub exists: bool, pub active: bool, pub enabled: bool }
    pub fn detect_layout(ctx: &Context) -> Result<Layout, Error>;
    pub fn state(ctx: &Context, unit: &str) -> Result<UnitState, Error>;   // is-active/is-enabled
    /// All three render for `ctx.settings.node.network` (§4) and emit ABSOLUTE paths in the
    /// conf file — systemd specifiers are expanded only in the two .service files (§4.2).
    pub fn render_node_unit(ctx: &Context) -> String;
    pub fn render_node_conf(ctx: &Context, login: &Login) -> String;
    pub fn render_wallet_unit(ctx: &Context) -> String;
    /// Carries an existing valid `rpc-login=fmonero:<32 hex>` through unchanged; draws 16 fresh
    /// bytes from /dev/urandom only when the line is absent, unreadable or malformed. This is
    /// what makes `setup --units` idempotent and what stops a rewrite from locking out a running
    /// node (§4.2). `rotate` forces a new one.
    pub fn existing_or_new_node_login(ctx: &Context, rotate: bool) -> Result<Login, Error>;
    #[derive(Clone, Debug, Serialize)]
    pub struct FileAction { pub what: String, pub path: PathBuf, pub status: FileStatus,
                            /// Present only for `dry_run`: the unified diff of the file's current
                            /// bytes against the rendered bytes (§4.2). `None` when Unchanged.
                            pub diff: Option<String> }
    #[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
    #[serde(rename_all = "lowercase")]
    pub enum FileStatus { Written, Unchanged, Skipped, Planned }
    /// Renders the three files, compares byte-for-byte against what is on disk, and (unless
    /// `dry_run`) writes the changed ones (conf at 0600), daemon-reloads and DISABLES both units.
    /// Returns one `FileAction` per file, which IS §5c's `setup.actions` verbatim. A
    /// `Vec<PathBuf>` could express neither the written/unchanged distinction that makes `setup`
    /// idempotent-checkable nor §4.2's promised `--dry-run` diff, which would force the Phase 1b
    /// agent either to change a frozen signature (forbidden) or to invent a second rendering path
    /// outside this module.
    pub fn install(ctx: &Context, dry_run: bool) -> Result<Vec<FileAction>, Error>;
    pub fn start(ctx: &Context, unit: &str, cancel: &CancelToken) -> Result<(), Error>;
    pub fn stop(ctx: &Context, unit: &str, cancel: &CancelToken) -> Result<(), Error>;
    /// **Probes first, and starts the unit only when the probe fails**, then polls `probe` until
    /// it succeeds or the deadline. That order is what lets every AMBER wallet row of §4b
    /// (`wallet open|close|balance|…`) run against `fmonero-mock` with no unit installed at all;
    /// "start, then probe" would spawn `systemctl start` against a unit that does not exist on
    /// every one of them. fbittorrent's `backend::ensure_running` shape
    /// (fbittorrent/src/backend.rs:17-29).
    pub fn ensure_running(ctx: &Context, unit: &str, probe: &dyn Fn() -> bool,
                          deadline: Duration, cancel: &CancelToken) -> Result<bool, Error>;
    /// The two questions of §2i that come **before any `systemctl` call**, both answered in
    /// milliseconds: the unit file exists in `paths.systemd_user_dir` (else
    /// `Error::Backend{reason:"unit_missing"}`, hint `fmonero setup --units --yes`) and `binary`
    /// is on PATH (else `Error::Backend{reason:"not_installed"}`, hint `omarchy pkg add monero`).
    /// The second is skipped under `Systemctl::Fixture`, which stands in for the machine's daemon
    /// as well as for its systemd (§2i). Called by `node_session::{up, restart}` and — after the
    /// probe, immediately before `start` — by `ensure_running`, so `wallet open` and
    /// `wallet create` get it too. (v5.1)
    pub fn preflight(ctx: &Context, unit: &str, binary: &str) -> Result<(), Error>;
    /// Which daemon a unit runs, so `ensure_running` can ask `preflight`'s second question
    /// without a second parameter — its signature is frozen (§5b) and its two callers would
    /// otherwise each name the binary, the arrangement §2a.3 rejects. (v5.1)
    pub fn unit_binary(ctx: &Context, unit: &str) -> &'static str;
    /// `systemctl --user stop` then `reset-failed` for a unit **we** started that never answered,
    /// returning the clause the caller appends to its error. Called by `ensure_running` and by
    /// `node_session::up` on their deadline, and by nothing else: a unit fmonero did not start is
    /// not fmonero's to stop (§7's axiom, and §2i). (v5.1)
    pub fn stop_after_failed_start(ctx: &Context, unit: &str, cancel: &CancelToken) -> String;
    /// `RestartSec=` in both rendered units, quoted in `preflight`'s message (§4.2). (v5.1)
    pub const RESTART_SEC: u32;
    /// Two probes (§2i): `<paths.proc_root>/*/cmdline` for a monerod already serving `data_dir`,
    /// and `fcntl(F_GETLK)` on `<data_dir>/lmdb/lock.mdb`. NEVER `flock`, which does not conflict
    /// with LMDB's fcntl lock and would report a live data dir as free.
    /// **The query must use `l_type: F_WRLCK`.** A running monerod holds a SHARED (`F_RDLCK`) lock
    /// on that byte — `mdb_env_share_locks` downgrades the setup-time exclusive lock
    /// (`external/db_drivers/liblmdb/mdb.c:5022-5057`) — and only a write-lock query conflicts
    /// with a held read lock. An `F_GETLK{F_RDLCK}` probe reports a live data dir as free.
    /// **Used ONLY by `node up` (as a start-time precondition) and by the doctor's
    /// `node_data_dir_single_writer`. NEVER by `detect_layout`** (§2i): it cannot tell our own
    /// monerod from a foreign one, so folding it into layout detection disables every node mutator
    /// the moment our own node starts.
    pub fn data_dir_in_use(ctx: &Context, data_dir: &Path) -> Result<Option<i32>, Error>;
    /// `systemctl --user show -p MainPID <unit>`, so `node up` can tell "our node is already
    /// running" (exit 0) from "someone else's monerod holds this data dir" (exit 3, §2i). Under
    /// `Systemctl::Fixture` it reads the `<main-pid>` column of that file instead (§2i).
    pub fn main_pid(ctx: &Context, unit: &str) -> Result<Option<i32>, Error>;
    /// **v5.2, and the reason the unit and `set_daemon` cannot disagree.** The three daemon
    /// facts, derived from `settings.node.{mode,url,remote,remote_trusted}` in one place and read
    /// by `render_wallet_unit`, `wallet_session::open`, `status`, `node info`, `node_mode::*` and
    /// the doctor's `wallet_daemon_matches_unit` — six callers of one derivation (§2m.3).
    #[derive(Clone, Debug, Serialize)]
    pub struct DaemonTarget { pub address: String, pub trusted: bool, pub ssl: SslSupport }
    /// `address` = `host_port(settings.node.url)` locally, `settings.node.remote` verbatim
    /// remotely; `trusted` = `true` locally, `settings.node.remote_trusted` remotely; `ssl` is
    /// derived from the ADDRESS — `Disabled` for a loopback literal host, `Autodetect` otherwise
    /// — so the local case follows from §2c rather than being a second rule, and
    /// `wallet set-daemon`, whose address is neither key, uses the same derivation on its argument
    /// (`ssl_for_address`, below).
    pub fn daemon_target(ctx: &Context) -> DaemonTarget;
    /// The address half of the rule above, exposed because `wallet set-daemon` needs it for an
    /// address that is in no setting (§4.1). Two callers, one derivation (§2a.3).
    pub fn ssl_for_address(address: &str) -> SslSupport;
    /// True only for Layout::UserUnit. Read by `NodeClient::mutators` (§2i) and by the doctor;
    /// it is not the thing callers check by hand — the mutators live on a type they cannot
    /// construct without it. There is no --force override.
    pub fn may_mutate_node(ctx: &Context) -> Result<bool, Error>;
}

// ---- wallet directory, logs, settings accessors ------------------------------
pub mod wallet_dir {
    /// `*.keys` basenames in settings.wallet.dir, sorted. No wallet bytes are read (§2b.6).
    /// Consumed by `wallet list` AND by the TUI's open-wallet modal (§7a), which is why it is
    /// a core function and not a subcommand body.
    pub fn list(ctx: &Context) -> Result<Vec<String>, Error>;
}
pub mod log {
    pub struct Tail { pub lines: Vec<String>, pub truncated: bool, pub path: PathBuf }
    /// Bounded read of the last `lines` lines (never more than 4 MiB from the end), with an
    /// optional substring filter. A missing file is `Ok(Tail { lines: vec![], .. })`, never an
    /// error — `node log` with no log yet is a GREEN row (§4b). Consumed by the CLI and the
    /// TUI's Log screen.
    pub fn tail(path: &Path, lines: usize, grep: Option<&str>) -> Result<Tail, Error>;
}

// ---- state, history, status ---------------------------------------------------
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)] pub enum Daemon { Node, Wallet }
/// The `state.json` document. Public and named here because §5b rule 2 requires it (it appears in
/// `with_state`'s closure) and because the doctor's `state_valid` fixer must be able to render it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StateDoc { pub last_open: Option<(String, NetType)>, pub last_error: Option<LastError>,
                      pub stalls: BTreeMap<String, Stall>, pub net_sample: Option<NetSample>,
                      pub cold: Option<ColdCache>,
                      /// v5.2: unix seconds of the last `refresh` FMONERO issued (§5's
                      /// `wallet.last_refresh_at`). Written only by `state::note_refresh`.
                      pub last_refresh: Option<i64> }
impl StateDoc { pub fn to_bytes(&self) -> Result<Vec<u8>, Error>; }
impl Settings { pub fn to_toml_bytes(&self) -> Result<Vec<u8>, Error>; }
pub mod fsinfo {
    /// Free bytes on the filesystem holding `path` (statvfs). The doctor's `disk_space` (§7.1)
    /// measures locally through this; it never reads `get_info.free_space`, which a restricted
    /// node reports as `2^64-1` (§1b.3).
    pub fn free_bytes(path: &Path) -> Result<u64, Error>;
}
pub mod state {
    /// Every writer here goes through tmp-at-0600 + rename(2) (§2b.7, §5), and every
    /// read-modify-write of state.json goes through `with_state` (§5's locking rule).
    /// Loads state.json under `flock(LOCK_EX)` on `paths.state_lock()` with a 2 s deadline,
    /// applies `f`, writes tmp-at-0600, fsyncs, renames, releases. A lock that cannot be taken
    /// within the deadline SKIPS the write rather than blocking `status` (§5).
    pub fn with_state<T>(ctx: &Context, f: impl FnOnce(&mut StateDoc) -> T) -> Result<T, Error>;

    // ---- pending.json: its own file, three writers, `status` reads it only (§2a.4) ----
    pub fn pending_send(ctx: &Context) -> Result<Option<PendingSend>, Error>;
    /// Writes the WHOLE record to `paths.pending_file()` and fsyncs. Called exactly once per send,
    /// by `send::relay`, before the first relay_tx (§2a.3, §2a.4).
    pub fn set_pending_send(ctx: &Context, p: &PendingSend) -> Result<(), Error>;   // fsync
    /// Flips one entry of the list to `relayed: true` and re-fsyncs before the next relay_tx.
    pub fn mark_relayed(ctx: &Context, txid: &str) -> Result<(), Error>;
    pub fn clear_pending_send(ctx: &Context) -> Result<(), Error>;

    // ---- state.json ----
    /// Records the wallet name `status` reports as `wallet.name` (§5) **and the wallet's own
    /// nettype**, which is the §2a.6 send-path authority. Written by `wallet_session::open` and by
    /// nothing else.
    pub fn set_last_open(ctx: &Context, name: &str, net: NetType) -> Result<(), Error>;
    /// Cleared by `wallet_session::close` and whenever a hot-path `get_height` answers
    /// `-13 NOT_OPEN` — so a wallet closed behind fmonero's back does not leave a stale name on
    /// the bar widget and in the TUI status line.
    pub fn clear_last_open(ctx: &Context) -> Result<(), Error>;
    pub fn last_open(ctx: &Context) -> Result<Option<(String, NetType)>, Error>;
    /// Read-only load of the whole document. Exists because the doctor's `state_valid` fixer (§7.1)
    /// must render its replacement bytes and push them through `mutate(WriteFile{..})`;
    /// `with_state` writes the file itself and would bypass the backup, the sha256 pair and
    /// `actions.jsonl` — and therefore `doctor undo`.
    pub fn load(ctx: &Context) -> Result<StateDoc, Error>;
    pub fn set_last_error(ctx: &Context, e: &Error, text: Option<String>) -> Result<(), Error>;
    /// v5.2. Records "now" as the last refresh fmonero issued. Its ONLY caller is
    /// `wallet_session::refresh`; `status` reads it and never writes it (§2e).
    pub fn note_refresh(ctx: &Context) -> Result<(), Error>;
    pub fn note_stall(ctx: &Context, who: Daemon) -> Result<(), Error>;
    pub fn stall_remaining(ctx: &Context, who: Daemon) -> Result<Option<u64>, Error>;
    pub fn net_sample(ctx: &Context, s: NetStats) -> Result<Option<(u64, u64)>, Error>; // -> bps
}
pub mod history {
    /// Postcard snapshot at cache_dir/history.bin (mode 0600, §2b.7), header
    /// { magic b"FMHX", format_version u32, wallet_fp [u8;16], high_water u64, built_at i64 }.
    /// wallet_fp = first 16 bytes of sha256(primary_address) — the address is public (§2b).
    /// A header mismatch is STALE, not corrupt: ignored and refetched, never an error.
    /// format_version is bumped on ANY change to a serialized struct (postcard is not
    /// self-describing). Writes go to history.bin.tmp.<pid> then rename(2).
    ///
    /// **What the cache may hold** (§1b.4 is the evidence for each clause):
    /// - Confirmed `in`/`out` rows only, and only those with
    ///   `height <= wallet_height - CRYPTONOTE_DEFAULT_TX_SPENDABLE_AGE (10)`. `high_water` is
    ///   that height, never the tip.
    /// - NEVER `confirmations`, `locked` or `suggested_confirmations_threshold`: wallet-rpc
    ///   recomputes them per response from **the wallet's** current height, so a cached copy is a
    ///   wrong answer about money one block later. `query` recomputes them the same way upstream
    ///   does, against `q.wallet_height` (`wh`) and never against the node's height (§1b.4):
    ///
    ///     confirmations = if height == 0 || height >= wh { 0 } else { wh - height }
    ///
    ///     age_ok       = height != 0 && height + 10 <= wh          // SPENDABLE_AGE, :7313
    ///     spendtime_ok = if unlock_time == 0                  { true }
    ///                    else if unlock_time < 500_000_000    { wh >= unlock_time }   // a HEIGHT
    ///                    else                          { adjusted_now + 120 >= unlock_time }
    ///                    // adjusted_now is LOCAL time — upstream's own fallback when no daemon
    ///                    // answers (wallet2.cpp:7331-7333), and `wallet history` contacts no
    ///                    // node. 120 is the V2 leeway; the V1 60 s applies only below the V2
    ///                    // height (mainnet 1 009 827), which no row we show reaches (§1b.4).
    ///     locked       = !(age_ok && spendtime_ok)
    ///
    ///   Two independent conditions, not a maximum, and `unlock_time` is an absolute height OR a
    ///   unix timestamp — never a delta added to `height` (`wallet2.cpp:7308-7344`,
    ///   `cryptonote_config.h:40`, `:49`, `:89-90`). `pending` and `pool` rows are unconditionally
    ///   locked (`wallet_rpc_server.cpp:551`, `:581`).
    ///   With `q.wallet_height == None` (`--cached`, zero RPC) **both fields render as `null`**,
    ///   never as 0 (§5's shape rules, §4.1). The authority on spendable funds is always
    ///   `get_balance.unlocked_balance`; the history column is labelled `locked (est.)`.
    /// - NEVER `pending`, `failed` or `pool` rows. `get_transfers` applies its height filter only
    ///   to `in` and `out`; those three lists come back in full every time. `pending` and `failed`
    ///   are local (`get_unconfirmed_payments_out`) and are fetched fresh on every top-up and
    ///   replace, never merge; a cached row whose txid reappears in a fresh list is replaced.
    ///   **`pool` is sent only when the query asks for pool rows**, because that branch alone runs
    ///   `update_pool_state`/`process_pool_state` — a daemon round trip (`:2939-2950`) that throws
    ///   when the node is unreachable and turns the WHOLE response into epee's `code: 0` (§1b.4).
    ///   Sending it unconditionally would make `wallet history` — a read-only command documented
    ///   `N: no` — fail whenever the node is down.
    /// - `refresh` fetches from `high_water.saturating_sub(720)` (≈ one day) rather than from
    ///   `high_water`, so a reorg inside that window is corrected instead of being cached
    ///   forever; the server's rows are authoritative for the re-read window. It **must send
    ///   `filter_by_height: true` together with `min_height`**: `on_get_transfers` reads the bounds
    ///   only inside `if (req.filter_by_height)` (`wallet_rpc_server.cpp:2890-2895`), so without
    ///   the flag every top-up silently refetches the whole history — the difference between the
    ///   §2l (d) budget and a full download, and a response that looks correct either way.
    /// - Confirmed `block` (coinbase) rows are cached on the same terms as `in` rows; they arrive
    ///   inside the same `in` list (§1b.4).
    pub fn load(ctx: &Context, account: u32) -> Result<Vec<Transfer>, Error>;
    /// The incremental top-up. **Its only caller is `query`** (§5b rule 1): the top-up, the
    /// `get_height` and the row merge are one operation and splitting them across callers is how
    /// the node's height ends up in `confirmations`.
    pub fn refresh(ctx: &Context, w: &WalletClient, account: u32) -> Result<usize, Error>;
    /// `w` is `Some` unless `q.cached`. With `Some(w)` this is the ONE place the top-up happens:
    /// one wallet `get_height`, one `get_transfers` with `filter_by_height: true` (and `pool` only
    /// when `q.kinds` asks for it), confirmed `in`/`out`/`block` rows merged into the snapshot, and
    /// the `pending`/`failed`/`pool` rows returned straight from that response and never cached.
    /// It **writes** `q.wallet_height` from its own `get_height` and `q.adjusted_now` from the local
    /// clock, so no caller can hand it the NODE's height; with `w: None` it serves the snapshot
    /// alone, issues nothing, leaves both `None`, and both derived fields render `null` (§2l (d)).
    /// A `get_transfers` that failed because of the `pool` branch (epee's `code: 0` with the node
    /// down, §1b.4) is retried **once** without `pool`; the rows are returned and the caller renders
    /// `pool_unavailable: true` (§5c). `&mut` only so those two derived fields can be filled — the
    /// query itself is not mutated.
    pub fn query(ctx: &Context, w: Option<&WalletClient>, q: &mut HistoryQuery)
        -> Result<Vec<Transfer>, Error>;
    /// The ONLY implementation of §1b.4's two derived fields. `wh` is the WALLET's height (never
    /// the node's); `None` when there is none (`--cached`), in which case both render as JSON
    /// `null` and as `—` in a table — never 0 (§5's shape rules). `output.rs` (Phase 1b) and the
    /// TUI's Wallet screen and detail pane (Phase 2, a different agent in a different worktree) are
    /// two independently written callers, and the `unlock_time` height/timestamp split
    /// (`wallet2.cpp:7319-7344`) is the formula this document spends a paragraph warning about
    /// getting wrong. An invariant that lives in two callers holds in one (§2a.3).
    pub fn confirmations(t: &Transfer, wh: Option<u64>) -> Option<u64>;
    pub fn locked(t: &Transfer, wh: Option<u64>, adjusted_now: Option<i64>) -> Option<bool>;
    pub struct CacheHeader { pub magic_ok: bool, pub format_version: u32,
                             pub wallet_fp: [u8; 16], pub high_water: u64, pub built_at: i64 }
    /// `Ok(None)` when the file is absent; never an error. Exists because the doctor's
    /// `history_cache_fresh` check (§7.1) must read the header and may not touch core.
    pub fn header(ctx: &Context) -> Result<Option<CacheHeader>, Error>;
}
/// The §5 document. Five hot calls plus a 60 s-cached cold set, starts nothing (§2e), returns
/// Ok even when both daemons are down.
pub fn status(ctx: &Context) -> Result<Status, Error>;

pub mod agent {
    /// v5.2: `ROBOT_GUIDE` gains one section, `remote node`, carrying §2m.1's three keys, the
    /// §2m.2 grammar, the two commands, the nine node commands that exit 4 `remote_node`, and the
    /// §2m.6 caveats in the same words README uses — the two texts are read by the same audience
    /// and a privacy claim that differs between them is the drift §2j exists to prevent.
    pub const ROBOT_GUIDE: &str;
    pub const BUILD_STAGE: &str;
    pub const BUILD_NOTE: &str;          // the §4b GREEN/AMBER/RED summary, in prose
    /// Carries, besides `version`/`commands`/`exit_codes`:
    ///   "env":      [{name, purpose, default, test_only}] — FMONERO_STATE_DIR,
    ///               FMONERO_CONFIG_DIR, FMONERO_CACHE_DIR, FMONERO_WALLET_PASSWORD (the
    ///               documented way an agent opens a wallet with no TTY, §2b.1), NO_COLOR, CI,
    ///               and FMONERO_PROC_ROOT / FMONERO_SYSTEMCTL_FIXTURE /
    ///               FMONERO_SYSTEMD_USER_DIR (§2i, v5.1) marked `test_only: true`
    ///   "settings": the §4 key table with defaults, from `Settings::KEYS` — so v5.2's
    ///               `node.mode`, `node.remote` and `node.remote_trusted` appear with no edit here
    ///   "warnings": the §5 warning-id table, `remote_node` and `remote_node_trusted` included
    ///   "reasons":  the §4 `reason` discriminators, grouped by exit code, **generated from
    ///               `Error::REASONS`** so the documented list and the constructible set cannot drift
    ///   "commands": each with `output_format` ("json" everywhere but robot-docs, --help and tui,
    ///               §4) and `accepts_dry_run` (true for exactly six commands since v5.2, §4)
    /// Phase 3's ergonomics pass asserts every documented env var is read somewhere under
    /// `crates/` and that every variable read is documented — the round trip is what stops the
    /// list drifting.
    pub fn capabilities_json(ctx: &Context) -> serde_json::Value;
}

// ---- fmonero (bin) — frozen entry points --------------------------------------
// TuiArgs, DoctorArgs and SetupArgs are `#[derive(clap::Args)]` structs that live in
// `tui/mod.rs`, `doctor/mod.rs` and `setup.rs` respectively and are re-exported into main.rs's
// clap tree — so a Phase 2 agent adds a flag to its own screen without touching main.rs, which it
// does not own (§2k). `Out` is `output.rs`'s renderer handle.
#[derive(clap::Args, Clone, Debug)]
pub struct TuiArgs { pub node: bool, pub wallet: bool, pub send: bool, pub log: bool }
#[derive(clap::Args, Clone, Debug)]
pub struct DoctorArgs { pub fix: bool, pub only: Option<String>, pub dry_run: bool,
                        pub yes: bool, pub save: bool }
#[derive(clap::Args, Clone, Debug)]
pub struct SetupArgs { pub units: bool, pub plugin: bool, pub menu: bool, pub all: bool,
                       pub yes: bool, pub dry_run: bool, pub rotate_rpc_login: bool,
                       pub full_chain: bool }
#[derive(Copy, Clone, Debug)] pub struct Out { pub json: bool, pub color: bool }

pub fn tui::run(args: TuiArgs, ctx: Context) -> anyhow::Result<i32>;
pub fn doctor::run(args: DoctorArgs, ctx: Context) -> anyhow::Result<i32>;
pub fn setup::run(args: SetupArgs, ctx: Context, out: Out) -> anyhow::Result<i32>;
```

### 5b.1 The v5.2 amendments, in full

Twelve changes, all for remote node mode (§2m). Each is recorded in `DECISIONS.md` in the commit that
makes it, per the rule at the top of §5b. Nothing else in §5b moves.

| # | Change | Why it could not be additive |
|---|---|---|
| 1 | `send::build` and `send::build_sweep` take `gate: NodeGate<'_>` instead of `n: Option<&NodeClient>`; `NodeGate` is new | `None` already carries a meaning (refuse) that is the **opposite** of what remote mode needs (proceed). Keeping `Option` and documenting a second meaning is the §2a.3 arrangement on the money path — the one place this document refuses it (§2m.3). |
| 2 | `WalletClient::set_daemon` gains `ssl: SslSupport`; `SslSupport` is new | An omitted `ssl_support` means **autodetect**, not plaintext (`KV_SERIALIZE_OPT`, §1b.4), so "send nothing in local mode" is not the same as "send disabled". The parameter is how the two modes differ at all. |
| 3 | `NodeSettings` gains `mode: NodeMode`, `remote: String`, `remote_trusted: bool`; `NodeMode` is new | The mode is a setting; three keys with one invariant (§2m.1) and a typed enum for the one every consumer branches on. |
| 4 | `NodeStatus.reachable` becomes `Option<bool>` and `NodeStatus` gains `trusted: bool`; `WalletStatus` gains `last_refresh_at: Option<String>` | §5's shape rule: unknown is `null`, never a zero or a `false` standing in for it. `reachable: false` in remote mode would be a claim we cannot support (§1b.9 item 5). The plugin's `node.reachable === true` already reads `null` as not-up, so §6 needs no change for correctness — only for the new icon case (§6.4). |
| 5 | `StateDoc` gains `last_refresh: Option<i64>`; `state::note_refresh` is new | `postcard` is not involved (this is `state.json`, `serde_json`), and an absent key deserializes to `None` through `#[derive(Default)]`, so old state files load unchanged. |
| 6 | `wallet_session::refresh` is new | Three independently written callers of "refresh, then record that we did" (§2a.3). |
| 7 | `units::daemon_target` and `units::DaemonTarget` are new | Six callers of one derivation; the unit and `set_daemon` disagreeing is the failure §4.2 spends a paragraph on. |
| 8 | `node_mode::{set_remote, set_local}` and `node_mode::ModeChange` are new | §2m.5's nine steps compose settings, unit rendering and `systemctl`; `main.rs` is frozen and the doctor's fixer needs the same rendering. |
| 9 | `Settings::parse_remote_address` is new; `NodeClient::from_ctx` and `units::detect_layout` keep their signatures and change behaviour | One grammar for three callers; one remote-mode gate for nine commands. Behaviour changes are listed here too, because "compile against exactly these signatures" does not warn a Phase 2 agent that `from_ctx` can now fail for a new reason. |
| 10 | `wallet_session::open` keeps its signature and changes behaviour twice: its `set_daemon` arguments come from `units::daemon_target(ctx)` rather than from `Login::from_monerod_conf`, and **a `-38` is no longer always swallowed** — in remote mode it is exit 4 `backend`, `reason: remote_address_rejected` (§2m.3, §4.2) | Same reason as #9, on the one call every wallet command goes through. Until this row existed §5b's doc comment said a `-38` "is a warning, not a failure" full stop, which is the **opposite** of what §2m.3 and §4.2 require in remote mode — two sections of one document giving opposite instructions for one call, in the module a Phase 1b agent implements from. A signature that does not move is not a contract that did not move. |
| 11 | `units::remote_node_error(ctx, verb: Option<&str>)` is new, and `send::refuse_if_send_in_flight(ctx)` is the §2a.4 entry check's unrelayed half, extracted from `send::preflight` | §2m.3 prescribes the refusal's wording per verb (`fmonero node <verb> needs the local monerod; node.mode is remote (<addr>)`), and `NodeClient::from_ctx` — the structural gate — does not know which command called it. One constructor, two callers, `None` for the plural. The second is `node_mode`'s money guard: it needs the unrelayed check and **not** the resolve step that follows it, because resolving asks the wallet daemon those commands are about to restart (§2m.5). |
| 12 | `settings::{remote_host, remote_host_port, is_loopback_host, is_private_host}` and `paths::systemd_user_dir_is_sandboxed` are new | The first four are the address predicates §2m.2, §2m.3 and §2m.6 name, in one place rather than four; `units::ssl_for_address`, `status`'s `remote_node_trusted` and the doctor's `remote_node_untrusted` all read them. The fifth narrows §4.2's "a test never installs a unit" by exactly one case — a unit written into `FMONERO_SYSTEMD_USER_DIR` **while `Systemctl::Fixture` is also set** is not a real unit, and §4b's AMBER `node remote <addr> --yes` row has no other way to exist. Either half alone still refuses. |

## 5c. Per-command JSON payloads

`--json` is promised on every command (§4) and agents are the stated audience, but before v2 only
`status` (§5) and `doctor` (§7) had a documented shape and the envelope's `…` was undefined for the
other thirty. That is the one contract an implementer cannot discover by reading code that does not
exist yet: Phase 1b would invent a shape, the docs agent would document a different one, and
`tests/e2e.sh` would assert against neither.

**The rule.** A success payload is the envelope keys (`ok`, `schema`) plus exactly the keys listed
here, **at top level** — no wrapper object. Every monetary value appears twice, `*_pico` (integer)
and `*_xmr` (string). A list command puts its rows under a single plural key. Absent data is `null`,
never a missing key (§5's shape rules apply everywhere). A failure payload is always the §4 error
envelope — `ok`, `schema`, `code`, `code_name`, `error`, `hint`, `reason` — whatever the command.

**One exception, and it is exact, because it is the money surface.** `wallet send` and
`wallet sweep` refused at §2a.1 step 9 (`reason: fee_above_max`) or step 10 (`reason: needs_yes`)
emit the failure envelope **plus the complete `wallet send` key set of the table below**, with
`relayed: false`, `dry_run: false`, `txids: []`, and `fee_pico`/`fee_xmr`/`total_pico`/`total_xmr`/
`weight`/`txs` carrying the **built** transaction's real numbers: the fee §2a.1 exists to show may
not be human-mode-only, and `--json` is mandatory on a non-TTY (§4), so an agent asked to confirm a
fee would otherwise have to parse the human stderr line to learn it. `ok` is `false` and the exit
code is 3. Every **other** failure of these two commands (exits 2, 4, 10, 14, 15, 16, and exit 3
with `reason: pending_send` or `send_in_progress`) carries the plain envelope, because no
transaction was built and there is no fee to report. The Phase 1b gate therefore asserts **three**
key sets for `wallet send`: dry-run success, relayed success, and this refusal union.

**`tui` and `robot-docs` are exempt**: one is interactive (and exits 2 on a non-TTY stdout), the
other is plain text by contract (§4). The Phase 1b gate skips exactly those two and no others.

| Command | Keys |
|---|---|
| `wallet send`, `wallet sweep` | `relayed` (bool), `dry_run` (bool), `txids` (array, one entry per transaction — empty when nothing was relayed), `amount_pico`/`amount_xmr`, `fee_pico`/`fee_xmr`, `total_pico`/`total_xmr`, `dest`, `account_index`, `priority`, `weight` (the **sum** of the built transactions' weights — `Plan.total_weight`, §5b; a sweep has no single weight), `txs` (array of `{txid, amount_pico, amount_xmr, fee_pico, fee_xmr, weight, relayed}`, one entry per built transaction — the shape §5's `pending.txs` already uses, so a sweep's per-transaction amounts and fees are not invisible in JSON; `txids` stays as the flat array for agents that only need identifiers). **The keys are identical for a dry run, for the no-`--yes` refusal and for a relayed success**, so an agent parses one shape and reads `relayed` to learn what happened. |
| `wallet history` | `transfers` (array of `{txid, type, height, timestamp, amount_pico, amount_xmr, fee_pico, fee_xmr, confirmations, locked, note, address, subaddr_index, destinations}`), `cache_age_sec`, `high_water`, `cached` (bool), `pool_unavailable` (bool — `true` only when `--pool` was asked for and the daemon round trip inside `get_transfers` failed, so the rows are complete except for the pool, §4.1). `type` is `in`\|`out`\|`pending`\|`failed`\|`pool`\|`block` (§5b rule 3; `block` is a coinbase row, §1b.4). `confirmations` and `locked` come from `history::confirmations` / `history::locked` (§5b) and are **never recomputed locally**. `subaddr_index` is the object `{major, minor}` — `major` **is** the account index, so there is no separate `account_index` key (§1b.4). `destinations` is an array of `{address, amount_pico, amount_xmr}`, empty for `in` rows. **`confirmations` and `locked` are `null` with `--cached`**, where no wallet height is available (§4.1); they are never 0 standing in for unknown. |
| `wallet balance` | `account_index`, `balance_pico`/`balance_xmr`, `unlocked_pico`/`unlocked_xmr`, `blocks_to_unlock`, `time_to_unlock_sec`, `subaddresses` (array of `{address_index, address, label, balance_pico, balance_xmr, unlocked_pico, unlocked_xmr, num_unspent_outputs}`, present only with `--subaddresses`, else `null`) |
| `wallet address`, `wallet new-address` | `primary`, `addresses` (array of `{address_index, address, label, used}`), `selected` (the address `--index` chose, or the primary) |
| `wallet list` | `wallets` (array of names), `dir` |
| `wallet book list` | `entries` (array of `{index, address, description}`) |
| `wallet book add`/`remove` | `index`, `address`, `description` |
| `wallet validate` | `address`, `valid`, `integrated`, `subaddress`, `nettype`, `structural_only` (bool — true when the verdict came from `address::parse` alone) |
| `wallet clear-pending` | `cleared` (bool), `pending` (the §5 `pending` object — the same `PendingStatus` shape, rendered from the record as it was — or `null`) |
| `wallet refresh` | `blocks_fetched`, `received_money` |
| `wallet notes get`/`set` | `txids` (array), `notes` (array, index-aligned) |
| `wallet open` | `name`, `open` (bool), `unit_started` (bool), `nettype` (the wallet's own, §2a.6) |
| `wallet close` | `name` (the wallet that was closed), `open` (always false), `unit_started` (always false) |
| `wallet create` | `name`, `open` (**true** — `on_create_wallet` ends `m_wallet = wal.release()`, so the new wallet is left open and `wallet create` finishes `wallet_session::open`'s last three steps on it, §4.1), `unit_started`, `seed_backup_hint` (§2b.2); **never** `seed` |
| `wallet set-daemon` | `address`, `trusted` (bool), `ssl_support` (`"disabled"` \| `"autodetect"` — §2m.3 chooses by the address's shape), `authenticated` (bool \| null), **`persistent` (bool, always `false`)**. `persistent` is a key rather than a README sentence because the fact it carries is that this call is undone by the next `wallet open` (§4.2) — an agent that sets a daemon and then opens a wallet would otherwise have no way to learn that its change is gone, and the honest place to say so is the payload of the command that made it (§2m.5). |
| `node info` | `node` — the §5 `node` object verbatim, so one parser serves both — plus `txpool` (`{txs_total, bytes_total, fee_total_pico, fee_total_xmr, oldest, num_10m, num_double_spends}`), `sync` (`{height, target_height, spans, peers}` or `null` on a restricted node) and `hard_fork` (`{current_version, state, voting_on: {version, enabled, earliest_height, votes, threshold, window, voting}}` — **two different fork versions, kept apart**, §1b.3) and `rpc` (`{version_major, version_minor, release, hard_forks: [{hf_version, height}]}` from json_rpc `get_version`, §1b.3 `:2173` — where `version` is the **RPC** version packed `major<<16\|minor`, not the software version, which is `node.version` from `get_info`). That last key is why `NodeClient::get_version` exists: without a place to land, a frozen call with no consumer forces a Phase 1b agent either to break the §5c exact-key-set gate or to break the §5b freeze. The block-header fields of `node` are **`null`, not zeros**, when `get_last_block_header` answered `"BUSY"` (§1b.3). **In remote mode the key set is identical and `txpool`, `sync`, `hard_fork` and `rpc` are all `null`**, with `node` carrying §5's remote-mode shape — `layout: "remote"`, `url` = `settings.node.remote`, `trusted` = `settings.node.remote_trusted`, `reachable: null` and every measured field `null` (§2m.3). One key set for both modes is what lets §5c's exact-key-set gate run unchanged and one parser serve both; a shorter remote payload would make every consumer branch on the mode before it could read the address. |
| `node peers` | `connections` (array of `{address, host, port, state, height, pruning_seed, incoming, live_time, avg_download, avg_upload}` — `port` is parsed from the daemon's **string** and rendered as an **integer**, so both arrays agree, §1b.3), `white` and `gray` (arrays of `{host, port, last_seen}`), each `null` on a restricted node |
| `node bans` | `bans` (array of `{host, ip, seconds}`) |
| `node ban`/`unban` | `host`, `banned` (bool), `seconds` |
| `node limit` | `limit_up_kbps`, `limit_down_kbps`, `out_peers`, `in_peers`, `changed` (bool) |
| `node up`/`down`/`restart` | `unit`, `active` (bool), `reachable` (bool), `synchronized` (bool \| null), `waited_sec`, `already_running` (bool — `node up` against our own live node, §2i) |
| `node remote`, `node local` | `mode` (`"local"` \| `"remote"`), `address` (the address in force **after** the change — `settings.node.url`'s host:port in local mode, `settings.node.remote` in remote mode), `trusted` (bool), `ssl_support` (`"disabled"` \| `"autodetect"`), `actions` (`units::FileAction[]`, the same shape and the same renderer `setup` uses, §5b), `wallet_unit_restarted` (bool), `wallet_must_reopen` (string \| null — the name of the wallet that was open and was closed by the restart, §2m.5 step 7), `changed` (bool — false on an idempotent second run), `dry_run` (bool). **The two commands share one key set**, as `wallet send` and `wallet sweep` do, so an agent parses one shape and reads `mode` to learn which way it went. It is `node_mode::ModeChange` verbatim (§5b). |
| `node log` | `path`, `lines` (array of strings), `truncated` (bool) |
| `settings list` | `settings` (array of `{key, value, default, doc}`) |
| `settings get` | `key`, `value`, `default` |
| `settings set` | `key`, `old`, `new` |
| `setup` | `actions` (`units::FileAction[]` verbatim: `{what, path, status: "written"\|"unchanged"\|"skipped"\|"planned", diff}`, §5b), `dry_run` (bool) |
| `capabilities` | the §5b `agent::capabilities_json` object |
| `version` | `version`, `build_stage`, `build_note` |
| `doctor` | the §7 object |
| `doctor undo` | `run_id`, `undone` (array of `{seq, action, path, ok}`), `refused` (bool), `reason` |
| `doctor capabilities` | `checks`, `groups`, `classes` |
| `robot-docs` | plain text, never JSON (§4) — **exempt from the gate below** |
| `tui` | interactive; exits 2 on a non-TTY stdout — **exempt from the gate below** |

**Gate (Phase 1b).** `tests/e2e.sh` runs every command in §4.1 with `--json` against the mock or in
its empty state — **except `tui` and `robot-docs`, the two rows marked exempt** — and asserts, per
command, that the top-level key set is exactly the row above: no missing key, no extra key. The money rows' payloads (`wallet send` in all three outcomes,
`wallet sweep`) are additionally pasted into DECISIONS.md as real output; the rest are covered by
the assertion, because thirty payloads in a decisions log is bulk, not evidence.

---

## 6. Plugin `oliver.monero` (Quickshell bar widget, Omarchy 4)

### 6.1 Files and budgets

| File | Budget | Contents |
|---|---|---|
| `manifest.json` | ~55 lines | schemaVersion 1, id `oliver.monero`, name "Monero", version, author, license, description, `kinds:["bar-widget"]`, `entryPoints.barWidget:"Panel.qml"`, `barWidget{displayName, description, category:"Finance", allowMultiple:false, defaultSection:"right", defaults{}, schema[]}` |
| `Service.qml` | ≤ 380 | the state model (6.3), the poll, the action queue |
| `Panel.qml` | ≤ 700 | bar button + `KeyboardPanel` content (6.5) |
| `MoneroIcon.qml` | ≤ 95 | the Monero ⓜ glyph as `QtQuick.Shapes` + `PathSvg`; no image asset. **+15 in v5.2** for the remote badge of §6.4 — one more `Shape` in the badge slot the warning badge already uses |
| `Model.js` | ≤ 165 | pure formatters: `parseStatus(text)`, `shortAddr`, `age`, `rate`, `syncLabel`, `pendingLabel(pending, showAmounts)` (renders `status.pending` for the §6.5 pending row; `showAmounts` is the §6.2 `showBalance` flag), **`remoteLabel(node)`** (v5.2 — `"remote: <shortened addr>"` plus `" · trusted"` when `node.trusted`, for the §6.5 row that replaces the sync gauge) |
| `README.md` | — | docs agent |

Total QML under **1 340 lines**, the sum of the four hard per-file budgets above (`manifest.json`'s
"~55" is a shape, not a gate); the plugin gate records real line counts against them. **v5.2 raised
it from 1 310 by the +15/+15 above and by nothing else**: `Service.qml` gains one derived property
and `Panel.qml` trades the remote-mode sync-gauge block for a one-line row, so neither budget
moves. §7a's v5.1 lesson applies here too — the honest lever is the number, stated in this document
before the code is written, not a feature quietly dropped to fit one. The widget is thinner than fproton's because Status JSON already carries
pre-rendered `*_xmr` strings (§5) — the panel never does arithmetic on money.

### 6.2 Imports and host contract

`import qs.Commons` and `import qs.Ui`. `Panel.qml` is a `qs.Ui.Panel` with
`moduleName: "oliver.monero"`, `ipcTarget: "oliver.monero"`, `manageIpc: false` and its own
`IpcHandler` (`toggle`, `refresh`, `nodeUp`, `nodeDown`) as a convenience. The host injects `bar`,
`moduleName`, `settings`. Use `bar.barForeground` for bar-drawn content and
`bar.foreground/urgent/fontFamily/barSize` elsewhere. **Never `bar.run()`** (§1.5); always
`Quickshell.execDetached(argv)`.

Settings, every one read through `setting(name, fallback)` / `intSetting(name, fallback, min, max)`
with the fallback duplicating the manifest verbatim (§1.5): `binary` `"fmonero"`,
`refreshIntervalSec` 3 (1–60), `idleRefreshIntervalSec` 8 (2–600), `showBalance` `"on"`,
`tuiCommand` `"omarchy-launch-or-focus-tui --app-id=fmonero fmonero tui"`.
The two intervals differ because the two situations differ: **3 s while the panel is open**, when
the user is watching a sync percentage tick, and **8 s while it is closed**, when a bar widget needs
only enough to pick an icon state — and each poll is a process spawn, which is the expensive part of
the §2l budget.
`tests/plugin-smoke.sh` asserts every manifest default key has an in-code fallback.

### 6.3 `Service.qml` — one source of truth: the last Status JSON

```qml
property var status: ({ ok: false })
readonly property bool alive:      status.ok === true
readonly property var  node:       status.node || null
readonly property var  wallet:     status.wallet || null
readonly property var  pending:    status.pending || null
readonly property var  warnings:   Array.isArray(status.warnings) ? status.warnings : []
readonly property string lastError: String((status.last_error && status.last_error.text) || "")
readonly property bool nodeUp:     !!(node && node.reachable === true)
readonly property bool remote:     !!(node && node.layout === "remote")   // v5.2, §2m
readonly property bool synced:     !!(node && node.synchronized === true)
readonly property int  syncPct:    node && node.sync_percent !== null ? Math.floor(node.sync_percent) : 0
readonly property bool walletOpen: !!(wallet && wallet.open === true)
property string actionStatus: ""     // transient label, cleared 2.2 s after an action exits
property string localError: ""       // stderr of the last failed action
property int _desired: -1            // optimism bridge: -1 follow, 0 want down, 1 want up
readonly property bool busy: actionProc.running
```

`_desired` is set on a node toggle and cleared when a Status arrives whose `node.reachable` agrees,
or after 5 s, whichever is first. **In remote mode the node toggle is disabled and `_desired` is
never set**: `node.reachable` is `null` there, so the optimism bridge has nothing to converge on,
and `node up`/`node down` exit 4 `remote_node` anyway (§2m.3). `nodeUp` and `synced` already read
`null` as false because both compare with `=== true`, so no existing expression needs changing —
only the ones that would render a **number** do, and §6.4/§6.5 handle those.

**Polling.** One `Process { command: [bin, "status", "--json"] }` with `StdioCollector`s; a `Timer`
at `opened ? refreshMs : idleRefreshMs`, `triggeredOnStart`, that **skips the tick while the
process is running**; and a `pollWatchdog` `Timer { interval: 8000; repeat: false }` armed on the
launch that needs watching and never restarted while armed. Non-zero exit or unparsable stdout ⇒
`status = {ok:false}`, `localError` from stderr, and the last good `node`/`wallet` rows kept for
display behind a dimmed style. `ok === true` is the gate for every rendered value: the panel never
reads `status.node.height` without it. On first load, `Process { command: ["which", bin] }`;
failure renders "fmonero is not installed or not on PATH (~/.local/bin)" and pauses polling until
the next open.

**Actions.** A serialized FIFO (`_queue`, `pump()`), one `actionProc`, each followed by a 600 ms
`delayedRefresh`. The argv table is the whole mutating surface of the widget:

| UI action | argv |
|---|---|
| Node up | `[bin, "node", "up", "--json"]` |
| Node down | `[bin, "node", "down", "--yes", "--json"]` |
| Refresh wallet | `[bin, "wallet", "refresh", "--json"]` (shown only when `walletOpen`) |
| Copy primary address | `Quickshell.execDetached(["wl-copy", "--", String(wallet.primary_address)])` — argv, and the value came from our own JSON |
| Open TUI | `Quickshell.execDetached(["omarchy-launch-or-focus-tui", "--app-id=fmonero", "fmonero", "tui"])` — **constant literals only** (§1.5) |
| Send screen | `…, "fmonero", "tui", "--send"]` — constant literals only |
| Doctor | `…, "--app-id=fmonerodoctor", "fmonero", "doctor"]` — constant literals only |

Under a `system_unit` layout `node up`/`node down` exit 3 without issuing an RPC (§2i); the panel
renders that refusal's `hint` in `localError` like any other failure and its `ToggleSwitch` stays
bound to the real `node.reachable`, so the widget never appears to have done something it did not.

**The widget never opens, creates or restores a wallet, never sends, never sweeps, and has no
password field** (§2b.5). There is no argv in the table that can move money. `wallet open` is a
terminal action, and the panel's entry point for it is the TUI button.

### 6.4 Bar icon states (`MoneroIcon` inside a `BarIconButton`)

First match wins:

| Condition | Colour | Modifier |
|---|---|---|
| `!alive` | `Qt.darker(bar.barForeground, 1.55)` | crossed + warning badge |
| any warning with severity `error` (`send_may_be_in_flight`, `*_stalled`, `monero_not_installed`) | `bar.urgent` | warning badge |
| **`remote && walletOpen`** (v5.2) | `bar.barForeground` | filled glyph + **remote badge** |
| **`remote`** (v5.2) | `bar.barForeground` | outline glyph + **remote badge** |
| `!nodeUp` | `Qt.darker(bar.barForeground, 1.55)` | crossed |
| `!synced` | `bar.barForeground` | a **percent ring** around the glyph at `syncPct`, plus the number when `barSize` allows |
| `walletOpen` | `bar.barForeground` | filled glyph |
| otherwise (node up, synced, wallet closed) | `bar.barForeground` | outline glyph |

**The two remote rows sit above the three node rows deliberately.** `!nodeUp`, `!synced` and the
percent ring all read fields that are `null` in remote mode (§5), so left below they would render a
crossed glyph and a 0 % ring for a wallet that is working perfectly — a bar icon claiming the node
is down. Above them, the remote rows answer first and the only question left is the one remote mode
can answer: is a wallet open. The **remote badge** is a small chevron in the same badge slot the
warning badge uses, in `bar.barForeground` rather than `bar.urgent`: remote mode is a configuration,
not a fault.

Mouse: left toggles the panel; right toggles the node (`nodeUp ? nodeDown() : nodeUp()`); middle
refreshes. **In remote mode the right-click toggle is inert**: it runs nothing and sets
`localError` to `fmonero node local --yes`, because firing a command that is specified to exit 4
(§2m.3) would put a red error on the bar every time someone mis-clicks. Tooltip:
`Monero · <height> · <peers> peers · <sync>` and, when `walletOpen` and
`showBalance === "on"`, ` · <balance_xmr> XMR`; in remote mode it is
`Monero · remote: <addr>` plus ` · wallet <height>` when a wallet is open and the same balance
clause, and it never contains a height, a peer count or a sync phrase it does not have.

### 6.5 `Panel.qml` — layout and keyboard

`KeyboardPanel { anchorItem: button; owner: root; bar: root.bar; open: root.opened;
focusTarget: keyCatcher; contentWidth: panel.fittedContentWidth(Style.space(380));
contentHeight: panel.fittedContentHeight(column.implicitHeight, Style.space(520)) }` wrapping a
`PanelKeyCatcher` wrapping a `Flickable`.

Sections in cursor order: **header** (`PanelHero`: the Monero mark, `Node · <height>` or "Node
down", the sync state phrase, a `ToggleSwitch` bound to `nodeUp`/`busy` — **disabled in remote
mode**, §6.3) → **node detail** (sync
`MiniBar` with percent, peers in/out, mempool txs, last block age, net rates — not cursor targets;
**in remote mode this whole block is replaced by one row, `Model.remoteLabel(node)` —
`remote: <short addr>` plus ` · trusted` — and one dim line,
`fmonero does not talk to this node; monero-wallet-rpc does`**, which is why §6.1's `Panel.qml`
budget does not move. No gauge, no percent, no peer counts: every one of them is `null` in remote
mode and §2a.5's rule against arithmetic in QML has a sibling here — the panel renders what Status
JSON contains and never a substitute for what it does not, §5)
→ **warning** (visible when any warning; `bar.urgent` text + hint; Enter runs the hint when it is
`fmonero doctor`, else copies it) → **wallet** (when `walletOpen`: balance and unlocked, `↑ locked
for N blocks` when `blocks_to_unlock > 0`, the shortened primary address with a copy action, and a
Refresh row; when reachable but closed: one row "No wallet open — open it in the TUI"; when the
unit is down: "Wallet daemon is not running") → **pending** (when `status.pending`: `bar.urgent`,
`Model.pendingLabel(status.pending, showBalance === "on")` —
"Send in flight: <`total_amount_xmr`> → <short dest>" plus "· 1 of 3 relayed" when
`relayed_count < total_count`; Enter opens the TUI) → **footer** (Open TUI · Send ·
Doctor).

**`total_amount_xmr` is a pre-rendered string from Status JSON (§5b), not a sum the QML computes.**
A sweep's record holds several transactions, so a widget asked for a total would have to
`parseFloat` and add — money arithmetic in QML, which §2a.5 forbids in as many words ("no `f64`
touches a monetary value anywhere, in Rust or in QML"). The Rust side sums with
`Piconero::checked_add` and hands over the string; §6.7's grep makes that half of the rule
enforceable.

**The pending row honours `showBalance`.** `Model.pendingLabel` takes the flag as its second
argument — `Model.pendingLabel(status.pending, showBalance === "on")` — and with
`showBalance !== "on"` the row reads `Send in flight` plus the `· 1 of 3 relayed` counter and
carries **neither the amount nor the destination**. The setting exists so an amount can be kept off
a bar other people can see, and pending is the row most likely to be on screen while someone is
looking over a shoulder; a `showBalance: "off"` that hides the resting balance and reveals the live
payment does not mean what its name says. The tooltip (§6.4) already gates on the same flag.

Keyboard. `PanelKeyCatcher` only *dispatches*: `moveRequested(dx,dy)` (Down/j, Up/k, Right/l,
Left/h), `activateRequested` (Enter/Space), `closeRequested` (Esc), `tabRequested(±1)`
(Tab/Shift-Tab → `bar.switchPanelFrom(root, direction)`), `textKey(t)`. `Panel.qml` owns the state
machine (`focusSection`, `selectedIndex`, section order, per-section activation), exactly as the
Tailscale panel does. Behaviour: Down/Up move within a section then between sections; Enter
activates; Esc closes; textKey `n` toggles the node, `r` refreshes the poll, `t` opens the TUI,
`c` copies the address, `d` opens the doctor. `h j k l` are consumed by the catcher and are not
shortcuts.

### 6.6 Install and dev loop

`ln -sfn ~/Projects/odcpw/fmonero/plugin/oliver.monero ~/.config/omarchy/plugins/oliver.monero`,
`omarchy-shell shell rescanPlugins`, `omarchy plugin enable oliver.monero`,
`omarchy bar put oliver.monero --before omarchy.tailscale`. **After every QML edit run
`omarchy-shell shell rescanPlugins`** (symlinks do not hot-reload, §1.5). Validate the repo
directory, never the installed link.

### 6.7 `tests/plugin-smoke.sh` (the gate; exits non-zero; < 10 s)

`omarchy-plugin-validate plugin/oliver.monero` on the repo dir; `jq -e` checks on
id/kinds/entryPoint/defaultSection; required files exist; `manageIpc: false` present; **no
`bar.run(` and no `execDetached(["bash"` anywhere**; every manifest default has an in-code
fallback; **no argv *element* in any QML file equals `send`, `sweep`, `open`, `create`, `restore`,
`transfer` or `relay`, and no QML file contains the string `password`** (§2b.5 as a grep);
**no QML file contains `parseFloat`, `Number(` or `/ 1e`** — the QML half of §2a.5, whose Rust half
(`grep -rn 'as f64'`) covers only `crates/`, with its own self-test fixture proving the grep fires
on a `parseFloat(status.pending.txs[0].amount_xmr)` line. The match
is on a whole argv element, not a substring, so the constant literal `--send` that opens the TUI's
Send *screen* (§6.3) passes while `[bin, "wallet", "send", …]` cannot — a substring grep would fail
on this design's own widget, and a grep that fails on correct code is a grep the first agent to hit
it relaxes. Its self-test: a fixture argv `["fmonero","wallet","send"]` must make the grep fire and
`["fmonero","tui","--send"]` must not. Then `omarchy-shell shell rescanPlugins`, sleep 2, and
`journalctl --user -t omarchy-shell --since <start>` filtered to `oliver.monero`, minus the benign
`Local plugin changed` / `Handler was registered but will not be used` lines, must contain none of
`failed:|Expected token|unavailable|is not a type|ReferenceError|TypeError`.

---

## 7. Doctor

`fmonero doctor` runs detectors, prints a table (or JSON
`{ok, run_id, checks:[{id, group, severity, status: pass|warn|fail|skip, detail, fixable, class,
fix_cmd}], summary}`), and exits per the rule below (11, 12 or 0). `--only GROUP` limits to one group
(`binaries`, `node`, `wallet`, `units`, `storage`, `install`, `security`). `doctor capabilities
--json` lists checks, groups and classes. The health idiom for scripts is
`fmonero doctor --json >/dev/null; echo $?`; there is no separate subcommand.

**Structure, unchanged from fproton because it is the doctor kernel's axioms 1–3:**
`probes.rs` is the only place a process is spawned, a file read or an RPC issued; `checks.rs` holds
every detector as a **pure function** `fn detect(&Probes) -> Verdict`; `mutate.rs` is the single
chokepoint every `--fix` mutation flows through; `report.rs` renders. Read-only by default: a run
without `--fix` writes nothing unless `--save`. **The doctor never stops a daemon it did not start,
never touches a wallet file, never removes a file, and never runs sudo** — the doctor kernel's
axiom, unnarrowed. v0.1 had one candidate for an exception, `history_cache_fresh`, and it is not
worth one: the check is severity **info**, its subject is a cache §5b already treats as "STALE, not
corrupt: ignored and refetched, never an error", so the code already recovers from every condition
the fixer would fix. A `RemoveFile` variant, a verbatim backup path, a sha256 pair, a recorded
inverse, an undo path and a fixture, to buy the ability to `undo` the deletion of a file the next
`wallet history` regenerates, is ceremony with real cost — and removing the only member removes the
whole exception. The check keeps its diagnostic value as a report (§7.1, class D). A doctor that
cannot delete anything is both leaner and a stronger safety claim than one that can delete one
thing carefully.

**`--fix` goes through one `mutate(Action)`.** `Action` is
`WriteFile{path, bytes, mode}` (verbatim backup + sha256 before and after),
`CreateDir{path, mode}` (inverse: `rmdir`, and only while it is still empty),
`CreateSymlink{link, target}` (inverse: remove the link), and
`RunArgv{argv, inverse_argv}`. `CreateDir` exists because `wallet_dir_exists` creates a *directory*,
which `WriteFile` cannot do, and a fixer that cannot be routed through the one chokepoint is a fixer
that gets written around it, as `RunArgv ["mkdir", …]` with no recorded inverse. Naming it costs one
line and keeps the doctor kernel's axioms 1–3 true.

Every variant appends one line to `runs/<id>/actions.jsonl` (append-only, fsync) with
`{seq, class, action, before, after, inverse, started_at, ok}`. `before`/`after` are: the sha256 of
the file for `WriteFile`; the symlink target or `null` for `CreateSymlink`; whether
the directory existed for `CreateDir`; and `null` for `RunArgv`, whose reversal is `inverse_argv`
alone. `doctor undo <run-id|latest>` replays it in reverse and refuses (exit 13) if a backup is
missing or its hash mismatches. `--dry-run` prints the planned mutations and changes nothing
(verified by `git status` plus a state-dir checksum). Artifacts live at
`~/.local/state/fmonero/doctor/runs/<UTC basic>__<run-id>/{report.json,actions.jsonl,probes/,backups/}`,
**`<run-id>` being 8 lowercase hex characters from `/dev/urandom`** — so a directory is e.g.
`20260916T140211Z__3f9a1c72`, and `doctor undo` takes either that id or `latest`. `…/doctor/latest`
is updated by symlink-to-temp + rename; each probe file is truncated at 64 KiB with a trailer,
redacted at capture per §2b.8, and written 0600; every write prunes `runs/` to the newest 10.

**Exit codes**, because "a fixer failed, or fixed only part" left the common case undefined: a run
**without** `--fix` exits **11** if any check is `fail`, else 0. A run **with** `--fix` exits **12**
if any attempted fixer failed or fixed only part; **11** if every fixer succeeded but `fail` checks
remain that are not fixable; **0** only when no `fail` remains. `--fix` alone performs every fixer in
v0.1; `--yes` is accepted and ignored, so `fmonero doctor --fix --yes` is always safe to script.

Safety classes, as fproton:

| Class | Runs under | Reverts by | Checks here |
|---|---|---|---|
| A — fmonero-owned state | `--fix` | file backup | `state_valid`, `settings_valid`, `state_perms` |
| B — fmonero-created integration | `--fix` | recorded inverse argv/symlink | `units_present`, `unit_loopback`, `unit_no_node_wants`, `monerod_conf_mode`, `wallet_dir_exists`, `wallet_dir_matches_unit`, **`wallet_daemon_matches_unit`** (v5.2), `wallet_port_consistent`, `unit_enabled_state`, `plugin_linked`, `plugin_enabled`, `menu_entries` |
| D — never auto-fixed | never | — | everything touching a wallet file, `/etc/monerod.conf`, the system `monerod.service`, the blockchain data dir, or a `pending_send` — and **`pending_send` (§7.1) is report-only in every state, a fully-relayed record included**: the doctor prints it and the two commands and mutates nothing. Of all the doctor's checks it is the only one whose fixer could lose money, so the summary table and the detail table must not disagree about it, and a class-A entry here would be a licence to write exactly the mutation §2a.4 exists to prevent. |

There is no class C. fproton's table carried one for "foreign but reversible" with no members, and a
class with no members is a `--fix --yes` branch nothing exercises, a name nothing means, and a
question ("is `doctor --fix` alone legal?") every fresh doctor agent has to ask. A future fixer that
touches something foreign but reversible gets its own class then, with its first member.

### 7.1 The checks

| id | group | sev | class | detect | fixable | fix |
|---|---|---|---|---|---|---|
| `monerod_installed` | binaries | error | D | `proc::which("monerod")` and `monerod --version` under a 3 s deadline. **In remote mode the verdict is `warn:not_needed_in_remote_mode`, not `fail`** (v5.2): remote mode uses no local daemon, and a `fail` on a machine that is configured correctly is the check an agent relaxes (§7). The detail says `fmonero node local --yes` is what would require it | no | `omarchy pkg add monero` |
| `wallet_rpc_installed` | binaries | error | D | same for `monero-wallet-rpc` | no | `omarchy pkg add monero` |
| `version_supported` | binaries | warn | D | parse `monerod --version`; `pass` on 0.18.x, `warn` otherwise naming what fmonero was built against | no | — |
| `node_layout` | node | info | D | §2i: user unit / system unit / remote / none. **In remote mode it is the one `node` check that still runs and it `pass`es**, with the detail naming `settings.node.remote`, the trusted flag and the `ssl_support` in force — the group's only remaining source of truth, so that `doctor --only node` is never an empty table (§2m.3) | no | `fmonero setup --units` |
| `node_reachable` | node | error | D | `get_info` under 3 s | no | `fmonero node up` |
| `remote_node_untrusted` | node | warn | D | **new in v5.2, §2m.6.** `skip:local_mode` when `node.mode == local`. In remote mode: `warn` when `node.remote_trusted` is true **and** the host is not a literal address in `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `100.64.0.0/10` (CGNAT — where Tailscale lives), `169.254.0.0/16`, `::1`, `fc00::/7` or `fe80::/10`; `pass` otherwise. **A DNS name is never treated as private**: classifying one means resolving it, which is a network call inside a detector §7 requires to be a pure `fn detect(&Probes) -> Verdict`, and whose answer a name server can change between runs — so a private node reached by name warns, and the detail says to use its address if the warning is wrong. The detail also says what `trusted` costs: `select_available_outputs_from_histogram` puts the wallet's own unspent amounts in the request (`wallet2.cpp:12281-12282`) | no | `fmonero node remote <addr> --yes   # without --trusted` |
| `node_unrestricted` | node | warn | D | `is_restricted()` — `sync_info` → `-32601` (§1b.2) | no | detail names which panes go dark; for our own unit, `fmonero setup --units` |
| `node_synced` | node | warn | D | `get_info.synchronized`; detail carries `sync_percent` and the ETA from two height samples | no | — |
| `node_peers` | node | warn | D | `outgoing_connections_count > 0` unless `offline`; reports `skip:node_restricted` rather than `warn` when `get_info.restricted` (the count is 0 by construction, §1b.3) | no | `fmonero node limit --out-peers 16` |
| `node_clock` | node | error | D | `abs(get_info.adjusted_time - local now) <= 60 s`. A skewed clock breaks unlock times and peer handshakes. | no | `timedatectl set-ntp true` |
| `node_data_dir_single_writer` | node | error | D | §2i `data_dir_in_use`: the `/proc` scan **and** the `fcntl(F_GETLK)` probe on `<data-dir>/lmdb/lock.mdb` together find at most one `monerod` | no | detail names the PID |
| `wallet_rpc_reachable` | wallet | warn | D | `get_version` under 5 s | no | `systemctl --user start fmonero-wallet-rpc.service` |
| `wallet_rpc_auth` | wallet | error | D | the login file exists, has **no group and no other permission bits** (§2c), and a `get_version` using it returns 200. When the file is absent but the unit is active, the detail names **both** the port it looked for and the port the unit binds (§4.2) | no | `systemctl --user restart fmonero-wallet-rpc.service` |
| `wallet_daemon_link` | wallet | warn | D | wallet `get_height` within 100 blocks of node `get_info.height`. **`skip:remote_node` in remote mode**: the node's height is its half of the comparison and no wallet-RPC method reports it (§1b.9 item 5) | no | `fmonero wallet refresh` |
| `wallet_daemon_matches_unit` | units | error | **B** | **new in v5.2.** The installed wallet unit's `--daemon-address` equals `units::daemon_target(ctx).address` byte for byte, its trust flag is `--trusted-daemon` or `--untrusted-daemon` exactly as `daemon_target` says, and its `--daemon-ssl` value matches. It is its own check rather than a clause of `units_present` because it is the one skew with a *silent* symptom: a `monero-wallet-rpc` started before any `wallet open` auto-refreshes against whatever its argv said (§1b.9 item 3), so a unit naming the old node keeps a wallet syncing from a node `status` does not name — and `units_present`'s byte comparison, which would also catch it, reports the generic `unit_drift` and buries the reason. Skipped `skip:unit_missing` when the file is absent | **yes** | `WriteFile{unit path, units::render_wallet_unit(ctx), 0o644}` + `RunArgv{["systemctl","--user","daemon-reload"], inverse: the same}`, and the detail names `fmonero node remote <addr> --yes` as the command that also restarts the daemon |
| `wallet_daemon_auth` | wallet | error | D | **`skip:remote_node` in remote mode** (v5.2): there is no local credential to be missing — `set_daemon` sends no `username`/`password` at all there (§2m.3) — and both of its other terms, the node's height and its `synchronized` flag, are unobservable. Otherwise: one-shot and honest about what it cannot distinguish, because §7's architecture makes every detector a pure `fn detect(&Probes) -> Verdict` over **one** run's probes — "stuck while the node advances" is a two-sample-over-time observation and would force either a sleep inside a probe or a quiet weakening to "the wallet is behind", which fires on every normal sync. So: `fail` when our `monerod.conf` carries `rpc-login`, the wallet is reachable, the node is reachable, `node.synchronized == true` **and** `wallet_height + 100 < node_height`; `skip:node_syncing` when the node is not synchronized; `pass` otherwise. The detail says that a synced node with a wallet far behind is the signature of a missing daemon credential (§4.2) and that the check cannot distinguish it from a wallet still catching up on a large restore height. | no | `fmonero wallet set-daemon 127.0.0.1:18081 --trusted --yes` |
| `wallet_nettype_matches` | wallet | warn | D | the open wallet's own nettype, recorded by `wallet_session::open` from its primary address (§2a.6), equals `settings.node.network`; `skip:wallet_closed` when nothing is open. The send path follows the **wallet**, so a mismatch is a configuration fact and not a money risk — but it means `wallet.dir`, the unit's ports and the node the wallet talks to were rendered for a different network | no | `fmonero settings set node.network <the wallet's> && fmonero setup --units --yes` |
| `wallet_dir_exists` | wallet | warn | B | `settings.wallet.dir` **and every ancestor** exists, mode 0700. A missing *ancestor* is what stops the unit from starting: `monero-wallet-rpc` creates only the leaf, with one `mkdir(dir, 0700)`, and treats any failure but `EEXIST` as fatal (`wallet_rpc_server.cpp:325-340`, §4.2) | **yes** | `CreateDir{path, mode: 0o700}` **applied to each missing ancestor in turn, one `FileAction` per level**, so each stays individually undoable |
| `wallet_dir_matches_unit` | units | error | B | the wallet unit's `--wallet-dir` argument equals `settings.wallet.dir` byte for byte — both absolute, which is why the unit renders the path rather than `%h` (§4.2) — the daemon resolves `<wallet-dir>/<filename>` and a name may not contain `/`, so a mismatch is unbridgeable and makes `wallet list` read a directory `open_wallet` never writes | **yes** | rewrite the unit from the setting (backup first) |
| `wallet_port_consistent` | units | error | B | the wallet unit's `--rpc-bind-port` equals `settings.wallet.port` and the port in `settings.wallet.url`; otherwise `Paths::wallet_login_file(port)` looks for a login file the daemon never wrote (§4.2) | **yes** | **`settings.node.network` is the authority, not either observed value** (§4): the expected port is 18083 \| 38083 \| 28083 for mainnet \| stagenet \| testnet. The fixer rewrites `settings.wallet.port`, the port inside `settings.wallet.url` and the unit's `--rpc-bind-port` to that one value, in that order, each through `mutate` with a backup. Three-way disagreement with no stated authority is a coin flip inside a class-B fixer, and one side of it — rewriting the settings to match the unit — changes `Paths::wallet_login_file(port)` under a *running* daemon and produces exactly the symptom this check exists to prevent. **Refused (`skip:wallet_rpc_active`, no mutation) while the wallet unit is active**: rebinding a live daemon's port from under it is how a running wallet becomes unreachable mid-send. The detail then names the three observed values and `systemctl --user stop fmonero-wallet-rpc.service` as the precondition. |
| `pending_send` | wallet | error | D | the `pending.json` record has **any** entry with `relayed == false`; a fully-relayed record that the next `send` would resolve is reported `info`, not `fail` (§2a.4) | **no — class D** | `fmonero wallet history --limit 5` then `fmonero wallet clear-pending --yes`. Never automated: only a human can decide whether that transaction landed (§2a.4). |
| `units_present` | units | warn | B | both unit files exist and match `units::render_*` byte-for-byte. **This is also the upgrade path for v5.1's two unit changes** — the absolute `--config-file` and the `StartLimit*` pair (§4.2) — so a unit written by an older build reports `warn:unit_drift` and the class-B fixer rewrites it; no migration code exists or is needed | **yes** | rewrite them (backup first) |
| `unit_loopback` | units | error | B | both units' `--rpc-bind-ip` is `127.0.0.1` and neither carries `--confirm-external-bind` | **yes** | rewrite |
| `unit_no_password_in_argv` | units | error | D | neither unit's `ExecStart` contains `--password`, `--rpc-login`, `--daemon-login` or a `--password-file` (§2b.1, §4.2), **and both units' `--log-level` is `0`** — above 0 the daemon logs HTTP headers and wallet internals into a file §2b.7 exempts from the 0600 rule, in a directory that holds the daemon's credential (§4.2) | no | detail names the offending flag |
| `unit_no_node_wants` | units | error | B | neither unit's `Wants=` or `Requires=` names a `monerod` unit — `Wants=` would start a node around the §2i single-writer check (§4.2) | **yes** | rewrite (backup first) |
| `unit_enabled_state` | units | info | B | both units are **disabled** (on-demand, §4.2) | **yes** | `systemctl --user disable <unit>` |
| `monerod_conf_mode` | security | error | B | `~/.config/fmonero/monerod.conf` is mode 0600 (§2c) | **yes** | `chmod 600` |
| `node_auth` | security | warn | D | the node required and accepted a credential — read from `NodeClient::authenticated()` (§5b), which reports the handshake and not the presence of an `rpc-login=` line. **`skip:remote_node` in remote mode**: no `NodeClient` is constructed, and a public node's authentication is not ours to have an opinion about | no | `fmonero setup --units` (skipped with a note for a `system_unit` layout, which is not ours to change) |
| `state_perms` | security | error | A | every file under `state_dir`, `cache_dir` and `config_dir` **that fmonero itself writes** is 0600 and every directory 0700 (§2b.7) — these hold recipient addresses, amounts and the wallet name. The run directory's contents are **excluded by name**: `monero-wallet-rpc.*.login` settles at 0400 by `private_file`'s own dance (§2c) and `monero-wallet-rpc.log` is whatever the daemon chose, so a literal "every file is 0600" would report `fail` on a perfectly healthy machine — and a check that fails on correct systems is the check an agent relaxes. Those two are checked instead for **no group and no other permission bits**, and a violation is reported `fail` with `fix_cmd: systemctl --user restart fmonero-wallet-rpc.service`, **class D — never chmod'd**, because the daemon recreates them on every start and `StateDirectoryMode=0700` (§4.2) is the real fix | **yes**, for fmonero's own files only | `RunArgv` chmod, with the prior modes as `inverse_argv` |
| `ports_owned` | security | warn | D | who listens on 18080 / 18081 / `wallet.port`, read **through `paths.proc_root`** (`<proc_root>/net/tcp`, `<proc_root>/*/fd`) so the §7.1 fixture drives it: `pass` when it is our unit's PID or nothing, `warn` when it is another process, **`fail` when anything is bound to a non-loopback address** | no | detail names the PID and the bind address |
| `disk_space` | storage | warn | D | free bytes on `node.data_dir`'s filesystem ≥ 120 GB for a pruned chain (§1.3), measured locally — **never** from `get_info.free_space`, which a restricted node reports as `2^64-1` (§1b.3). **`skip:remote_node` in remote mode**: the floor is a chain floor and remote mode stores no chain; warning about 120 GB on a machine that will never hold a block is the alarm that teaches people to ignore the group | no | detail says the floor is an **estimate, AMBER**, and names the measured free space |
| `prune_configured` | storage | info | D | `prune-blockchain=1` in our conf when `settings.node.prune`; **`skip:remote_node` in remote mode**, where the conf is rendered but unused | no | `fmonero setup --units` |
| `binary_on_path` | install | warn | D | `~/.local/bin/fmonero` resolves (`~/.cargo/bin` is not on the shell's PATH, §1.5) | no | `scripts/install.sh --yes` |
| `state_valid` | install | warn | A | `state.json` parses | **yes** | rewrite the default |
| `settings_valid` | install | warn | A | `config.toml` parses and every key is in the §4 table | **yes** | rewrite the invalid key to its default |
| `history_cache_fresh` | install | info | **D** | `history::header` (§5b) — magic/version/fingerprint match and the high-water height is within 1 000 of the wallet's | no | `fmonero wallet history --limit 1   # rebuilds the snapshot` — a stale header is already ignored and refetched (§5b), so the code recovers on its own and the doctor does not need a delete verb to say so (§7) |
| `plugin_linked` | install | warn | B | `~/.config/omarchy/plugins/oliver.monero` resolves **into this repo's main checkout** (validate the resolved path). Note the blind spot this shares with the plugin gate: the link resolves to the main checkout, never to a Phase 2 worktree — which is why the plugin agent works in the main checkout (§8). | **yes** | `fmonero setup --plugin` |
| `plugin_enabled` | install | warn | B | the id is in `shell.json` per `omarchy plugin list` | **yes** | `omarchy plugin enable` + `omarchy bar put` |
| `menu_entries` | install | info | B | every id from `assets/menu.jsonc` is present | **yes** | `fmonero setup --menu` (idempotent; the check asserts re-running is a no-op) |

**Every fixable row names the `Action` it emits**, so no agent has to guess between `units::install`
and `render_* + WriteFile` — a guess that decides whether `doctor undo` works at all:

- `units_present` · `unit_loopback` · `unit_no_node_wants` · `wallet_dir_matches_unit` ·
  `wallet_port_consistent` · **`wallet_daemon_matches_unit`** (v5.2) → `WriteFile{unit path, units::render_*(ctx), 0o644}` **followed by**
  `RunArgv{["systemctl","--user","daemon-reload"], inverse: the same}`. A unit rewritten without a
  reload is a fixer that reports success and changes nothing.
- `monerod_conf_mode` → `RunArgv{["chmod","600",path], inverse: ["chmod","<old>",path]}`.
- `settings_valid` → `WriteFile{config_file(), Settings::to_toml_bytes(&repaired), 0o600}`.
- `state_valid` → `WriteFile{state_file(), StateDoc::default().to_bytes(), 0o600}`.
- `wallet_dir_exists` → one `CreateDir{dir, 0o700}` per missing ancestor.
- `plugin_linked` → `CreateSymlink{plugin_link(), the repo's plugin dir}`.
- `plugin_enabled` · `menu_entries` · `unit_enabled_state` · `state_perms` → `RunArgv` with the
  documented inverse.

**No fixer calls `Settings::save`, `state::with_state` or `units::install`**: all three write the
file themselves, which bypasses the backup, the sha256 pair and `actions.jsonl` — and therefore
`doctor undo`. That is why §5b exposes `Settings::to_toml_bytes`, `StateDoc::to_bytes` and
`state::load`: a fixer that has no byte source cannot be routed through the chokepoint, and §7's own
argument is that such a fixer gets written around it. `disk_space` measures through
`fsinfo::free_bytes` (§5b), never `get_info.free_space`.

**Remote mode is a precondition like any other, and `skip:remote_node` is how it is reported**
(v5.2, §2m.3). The full set that skips: every `node` check except `node_layout` (which passes and
names the address) and `remote_node_untrusted` (which is only meaningful there);
`wallet_daemon_link` and `wallet_daemon_auth`; `node_auth`; `disk_space` and `prune_configured`.
`monerod_installed` warns rather than fails. Everything else — the `units` group including the new
`wallet_daemon_matches_unit`, `monerod_conf_mode`, `state_perms`, `ports_owned`, the whole `install`
group and every remaining `wallet` check — runs unchanged, because remote mode moves the daemon, not
the machine. In particular **`unit_loopback` is unchanged and must stay so**: it asserts the two
units' own `--rpc-bind-ip`, which is still `127.0.0.1` in both modes and has nothing to do with the
address they dial (§2c).

Every check whose precondition is missing reports **`skip:<reason>`** with the reason spelled out —
`skip:monerod_not_installed`, `skip:node_unreachable`, `skip:wallet_closed`, `skip:node_restricted`,
`skip:remote_node`, `skip:local_mode`
— never `pass`. On this machine **in the default local mode**, `doctor --json` must report `monerod_installed` and
`wallet_rpc_installed` as `fail:not_installed`, every `node`/`wallet`/`storage` check as `skip`, the
`install` and `security` groups with real verdicts, and **exit 11**; that is the Phase 2 doctor gate
and the §4b GREEN row, and v5.2 leaves it exactly as it was. Remote mode is a **second** §4b row
with its own expectation (§4b), not a change to this one.

**Fixtures where parsing happens, a table test where it does not.** A captured-input fixture in
`tests/fixtures/doctor/<check-id>/{input/,expect.json}` is required for the seven detectors that
parse structured text: `node_layout`, `node_data_dir_single_writer`, `ports_owned`,
`units_present`, `unit_no_password_in_argv`, `settings_valid`, `history_cache_fresh` — the first two
driven by `FMONERO_PROC_ROOT` and `FMONERO_SYSTEMCTL_FIXTURE` (§2i). The rest are
one-line predicates covered by a single table-driven unit test in `checks.rs`.

---

## 7a. TUI (`fmonero tui`) — one agent, one pass, ≤ 4 700 lines across `src/tui/{mod,theme,screens,send,subs}.rs` (`tests.rs` excluded)

**The budget is the measured figure, and it is a ceiling against growth rather than a target**
(v5.1). v5 asked for 1 800 — fproton's 1 200 plus a fourth screen, a send state machine, two modals
and a QR canvas — and the implementation came in at 4 557 lines, which the Phase 2 TUI gate, the
Phase 3 ledger and the Phase 4 fixer each reported (`ux-19`) and none closed. fproton's own TUI
missed its 1 200-line budget by the same 2.2× ratio, and **every screen, key and modal §7a specifies
is implemented** — so the honest lever was the number, not the feature set: closing `ux-19` by
deleting a specified screen would have traded a missed budget for a missing feature. 4 600 was
4 557 rounded up; **v5.2 raises it to 4 700, measured 4 655**, and says so here before the code was
written, which is the rule the sentence after this one sets. The 98 lines are §2m's Node screen:
the remote hero this section's own table specifies (address, trust flag and ssl mode, the wallet's
height and last refresh, one dim line of explanation, and **no** gauge — every field one would need
is `null`), the `u`/`d`/`L` toast, the `?` overlay's four marks, and the three-line `NodeGate` arm in
`subs.rs`. Nothing was deleted to fit: `scripts/check.sh` fails above the ceiling, and the next
agent to add 2 000 lines the way these five files got their first 2 600 has to say so in this
document first. `theme.rs` (≈200 lines)
is copied verbatim from fproton and does not count against the design effort, only the budget;
`tests.rs` is test code and is excluded by name, as the gate is.

**Runtime.** The ftui 0.7 Elm loop: one `struct Tui` implementing `ftui::Model`;
`App::fullscreen(tui).run()`; no mouse in v0.1. It owns a `Context` clone and calls `fmonero-core`
**in process** — it never spawns `fmonero`. Reference code: `fproton/crates/fproton/src/tui/{mod,
screens,subs,theme}.rs` and `~/.cargo/registry/src/*/ftui-0.7.0/examples/`. Widget names: §1.4.

**Chrome (every screen).** Row 0: tab strip `Node · Wallet · Send · Log` (`widgets::Tabs`). Rows
1..h-2: the screen. Row h-1: `widgets::StatusLine` =
`[node badge] [height] [peers] [wallet badge] [balance] │ [spinner + action label when busy] │ ? help`.
Badge colours: green for synced / wallet open, yellow for syncing / wallet closed, dim for down,
red for `state == error` or a `pending_send`.

| Screen | Data | Shows |
|---|---|---|
| **Node** (default) | `core::status(&ctx)` every 1 s; `node info` extras on entry and on `r` | Hero: sync `MiniBar` with percent and `height / target_height`, ETA from two samples, nettype, version, hard-fork version. Detail: peers in/out with a `Table` of the top 10 connections (`get_connections`: host, state, height, pruning seed, up/down rate), a 60-sample `Sparkline` of `rx_bps`, mempool (txs, bytes, oldest, fee total), fee estimate with the four `fees[]` levels, net limits. On a restricted node every pane that needs a gated method renders "not available on a restricted node" with the method name — never a zero, and never a poisoned `get_info` field (§5). On a `system_unit` layout the hero adds "Node owned by the system unit — read-only" and `u`, `d`, `L` and the ban actions are disabled with the §2i text. **On a `remote` layout (v5.2, §2m) the screen is a different, much smaller one** and the 1 s poll makes no node call at all: the hero reads `Remote node · <address>` on one line, `untrusted` or `trusted` and the `ssl` mode on the next, and — when a wallet is open — `wallet <height> · last refresh <age>` from `status.wallet.height` and `status.wallet.last_refresh_at`. Under it, one dim line, verbatim: `fmonero does not talk to this node — monero-wallet-rpc does. Its own sync state cannot be seen from here.` Every pane that needs a gated method (peers, sparkline, mempool, fees, net limits) renders the same "not available" treatment a restricted node gets, with the reason `remote node` instead of a method name. **No sync `MiniBar`, no percent, no ETA**: all three are `null` in remote mode and a gauge at 0 % is the rendered untruth §2j forbids — the same reason §6.4 puts the remote rows above the node rows. |
| **Wallet** | `get_balance`, `get_address`, `history::query` | Header: balance / unlocked / `locked for N blocks (≈ M min)`. Left: subaddress `Table` built from `get_address.addresses[]` (index, label, address, used), with per-row balances joined in from `get_balance.per_subaddress` where present and `—` where absent (that list holds only funded subaddresses, §5); `Enter` = copy, `a` = new address, `l` = label. Right: a `VirtualizedList` of history rows (date, type badge, amount, fee, confirmations, note) with `/` filter and `t` cycling the type filter (all/in/out/block/pending/failed/pool). **`confirmations` and `locked` come from `history::confirmations` / `history::locked` (§5b) and are never recomputed here** — `output.rs` and this screen are two independently written callers of the `unlock_time` height/timestamp formula (§2a.3). `Enter` on a row opens a detail pane (txid, `destinations[]` from `Transfer::destinations` — §5b carries the field precisely so this pane can be built — payment id, unlock time, `subaddr_index` as `major/minor`, double-spend flag) and `n` edits its note. The `locked` column is labelled `locked (est.)` and shows `—` rather than a value on a `--cached` read (§4.1). |
| **Send** | the §7a state machine | Three `TextInput`s (To, Amount, Note), a priority chip row showing the node's own `fees[]` for each level (`0` is drawn as an alias of the lowest chip, §1b.4), an account selector, and a live validation line. **One transaction only — sweep is CLI-only in v0.1** (§0): `all` in the Amount field is refused inline naming `fmonero wallet sweep`. |
| **Log** | `node log` tail | `ftui`'s log viewer over the tail of `settings.node.log_file`, `/` to search, `f` to toggle follow, `L` to cycle the node's `set_log_level` 0–4 (needs `--yes`-equivalent: a confirm modal, because it is a mutation). |

**In remote mode `u`, `d`, `L` and the ban actions raise a toast instead of an action**, reading
`remote node mode — run: fmonero node local --yes`, and the `?` overlay marks those four keys
`(remote mode: disabled)` from the same table. The toast names a command rather than describing a
state because §9's "One Rule" applies to the TUI as much as to the CLI, and because it is the only
place a TUI user learns the mode is reversible. The TUI does **not** offer `node remote` /
`node local` itself: they restart a daemon and close an open wallet (§2m.5), and a modal that does
that from inside a session holding that very wallet is a screen this document would have to specify
a recovery for. Same reason §7a gives for cutting sweep — an affordance with no specified confirm
layout is an under-specified screen invented by the TUI agent.

**Keymap**, which is also the exact content of the `?` overlay — one `&[(&str,&str)]` table so help
and behaviour cannot drift: `q`/`Ctrl+C` quit (refused with a status-line note while an action is in
flight; a second press within 2 s cancels and quits) · `?` help · `Tab`/`Shift+Tab`/`1 2 3 4`
screens · `r` refresh · `Esc` close overlay, else cancel the in-flight action · **Node:** `u`/`d`
node up/down (both through a confirm modal), `b` bans, `L` log level · **Wallet:** `o` open wallet
(modal), `c` close, `a` new address, `l` label, `n` note, `R` wallet refresh, `Q` show the selected
address as a QR, `/` filter, `t` type filter · **Send:** `Tab` next field, `Enter` build,
`p` cycle priority, `@` pick from the address book (no sweep key: §0 cuts sweep from the TUI) ·
**Log:** `/` search, `f` follow.

**Subscriptions.**
`Every(1 s)` → `Msg::Poll`, run in a `Cmd::task`, **dropped if one is in flight** (`poll_inflight`,
§2d). 1 s is the cadence fproton's TUI already uses and it sits well below perception against a
two-minute block time, so nothing visible is ever more than a second stale. `Every(120 ms)` →
`Msg::Tick`, which is a **spinner frame budget, not a data poll** — it touches no daemon — used for
the busy indicator and the QR's blink-free redraw.
Long actions are a **cancellable `Subscription`, never a `Cmd::task`**, because a `Cmd` cannot be
cancelled — fproton's `ActionSub` shape (`fproton/crates/fproton/src/tui/subs.rs`), with one
deliberate difference. fmonero's `Action` is **not** `Clone` (it carries a password and a `Plan`
that owns the §2a.11 lock), while `Subscription::run` takes `&self`
(`ftui-runtime-0.7.0/src/subscription.rs:46`) and fproton's `subscriptions(&self)` therefore
constructs its `ActionSub` with `action: action.clone()` (`fproton/crates/fproton/src/tui/mod.rs:740`).
From a shared borrow nothing can be moved out, and `send::relay` takes its `Plan` **by value**, so
the action is held behind interior mutability and moved out once: the model holds
`action: Option<(SubId, Arc<Mutex<Option<Action>>>, Instant)>`, `subscriptions()` clones the `Arc`
and never the `Action`, and `run` does
`let Some(action) = self.action.lock().unwrap().take() else { return; };` before consuming it —
which is also exactly what makes "the variant is dropped and its `String` zeroed as soon as the
subscription takes it" true rather than aspirational. The runtime de-duplicates by `SubId` and does
not restart an unchanged subscription, so `run` is entered once per action; an empty slot returns
without sending a message. Without this the three constraints — `run(&self)`, a by-value `Plan` and
a non-`Clone` `Action` — cannot all hold, and a Phase 2 agent resolves it in its first hour by
deriving `Clone` (a second copy of the password and of the lock's owner) or by rewriting a frozen
signature.

```rust
pub enum Action {
    NodeUp, NodeDown,                // -> node_session::up / down (§5b) — never units::start
    WalletOpen { name: String, password: String },
    WalletRefresh { start_height: u64 },
    BuildTx(SendRequest),            // -> send::build  (§5b)
    Relay(send::Plan),               // -> send::relay — the SAME plan (§2a.3)
}
```

**`Action` derives neither `Debug` nor `Clone`.** `WalletOpen` carries the wallet password, and the
zeroing rule below is only true while exactly one copy exists — a derived `Clone` leaves a second
heap allocation nobody zeroes, and a derived `Debug` (the reflex on an ftui action enum) puts the
password into any panic message, any `dbg!` and any future log line, none of which §2b's key-name
grep reads, because that grep reads command output. `Debug` is **hand-written** and prints
`WalletOpen { name, password: "<redacted>" }`, exactly as §5b does for `BuiltTx::metadata`; the
model's `action` field is formatted through it. *Test:* a unit test asserts
`format!("{:?}", Action::WalletOpen { name: "w".into(), password: "hunter2".into() })` does not
contain `hunter2`, and `scripts/check.sh` greps `crates/fmonero/src/tui` for a `derive(…Clone…)` or
`derive(…Debug…)` on `Action`.

`Action::NodeUp`/`NodeDown` call `node_session::up`/`down` (§5b), **not** `units::start`/`stop`: the
§2i single-writer precondition — the only guard against two `monerod` on one LMDB (§1.2) — exists in
exactly one place, and the TUI's copy of it would be the one nobody reviews.

**There is no `Sweep` variant, and that is a decision rather than an omission.** Sweep is CLI-only
in v0.1 (§0): the Send screen builds exactly one transaction, typing `all` in the Amount field is
refused inline with
`sweep is not available in the TUI — run: fmonero wallet sweep <addr> --dry-run`, and the `?`
overlay says the same. `send::build_sweep` therefore has one caller (`wallet sweep`) and the confirm
modal never has to render a transaction list. A frozen `Sweep(SweepRequest)` with no key, no
affordance and no specified multi-transaction confirm layout would be either dead code Phase 1b must
still write and test — the thing §5b rule 1 forbids — or an under-specified money screen invented by
the TUI agent, and it is the screen that empties an account.

`Action::WalletOpen` runs `wallet_session::open` (§5b), **not** an open sequence of its own: the
`set_daemon` step whose omission means the wallet can never sync (§4.2) exists in exactly one place.

**The TUI never calls `WalletClient::relay_tx`, never calls `build_transfer`/`build_sweep_all`, and
never writes the `pending.json` record itself.** Those are `send::build` and `send::relay`, which the CLI
also calls; §2a's guarantees are properties of core, not of two independently written callers.

The model holds `action: Option<(SubId, Arc<Mutex<Option<Action>>>, Instant)>` (above) and
`subscriptions()` returns the `ActionSub` only while it is `Some`. `Esc`/quit sets it to `None` → `StopSignal` → `CancelToken` →
core returns `Interrupted`. **`Action::Relay` is the one action that ignores the StopSignal**: once
`relay_tx` is on the wire, cancelling it tells you nothing about whether the node got it, and
the `pending.json` record (§2a.4) is the mechanism that covers it. The help overlay says so.
`Action::WalletOpen` is the only place a password lives in the model, and the variant is dropped
(and its `String` zeroed) as soon as the subscription takes it.

**The send state machine** — the load-bearing screen. States and the only legal transitions:

```
Form ──validate──▶ Validated ──▶ Building ──▶ Built ──confirm+Enter──▶ Relaying ──▶ Sent
  ▲                    │             │           │                        │           │
  └──── edit ──────────┘   Esc ──────┘           └── Esc: discard ────────┘           └─▶ Form (toast)
                                                 └── Esc while Relaying: refused, with a note
```

1. **Form.** Each keystroke runs `address::parse` and `Piconero::parse` locally and paints the
   validation line green/red instantly. Nothing is sent.
2. **Validated.** `Enter` runs `validate_address` (one RPC, in a `Cmd::task`) and checks the
   nettype against the wallet's; a failure returns to Form with the reason.
3. **Building.** `ActionSub(Action::BuildTx(SendRequest))` → `send::build`, which runs the
   `validate_address` nettype check, the §2a.4 entry check and the `synchronized` gate before
   building once with `do_not_relay: true, get_tx_metadata: true`. The status line shows
   `building… Esc to cancel` with the elapsed seconds against the 120 s deadline.
4. **Built → the confirm modal**, which is the whole safety surface and therefore is spelled out
   here: a bordered `Dialog` showing **To** (the full address, wrapped, never elided — an elided
   address is how you send to the wrong one), **Amount** in XMR and in piconero, **Fee** in XMR and
   in piconero *from the built transaction*, **Total**, **Account**, **Priority** and **Weight**;
   then the literal line `Type "send" and press Enter to broadcast · Esc to discard`. The modal
   requires typing the word, not just Enter — this is the TUI's `--yes`.
5. **Relaying.** `ActionSub(Action::Relay(Plan))` → `send::relay`, passing an `announce` closure
   that appends to the Log screen's pane (§5b — core never prints, §2g; the CLI's sink is stderr).
   `relay` announces the built recipient/amount/fee/total/priority/account, writes and fsyncs
   the `pending.json` record, relays, compares each returned `tx_hash` with the built txid (§2a.9), marks
   each entry relayed and resolves the record. On success: a toast with the txid, jump to Wallet with
   that row selected, and the form clears. On failure: the record stays, the error is shown with its
   `code_name`, and the Send screen refuses new builds until the record resolves or
   `wallet clear-pending` runs.

**Modals.** *Open wallet:* a wallet-name `VirtualizedList` (root re-export, §1.4) from
`wallet_dir::list(&ctx)` — the core function, not the subcommand — then a masked
`TextInput::with_mask('•')` (§1.4) whose buffer is overwritten with zeroes on close (§2b.4); `Esc` cancels,
`Enter` starts `Action::WalletOpen`, which runs `wallet_session::open` (§5b) so the modal cannot
omit or reorder the `set_daemon` step. *Confirm:* used by node up/down, log level and the send
confirmation; it renders the exact operation and needs `y` (or the typed word, for send).
*QR:* the selected address drawn with `qrcode` onto an `ftui_extras::canvas::Canvas` in half-block
mode, with the address text beneath it; `Esc` closes.

**Empty, error and hang states — each must render, none may be blank:** no `monerod` binary →
"Monero is not installed. `omarchy pkg add monero`, then `fmonero setup --units`"; node down →
the Node hero becomes "Node is not running — press u to start it"; node restricted → each gated
pane names the method it needs; **remote mode → the Node screen's remote hero above, never a blank
pane and never a zeroed gauge**; wallet daemon down → "Wallet daemon is not running (press o to
start it and open a wallet)"; wallet closed → "No wallet open — press o"; syncing → the gauge, the
percent and the ETA, never a spinner alone; a §2d breaker armed → a persistent yellow row naming
the seconds left, and `u`/`d`/`R` refused with the same text; `pending_send` → a red banner on
every screen with the txid and the two commands; terminal too small → §7a resize.

**Resize.** `view()` returns immediately on a zero area.

| Tier | Width | Layout |
|---|---|---|
| Wide | ≥ 100 cols | Node: two columns (hero + peers table \| sparkline + mempool + fees). Wallet: subaddresses \| history side by side. Send: form and a live preview pane. |
| Medium | 60–99 | single column, tables lose their least important columns (peers drops pruning seed and rates; history drops fee and note), no sparkline. Send: form only. |
| Narrow | < 60 or < 20 rows | status line + hero + the one focused list; tabs collapse to `1·2·3·4`. The send confirm modal still renders in full — it is never abbreviated. |
| Tiny | < 40×10 | one line: `fmonero — terminal too small (need 40x10)` |

One render test per tier per screen draws into a `Buffer` and asserts no panic and that the badge
is present; a `ProgramSimulator` test drives Form → build → confirm → relay against the mock and
asserts exactly one `transfer` and one `relay_tx`; a tmux smoke script opens the TUI on a real pty,
sends `1 2 3 4 ? q`, and asserts a clean exit.

**Theme.** `theme.rs` copied verbatim from fproton (reads
`~/.local/state/omarchy/current/theme/colors.toml`, neutral fallback) plus one fmonero-specific
`fn badge(&self, s: &str) -> Style`. **No literal colour appears anywhere in `src/tui/**` outside
`theme.rs`.** Re-read when the theme file's mtime changes, checked on the 1 s poll.

**Exit code.** `Ok(0)` on a clean quit; if the session's last action failed, that error's own
`exit_code()` from §4a; `Ok(130)` on Ctrl+C. The TUI never invents an exit code.

---

## 8. Phases, ownership, exit gates

Every phase names its owner, its inputs, its exact deliverables, the commands that close its gate
and a time budget. **File ownership is disjoint**: an agent that needs a file it does not own stops
and reports. Commit messages are `<area>: <what>` with the trailer
`Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.

**Remote node mode (§2m) is a post-Phase-4 amendment, not a tenth phase.** Phases 0–4 are history;
v5.2's work is one agent on `main`, in the main checkout, with the same gates and the same
`DECISIONS.md` discipline as the fix-up pass that produced v5.1. Its deliverable list is mechanical
because this document is the spec: §5b.1's nine signature changes (each recorded in `DECISIONS.md`
in the commit that makes it), the §4.1 rows, the §5/§5c shapes, the two §7.1 checks, the §6 and §7a
changes, §2m.7's README section, and §4b's new GREEN, AMBER and LIVE rows filled with real output.
`Cargo.toml` is **not** touched (§2c, §2k): remote mode adds no dependency. Its gate is the Phase 3
list — `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
`scripts/check.sh --full`, `tests/e2e.sh --fast` and `--full`, `tests/plugin-smoke.sh`,
`tests/tui_tmux.sh`, the four non-negotiable greps — plus the §4b LIVE run against a real public
node, and the same closing rule the fix-up inherited: an assertion is never relaxed to make a test
pass, and the suite leaves the machine exactly as it found it (`node local --yes` is the last line
of the LIVE rehearsal for that reason).

**`DECISIONS.md` is written by one agent per phase, and never by two in parallel.** Phases 1a, 1b, 3
and 4 append to it directly; whichever phase is currently running on `main` owns it. Each **Phase 2**
agent instead writes its evidence to `decisions/phase2-<name>.md` — a file only it owns, which
cannot conflict — and **Phase 3, as its first step after the four merges**, concatenates the four in
the order docs, plugin, doctor, tui into `DECISIONS.md` under one dated heading, then deletes them.
File ownership is disjoint everywhere else in §8 deliberately, and the one shared-write file was
also the one whose conflicts no build gate can catch: two parallel Phase 2 agents appending to the
tail of an append-only file stop Phase 3's one-at-a-time merge loop at `git merge --no-ff`, in a
file whose resolution is "keep both, in some order" — pure ceremony that a tired integrator resolves
by deleting one side's evidence.

**Worktrees, as commands rather than as an instruction.** Every phase but 0 and 2 works on `main` in
the main checkout. Phase 0 and each Phase 2 agent run, from the main checkout:

```bash
git worktree add ../fmonero-mock   -b phase0/mock   main      # Phase 0, alongside 1a
git worktree add ../fmonero-docs   -b phase2/docs   gate-1a
git worktree add ../fmonero-tui    -b phase2/tui    gate-1b
git worktree add ../fmonero-doctor -b phase2/doctor gate-1b
git checkout -b phase2/plugin gate-1b     # plugin: main checkout, no worktree (below)
```

The bases are the annotated tags the 1a and 1b gates create as their last step, not a description
of a commit: `docs` starts while 1b is still committing to `main`, so "main at the 1a gate" would
have it silently basing on a moving tip. Each agent
works only inside its own tree, commits before returning, and does not push.

**The plugin agent is the exception: it works in the main checkout, on branch `phase2/plugin`.** Its
gate requires `tests/plugin-smoke.sh` green with the plugin symlinked and enabled on this machine,
and the installed symlink is
`~/.config/omarchy/plugins/oliver.monero → ~/Projects/odcpw/fmonero/plugin/oliver.monero` — the main
checkout. From a worktree its QML edits would be invisible to `omarchy-shell shell rescanPlugins`
and its own gate would test someone else's files, which it would discover after three hours of QML
that never loaded. It still touches only its owned files, and it is scheduled so that no other agent
holds `plugin/` at the same time.

### Phase 0 — fixtures and the mock server (one agent; starts with 1a in its own worktree, gate runs after the 1a gate; ~90 min)

**Owns** `tests/fixtures/**` and `crates/fmonero-mock/src/**`.
`crates/fmonero-mock/Cargo.toml` and the workspace `members` list are **Phase 1a's** (§3.1): the two
ownership sets must be disjoint (§2k), and Phase 0's gate command `cargo build -p fmonero-mock`
cannot resolve until 1a has written the workspace manifest.

**It works in its own worktree** — `git worktree add ../fmonero-mock -b phase0/mock main` — so that
two agents never share one working tree. It writes fixtures and mock source while 1a runs, rebases
onto `gate-1a` when that tag lands, runs its gate there, and **Phase 1b merges `phase0/mock` before
its own first commit**. Two things make the worktree necessary rather than tidy: 1a's gate runs a
bare `cargo build --release` and `cargo clippy --all-targets`, which build *every* workspace member,
so in a shared tree 1a's build would see whatever half-written mock source Phase 0 happened to have
on disk at that instant; and §8's preamble is careful about concurrent working trees everywhere
else.

**The cycle that made this a real blocker, and how it is broken.** §3.1's `members` list names
`crates/fmonero-mock`, so 1a's bare `cargo build --release` cannot succeed until
`crates/fmonero-mock/src/main.rs` exists — which is Phase 0's file, and Phase 0 cannot build until
1a has written the manifest. **Phase 1a therefore also writes `crates/fmonero-mock/src/main.rs`
containing exactly `fn main() {}`**, the placeholder that lets the workspace build, explicitly
handed over for Phase 0 to overwrite wholesale (it is test-only and never shipped, so §2h does not
apply). One line, no ownership boundary moved, and a gate command that can actually execute — the
very first command of the very first phase.

**Inputs:** §1b of this file and the Monero source at §1.6.

**Deliverables.**

1. `tests/fixtures/node/*.json` — `get_info.json`, `get_info-syncing.json`,
   `get_info-restricted.json` (the poisoned fields of §1b.3, so the §5 null rules are testable),
   `get_last_block_header.json`,
   **`get_last_block_header-syncing.json`** (`{"block_header":{…every field 0…},"status":"BUSY",
   "untrusted":false}` — the `CHECK_CORE_READY()` answer of §1b.3, selected by `--scenario syncing`,
   without which nothing in this plan can fail on a 56-year-old block),
   `sync_info.json`, `get_connections.json`, `get_peer_list.json`,
   `get_bans.json`, **`get_transaction_pool_stats.json`** (the wire name — the json_rpc method and
   the legacy endpoint are both `get_transaction_pool_stats`, §1b.3; naming it after
   `NodeClient::get_txpool_stats` would be a silent 404 that this phase's `get_info` smoke test
   cannot catch), `get_fee_estimate.json`, `get_version.json`,
   `hard_fork_info.json`, `get_net_stats.json`, `get_limit.json`,
   **`set_limit.json`** (echoes `limit_up`/`limit_down`), **`out_peers.json`** and
   **`in_peers.json`** (each echoing the count), **`set_bans.json`** (the bare
   `{status, untrusted}` base) — the four without which `node limit --up 1024`,
   `node limit --out-peers N`, `node ban` and `node unban` are AMBER rows with no response to serve
   and no way to fill §5c's `limit_up_kbps`/`out_peers`/`in_peers`/`banned` keys —, and
   `error-restricted.json` (`{"jsonrpc":"2.0","id":"0","error":{"code":-32601,"message":"Method not found"}}`).
   **Fixture file names are exactly the json_rpc method name or the legacy endpoint name**, so the
   dispatcher needs no translation table.
2. `tests/fixtures/wallet/*.json` — `get_balance.json` (with `per_subaddress`, `blocks_to_unlock`
   and `time_to_unlock` populated), `get_address.json`, `get_accounts.json`, `get_height.json`,
   `get_transfers.json` (at least one row of every `type`: in, out, **block**, pending, failed,
   pool — the `block` row is a coinbase incoming row inside the `in` list, §1b.4, and without it no
   test in this plan would ever produce the row that breaks a five-variant `TransferKind`),
   `get_transfer_by_txid.json`, `transfer-do-not-relay.json` (a `single_transfer_response` with a
   **non-zero `fee`** and a `tx_metadata` of plausible hex), `relay_tx.json`, `sweep_all.json`
   (a `split_transfer_response` with **two** transactions, so the list path is exercised),
   `validate_address-*.json` (valid standard, valid subaddress, valid integrated, invalid,
   wrong-nettype), `get_address_book.json`, `get_version.json`, and
   `error-not-open.json` (`{"error":{"code":-13,"message":"No wallet file"}}`),
   `error-not-enough-money.json` (`-17`), `error-wrong-address.json` (`-2`).
3. `tests/fixtures/address-forms.json` — one object per row of `address::PREFIXES` (§5b):
   `{net, kind, prefix, leading, len, example}`, where **`leading` is an array** — testnet standard
   is `["9", "A"]` and every other form has one element (§1b.8). The nine forms.
   **This is the file that makes the FCMP++ fork a one-line change** (§1b.1).
4. `tests/fixtures/doctor/<check-id>/{input/,expect.json}` for the seven parsing detectors of §7.1,
   including a synthetic `/proc` tree for `node_layout` and `node_data_dir_single_writer` and a
   `/proc/net/tcp` sample for `ports_owned`.
5. `tests/fixtures/README.md` — per-file provenance, in fproton's format
   (`fproton/tests/fixtures/README.md`), with the captured/derived distinction stated up front and
   the exact `file:line` each derived field came from.
6. **`crates/fmonero-mock`** —

   ```
   fmonero-mock --role node|wallet --port N --fixtures DIR --log FILE
                [--restricted] [--auth USER:PASS] [--delay-ms N] [--rotate-nonce-after N]
                [--scenario NAME] [--fail METHOD=CODE:MESSAGE]…
   ```

   Serves `POST /json_rpc` by dispatching on `method`, and the legacy `POST /<name>` endpoints,
   from the fixture directory. Every flag exists because without it something in §4b cannot be
   tested:
   - **`--role`** selects the `node/` or `wallet/` subdirectory, so `get_version` and `get_height`
     — which both daemons answer and which have a fixture under both — are unambiguous. A test that
     needs both daemons runs two instances.
   - **`--scenario NAME`** makes a method prefer `<method>-<scenario>.json` when it exists and fall
     back to `<method>.json`; `--scenario syncing` and `--scenario restricted` are what select
     `get_info-syncing.json` and `get_info-restricted.json`.
   - **`--delay-ms N`** delays every answer by N ms. §2a.11's two-process test needs it: against an
     instant mock the first `send --yes` can finish build → relay → resolve before the second reaches
     `flock`, and the only test of the interlock that can double-spend becomes a coin flip.
   - **`--fail get_balance=-13:'No wallet file'`**, repeatable, makes that method answer that
     JSON-RPC error until the process exits. **This is the only way the §1b.4 error-code → exit-code
     table can be tested**, and that table is a money surface; `--restricted` covers only `-32601`.
   - **`--restricted`** answers `-32601` for every gated method of §1b.2 and 404 for every gated
     legacy endpoint.
   - **`--auth`** implements the epee Digest challenge of §1b.7 — **two** `WWW-Authenticate`
     headers, `algorithm=MD5` and `MD5-sess`, the per-nonce `nc` counter, and a nonce rotation on
     demand — so the client's selection and reset logic is actually tested.
   - **`validate_address` dispatches on the request's `address`**: the fixture whose
     `_request_address` matches, else `validate_address-invalid.json`.
   - **`--log FILE` is required**, and appends one JSON object per request
     (`{seq, ts_ms, path, method, body}`). §2a's central tests read it.

   - **The mock speaks HTTP/1.1 keep-alive**, and this is not an optimisation. It sends
     `Content-Length` on every response, never `Connection: close`, and serves successive requests
     on the same accepted socket until the client closes it. The Digest session (`realm`, `nonce`,
     server-side `nc`) is **per connection**, mirroring epee's `m_auth`
     (`http_protocol_handler.h:186`, §1b.7); a new connection starts with no nonce and answers 401
     once. `--rotate-nonce-after N` issues a fresh nonce after N authorized requests on a
     connection, which is what drives the §2c `nc`-reset test. Without keep-alive the §2c
     assertion that "the second request on one nonce carries `nc=00000002`" is **unwritable**:
     an accept-respond-close loop makes `ureq` open a fresh connection per request, the client
     correctly resets `nc` to 1 every time, and the only test of the one non-obvious property of
     `digest.rs` — the property whose absence silently doubles the §2l budget — quietly disappears.

   The mock is **stateless** apart from the per-connection Digest session and the request log:
   `open_wallet` is recorded and answers `{}`; it does not gate later calls, so a `-13` path is
   produced with `--fail`, not with state.

   - **The accept loop spawns one std thread per accepted connection**, because two fmonero
     processes must be able to talk to one mock. §2a.4's concurrency test runs `status` in a loop
     while `send --yes` relays, and §2a.11's runs two `send --yes` at once; fmonero's agent holds an
     idle pool of one and keeps its socket for the life of the process (§2c), so a single-threaded
     accept loop parks every second client in the listen backlog behind a connection that is never
     closed — the test hangs or flakes, and it is the test that proves `pending.json` survives
     concurrent writers, the one interlock that can double-spend. "A test that needs two clients
     runs two mock instances" does not rescue it: both processes read the same
     `settings.wallet.url`, so they cannot be pointed at different ports. The Digest session
     (`realm`, `nonce`, server-side `nc`) stays **per connection**, i.e. per thread, mirroring
     epee's `m_auth` (`http_protocol_handler.h:186`); the only shared state is the request log,
     behind one `Mutex<File>` so the `{seq, ts_ms, path, method, body}` lines stay whole and
     ordered. That is the whole of the mock's concurrency — ten lines.

   Under 500 lines, no dependencies beyond `serde_json` and the std library.

**Why a Rust binary and not `mock_rpc.py`.** Three reasons, in order of weight: (a) the Digest
challenge and the request log are the *point* of the mock, and writing them twice — once in Python
for `e2e.sh` and once in Rust for the in-process unit tests — is two implementations that can
disagree about the thing we are testing; (b) a Python mock puts a second language and a Python
version in the gate, on a machine where `check.sh` otherwise needs only cargo and coreutils; (c)
`crates/fmonero-mock/src/main.rs` and the in-process `TcpListener` helper that
`fmonero-core`'s unit tests use (fbittorrent's `src/qbt_tests.rs:29-66` shape) are the same 150
lines of request parsing, shared by making the binary a thin `main()` over a `serve(routes)`
function. It is a workspace member with `publish = false`, is not a dependency of either shipped
crate, and `scripts/install.sh` builds `-p fmonero` only, so it can never reach `~/.local/bin`.

**Gate** (run after the 1a gate, since it builds against 1a's manifest). Every line of it must be
closable by **this** agent at the time it runs — at the 1a gate every `WalletClient` body still
returns `Err(Error::unimplemented(…))`, so anything asserting an fmonero exit code cannot be
proved here and is moved to 1b rather than waved through. Every JSON fixture parses
with `jq -e .`; every derived file starts with the provenance header; `cargo build -p fmonero-mock`
succeeds; `fmonero-mock --role node --port 18099 --fixtures tests/fixtures --log /tmp/m.jsonl &`
followed by `curl -s -d '{"jsonrpc":"2.0","id":"0","method":"get_info"}' 127.0.0.1:18099/json_rpc | jq .result.height`
prints a number; the same with `--restricted` and `method:"sync_info"` prints `-32601`;
`--scenario restricted` makes `get_info` return `restricted: true` and `free_space: 18446744073709551615`;
a `curl` that sends **two** requests over one connection (`curl -s d1 d2 --next`-style, or
`curl --http1.1` with two `-d` bodies to the same URL) is answered on one socket and the log shows
both; **two concurrent `curl` processes against one mock instance both get answers and the log shows
both requests with distinct `seq`** (the thread-per-connection accept loop); the request log has one
line per request; `address-forms.json` has as many objects as
`PREFIXES` has rows (asserted again by a unit test in Phase 1b). **`--fail` is exercised by the
Phase 1b gate, not by this one** — `--role wallet --fail get_balance=-17:x` making
`fmonero wallet balance` exit 14 is an assertion about the §1b.4 error map, which does not exist
until 1b implements it, and an agent that cannot close its own gate either blocks the pipeline for
five hours or marks it green dishonestly, which is the failure §2j exists to prevent.

### Phase 1a — API freeze (one agent; first; ~2 h)

**Owns** all **four** `Cargo.toml` files — the workspace root and one per crate,
`crates/fmonero-mock/Cargo.toml` included (§8 Phase 0); the `members` list names three crates plus
the root manifest (§3.1), and writing three leaves `members` pointing at a directory with no
manifest, so `cargo build --release` — the first command of the first gate — fails before anything
else runs —, `crates/fmonero-core/src/{lib,error,context,paths,settings,
state,proc,amount,address}.rs`, compiling **signature-only** modules for everything else in §5b
(bodies return `Err(Error::unimplemented("…"))` or empty data), `crates/fmonero/src/main.rs` (the
**full** clap tree for every command in §4.1, with every flag, every `after_help`, dispatch into
modules, and the §4a exit-code mapping), `crates/fmonero/src/output.rs` (the JSON/TTY/colour policy
of §4), the `tui::run` / `doctor::run` / `setup::run` stubs with their `*Args`,
`crates/fmonero/assets/menu.jsonc`, **and `crates/fmonero-mock/src/main.rs` containing exactly
`fn main() {}`** — the placeholder that lets the workspace build before Phase 0 lands, explicitly
handed over for Phase 0 to overwrite wholesale (it is test-only and never shipped, so §2h does not
apply, and without it 1a's own `cargo build --release` cannot run at all).

`error.rs`, `context.rs`, `paths.rs` and `proc.rs` are ports of fproton's, adapted to the §5b
names; `amount.rs` and `address.rs` are new and are **fully implemented here, with tests**, because
Phase 2's TUI needs the live validators on day one and they touch no daemon.
**`Settings::get`/`set`/`KEYS` are fully implemented here too** (including the Levenshtein
suggestion of §4), which is what makes this phase's `reason` assertion closable; everything else in
`settings.rs` and `state.rs` beyond the types is 1b's.

**Gate.** `cargo build --release`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`
all green; `fmonero --help`, `fmonero wallet --help`, `fmonero node --help`,
`fmonero doctor --help`, `fmonero tui --help` print the real §4 surface; `fmonero status --json`
prints a schema-1-valid object (mostly null);
**the `reason` key is present and correct on the failure path this phase can reach**:
`fmonero settings set nope 1` prints `"reason":"unknown_key"` (§4 — a frozen `Error` that cannot
carry `reason` is the defect this asserts against). The two `wallet send` exit codes of §2a.1 are
**not** asserted here: both run through `send::build`, whose body at 1a is a signature-only
`Err(Error::unimplemented(…))` → exit 1, and §2a.1 places `address::parse` inside `build` rather
than in `main.rs`. They are §4b GREEN rows and are asserted by the Phase 1b gate's §5c key-set run;
an agent that cannot close its own gate either blocks the pipeline or marks it green dishonestly
(§8 Phase 0). `cargo test -p fmonero-core amount
address` passes, including the
`PREFIXES.len() == address-forms.json.len()` assertion, the testnet-standard two-digit `leading`
case (§1b.8), and **the `to_xmr_string` round trip** —
`Piconero::parse(x.to_xmr_string()) == x` for every sampled `u64`, plus the three worked examples of
§5b;
**the §5b rule-3 round-trip test passes** — every variant of `Layout`, `Severity`,
`PendingKind`, `TransferKind` (`Block` ↔ `"block"` included), `Network`, `NetType`, `AddrKind` and
`FileStatus` asserted against the literal string §5/§5c/§7 promises — the doctor's `Verdict` status
is **not** here: it lives in `crates/fmonero/src/doctor/checks.rs`, which this phase does not own and
which does not exist until Phase 2, whose doctor gate asserts it instead;
**`cargo doc -p fmonero-core` lists every type named in §5b**, which is what makes
"compile against exactly these signatures" true rather than aspirational. **After this gate nobody edits a
`Cargo.toml`** (§2k). **Last step of the gate: `git tag -a gate-1a -m 'API freeze'`** — Phase 2's
docs worktree bases on this tag, and a base commit that exists only as a description ("main at the
1a gate") is a base no agent can type. The docs agent may start.

### Phase 1b — core implementation (one agent; after 1a; ~5 h)

**Owns** the rest of `crates/fmonero-core/src/**` (`rpc/**`, `units.rs`, `history.rs`, `status.rs`,
`agent.rs`, `session.rs`, and the real bodies of `state.rs`/`settings.rs`), plus
`crates/fmonero/src/output.rs`
rendering, `crates/fmonero/src/setup.rs`, `crates/fmonero/src/prompt.rs`, `tests/e2e.sh` and
`scripts/check.sh`. **Its first act is `git merge phase0/mock`**, before its own first commit, so the
fixtures and the mock binary are on `main` from here on; it then runs
`git worktree remove ../fmonero-mock`. It implements every module behind the frozen
§5b API **without changing a
signature**, and keeps `cargo build` green between modules (build and commit after each).

Order, because it is also the dependency order: `rpc/digest` → `rpc/jsonrpc` → `rpc/node` →
`rpc/wallet` → `units` → `state` → `session` → `send` → `log` → `history` → `status` → `output` →
`setup` → `agent` → `e2e.sh`.

**Gate**, all pasted into DECISIONS.md with real output: `cargo build --release`, `cargo test`,
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`; `scripts/check.sh --full` green;
`tests/e2e.sh --full` green; `fmonero status --json | jq .` real output with both daemons absent
and again against the mock; the §2l **(a), (b) and (d)** timings measured as §1.4 says — (c) is RED
and is not measured or claimed;
**the `%h` guard, in the two forms that can actually fail** (§4.2):
`fmonero setup --units --dry-run | grep -n '%[hSt]'` finds nothing **in the rendered `monerod.conf`
hunk** (grep exits 1), and `cargo test -p fmonero-core units::render_node_conf_has_no_systemd_specifiers`
passes — a unit test that calls `units::render_node_conf` directly, so the check survives
`--dry-run` writing no file;
`fmonero setup --units --dry-run | grep -o -- '--wallet-dir [^ ]*' | cut -d' ' -f2` equals
`fmonero settings get wallet.dir` (§4.2) — read from the `--dry-run` **output**, because `--dry-run`
writes no file and `units::install` refuses to run under `FMONERO_STATE_DIR` at all, so grepping
`~/.config/systemd/user/fmonero-wallet-rpc.service` would exit 2 with no output and the gate could
not fail;
**with `fmonero-mock --role wallet --port 18083 --auth fmonero:t3st
--fail get_balance=-17:'not enough money'` running *and the matching login file written* —
`printf 'fmonero:t3st' > $FMONERO_STATE_DIR/run/monero-wallet-rpc.18083.login; chmod 600 …` —
`fmonero wallet balance` exits 14** (the §1b.4 error map, proven end to end, moved here from the
Phase 0 gate because it cannot be closed until these bodies exist; without the login file the client
sends unauthenticated requests, the mock answers 401 twice and it exits 4 `reason: auth_refused`,
§2c);
`cargo test -p fmonero-core history::locked` passes — `locked` is false for `unlock_time == 0` at
`wh = height + 10`, true at `wh = height + 9`, and `unlock_time = 1_900_000_000` is read as a
timestamp and not as a height (§1b.4);
`fmonero setup --units --dry-run` lists a `CreateDir` action for `wallet.dir` when it is absent
(§4.2);
`fmonero robot-docs | head -3` prints real text;
`fmonero capabilities --json | jq '.exit_codes | length'` is 18 and its `env`, `settings`,
`warnings` and `reasons` arrays are non-empty (§5b); **every command in §4.1 except `tui` and
`robot-docs` asserted against its §5c key set**,
with the money rows' payloads pasted into DECISIONS.md; the §4b matrix filled, every §4.1 command in
at least one list. **Last step: `git tag -a gate-1b -m 'core implemented'`.**
Then the three remaining Phase 2 agents start.

### Phase 2 — parallel, disjoint files (docs after 1a; tui, plugin, doctor after 1b)

Each agent: its own worktree, branch `phase2/<name>`, commits before returning, touches nothing
outside its list.

| Agent | Owns | Gate |
|---|---|---|
| **tui** (~4 h) | `crates/fmonero/src/tui/**`, `tests/tui_tmux.sh` | clippy clean; ≤ 4 600 lines across `{mod,theme,screens,send,subs}.rs`, `tests.rs` excluded (v5.1, §7a), enforced by `scripts/check.sh`; `fmonero tui` opens and quits cleanly on a real pty (`script -qc` or a tmux pane); every screen renders with the mock up and with both daemons absent; the four resize tiers have render tests; the `ProgramSimulator` send test asserts one `transfer` + one `relay_tx` with identical metadata, driven against `fmonero-mock --auth` **with the matching login file written first** (§2c); the `Action` redaction test (§7a); no `println!`; no literal colour outside `theme.rs`; a text capture of each screen in **`decisions/phase2-tui.md`** (§8 preamble), taken **against `fmonero-mock` serving `tests/fixtures/` only, never against a real wallet** — §2b.2's rule covers DECISIONS.md, and the Wallet screen renders a balance, a primary address and counterparty addresses; the capture's header states which mock scenario produced it |
| **plugin** (~3 h) | `plugin/oliver.monero/**` except README, `tests/plugin-smoke.sh`. **Works in the main checkout on branch `phase2/plugin`**, not a worktree — see the §8 preamble | `omarchy-plugin-validate plugin/oliver.monero` clean; `tests/plugin-smoke.sh` green with the plugin symlinked and enabled on this machine; the §6.7 money-grep and the `parseFloat`/`Number(`/`/ 1e` grep pass **and each fires on its own positive fixture**; real line counts recorded against the §6.1 budgets (sum ≤ 1 340 since v5.2) in **`decisions/phase2-plugin.md`** (§8 preamble) |
| **doctor** (~4 h) | `crates/fmonero/src/doctor/**`, `tests/fixtures/doctor/**` (handed over by Phase 0, and this agent may edit them) | `fmonero doctor --json \| jq` lists every §7.1 check with a real verdict on this machine and **exits 11** (binaries `fail:not_installed`, node/wallet/storage `skip:*`, install and security real); `doctor --fix --dry-run` changes nothing (`git status` + a state-dir checksum); class-A round trips (`corrupt → --fix → assert → undo latest → compare`) under `FMONERO_STATE_DIR`; `doctor capabilities --json` valid; `doctor --only security` works; **the `Verdict` status round trip (`pass`/`warn`/`fail`/`skip`) asserted against the literal strings of §7** (§5b rule 3 — moved here from the Phase 1a gate, which does not own `checks.rs`); evidence goes to **`decisions/phase2-doctor.md`** (§8 preamble) |
| **docs** (~2 h; starts after 1a) | `README.md`, `plugin/oliver.monero/README.md`, `scripts/{install,uninstall}.sh` | `bash -n` and `shellcheck` clean on both scripts; `scripts/install.sh --dry-run` prints every step it would take and touches nothing (verified by `git status` plus a `$HOME` checksum); the script follows `installer-workmanship` (set -euo pipefail, umask 077, a lock, `cargo build --release -p fmonero` only, atomic rename into `~/.local/bin`, never sudo, never outside `$HOME`, exit codes mirroring §4a) and its `--yes` path is **read for** idempotence, not executed — at `gate-1a` `setup::run` is still a stub returning `Error::unimplemented`, so `install.sh --yes` would exit 1 on its first real step and this agent could not close its own gate, and §4b already reserves the one real `setup` run for the Phase 4 verifier; README follows `readme-writing` + `de-slopify`, has a "Status" section mirroring §4b, and claims nothing RED; a reviewer can find every README claim in code or in the matrix; evidence goes to **`decisions/phase2-docs.md`** (§8 preamble) |

### Phase 3 — integrate (one agent; ~2 h)

Merge the four `phase2/*` branches into `main` **one at a time**, in order (docs, plugin, doctor,
tui — least to most likely to conflict), each landing green before the next, so a breakage can be
attributed to one branch:

```bash
git checkout main
for b in phase2/docs phase2/plugin phase2/doctor phase2/tui; do
  git merge --no-ff "$b" || { echo "resolve conflicts in $b, then: git commit"; exit 1; }
  cargo build --release || exit 1     # each merge lands green before the next
done
for w in docs doctor tui; do git worktree remove ../fmonero-$w; done   # plugin has no worktree
```

`git merge --no-ff phase2/docs phase2/plugin phase2/doctor phase2/tui` would be an **octopus**
merge, which refuses outright on any conflict ("Should not be doing an octopus") and otherwise
produces one opaque commit — it cannot do what the ordering rationale asks for. The loop stays four
branches: `phase0/mock` was already merged by Phase 1b (§8 Phase 1b). The cleanup loop
names three worktrees, not four, because the plugin agent works in the main checkout and
`../fmonero-plugin` never exists; `../fmonero-mock` is removed by Phase 1b after its merge.

**First step after the four merges:** concatenate `decisions/phase2-{docs,plugin,doctor,tui}.md`
into `DECISIONS.md` in that order under one dated heading, then `git rm` the four (§8 preamble).

Then fix cross-module breakage and run every gate:
`cargo fmt --check`; `cargo clippy --all-targets -- -D warnings`; `cargo test`;
`scripts/check.sh --full`; `tests/e2e.sh --fast` and `--full`; `tests/plugin-smoke.sh`;
`tests/tui_tmux.sh`. Plus the four greps that are non-negotiables, each with a self-test proving it
is not vacuous: core is silent (§2g); no shell strings (§2f); no `query_key` and no secret in
output (§2b); nothing RED claimed in README / `--help` / `robot-docs` (§2j). Regenerate
`docs/ROBOT.md`. Run the agent-ergonomics pass in lite mode (`--help` quality, hint runnability,
`capabilities` and `robot-docs` accuracy) — no audit workspace, no scorecard files. Commit.

### Phase 4 — review, fix, verify (~3 h)

Three reviewers in parallel, read-only, returning findings inline: (1) bugs and money safety, with
`multi-pass-bug-hunting`, focused on the §2a send path and the §2c auth path; (2) honesty —
README vs code vs the §4b matrix — with `mock-code-finder` and `reality-check-for-project`;
(3) plugin and TUI UX against §6 and §7a. One fixer confirms each finding **in code** before
changing anything and applies the confirmed ones. One verifier then runs `scripts/install.sh --yes`
**twice** (the only real `setup` run) and asserts the second run reports every action unchanged —
the idempotence claim the Phase 2 docs gate could not make, because at `gate-1a` `setup::run` was
still a stub — confirms the bar widget loads, re-runs `scripts/check.sh --full`,
re-fills §4b from real output, appends the result to DECISIONS.md, and reports honestly — including
that every RED row is still RED until the owner runs the §4b command list with Monero installed.

**And a fix-up pass closes what the verification found.** The verifier reports; it does not fix,
because the run that proves a claim and the change that makes it true must not be the same commit.
The first verification (2026-09-17) returned three findings and ten failing assertions, and every
one of them needed either a contract amendment or a change to the harness — so a fix-up phase
exists, it works against an amended brief (this document became **v5.1** there), it re-runs every
gate on this machine **and under a `HOME` that has never been set up**, and it re-fills §4b again.
Its ledger is the last section of DECISIONS.md. Two rules it inherits from §2j: an assertion is
never relaxed to make a test pass — a test a correct installer breaks is fixed by giving the test
its own directory, not by deleting the assertion — and the suite must leave the machine exactly as
it found it, which `tests/e2e.sh`'s own last step now asserts.

---

## 9. Skills to read (paths; apply substance, skip ceremony — no beads, no NTM, no sibling workspaces)

- **CLI / agents:** `~/.claude/skills/agent-ergonomics-and-intuitiveness-maximization-for-cli-tools/references/QUICKREF.md`
  and `references/CHEAT-SHEET.md`. The Polish Bar is the §4 checklist; the "One Rule" is why every
  error carries a runnable hint.
- **Doctor:** `~/.claude/skills/world-class-doctor-mode-for-cli-tools/assets/skill-card.md`,
  `references/methodology/KERNEL.md` (axioms 1–3, 7, 13–15), `references/COOKBOOK.md` patterns 4
  and 8. Axiom 1 (one `mutate` chokepoint), axiom 2 (backup before mutation), axiom 3 (a recorded
  inverse) and axiom 7 (read-only by default) are §7 verbatim, **including "never deletes a file",
  which §7 keeps unnarrowed**: v0.1's one candidate for an exception, `history_cache_fresh`, is an
  `info`-severity report on a cache the code already ignores and refetches when stale, so the whole
  `RemoveFile` apparatus — backup, hash pair, recorded inverse, undo path, fixture — bought nothing
  and was removed with its single member. A doctor that cannot delete anything is a stronger safety
  claim than one that can delete one thing carefully. `CreateDir` stays, because `wallet_dir_exists`
  genuinely cannot be expressed without it and a fixer that cannot be expressed as an `Action` is a
  fixer that gets written around the chokepoint.
- **TUI:** `~/.claude/skills/frankentui/SKILL.md` and `references/TUI_POLISH_PLAYBOOK.md` **§1 and
  §3 only** — its `file:line` references point at a repo that is not on this machine, so take the
  capability gates and the upgrade checklist and ignore the citations. Do **not** read
  `tui-glamorous` (Go/Charmbracelet).
- **Omarchy:** `~/.claude/skills/omarchy/plugins.md` and `theming.md`.
- **Docs and install:** `~/.claude/skills/readme-writing/SKILL.md`,
  `~/.claude/skills/de-slopify/SKILL.md`, `~/.claude/skills/installer-workmanship/SKILL.md`.
- **Quality gates:** `~/.claude/skills/multi-pass-bug-hunting/SKILL.md`,
  `~/.claude/skills/mock-code-finder/SKILL.md`,
  `~/.claude/skills/just-say-no-to-process-porn-and-ceremony/SKILL.md`,
  `~/.claude/skills/reality-check-for-project/SKILL.md`,
  `~/.claude/skills/testing-real-service-e2e-no-mocks/SKILL.md` — the last one is why §4b keeps a
  RED list at all: a mock proves the wire format, never the behaviour, and this document says so
  everywhere it matters.
- **Planning:** `~/.claude/skills/planning-workflow/SKILL.md` — the three properties this document
  is built to satisfy: self-contained, dependency-aware, justified.
