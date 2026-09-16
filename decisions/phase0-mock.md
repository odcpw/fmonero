# Phase 0 — fixtures and the mock server

Agent: Phase 0, branch `phase0/mock`, own worktree, 2026-09-16/17.
Owned: `tests/fixtures/**`, `crates/fmonero-mock/src/**`, this file. Nothing else was touched — in
particular no `Cargo.toml` (Phase 1a's, §8) and no `crates/fmonero-mock/Cargo.toml`.

Phase 1b folds this file into `DECISIONS.md` (§8 preamble).

## What was built

- **`tests/fixtures/node/`** — 23 files. The three `get_info` variants (synced, syncing, and the
  restricted one carrying every poisoned field of §1b.3 with the line that poisons it), the
  `CHECK_CORE_READY()` `"BUSY"` block header, `sync_info`, `get_connections`, `get_peer_list`,
  `get_bans`, `set_bans`, `get_transaction_pool_stats`, `get_fee_estimate`, `get_version`,
  `hard_fork_info` (+ a voting variant), `get_net_stats`, `get_limit`, `set_limit`, `out_peers`,
  `in_peers`, `stop_daemon`, `set_log_level`, `error-restricted`.
- **`tests/fixtures/wallet/`** — 36 files, including a `get_transfers` with a row of all six `type`
  values (the coinbase `block` row inside the `in` list) and both branches of the `unlock_time`
  formula, the build/relay/resolve trio sharing one txid, a two-transaction `sweep_all`, five
  `validate_address` fixtures dispatched on `_request_address`, and the three reference error shapes.
- **`tests/fixtures/address-forms.json`** — the nine forms, array-valued `leading`, nine real
  checksum-valid examples from Monero's own test suite.
- **`tests/fixtures/doctor/`** — 22 cases over the seven parsing detectors of §7.1.
- **`tests/fixtures/README.md`** — per-file provenance with `file:line`, in fproton's format. **It is
  the reference for the mock's argv, the Digest rules, the log shape, every deviation and every
  addition**; this file records only the gate and the decisions.
- **`crates/fmonero-mock/src/main.rs`** — a thin `main()` over `serve(routes)`.

Every field name and type was taken from the `*_commands_defs.h` structs and re-read in the
v0.18.5.1 clone; the citations are in each fixture's `_provenance`. The cross-fixture arithmetic was
**recomputed, not copied from DESIGN.md**: the base58 leading-digit sets, the 5 GiB rounding
(123480309760), `confirmations` and `locked` for all seven history rows, the sweep's
amount + fee = balance identity, the `inet_addr` byte order of `peer.ip`, and the postcard header bytes.

## Decisions taken where DESIGN.md left the form open

1. **JSON provenance is a top-level `_provenance` string whose FIRST LINE is §1.1's exact sentence.**
   §1.1 prescribes the key; making its first line the literal header sentence is what makes the gate
   mechanical (`jq -re '._provenance' f | head -1`). Text fixtures carry the sentence as line 1.
2. **The mock strips every top-level key beginning with `_` before serving.** One rule covers
   `_provenance` and `_request_address`; no Monero RPC response field starts with an underscore.
3. **Doctor case variants use fproton's `<check-id>__<variant>` suffix** (its own
   `conflicting_tunnel__exit_node`).
4. **`expect.json` pins `{id, group, severity, class, status, fixable}` plus `detail_contains`**, never
   whole prose — a fixture that fails on a reworded message is a fixture that gets relaxed. `_env` and
   `_settings` say how to set each case up; `_settings` is written into `config.toml` directly, because
   `node_layout__remote` needs a `node.url` that `settings set` refuses (§2c).
5. **`FIXTURE_ALIAS`, one entry.** §8 names the transfer fixture `transfer-do-not-relay.json` while the
   wire method is `transfer`. An explicit one-row table beats both a duplicate file and a
   "single `<method>-*` match" rule that would silently stop working when a second transfer scenario
   is added.
