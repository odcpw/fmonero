# fmonero: a faster Monero front end on the FrankenSuite

Research statement, 2026-09-16. Proposed home: `~/Projects/odcpw/fmonero` and `github.com/odcpw/fmonero`.

## 1. Short answer

Yes, and it is the same shape of project as fproton and fbittorrent. The official Monero GUI is
a Qt5/QML wallet that links the wallet engine (wallet2) in-process and spawns and babysits
`monerod` as a child. Its slowness is mostly the QML layer and the node babysitting, not the
protocol. A Rust CLI + FrankenTUI app that talks JSON-RPC to `monerod` for the node and to
`monero-wallet-rpc` for the wallet drops every one of the GUI's own bottlenecks, starts in
milliseconds, and keeps the exact wallet engine Monero ships, so it stays correct across the
coming FCMP++ hard fork.

The FrankenSuite piece that carries this is FrankenTUI (`ftui` 0.7). The other suite members
(asupersync, frankensqlite, frankensearch) should not be used here, for concrete reasons in §4.

## 2. What monero-gui actually is

Verified against `monero-project/monero-gui@7691273` (release v0.18.5.2, 2026-07-21, bundles
core v0.18.5.1):

- **Qt 5.12+ / QtQuick.** 109 QML files, 26,869 lines of QML, only 8,362 lines of C++. Nearly all
  UI logic lives in QML/JS.
- **Wallet engine in-process.** It links `wallet_api` (the wallet2 C++ API) directly. There is no
  `monero-wallet-rpc` anywhere. wallet2 itself speaks HTTP to the daemon. The refresh loop is a
  thread with a fixed 10-second interval.
- **It spawns and manages monerod.** `DaemonManager.cpp` builds the argv (`--detach`,
  `--prune-blockchain`, `--bootstrap-daemon-address`, `--no-sync`, `--max-concurrency`) and
  launches it with `QProcess::startDetached`. Readiness is polled by shelling out to
  `monerod sync_info` every 2 s and grepping for `Height:`. Stop is `monerod exit`, then `pkill`.
- **"Simple mode" is a local monerod with `--no-sync` proxying a remote node.** "Advanced" is a
  real syncing node or a remote node.
- **It ships a "low graphics mode"** (`QMLSCENE_DEVICE=softwarecontext`) because the QML scene
  graph is a recurring CPU problem.

So "a front end to monerod" is only a third of it. It is a wallet UI, a wallet-engine host, and a
node process manager, all in one Qt process.

## 3. Where the slowness comes from

Three separate sources. A replacement fixes the first two and inherits the third.

| Source | Evidence | Fixed by a TUI replacement? |
|---|---|---|
| QML rendering and CPU | #2878: 100 % CPU on the idle language screen; maintainers refuse to default to the software renderer. Older #2238, #2013, #935, #757. | Yes. Diff-based text rendering costs nothing at idle. |
| QML list models and UI-thread blocking | #4500: Receive page hangs with 30k subaddresses. #3896: 3 s to populate a Send row from the address book; the GUI gets laggier the longer it is open. #4463: scan by txid freezes the UI. #3821: `QML binding loop detected` spam. | Yes. Virtualized lists, and every RPC call runs off the UI thread as a `Cmd::task` or `Subscription`. |
| Node sync I/O blamed on the GUI | #4259, #4489, #4380: LMDB random I/O and an 83 GB mmap make the machine crawl; users blame the GUI because it bundled the node. | Partly. We can show honestly what the node is doing, throttle it, and never block on it, but the node is the node. |
| wallet2 scan speed | Inherited by GUI, Feather, Cake, Monerujo, Stack, wallet-cli, wallet-rpc alike. Already multi-threaded, pulls `/get_blocks.bin`. Cuprate claims monerod's RPC serving is the bigger bottleneck under load. | No, unless we replace the engine (§5, option C). |

