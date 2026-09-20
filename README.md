# fmonero

A Monero node and wallet front end for Omarchy. A Rust CLI and engine, a FrankenTUI app, a
`doctor`, and a thin Quickshell bar widget (`oliver.monero`). Monero's own daemons stay
underneath and keep doing the cryptography: `monerod` serves the chain, `monero-wallet-rpc`
owns the wallet file. fmonero is what you use, and it keeps answering while both of them are
busy.

It replaces `monero-gui`, which carries 109 QML files and 26 869 lines of QML, runs the
`wallet2` engine in its own process, and babysits its `monerod` child by shelling out to
`monerod sync_info` every two seconds. fmonero starts in milliseconds and costs nothing at
idle.

Read **[Status](#status)** before you believe anything here. Monero 0.18.5.1 **is** installed on
the machine this was built on. A `monero-wallet-rpc` has been driven there against a public node
since 2026-09-17 (the LIVE class below). A local `monerod` has started there exactly **once**, for
thirty seconds on 2026-09-19, when a verification sandbox missed its own `systemctl` fixture: it
reached block 40 536 of 3 765 450 and left about 147 MB in `~/.bitmonero` (`DECISIONS.md`, V18).
Nothing was measured by that run, so every row below that needs a live local node is still RED and
unverified.

## Why

**The daemons are slow in ways a front end must design around.** `monero-wallet-rpc` serves
RPC on exactly one thread, and while the wallet is behind the tip it deliberately blasts
through block chunks, yielding that thread for 200 to 300 ms per refresh period (upstream's own
comment says that otherwise "other calls to the RPC will hang"). A fresh `monerod` answers
`status: "BUSY"` with an all-zero response for its entire initial sync, which is days. A front
end that treats either as an error is unusable for the first week, and one that renders the
zeros lies about the chain.

So fmonero is built for daemons that are up but not ready:

- `fmonero status` never starts anything and never calls a wallet mutator. It makes three
  loopback RPCs with the wallet closed and five with it open, plus two
  `systemctl --user is-active` spawns, which are the expensive part. Its budget with both
  daemons absent is 60 ms; `DECISIONS.md` carries the measurement.
- A daemon `status` other than `"OK"` is not a result. `"BUSY"` becomes a `null` field, never
  the value-initialised zero the daemon sent. Unknown is always `null`: never an absent key,
  never `{}`, never a zero standing in for unknown.
- A restricted node poisons part of its own `get_info` (uptime 0, free space `2^64-1`, all four
  peer counts 0). Those fields are reported as `null` with a warning that says which panes have
  no data source, instead of a 56-year uptime and a false "no peers" alarm.
- Every external process runs in its own process group under a deadline (SIGTERM, then SIGKILL
  two seconds later) with all three stdio pipes closed off. Every HTTP call has a timeout,
  which is set in the one function that builds the HTTP agent.
- A call killed at its deadline arms a 60 s breaker, so the next command fails fast with a
  runnable hint instead of queueing behind another doomed call. A wallet that is more than 100
  blocks behind is classified as syncing rather than stalled, because a breaker that fires on
  correct behaviour is a breaker you disable.
- argv only. No `sh -c` and no shell strings, in Rust or in QML.
- Loopback plaintext only, as a property of the binary: the HTTP client is built with no TLS
  backend at all, and both clients reject any URL that is not `http://` to a literal loopback
  address.
- MD5 Digest authentication to both daemons. Loopback binding alone does not stop a web page
  from POSTing `sweep_all` to `127.0.0.1:18083`; it cannot read the reply, but a sweep does not
  need a readable reply. Digest makes the request fail before the handler runs.
- No wallet secret ever passes through fmonero's argv, output, state files or logs, and no seed
  ever enters the process at all.

## Install

```bash
scripts/install.sh --dry-run          # print the plan
scripts/install.sh --yes              # build, install to ~/.local/bin, wire up Omarchy
```

That is `cargo build --release -p fmonero`, an atomic install of `~/.local/bin/fmonero`, and
`fmonero setup --all --yes`. It has been run for real, twice in a row, and the second run
reported **all seven** of its actions `unchanged` while leaving every artefact byte-identical to
the first run's — so it is safe to re-run after a `git pull`. `omarchy plugin enable` and
`omarchy bar put` are skipped when `~/.config/omarchy/shell.json` already carries the widget,
which is the file Omarchy itself writes; before that pre-check existed both were re-executed and
reported `written` on every run, a reporting defect recorded as finding 2 of the Phase 4
verification in `DECISIONS.md`.
`--dry-run` prints the whole plan and changes nothing.
`scripts/uninstall.sh` reverses it (`--menu` also strips the menu entries, `--purge` also
deletes fmonero's own config, state and cache). Neither script uses sudo, writes outside
`$HOME`, or touches a wallet file; the uninstaller refuses `--purge` while a pending-send
record exists, and never deletes your wallets or `~/.bitmonero` under any flag.

Monero itself comes from the distribution, and fmonero never installs it:

```bash
omarchy pkg add monero                # Arch extra/monero 0.18.5.1
fmonero setup --units --yes           # two systemd USER units + ~/.config/fmonero/monerod.conf
fmonero node up --wait                # start monerod and wait for it to report itself synced
```

The first two have run; the third has not, because no `monerod` has ever been started on this
machine. On a machine that is set up but has **no** Monero package, `fmonero node up`,
`fmonero node restart` and `fmonero wallet open` exit **4 `not_installed`** within milliseconds,
with `omarchy pkg add monero` as the hint, and start nothing: the unit file and the daemon binary
are both checked before any `systemctl` call. A unit fmonero did start that never answers is
stopped again rather than left retrying. If you do not want a local chain at all, skip this
section and read [Public node](#public-node). See [Status](#status).

`setup --units` writes `fmonero-monerod.service` and `fmonero-wallet-rpc.service` into
`~/.config/systemd/user/` and leaves both **disabled**: they start on demand, not at login.
The node's config file carries a generated `rpc-login` credential at mode 0600, is never
rotated by a second `setup`, and is redacted everywhere `setup` prints a diff. Both units bind
`127.0.0.1`; neither is given `--restricted-rpc`, which would block `relay_tx`.

If Arch's own system `monerod.service` is enabled, fmonero detects it, uses it read-only, and
refuses to start a second `monerod` or to mutate one it does not own. Two `monerod` on one LMDB
data directory corrupt it, so `node up` first checks the POSIX record lock on
`<data-dir>/lmdb/lock.mdb` and scans `/proc` for a `monerod` already serving that directory.

The Omarchy shell's PATH includes `~/.local/bin` but not `~/.cargo/bin`, so install there.

## Public node

You do not need a local chain. `monero-wallet-rpc` will talk to anybody's node, and fmonero can
point it at one and keep it there — which is the whole of remote node mode. **fmonero itself
never speaks to that node**: its own HTTP client accepts loopback `http://` only and the binary
contains no TLS stack at all, so the one process that dials a stranger is the one upstream wrote
for the purpose.

```bash
fmonero node list --probe                 # public nodes from monero.fail, then probe the top 20
fmonero node remote <host:port> --yes     # a public node; never --trusted
fmonero wallet create <name> --yes        # or: fmonero wallet open <name>
fmonero wallet refresh                    # the first contact with the node
fmonero node local --yes                  # the way back
```

The first line exists because the second one needs an address, and until v0.5 this README wrote
that address as `<host>:18089` — a placeholder, not a node. `node list` serves a cached list of
clear-net public nodes from **monero.fail** and, with `--probe`, asks the top twenty for
`get_info` from *this* machine; `--refresh` refetches, `--limit N` bounds both what is printed
and what is probed, `--all` prints every usable candidate. The TUI has the same list on its Node
screen (`f` refresh · `p` probe · `Enter` use · `a` add your own · `/` filter), which is the way
to do this without typing an address at all.

**What asking for the list costs, in four sentences.** Fetching it tells **monero.fail** this
machine's IP address and the time. Probing tells **each probed node** the same. **Nothing is
contacted automatically except the node you already use**, and that only while the TUI's Node
screen is showing (`node.remote_probe_sec`, default 60, `0` to turn it off) — `status`, the bar
widget and the doctor spawn nothing, ever. And monero.fail's `available` / `checks` /
`last_height` are **monero.fail's measurements from monero.fail's vantage point**: not a
recommendation, and not this machine's — a node rated healthy there may be unreachable from here,
which is why `--probe` exists and why its numbers are printed in their own columns under their
own heading. If monero.fail is down or changes shape, `node list` says so and nothing else in
fmonero is affected: the cache is served stale with its age, and every other command works with
no list at all.

The list and the probes are **`curl` child processes**, argv-only and deadline-bounded. fmonero's
own HTTP client is unchanged: loopback `http://` only, no TLS stack in the binary, no port bound.
`scripts/check.sh` asserts that no file but `crates/fmonero-core/src/nodes.rs` names curl as a
process, with a planted argv proving the check fires. One honest discrepancy: curl verifies TLS
certificates and `--daemon-ssl autodetect` does not, so an `https://` node with a bad certificate
probes `reachable: false, reason: tls` while the wallet would have reached it anyway by falling
back to plaintext. Neither figure is adjusted to match the other.

Create the wallet while the node is answering. In remote mode `monero-wallet-rpc` takes the new
wallet's restore height from that node — over the address in the unit's argv, before fmonero
gets a word in — so a create against an unreachable node produces a wallet that scans from
genesis, and one against a node reporting a height above the real tip produces a wallet that
will never see its funds. `monero-wallet-cli --wallet-file <wallet.dir>/<name>` and `rescan_bc`
is the only way back: v0.1 ships no rescan command.

Bound every send with `--max-fee`. The reason is below.

**What the node learns.** This machine's IP address and the timing of every request, including
the burst that accompanies a send. Which blocks the wallet asks for, which bounds the wallet's
restore height and therefore its age. And, if you mark it trusted, the amounts the wallet holds
— those travel *inside* the request.

**What the node can do.** Three things, with three different costs:

- **Lie about the chain tip.** A transaction built against a stale tip is **rejected when it is
  relayed**: exit 16, funds unaffected, because no output is marked spent until a relay
  succeeds. A failed command, never a loss.
- **Lie about the fee.** This is the one that costs real money and the one nothing detects: the
  wallet takes the node's fee-per-byte verbatim, the transaction relays *successfully*, and the
  excess goes to a miner. `--max-fee AMOUNT` on `wallet send` and `wallet sweep` is the bound
  that answers it, and it is re-checked immediately before the broadcast.
- **Choose your decoys.** The output distribution and the ring members every transaction's
  decoys are drawn from come from the node, so a hostile one can degrade the ring signature's
  plausible deniability. This is a privacy loss rather than a fund loss, and `--trusted` has
  nothing to do with it: marking the node untrusted does not reduce it.

**Never mark a public node `--trusted`.** With the flag set, the wallet asks the daemon about
its own unspent amounts by listing them in the request. `--trusted` exists for a node you run
yourself, on a LAN or a private overlay; `fmonero doctor` warns whenever it is set for a host
that is not a private address.

**Two limits, plainly.** TLS here is `--daemon-ssl autodetect`, which is **opportunistic and
unauthenticated**: it tries TLS, falls back to plaintext if the server does not speak it, and
does not verify the certificate. Typing `https://` does not change that. And **Tor is not in
v0.1** — the node sees your IP address, and no flag in this build hides it.

`fmonero status --json` reports `node.layout: "remote"`, the address in force, the trusted flag,
and `node.reachable: null` — null rather than false, because no wallet-RPC method reports a
remote daemon's own state and fmonero will not print a number it did not receive. Every node
command that needs the local daemon (`up`, `down`, `restart`, `peers`, `bans`, `ban`, `unban`,
`limit`, `log`) exits 4 `remote_node` and names `fmonero node local --yes`; `node info` still
works and reports the address, the trusted flag and the mode.

## Status

The project tracks every (command, scenario) pair in one of three classes, defined in
`DESIGN.md` §4b. A row counts as verified only once `DECISIONS.md` carries its real command
output, and `fmonero version --json` carries `build_stage` and `build_note` for the build you
actually have: this section mirrors the matrix as it stood after the v5.1 fix-up on 2026-09-17,
but that field, not this section, is the authority on how much of the list below the binary in
your hands has run.

| Class | What it means |
|---|---|
| GREEN | Verified end to end on this machine, with no daemon running, by `tests/e2e.sh` and the phase gates. |
| AMBER | Verified against `crates/fmonero-mock`, a test-only server that replays fixtures derived from the Monero source at tag v0.18.5.1. This proves our wire format, our parsing and our error mapping. It proves nothing about a real daemon. |
| LIVE | A real `monero-wallet-rpc` against a real public node, on a scratch wallet with no funds. **Run for the first time on 2026-09-17**, against `xmr-node.cakewallet.com:18081`, by an independent verifier; the full transcript is in `DECISIONS.md` under "Remote node mode verification". It is the only class in which this project has ever *deliberately* used a real Monero node; the one accidental local `monerod` start (V18) measured nothing and left every local row RED. |
| RED | Never run. Unverified, and not claimed to work anywhere in this README, in `--help` or in `robot-docs`. |

**GREEN.** `status` with both daemons absent (exit 0, `reachable: false`, warnings);
`wallet list`; `wallet validate` on the structural path; `settings list|get|set`;
`capabilities`; `robot-docs`; `version`; `setup --dry-run`; `doctor`, which exits 11 with
the three binaries checks passing, `node_reachable` the one failure (`node_unreachable`), and
every other node, wallet and storage check skipped or warned with a stated reason; `doctor capabilities` and the `doctor undo latest` round trip;
`tui` opening, rendering every screen's empty state and quitting cleanly; `node up` and
`wallet open` with no unit (exit 4 `unit_missing`); `node up`, `node restart` and `wallet open`
with the units installed and no Monero binary (exit 4 `not_installed`, in milliseconds, starting
nothing); `setup --units` below the 120 GB free-space floor (exit 3 `full_chain_refused`, with
`--full-chain --yes` overriding it); `wallet send` with no wallet service (exit 4 at
the reachability probe, before any build, so a bad address is still exit 15 and an unresolved
pending send is still exit 3); `node down --dry-run` (exit 2, because `--dry-run` is implemented
by exactly six commands); `wallet clear-pending`; `node log` with no log file; bare `fmonero`
on a pipe and on a pty; every `--help`; every usage error; amount and address parsing; and the
60 ms `status` budget.

Also GREEN since Phase 4's verifier ran it: `setup --all --yes` for real, through
`scripts/install.sh --yes`, twice — the two systemd user units (both left disabled),
`~/.config/fmonero/monerod.conf` at 0600 with its `rpc-login` carried through unrotated, the
plugin symlink, and the menu entries; the bar widget loading in the live Omarchy shell and
polling that binary's own `status --json`; the `--wallet-dir` in the installed unit being
`settings.wallet.dir` byte for byte; the stagenet and `--rotate-rpc-login` renderings through
`--dry-run`; and the doctor's `units`, `install` and `security` groups — 17 checks — passing
against the files that run actually wrote.

Every one of those rows is reproducible on a machine that has already been set up, which is
worth stating because two of them were not. `tests/e2e.sh` used to read the real
`~/.config/systemd/user` and the real `wallet.dir`, so five of its assertions were silently
asserting "nothing is installed here" and failed the first time something was; they run in the
case's own sandboxed directories now, and the last step of the file asserts that no case left
one of your units running.

**AMBER.** `node info|peers|bans|ban|unban|limit`;
`wallet open|close|create|balance|address|new-address|history|refresh|book|notes|use-node`;
`wallet send --dry-run`, `wallet send` without `--yes`, and `wallet send --yes` (asserted to
issue exactly one `transfer` and one `relay_tx` carrying that response's own metadata);
`wallet sweep` in every form, asserted to carry `subaddr_indices_all: true`; `status` with both
daemons answering; the restricted-node and `"BUSY"` degradations; Digest authentication
including the per-nonce counter reset; the wallet-RPC error map, one case per mapped code; the
deadline, breaker and `--force` paths; and the layout refusals against a synthetic `/proc` and
`systemctl` fixture.

**LIVE — run on 2026-09-17 against `xmr-node.cakewallet.com:18081`, in a scratch sandbox, on a
wallet with no funds.** `node remote --dry-run` and `node remote --yes` (idempotent on the second
run, with the installed unit carrying `--daemon-address`, `--untrusted-daemon` and
`--daemon-ssl autodetect`); `wallet create` (three of four runs; see the defect below);
`wallet open`; `wallet refresh`, which fetched a real block from that node; `wallet balance`
(zero, as an empty wallet should be); `wallet address`; `wallet history --limit 5` (empty);
`wallet close`; `status --json`, reporting `node.layout: "remote"`, `node.reachable: null` and a
wallet height of 3 764 638; `node info`; the nine node commands that need the local daemon, each
exiting 4 `remote_node`; the TUI's Node screen on a real pty; and `node local --yes`, back to the
local render. `wallet send --dry-run` reached **exit 14 `insufficient_funds`** — past the remote
refresh gate, which is the point of the row: the wallet's `last_refresh_at` advanced before the
build, nothing was relayed, and no pending record was written. A real send or sweep of any amount
is still RED, in this mode as in the other.

**LIVE, a second round on 2026-09-18 — the TUI, driven by an independent verifier on a real
150×40 pty.** A wallet was **created through the TUI** for the first time, against a real
`monero-wallet-rpc` and a real public node: the four-field modal from the keyboard alone, an inline
refusal for a name that already existed, the seed-backup modal carrying core's own hint, then a
balance of `0.0000 XMR` and a clean close — and no seed was ever displayed. The public-node picker
was driven against the real monero.fail (`node list --refresh --probe --limit 8`: eight probed,
eight reachable, one height between them, and the node this machine is configured on the slowest of
the eight at 3 344 ms against 43 ms for the fastest) and its confirm modal really switched the node,
re-rendering the sandboxed wallet unit through the same core call the CLI makes. The **mouse** was
driven with real SGR-1006 reports written to the pane, which is byte-for-byte what a terminal sends:
rows select and activate, chips fire, the wheel scrolls the list under the cursor and does nothing
over the tab strip, right-click is `Esc`. That round found the tab strip's hit rects two columns per
tab out of line — clicking `Send` selected `Log` — and it is fixed, with a test that clicks the
**rendered** label rather than the hit map; the full transcript of both, and of the picker's address
column, is in `DECISIONS.md` under "TUI UX round verification" and "TUI UX fix-up".

**Three defects that live run found, all three since fixed and re-checked against the same public
node.** `fmonero doctor --fix` used to rewrite `node.mode` from `remote` back to `local` on a
correctly configured machine, from a `settings_valid` warning that was itself false; the check now
validates the whole document rather than one key at a time, and `doctor --fix --dry-run` in remote
mode plans nothing. `wallet create` against a remote node could exit 5 `killed_at_deadline` **after
the wallet file had been created**, taking its "back up your seed" hint with it; the create and open
sequences now run under `timeouts.refresh_sec` — every one of their calls waits behind the new
wallet's first refresh against the node — and the hint is printed the moment the wallet file exists,
on every path. Three consecutive `wallet create` against a real public node now exit 0 (4.7 s,
14.8 s, 6.8 s; the middle one is longer than the old 5 s deadline). And `send.lock` could answer
`send_in_progress` with nobody holding it, because a `flock` outlives a sibling thread's `fork`
until the child `exec`s; it now retries for 500 ms first, which is three orders of magnitude clear
of a fork window and one clear of a real holder. All three are written up in full, with their real
output, in `DECISIONS.md`.

**RED, and therefore unverified.** A real `node up`, `node down` or `node restart` against
`monerod`. Real sync progress, a real peer list, a real balance, a real `refresh`. A real send
or sweep of any amount. A real `open_wallet` against a real wallet file. `setup`'s refusal under
Arch's `system_unit` layout, which nothing has exercised — it needs Arch's own `monerod.service`
on the machine — and which keeps `setup` RED as a command even though the run above is GREEN.
The 120 GB figure itself, which is an estimate rather than a measurement (only the refusal it
drives is exercised). The warm `status` timing. Every doctor check in the node, wallet and
storage groups against a live daemon.

`DESIGN.md` §4b lists the exact commands that turn those green, in order, and they run on
**stagenet** first: stagenet coins are free from a faucet, so the send path is exercised the
first time with nothing at stake. `fmonero settings set node.network stagenet` and
`fmonero setup --units --yes` is the whole switch.

## CLI

Every command takes `--json` and prints one JSON object; a non-TTY stdout implies `--json`.
Failures are
`{"ok":false,"schema":1,"code":N,"code_name":"…","error":"…","hint":"<runnable command>","reason":"…",
"detail":"…"|null,"action":"…"|null}`
with the same exit code, and every human-mode error ends in a `Try:` line naming a runnable
command. `error` is one plain sentence; `detail` is the technical half of it — the transport's own
words, the name of the call — and it is printed in human mode only with `-v`.

```
fmonero                       TUI on a terminal; `status --json` when stdout is not a TTY
fmonero status                the whole Status document (schema 1); starts nothing
fmonero node up [--wait] [--wait-timeout SECONDS]
fmonero node down · node restart            (both need --yes; user unit only)
fmonero node info             sync, fork, fees, txpool, limits
fmonero node log [--lines N] [--grep TEXT]
fmonero node peers [--white|--gray] [--limit N]
fmonero node bans · node ban HOST [--seconds N] · node unban HOST
fmonero node limit [--up KBPS] [--down KBPS] [--out-peers N] [--in-peers N]
fmonero wallet list · open NAME [--password-file P] · close · create NAME [--language L]
fmonero wallet balance [--account N] [--subaddresses]
fmonero wallet address [--account N] [--index N] [--qr] · new-address [--label TEXT]
fmonero wallet history [--limit N] [--in] [--out] [--pending] [--failed] [--pool]
                       [--since HEIGHT] [--cached]
fmonero wallet send TO AMOUNT [--priority 0-4] [--note TEXT] [--max-fee AMOUNT] --yes
fmonero wallet sweep TO [--below AMOUNT] [--max-fee AMOUNT] --yes
fmonero wallet clear-pending [--yes] · validate ADDRESS · refresh [--start-height H]
fmonero wallet book list|add ADDRESS DESC|remove INDEX · notes get TXID|set TXID TEXT
fmonero wallet use-node ADDRESS [--trusted] --yes
fmonero settings list|get KEY|set KEY VALUE
fmonero setup [--units] [--plugin] [--menu] [--all]
fmonero doctor [--fix] [--only GROUP] [--dry-run] [--save] · doctor undo latest
fmonero tui [--node|--wallet|--send|--log]
fmonero capabilities · fmonero robot-docs · fmonero version
```

Global flags: `--json`, `--timeout <SECONDS>` (lowers every deadline, never raises one),
`-v`, `--no-color`, `--yes`, `--dry-run`, `--force`. `--dry-run` is implemented by exactly four
commands (`wallet send`, `wallet sweep`, `setup`, `doctor`) and exits 2 on any other, because a
flag that means "change nothing", accepted everywhere and ignored on a mutating command, is
worse than one that does not exist. `--force` ignores an armed stall breaker for one
invocation. It does nothing else: it never implies `--yes` and never bypasses a money check.

Exit codes: 0 ok · 1 failure · 2 usage · 3 safety (needs `--yes`) · 4 backend · 5 upstream
(a daemon errored or was killed at its deadline) · 6 not found · 7 ambiguous · 8 timeout ·
9 wallet locked · 10 node syncing · 11 doctor found failures · 12 a fixer failed ·
13 doctor refused · 14 insufficient funds · 15 invalid address · 16 tx rejected ·
130 interrupted.

Amounts are a decimal XMR string (`0.5`, `1.234567891234`, at most 12 decimals, no separators
and no exponent) or raw piconero with a `p` suffix (`250000000000p`). One XMR is exactly
1 000 000 000 000 piconero. Recipients are an address or `@label` from the wallet's address
book, matched case-insensitively; a bare word is never guessed at as a label.

### For agents

```
fmonero robot-docs                        # the handbook, once
fmonero capabilities --json               # commands, exit codes, reasons, settings, warning ids
fmonero status --json | jq .node.reachable
```

Branch on `code_name` plus `reason`, never on prose. `reason` is a stable discriminator within
an exit code, because exit 3 alone covers eight unrelated conditions and the reflex on exit 3,
appending `--yes`, is right for one of them and dangerous for the rest: `needs_yes` ·
`pending_send` · `send_in_progress` · `fee_above_max` · `foreign_node` · `data_dir_in_use` ·
`wallet_exists` · `full_chain_refused`. `capabilities.reasons` is generated from the table the
error type carries, so the documented set and the constructible set cannot drift.

With no TTY, `FMONERO_WALLET_PASSWORD` or `--password-file` is how a wallet gets opened;
`wallet open` exits 9 with that hint rather than waiting on a prompt that cannot appear.

## Money safety

The send path is the reason this project has a design document.

**The fee you are shown is the fee of the transaction that was built.** `send` and `sweep`
build once with `do_not_relay: true` and read the real `fee` off that response. fmonero never
estimates a fee, never multiplies the node's `get_fee_estimate` by anything, and never shows a
number it did not receive from a built transaction. Without `--yes` the built transaction is
discarded and the command exits 3, which is safe precisely because `do_not_relay` marks no
output as spent. With `--yes`, the same process relays that same transaction: there is no
second build anywhere in the path. `--dry-run` always beats `--yes`. `--max-fee AMOUNT` is the
one bound that survives the discard-and-rebuild, and it is enforced inside the relay step
rather than by the caller.

**A crash cannot double-send.** The whole record of what is about to be broadcast, every txid
in relay order, is written to `~/.local/state/fmonero/pending.json` at mode 0600 and fsynced
before the first `relay_tx`; each entry flips to relayed and is re-fsynced before the next
call. The transaction metadata is never written to disk, because it carries the transaction
secret key and crash safety needs only the txid. A later `send` that finds an unrelayed entry
refuses with exit 3. One that finds a fully relayed record asks the wallet about each txid and
clears the record when all of them are known, so the happy path leaves nothing behind and
`wallet clear-pending` stays the manual escape hatch instead of something you learn to type
after every send.

**Two senders cannot race.** A build reserves nothing: the outputs are marked spent by the
relay, not by the build, so two fmonero processes would each select from the same unspent set.
`send` therefore takes a non-blocking `flock` on `state_dir/send.lock` before the record check
and holds it until the plan is dropped, which makes the entry check, the build, the record
write, every relay and the resolve step one critical section. The TUI's confirm modal holds it
for as long as the human takes.

Also, by construction: amounts are `u64` piconero end to end, and no `f64` touches a monetary
value anywhere, in Rust or in QML — the widget formats money from a pre-rendered string, and the
only `f64`s in the tree are a sync percentage, a byte rate, a byte-size formatter and a disk
figure, each named in `scripts/check.sh`. Recipients are
validated twice, structurally and then by the wallet service, and a recipient on the wrong
network is refused by name, with the wallet's own primary address deciding which network that
is. The request builder has no `unlock_time`, no `payment_id`, no `ring_size` and no
`subtract_fee_from_outputs` field, so none of them can be sent. `sweep` always sends
`subaddr_indices_all: true`, because without it the daemon picks one subaddress at random,
reports success, and leaves the rest. The relayed txid is compared with the one that was shown.

Every guarantee in this section is asserted against the mock server, which puts it in AMBER: it
proves the request shape, the call count, the record on disk and the refusal codes. A real
broadcast remains unverified, and `DESIGN.md` §4b has the stagenet rehearsal that changes that.

`wallet create` does not print the seed: a seed in terminal scrollback is a seed in your shell
history tooling. It prompts twice on a TTY, refuses an empty password unless you ask for one,
and ends by telling you to run `monero-wallet-cli --offline` and type `seed` before funding the
wallet. Restoring from a seed is not an fmonero command at all, which is what keeps every seed
out of this codebase.

## The widget

`plugin/oliver.monero` is a Quickshell bar widget for Omarchy 4. It draws the Monero mark in
the bar, coloured by node and wallet state, crossed out when your node is not answering, ringed
with a percentage while something is catching up, and badged when something is wrong.

Its panel answers one question and then gets out of the way. **One header line of at most five
words** — `Public node · synced · wallet closed`, `Wallet syncing 63%`, `Wallet open · 1.2345 XMR`
(the balance at four decimals, truncated: the bar never shows more than you have, and the panel's
own `Balance` row carries the exact figure) —
with at most one supporting fact under it, and then **only the rows that carry information for
the state you are in**: which node, whether it is synced and when it was last checked; the balance
and the address with a copy action when a wallet is open, or one row reading `Wallet closed` when
it is not; a payment on its way if there is one; and anything actually wrong, each with something
you can do about it. Two or three buttons: `Open wallet…` or `Check for payments`, `Choose node…`,
and `Start your node` / `Stop your node` when the node is yours.

A closed wallet is a **state and never a warning** — on a correctly set-up machine using a public
node the wallet service is meant to be down until you open one. The check's age always travels with
its block number, because the measurement came from an earlier invocation; while the panel is open
its poll asks fmonero to refresh that check in the background, so it stops going stale on the bar.
There is no "start the wallet service" button, and that is deliberate: fmonero has no `wallet up`,
so the button would have to be `systemctl --user start …`, which would skip the preflight that
turns a missing service into a readable error. `Open wallet…` opens the screen where it starts with
a progress indicator instead.

**While a wallet is catching up the widget says so and says it keeps going.** The header reads
`Wallet syncing 63%`, the icon's ring draws that progress, and the panel adds `This keeps going in
the background.` Closing the TUI's window does not stop it — the scanning happens inside the wallet
service — which is the same thing fbittorrent does with a transfer.

Everything it knows comes from `fmonero status --json`. The QML holds no business logic and
does no arithmetic on money. It cannot open a wallet, create one, send or sweep: there is no
password field and no such argv in it, and the smoke test greps for both. The widget itself is
verified by `tests/plugin-smoke.sh`; the daemon numbers it displays are unverified, like every
other real-daemon figure here. See `plugin/oliver.monero/README.md`.

## The TUI

`fmonero tui` is a FrankenTUI app with four screens: Node, Wallet, Send and Log. It calls the
same Rust library in process, so it never shells out to `fmonero`. Long actions run as
cancellable subscriptions; `Esc` cancels a build or a wallet open, and the one action that
ignores cancellation is the relay, because cancelling a transaction already on the wire tells
you nothing about whether the node got it. `q` refuses while an action is in flight and quits
on a second press.

**`Tab` and `Shift+Tab` always switch screens** — on every screen, in every focus state,
including while a field has focus, while a modal is open and while an action is in flight. There
are exactly two exceptions and both hold something that must not be silently dropped: the send
confirm modal (a built transaction) and the create-wallet modal (a typed password). Those two
consume the key and say `Esc to discard first` on their own last line. Inside the Send form the
fields move with `↑↓`, and inside a modal `↑↓` moves between its controls while `Enter` advances
from every one but the last, where it submits. `1 2 3 4` are the direct route; `⇥ screens` is
written on the status bar of every screen, so the rule can be read from wherever you are.

**The mouse is a peer, not an afterthought.** Click a tab, a table or list row (click selects,
click again opens — there is no double-click interval to get wrong), a Send form field, a
priority chip, either arrow of a `◂ value ▸` selector, any modal button, and every `[key]` chip:
if a screen tells you to press a key, that text *is* the button. The wheel scrolls the list under
the cursor. Right-click is `Esc` for whatever has focus. Nothing is reachable by hover alone, and
nothing clickable can broadcast a transaction or discard a built one — the confirm modals are the
safety surface and the mouse gets no shortcut through them. `fmonero settings set tui.mouse false`
gives the terminal its own selection and scrollback back.

**Node** carries the public-node picker (above), in both modes. With a public node it also
carries one ` This node ` pane built from the last check — `block 3 765 346`, `synced`, `answered
in 240 ms`, `checked 2m ago` — with `Not checked yet.` and `No answer when checked 2m ago.` as its
other two honest states, and `[f] Choose another node` beside them. **Wallet** with nothing open
is a panel with three buttons rather than an empty table: `[o] Open a wallet` (which names the
wallets it will find — `(1 wallet on this machine: moneroli)` — so you can see *why* it will not
help on a fresh machine), `[c] Create wallet…` and `[W] Start the wallet service` — or `[W] Stop the wallet service`
in the one state where that key stops it — over one line
saying which of `not running yet. It starts when you open or create a wallet.` / `not running` /
`starting…` / `running` / `running, but not answering yet` the service is, and one saying which node
is in force. There is no balance pane above it: a screen whose subject is that no wallet is open has
no balance to frame. `c` is create and `x` closes. Creating a wallet asks for a name, a password twice and a
seed language, then shows the backup command — and **fmonero never displays a seed**, anywhere:
a terminal scrollback is not a safe place for one. `fmonero settings set wallet.autostart true`
starts the wallet service when the TUI launches; it is off by default, because a tool that starts
something on launch has done something you did not ask for.

**Closing the TUI's window does not stop anything.** A wallet that is behind keeps scanning inside
the wallet service, and the Wallet screen says so while it happens: `Syncing 63% · 240 000 blocks
left.` over `This keeps going in the background, even after you close this window.` — one sentence,
`fmonero_core::SYNC_KEEPS_GOING`, which the two TUI screens, the bar widget and the warning behind
them all quote. The bar widget carries the same progress, so the window is somewhere to look rather
than somewhere to wait.

**No screen in the TUI tells you to run a command.** Every hint names a key or a chip — `Press o to
open a wallet`, `[Choose node…]` — and anything you do more than once has one. Four places still
print a command, each because fmonero has no code that could do the job from inside the app
(installing a package, reading a seed, restoring from one, and `wallet sweep`, which v0.1 keeps out
of the TUI); each is phrased `In a terminal: …`, and `docs/COPY.md` §2 lists them with the reason.

Paste an address into Send's **To** field with your terminal's paste shortcut (Ctrl+Shift+V
or Shift+Insert in the standard Omarchy terminal configuration). Paste also works in the amount,
note, and other text fields. Pasted line breaks stay text; they never submit the form or confirm
a payment. Use ↑↓ to move between Send fields.

The Send screen validates each keystroke locally, then builds once and shows a confirm modal
with the full destination address (never elided), the amount, the real fee, the total, the
account and the fee level. That modal wants the word `send` typed, not just Enter:
it is the TUI's `--yes`. Sweep is not in the TUI in v0.1.

## The doctor

`fmonero doctor` runs detectors over captured input and prints a table (or JSON with `--json`):
one row per check: what was checked, then `OK`, `needs attention`, `not working` or
`not checked: <why>`, then the detail under it and, where one exists, a command you can run
yourself. `--json` carries the same rows with their ids, statuses and reasons, which is what agents
branch on. Exit 11 means it found failures, so
`fmonero doctor --json >/dev/null; echo $?` is the health check for scripts. A check whose
precondition is missing reports `skip:<reason>`, never `pass`.

`--fix` applies only repairs that are reversible, and every mutation goes through one
chokepoint that appends its inverse to `runs/<id>/actions.jsonl` — with a verbatim backup and a
SHA-256 before and after for every file it rewrites, and a recorded inverse for everything else
(a created directory records whether it existed, a symlink its old target, an argv its reversal).
`fmonero doctor undo latest` replays that log in reverse and refuses with exit 13 if a backup is
missing or its hash no longer matches. The doctor never stops a daemon it did not start, never
touches a wallet file, never deletes a file — an undo moves a file the run created into that
run's `quarantine/`, and removes only a directory the run created itself, while it is still
empty — and never runs sudo. The pending-send record is report-only in every state, a fully
relayed one included: it is the one check whose fixer could lose money.

## Layout

```
crates/fmonero-core/     the engine: rpc/{digest,jsonrpc,node,wallet}, send, session, units,
                         history, status, state, settings, amount, address, proc, agent
crates/fmonero/          the binary: clap surface, output, setup, prompt, doctor/, tui/
crates/fmonero-mock/     test-only fixture server; publish = false, never installed
plugin/oliver.monero/    the Quickshell bar widget
scripts/install.sh · scripts/uninstall.sh · scripts/check.sh
tests/e2e.sh · tests/plugin-smoke.sh · tests/fixtures/
docs/ROBOT.md            generated by `fmonero robot-docs`, never hand-edited
docs/FEASIBILITY.md      the research statement and the architecture decision
DESIGN.md                the contract every agent works from
DECISIONS.md             append-only log of non-obvious choices, with real command output
```

Core is silent: no function in `fmonero-core` writes to stdout or stderr, and a grep enforces
it. All output is produced by the binary. Every core function returns a typed error with one
variant per exit code, and every error carries a runnable hint.

## Config

`~/.config/fmonero/config.toml`, through `fmonero settings`:

```toml
[node]
network  = "mainnet"                    # mainnet | stagenet | testnet, one at a time
url      = "http://127.0.0.1:18081"     # loopback http only
unit     = "fmonero-monerod.service"
data_dir = "~/.bitmonero"
log_file = "~/.bitmonero/bitmonero.log"
prune    = true                         # a full chain does not fit on this machine

[wallet]
url      = "http://127.0.0.1:18083"     # 18082 is monerod's ZMQ port
unit     = "fmonero-wallet-rpc.service"
dir      = "~/.local/share/fmonero/wallets/mainnet"
port     = 18083
account  = 0
priority = 2                            # normal; 0 is an alias of the cheapest tier

[timeouts]
node_sec = 3
wallet_sec = 5
refresh_sec = 120
transfer_sec = 120
open_sec = 300
systemctl_sec = 10

[tui]
poll_ms = 1000
tick_ms = 120
```

A `~` you type is expanded once when the file loads, and paths are written back absolute.
An unknown key is redirected rather than merely refused: `settings set` names the nearest real
key and makes its hint that exact runnable command. Switching `node.network` moves the ports,
the wallet directory and both units together, so `setup --units` has to run again; the unit
names never change.

State lives in `~/.local/state/fmonero/` (`state.json` for the stall breakers, the net-rate
sample pair, the 60 s cold cache and the last error; `pending.json` for the send record;
`send.lock`; doctor runs and backups), the transfer-history snapshot in
`~/.cache/fmonero/history.bin`, wallets in `~/.local/share/fmonero/wallets/<network>/`, and
the chain wherever `node.data_dir` points. Everything fmonero creates is mode 0600, every
directory 0700, at the target mode rather than by a `chmod` afterwards. The `run/` subdirectory
of the state directory is the wallet service's working directory: `monero-wallet-rpc` generates
its own RPC credential there on every start and deletes it on exit, and fmonero reads that file
and never writes it.

## Limitations

v0.1 is deliberately narrow. None of the following exists, not as a stub, a flag or a menu
entry:

- Seed entry. `wallet restore` is cut, so no seed ever reaches this code.
  `monero-wallet-cli --restore-deterministic-wallet` does it with word-by-word validation and a
  restore-height prompt; `wallet list` picks the wallet up the moment it exists.
- A send that needs more than one transaction. `transfer` refuses outright with `TX_TOO_LARGE`
  when the wallet would have to split, which is the ordinary outcome on a wallet of many small
  outputs. fmonero builds exactly one transaction and, on that error, tells you to sweep or to
  send less.
- Multisig, hardware wallets, mining, Tor and i2p, proofs, cold signing, key or seed display,
  rescans, account tags, freeze and thaw. Each is a separate screen and a separate safety story.
- Several wallets open at once. `monero-wallet-rpc` holds exactly one.
- Cuprate as a node backend. `sync_info`, `get_connections`, the peer list, bans, txpool stats
  and net stats all return `not_available`, so it cannot drive this dashboard.
- Sweep in the TUI, a doctor screen inside the TUI, desktop notifications
  wired on, ZMQ, a CI workflow and a CHANGELOG.
- Mouse **drag** and any hover-only affordance. Clicking is in (see "The mouse is a peer"
  above); dragging a splitter, reordering a list and a highlight a keyboard cannot produce are
  not, because an affordance only a pointer can reach is one a keyboard user does not have.

Two honest estimates rather than measurements: a pruned mainnet chain is put at 100 to 150 GB,
and the doctor checks free space against a 120 GB floor while saying in the detail that the
floor is an estimate. FCMP++ and Carrot are not activated (the mainnet fork table ends at v16),
and fmonero is arranged so that adding a Carrot address form is one row in one table plus one
fixture object, with a test that fails if you add one without the other.

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