6. **No fallback answer for an unknown method.** An unknown json_rpc method gets `-32601` (what
   `monerod` answers) and an unknown legacy path 404. An "answer `{}` for anything" fallback would hide
   a missing fixture behind a passing test, so every method the mock serves has a file.

## Deviations, with reasons

Full text in `tests/fixtures/README.md`, "Deviations". In brief:

1. **`connection_info.ssl` omitted** — §1b.3 lists it, but it has no `KV_SERIALIZE` entry
   (`cryptonote_protocol_defs.h:52` vs the map at `:89-115`) and never reaches the wire. A required
   `ssl` field in `Connection` would fail to deserialize every row of a real `get_connections`.
2. **`get_accounts`' request has a third field, `regexp`** (`:269`), which §1b.4's row omits. Harmless
   — fmonero sends none of the three — but the row is incomplete.
3. **The request log carries `conn`, `status` and `nc` beyond §8's five keys.** Each is needed by a
   gate §8 itself specifies; every §8 key keeps its meaning. Without `conn` the keep-alive and
   concurrency gates cannot tell one socket from two.
4. **`address-forms.json` wraps its nine objects under `forms`** so the file can also carry
   `_provenance` as §1.1 requires. Count is `jq '.forms | length'`.
5. **The mock is 597 rustfmt-formatted lines, not under 500 (§8).** See below.
6. **`--fail` on a legacy endpoint ignores the CODE** and puts the MESSAGE in `status`, which is the
   daemon's error channel there (§1b.3). A legacy endpoint has no JSON-RPC error channel.

### The line budget, honestly

```
total (rustfmt-formatted) : 597
tests module              :  52
non-test                  : 545
non-test code only        : 425      <- the implementation
comment lines             :  89      <- the file:line citations
```

`rustfmt` at the workspace's default settings (no `rustfmt.toml`; `max_width = 100`, default
`use_small_heuristics`) is what fixes this number: the same source is ~100 lines shorter unformatted,
and `cargo fmt --check` is a Phase 1b and Phase 3 gate, so the formatted count is the real one. Three
rounds of compaction took it from 720 to 597, including discovering that `rustfmt` explodes a 64-entry
`u32` array one-per-line when the literals carry `_` separators (11 chars is past its "short item"
threshold) — dropping the separators alone saved 55 lines.

The single largest item §8 forces on itself is the inline MD5: capping dependencies at `serde_json`
plus std costs ~55 lines for the `K` table and the compression function, and epee's Digest is MD5-only
(`http_auth.cpp:155`), so there is no way to verify a Digest response without it.

Every remaining route to <500 deletes something real — the three self-tests, which prove the MD5 and
the RFC 2617 formula the entire Digest half rests on, or the `file:line` citations. Long-form rationale
was already moved out of the source into `tests/fixtures/README.md` specifically to claw lines back.
Phase 0 kept both and reports the overage rather than making that trade silently.

## Additions beyond §8's deliverable list

Twelve wallet fixtures and three node fixtures §8 does not name, each because a test DESIGN.md names
elsewhere has no input without it. The table is in `tests/fixtures/README.md`, "Additions". The two
that matter most: `relay_tx-mismatch.json` (§2a.9's test needs a **successful** `relay_tx` whose
`tx_hash` disagrees with the build — `--fail` yields an error, a different code path) and
`get_transfer_by_txid-failed.json` (§2a.4 needs a **successful** response whose `type` is `failed`).
Also `store.json` and `set_tx_notes.json`: without them every `wallet send --yes` against the mock 404s
at its last two steps, i.e. **after the money moved**.

## Design gap reported to the orchestrator