Upstream closed a 2025 request to rewrite in Rust/GTK4 as "not actionable" (#4499). Feather
already proves the thesis: same wallet2 core, Qt Widgets instead of QML, no bundled node, and it is
widely regarded as far lighter. A TUI goes one step further.

## 4. What the FrankenSuite gives us

| Component | Verdict | Why |
|---|---|---|
| **FrankenTUI `ftui` 0.7.0** (crates.io 2026-09-08; v0.8.0 tagged, not yet published) | **Use it. The one load-bearing suite dependency.** | Elm-style `Model`/`Cmd`/`Subscription`, native TTY backend, blocking event loop, no async runtime. Every widget on the list exists: `Table` with sort/filter, `VirtualizedList`, `Sparkline` and `LineChart`, `ProgressBar`, `LogViewer` with regex search and follow, `TextInput` and `Form` with validators, `Dialog::confirm`, `Toast`/`NotificationQueue`, `CommandPalette`, mouse hit regions, `Canvas` (QR codes in half-block mode), inline or fullscreen mode. Testing: `ProgramSimulator`, `HeadlessTerm`, `ftui-harness` golden snapshots. Two gaps, both already solved in fproton: a key-value panel helper and the Omarchy palette loader (`theme.rs`, copied verbatim between the two repos). |
| **asupersync 0.5.0** | **Do not use.** | Nightly-only with default features (E0554 on stable 1.98; stable only with `default-features = false`). 134 crates and ~52 s `cargo check` for an empty binary. Its HTTP client has Basic and Bearer auth but no Digest, and `monerod --rpc-login` is MD5 Digest. No ZMQ. FrankenTUI's loop is blocking anyway, so a runtime would sit on a side thread and still bridge through `std::sync::mpsc`. `ureq` 3.4 with a global timeout plus one thread per poller is identical in behaviour and stable-Rust. |
| **frankensqlite `fsqlite` 0.4.2** | **Do not use (now).** | Nightly-only on x86_64: an unconditional `feature(core_intrinsics)` gate in `fsqlite-pager`, no flag to turn it off. Hard dependency on asupersync. fproton already tried and rejected it on 2026-09-14 for exactly this reason. Postcard snapshots with versioned headers cover tx history and node stats fine. |
| **frankensearch 0.6.0** | Out of scope. | A wallet has a few thousand notes and addresses. Substring or fuzzy match in memory answers in microseconds. |
| **ntm, ubs, dcg, cass, br/bv** (installed) | Dev loop, as with fproton. | `rch` and `doctor_frankentui` are not installed; neither existing project uses them. fbittorrent's tmux smoke script is the house equivalent of doctor_frankentui. |

Licence note: the ftui crates are `LicenseRef-MIT-OpenAI-Anthropic-Rider`. Fine for this, but
not SPDX, so `cargo-deny` would need an exception.

## 5. Architecture: one way for the node, three for the wallet

### Node (settled)

Talk to a local, unrestricted `monerod` on `127.0.0.1:18081`. Two transports on one port:
`POST /json_rpc` and legacy `POST /get_info`-style endpoints. Everything a dashboard wants
(`sync_info`, `get_connections`, `get_peer_list`, bans, `get_net_stats`, `out_peers`/`in_peers`,
`stop_daemon`, `set_log_level`) is blocked on restricted RPC, so a remote or public node only ever
yields `get_info`, headers, fee estimate, txpool stats and version. Design for local-first,
degrade honestly for remote. Push events exist via `--zmq-pub` (topics `json-minimal-chain_main`,
`json-minimal-txpool_add`), but a 1 to 2 s poll of three cheap calls makes ZMQ unnecessary in v0.1.

Cuprate (pure-Rust node, 0.1.0-preview, 2026-08-01) is not a dashboard backend yet: `sync_info`,
`get_connections`, peer list, bans, txpool stats and net stats all return `not_available`. Worth
a compatibility flag later, not a target now.

### Wallet (the real decision)

| Option | What it is | For | Against |
|---|---|---|---|
| **A. Drive `monero-wallet-rpc`** (recommended) | Spawn the wallet daemon as a systemd user unit, exactly how fbittorrent runs `qbittorrent-nox`, and call its JSON-RPC: `get_balance`, `get_address`, `create_address`, `get_transfers`, `transfer`, `sweep_all`, `validate_address`, `refresh`, `open_wallet`/`close_wallet`, `setup_background_sync`, proofs, URIs, address book. | Zero crypto risk. Every feature Monero ships, including background sync (view-key-only scanning while the spend key stays encrypted, in 0.18.5.1). First and for a while only engine that will spend under FCMP++/Carrot. Same speed as Feather. Pure Rust on our side, stable toolchain. | A second daemon to manage. It has **no default port**: `--rpc-bind-port` is mandatory. `--rpc-login` is MD5 Digest, which no Rust client in either house repo implements: bind loopback and use `--disable-rpc-login`, or write ~100 lines of Digest. The typed Rust client `monero-rpc` 0.5.1 exists but is reqwest-based; a hand-rolled `ureq` client in the fbittorrent style is 300 lines and fits the house. |
| **B. Embed wallet2 via FFI** (`monero_c` shim, how Cake and Feather do it) | Link the C++ engine into our binary. | No second process. | A C++ Monero build in our tree, Boost and OpenSSL linkage, long builds, not the house style. Same speed as A. |
| **C. Pure Rust `monero-wallet` 0.2** (monero-oxide, ex-monero-serai) | `Scanner`/`ViewPair` for scanning, `SignableTransaction` for CLSAG sends. | No C++, fastest iteration, view-only scanning today. | Explicitly "not a wallet": we would own persistence, output selection, reorg handling. CLSAG only, no FCMP++ or Carrot. Block fetch is plain JSON, slower than wallet2's binary path. The FCMP++ fork is likely within ~12 months and it would not be able to spend. Good for a future read-only "watch" mode, not for v0.1 sending. |

Recommendation: **A**. It is the option where the frontend's speed is ours and the money-handling
correctness is Monero's.

## 6. Proposed v0.1 scope for `fmonero`

Copy fproton's skeleton and fbittorrent's two backend pieces (the `ureq` client and the on-demand
systemd user unit).

- **Workspace:** `crates/fmonero-core` (silent library, typed `Error`, one variant per exit code)
  and `crates/fmonero` (clap binary with `output.rs`, `setup.rs`, `doctor/`, `tui/`,
  `assets/menu.jsonc`); `plugin/oliver.monero/`; `scripts/{install,uninstall,check}.sh`;
  `tests/{e2e.sh,plugin-smoke.sh,fixtures/}`; `DESIGN.md`, `DECISIONS.md`, `CLAUDE.md`,
  generated `docs/ROBOT.md`.
- **Dependencies:** `ftui 0.7` (crossterm), `ftui-extras 0.7` with `canvas` and `charts`
  (optionally `forms`, `validation`), `ureq 3.4` (json), serde, clap 4.6, toml, dirs, jiff,
  postcard, sha2, `qrcode` for receive addresses, `tiny-keccak` for address checksums. No
  asupersync, no fsqlite, no tokio. Stable toolchain.
- **CLI** with the shared envelope (`--json` implied on non-TTY, `ok`/`schema`, `hint` on every
  error, exit codes 0 to 8 identical to fproton, 9+ tool-specific such as `wallet_locked`,
  `node_syncing`):
  - `fmonero status` (never starts anything), `node up|down|log|peers|bans`, `wallet open|close|
    balance|address|history|send|sweep`, `send` needs `--yes` and prints the exact transfer
    including fee, `--dry-run` everywhere, `settings`, `setup`, `doctor`, `tui`, `capabilities`,
    `robot-docs`.
- **Setup** writes two systemd user units: `monerod` (pruned, `--data-dir ~/.bitmonero`, loopback
  unrestricted RPC on 18081, optional `--zmq-pub`) and `monero-wallet-rpc` (`After=`/`Wants=`
  the node, `--rpc-bind-port` on a chosen loopback port, `--wallet-dir`, `--disable-rpc-login`).
  Also links the bar widget and merges the menu entries, as fproton does. The Arch package ships a
  *system* unit running as user `monero` with `/etc/monerod.conf`; the doctor must detect that
  layout too and refuse to run two nodes on one data dir.
- **TUI:** four screens. Node (sync gauge, height vs target, peers in/out, net rate sparkline,
  mempool, fee estimate, version, hard fork), Wallet (balance and unlocked, subaddress list,
  virtualized history with sort and filter), Send (form with live address validation: base58,
  95 or 106 chars, mainnet prefix 4 or 8, keccak checksum; amount; priority; confirm dialog
  echoing fee), Log (monerod log tail with regex search). Omarchy theme with live reload,
  resize tiers, a 1 s poll and 120 ms tick, `Cmd::task` for reads, cancellable `Subscription`
  for `transfer` and rescans.
- **Bar widget `oliver.monero`:** `Process { command: [bin, "status", "--json"] }` on a 3 s/8 s
  timer; icon states syncing/synced/wallet-locked/backend-down; panel shows height, peers,
  balance if the wallet is open; one click to the TUI; hides while both daemons are down.
- **Doctor:** binaries present (`extra/monero`), which node layout is in use, RPC reachable and
  unrestricted, ports 18080/18081 free or owned by us, prune status, free disk versus chain size,
  wallet-rpc up and bound to loopback only, no `--rpc-login` without a client that speaks Digest,
  clock sane, `omarchy` plugin linked.
- **Explicitly not in v0.1:** multisig, hardware wallets, mining controls, Tor/i2p, Cuprate,
  the pure-Rust scanner, multiple wallets open at once.

## 7. What stays slow, and the risks

- **Wallet scanning is wallet2's speed.** Same as Feather. Background sync and a local node make it
  as good as it gets today. This project cannot make a first sync faster than the engine.
- **The node is the node.** A pruned mainnet chain is roughly a third to a half of the ~290 GB full
  chain (estimate, not measured). You have 238 GB free and 30 GB RAM, which is enough for pruned
  but should be checked by the doctor before `setup` writes the unit.
- **FCMP++ is not activated on mainnet** (mainnet hard fork table still ends at v16, August 2022;
  beta stressnet since 2026-05-06; no fork date set; articles claiming a January/March 2026
  activation are wrong). Expect a v17 fork with Carrot addresses within about a year. Option A
  rides it for free; the send form's address validator will need the new prefix then.
- **monero-wallet-rpc quirks:** mandatory `--rpc-bind-port`, no default; MD5 Digest auth; the
  refresh model is still poll-based, so a 1 s UI poll is fine but `refresh` calls should not pile
  up (a `poll_inflight` guard, as in fproton).
- **`ftui` 0.8.0** is tagged but not on crates.io yet; nothing in it matters for Linux.

## 8. Machine facts (verified today)

| Fact | Value |
|---|---|
| Arch `extra/monero` | 0.18.5.1-2: `monerod`, `monero-wallet-cli`, `monero-wallet-rpc`, system unit, `/etc/monerod.conf` |
| Arch `extra/monero-gui` | 0.18.5.2-2 (not installed) |
| AUR | `feather-wallet` 2.8.1 |
| Monero on this machine | nothing installed, no `~/.bitmonero` |
| Disk / RAM / cores | 238 GB free on `/home`, 30 GB, 8 |
| Rust | 1.98.0 stable |
| Omarchy | 4.0.3; plugins `oliver.protonvpn`, `oliver.torrents`, `oliver.background-weather` |
| ftui in registry | 0.7.0 (used by fproton), 0.6 (fbittorrent) |

## 9. Next steps

1. Create `~/Projects/odcpw/fmonero` and the private `odcpw/fmonero` repo.
2. Write `DESIGN.md` v1 from §5 and §6 in fproton's section layout (scope, machine facts,
   non-negotiables, CLI table, verification matrix, status JSON, plugin, doctor, TUI, phases).