**Six §7.1 checks must read the two unit FILES, and §2i freezes no override that points them at a
fixture directory.** §2i's seam is `FMONERO_PROC_ROOT` and `FMONERO_SYSTEMCTL_FIXTURE`, and
`Systemctl::Fixture` answers only is-active / is-enabled / exists / MainPID. `units_present`,
`unit_loopback`, `unit_no_password_in_argv`, `unit_no_node_wants`, `wallet_dir_matches_unit` and
`wallet_port_consistent` read `~/.config/systemd/user/fmonero-*.service`, and `units::install` refuses
to run at all when `FMONERO_STATE_DIR` is set (§4.2), so no test may write a real unit.

`probes.rs` is the Phase 2 doctor agent's and §2k forbids Phase 0 from touching it. The fixture bytes
are in place; the two resolutions are named in the affected `expect.json` files:

- **(a)** add `FMONERO_SYSTEMD_USER_DIR` to `Paths`, honoured only when `FMONERO_STATE_DIR` is set —
  the same guard §2i puts on its own two overrides. One field, one line. The `_env` keys assume this.
- **(b)** have `probes.rs` resolve the unit directory relative to `FMONERO_STATE_DIR`.

**Until one is chosen those six cases cannot run and must be recorded as skipped, never as passing**
(§2j). A related, smaller instance: `node_data_dir_single_writer__via_config_file`'s cmdline names
`/etc/monerod.conf`, which no agent on this machine can create; the case needs the harness either to
rewrite that argv at setup time (it is one NUL-joined string, and the file is generated) or to resolve
a conf path through `paths.proc_root`.

## Bugs this gate found in Phase 0's own work

1. **The nonce was sharing the connection counter**, so accept-order `conn` ids in the log skipped
   (0, 2, 3, …) and read as a lost connection. Fixed with a dedicated `nonces: AtomicU64`. Found only
   because the gate prints the log.
2. **Five doctor text fixtures had a *variant* of §1.1's header sentence** ("… source and DESIGN.md
   §2i, not captured …"), which the mechanical check rejected. Normalised to the exact sentence with
   the elaboration on line 2.
3. **`curl --digest` cannot drive §2c's `nc` assertions at all.** It re-runs the 401 handshake per
   transfer — even with `--next` on one handle and one socket — so the `nc` it sends never leaves
   `00000001` and a nonce rotation is invisible. The property needs a client that keeps one session
   across requests on one connection, which is exactly the shape §2c gives fmonero's `ureq` client.
   A 70-line Python client was written to close the mock's half; the client's half stays in Phase 1b.
   **This is worth knowing before Phase 1b writes that test.**

## Gate

§8's Phase 0 gate. Run from the worktree root on 2026-09-17. Two lines are **not** closable by this
phase and are recorded as such, not waved through (§2j):

- **`cargo build -p fmonero-mock`** cannot resolve: `crates/fmonero-mock/Cargo.toml` and the workspace
  `members` list are Phase 1a's (§8), and Phase 1a was writing them on `main` while this ran. The
  substitute, which §8's own worktree paragraph anticipates: the source was copied into a throwaway
  crate at `temp/mock-check/` (gitignored) depending on `serde_json` only, and there
  **`cargo build --release`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` and
  `cargo test` are all green.** The file committed here is the `cargo fmt` output, so Phase 1b's
  `cargo fmt --check` on the merged tree cannot fail on it. `cargo build -p fmonero-mock` itself is
  **Phase 1b's** line, after it merges this branch — and it will overwrite the `fn main() {}`
  placeholder Phase 1a left at that path, as §8 says.
- **`--fail` end to end** is §8's own deferral: `--role wallet --fail get_balance=-17:x` making
  `fmonero wallet balance` exit 14 asserts the §1b.4 error map, which does not exist until Phase 1b.
  The flag is implemented and its wire effect is visible in the transcript below only indirectly; the
  exit-code assertion is Phase 1b's.

Also substituted, trivially: the log path is a scratch directory rather than `/tmp/m.jsonl`, and §8's
"two `-d` bodies to the same URL" became `curl --next`, because two `-d` bodies are **concatenated**
into one invalid JSON document — it proves keep-alive while sending garbage, and `--next` proves it
while sending two real requests.

```console
$ cargo build --release --offline          # in temp/mock-check
    Finished `release` profile [optimized] target(s) in 3.63s
$ cargo clippy --all-targets --offline -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.36s
$ cargo fmt -- --check                     # no diff
$ cargo test --offline
running 3 tests
test tests::md5_matches_rfc1321_edges ... ok
test tests::auth_params_splits_quoted_and_bare_values ... ok
test tests::digest_matches_rfc2617_and_differs_from_md5_sess ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The gate itself, verbatim:

```console
=== gate 1: jq -e . on every fixture
  82 JSON fixtures, 0 unparsable
  PASS  jq -e . on all 82

=== gate 2: provenance header in every derived file
  82 JSON fixtures (ALL of them, doctor expect.json included), 0 without the header line
  PASS  provenance header (JSON, _provenance[0])
  18 derived text fixtures under doctor/, 0 without the header line
  PASS  provenance header (text, line 1)

=== gate 3: address-forms.json object count
  forms: 9
  PASS  nine address forms
  forms whose leading set has more than one member: 1
  PASS  array-valued leading, one two-digit form

=== gate 4: get_info height over json_rpc
  .result.height = 3541122
  PASS  get_info height is a number

=== gate 7: two requests on ONE connection (keep-alive)
  req1 http=200 new_connections=1
  req2 http=200 new_connections=0
  req1 -> height 3541122
  req2 -> fees [20000,80000,320000,4000000]
  distinct conn ids in the last two log lines: 1
{"seq":1,"conn":1,"method":"get_info"}
{"seq":2,"conn":1,"method":"get_fee_estimate"}
  PASS  two requests answered on one socket, both logged

=== gate 8: two CONCURRENT curls, one mock, distinct seq
  get_info height=3541122   sync_info height=3541122
{"seq":3,"conn":2,"method":"get_info"}
{"seq":4,"conn":3,"method":"sync_info"}
  distinct seq: 2   distinct conn: 2
  PASS  both concurrent clients answered, distinct seq and conn

=== gate 9: one log line per request
  log lines so far: 5 (1 + 2 + 2 = 5 requests issued)
  PASS  one line per request
  every line is a JSON object with the §8 keys plus the documented superset:
  PASS  log keys

=== gate 5: --restricted makes sync_info answer -32601
  sync_info .error.code = -32601
  PASS  restricted json_rpc -> -32601
  POST /get_peer_list HTTP status = 404   (epee answers 404, NOT -32601)
  PASS  restricted legacy -> 404
  get_info still answers under --restricted: height = 3541122  (it is not a gated method)
  PASS  ungated method still served

=== gate 6: --scenario restricted picks get_info-restricted.json
{"restricted":true,"free_space":18446744073709551615,"version":"","start_time":0,"database_size":123480309760}
  restricted, free_space = true 18446744073709551615
  PASS  --scenario restricted

=== gate A: --auth, 401 then 200 with a correct Digest (curl --digest)
  unauthenticated POST:
    HTTP/1.1 401 Unauthorized
    WWW-authenticate: Digest qop="auth",algorithm=MD5,realm="monero-rpc",nonce="cfe37b6579d3642d9247869c0d6df281",stale=false
    WWW-authenticate: Digest qop="auth",algorithm=MD5-sess,realm="monero-rpc",nonce="cfe37b6579d3642d9247869c0d6df281",stale=false
  with --digest:
    .result.height = 3541122
  PASS  401 then 200 with a correct Digest
  the log shows the 401 and the 200, and the nc the client sent:
    {"seq":0,"conn":0,"status":401,"nc":null}
    {"seq":1,"conn":1,"status":401,"nc":null}
    {"seq":2,"conn":1,"status":200,"nc":"00000001"}
  wrong password is refused:
    HTTP 401
  PASS  wrong password -> 401

=== gate B: validate_address dispatches on the request address
  42ey1afDFnn4.. -> {"valid":true,"integrated":false,"subaddress":false,"nettype":"mainnet"}
  8AsN91rznfkB.. -> {"valid":true,"integrated":false,"subaddress":true,"nettype":"mainnet"}
  4BxSHvcgTwu2.. -> {"valid":true,"integrated":true,"subaddress":false,"nettype":"mainnet"}
  53teqCAESLxe.. -> {"valid":true,"integrated":false,"subaddress":false,"nettype":"stagenet"}
  42ey1afDFnn4.. -> {"valid":false,"integrated":false,"subaddress":false,"nettype":""}
  totally-not-.. -> {"valid":false,"integrated":false,"subaddress":false,"nettype":""}
  _provenance and _request_address are stripped from every served payload:
    _provenance occurrences in a served get_balance: 0
  PASS  meta keys stripped
  transfer resolves through FIXTURE_ALIAS to transfer-do-not-relay.json:
{"tx_hash":"a1e7b3c05d928f4617ac0e3b85d2f719c46b0a8e3f512d7409bc6ea18d35047f","fee":30720000,"tx_key":"","has_metadata":512}
  an unknown method answers -32601, as monerod does:
{"code":-32601,"message":"Method not found"}

=== gate C: --rotate-nonce-after forces a stale 401; the client resets nc to 1
  client view (one socket, one session, seven requests, rotation after two authorized):
    401 nc=-         stale=false challenges=2 nonce=99e1fb17
    200 nc=00000001  stale=-     challenges=0 nonce=99e1fb17
    200 nc=00000002  stale=-     challenges=0 nonce=99e1fb17
    401 nc=00000003  stale=true  challenges=2 nonce=90115ec2
    200 nc=00000001  stale=-     challenges=0 nonce=90115ec2
    200 nc=00000002  stale=-     challenges=0 nonce=90115ec2
    401 nc=00000003  stale=true  challenges=2 nonce=ef6d52fc
  the log the mock wrote:
    {"seq":0,"conn":0,"status":401,"nc":null}
    {"seq":1,"conn":0,"status":200,"nc":"00000001"}
    {"seq":2,"conn":0,"status":200,"nc":"00000002"}
    {"seq":3,"conn":0,"status":401,"nc":"00000003"}
    {"seq":4,"conn":0,"status":200,"nc":"00000001"}
    {"seq":5,"conn":0,"status":200,"nc":"00000002"}
    {"seq":6,"conn":0,"status":401,"nc":"00000003"}
  distinct conn ids: 1     nc on each 200, in order: 00000001 00000002 00000001 00000002 
  PASS  all seven requests on one socket
  PASS  nc increments on one nonce, and resets to 1 after the rotation
  401s at seq: 0 3 6   (seq 0 is the opening challenge; 3 and 6 are the stale=true rotations)

=== 19 passed, 0 failed
```

`gate 2` covers the whole fixture set with ONE rule and no exclusions: all **82** JSON files carry
§1.1's sentence as the first line of `_provenance`, and all **18** derived text files carry it as line
1. It did not start that way — the doctor `expect.json`s and five text fixtures had written variants of
the sentence ("… source and DESIGN.md §2i, not captured …"), which the mechanical check rejected.
Normalising them all to the exact sentence, with whatever the variant added moved to line 2, replaced a
`! -path '*/doctor/*'` exclusion with a rule that has no exceptions to remember.

`gate C` is the one worth reading twice. Seven requests, **one socket** (`conn: 0` throughout), against
`--rotate-nonce-after 2`: `nc=00000001` 200, `nc=00000002` 200, then a **`stale=true` 401 with a fresh
nonce**, then `nc=00000001` again. That is §2c's (i), (ii) and (iii) proved at the mock's end — the
client picks the MD5 challenge and never MD5-sess (two challenges are offered on every 401 and only the
MD5 HA1 is accepted), the counter increments within a nonce, and it resets to 1 when the nonce changes.