3. Phase 0 in parallel: install `extra/monero`, capture real `get_info`, `sync_info`,
   `get_transfers`, `get_balance` fixtures from a stagenet wallet into `tests/fixtures/`.
4. Phase 1a: freeze the core API (`NodeClient`, `WalletClient`, `Status` schema, `Error`).
5. Then the fproton phase plan: core, then TUI, plugin and doctor in parallel, integrate, review.

## 10. Sources

- monero-gui `7691273`: `CMakeLists.txt`, `src/CMakeLists.txt`, `src/daemon/DaemonManager.cpp`,
  `src/libwalletqt/Wallet.cpp`, `main.qml`; issues #2878, #4500, #3896, #4463, #3821, #4259,
  #4489, #4380, #4499; release v0.18.5.2.
- monero core `9e3a310`: `src/rpc/core_rpc_server.h` (restricted map), `src/rpc/zmq_pub.cpp`,
  `src/wallet/wallet_rpc_server.{h,cpp}` (`rpc-bind-port` required), `src/wallet/wallet2.cpp`,
  `src/cryptonote_config.h` (ports), `src/hardforks/hardforks.cpp`, `contrib/epee/src/http_auth.cpp`;
  PR #8619 (background sync), #10605.
- FCMP++ timeline: monero.observer stressnet posts, getmonero.org roadmap, CCS jeffro256 2026Q1.
- Feather `948773c` (Qt6 Widgets, no monerod spawn). Cake v6.4.4 and Monerujo v4.1.7 via wallet2
  FFI. MyMonero sunset 2026-01-06. monero-lws v1.0.1 (2026-09-09).
- Rust: `monero-rpc` 0.5.1 (2026-07-18), `monero` 0.22.0, `monero-wallet` 0.2.0 (monero-oxide),
  Cuprate `4f8fcd1` handlers returning `not_available`, `vdo/muff` (ratatui proof of the stack).
- FrankenSuite: crates.io `ftui` 0.7.0 and sub-crates, `asupersync` 0.5.0, `fsqlite` 0.4.2,
  `frankensearch` 0.6.0; registry sources under `~/.cargo/registry/src/`; measured `cargo check`
  on stable for asupersync default (E0554) and no-default-features (52 s); `fsqlite-pager/src/lib.rs:2`.
- House pattern: `~/Projects/odcpw/fproton` (`DESIGN.md`, `DECISIONS.md` 2026-09-14 fsqlite entry,
  `crates/fproton/src/tui/{mod,subs}.rs`, `plugin/oliver.protonvpn/`), `~/Projects/odcpw/fbittorrent`
  (`src/qbt.rs`, `src/backend.rs`, `src/setup.rs`).
