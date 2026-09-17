# Phase 2 — doctor

Agent: Phase 2 `doctor` (DESIGN.md §8's Phase 2 table, row **doctor**). Branch `phase2/doctor`,
based on `gate-1b`, in its own worktree. Date **2026-09-17**.

**Owns** `crates/fmonero/src/doctor/**`, `tests/fixtures/doctor/**` (handed over by Phase 0, and
this agent may edit them — §8) and this file. Nothing else was touched: `main.rs` already
dispatches `doctor::run(args, ctx)` and `DoctorArgs` kept every field it froze at the 1a gate
(§2k, §5b).

## What was built

Five files, ported from `fproton/crates/fproton/src/doctor/{mod,checks,probes,mutate,report}.rs`
and adapted to §7.1's table, plus one test module:

| File | Lines | What it is |
|---|---|---|
| `probes.rs` | 1 269 | the **only** place the doctor spawns a process, reads a file or issues an RPC |
| `checks.rs` | 1 961 | **37** detectors, each a pure `fn(&Probes) -> Verdict`, in one static `CHECKS` table |
| `mutate.rs` | 1 598 | the **one** `--fix` chokepoint: scopes, verbatim backup, sha256 pair, `actions.jsonl`, `undo` |
| `report.rs` | 433 | `report.json` / `--json`, the human table, `doctor capabilities` |
| `mod.rs` | 511 | groups, `--only` parsing, the run loop, exit codes, run artifacts |
| `fixture_tests.rs` | 494 | the 22 `tests/fixtures/doctor/` cases, the class-A round trip, the read-only assertion |

Architecture, unchanged from §7 and the doctor kernel's axioms 1–3 and 7: detect-then-fix,
one mutation chokepoint, verbatim backup before the mutation, a recorded inverse, read-only by
default. The doctor **never removes a file**, never stops a daemon it did not start, never
touches a wallet file, never touches `pending.json`, and never runs sudo. `Action` has exactly
four variants — `WriteFile`, `CreateDir`, `CreateSymlink`, `RunArgv` — and no `RemoveFile`.

## Gate results

Every line of §8's doctor gate was run, on this machine, on 2026-09-17. The script is
reproduced at the end of this file and its full transcript follows it. Summary:

| Gate line | Result |
|---|---|
| `fmonero doctor --json \| jq` lists every §7.1 check with a real verdict and **exits 11** | **PASS** — 37 checks, exit 11 |
| binaries at `fail:not_installed`, exactly those two | **PASS** — asserted in jq |
| every `node`/`wallet`/`storage` check at `skip:*` | **PASS** — asserted in jq |
| `install` and `security` with real verdicts | **PASS** — every row carries a non-empty detail |
| `doctor --fix --dry-run` changes nothing (`git status` + a state-dir checksum) | **PASS** — both in a sandbox (5 planned) and against the real machine (4 planned); state dir, `$HOME` (mode+size+mtime of every file) and `git status` all byte-identical |
| class-A round trip: corrupt → `--fix` → assert → `undo latest` → compare | **PASS** — `sha256sum -c` on the corrupt bytes after undo: OK |
| …and an undo whose backup hash no longer matches | **PASS** — exit 13, `refused: true` |
| `doctor capabilities --json` valid | **PASS** — 37 checks, 7 groups, 3 classes, no class-D row auto-fixable |
| `doctor --only security` works | **PASS** — and every other group, and an unknown one exits 2 naming all seven |
| the `Verdict` status round trip (`pass`/`warn`/`fail`/`skip`) against §7's literal strings | **PASS** — `doctor::checks::tests::the_verdict_status_round_trips_through_its_documented_spelling` |
| fixture-driven tests for the parsing detectors | **PASS** — all 22 cases, all seven §7.1 detectors covered (asserted) |
| `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` | **PASS** — 31 tests, 0 failures, no warnings |

Nothing was skipped and nothing was waved through. The doctor needs no live daemon, so — unlike
most of §4b — **this phase's gate has no RED line**: every node/wallet/storage row is exercised
as a `skip:<reason>`, which is the verdict §7 and §4b both prescribe for this machine, and the
detectors behind them are exercised by the fixtures instead. What a fixture cannot prove is
named in "Not proved" below.

## Design deviations, each with its reason

### 1. A sixth §7.1 seam: `FMONERO_SYSTEMD_USER_DIR`, resolved inside `probes.rs`

Phase 0 reported this gap and named two resolutions (`tests/fixtures/README.md`, "Design gaps
reported"): six §7.1 checks — `units_present`, `unit_loopback`, `unit_no_password_in_argv`,
`unit_no_node_wants`, `wallet_dir_matches_unit`, `wallet_port_consistent` — read
`~/.config/systemd/user/fmonero-*.service`, §2i freezes only `FMONERO_PROC_ROOT` and
`FMONERO_SYSTEMCTL_FIXTURE`, and `units::install` refuses to run at all under
`FMONERO_STATE_DIR` (§4.2), so no test may write a real unit.

**Taken: the second resolution.** `probes::systemd_user_dir(ctx)` honours
`FMONERO_SYSTEMD_USER_DIR` when `FMONERO_STATE_DIR` is also set — the same guard §2i puts on its
own two overrides — and falls back to `ctx.paths.systemd_user_dir`. The first resolution (a
field on `Paths`) was the one Phase 0's `_env` keys assumed, but `crates/fmonero-core` is frozen
and is not this phase's to edit (§2k); this one lives entirely in a file this phase owns and
needs no signature change.

**Consequence for DESIGN.md:** §2i's test-seam paragraph and `agent::capabilities_json`'s `env`
list (§5b) should both name `FMONERO_SYSTEMD_USER_DIR`, `test_only: true`, beside
`FMONERO_PROC_ROOT` and `FMONERO_SYSTEMCTL_FIXTURE`. `agent.rs` is Phase 1b's file and is not
this phase's to edit (§2k), so the row is **reported, not added**.

`scripts/check.sh --full` still passes today, and precisely because its grep checks only one
direction — every *documented* variable is read. §5b asks for the **round trip** ("every
documented env var is read somewhere under `crates/` **and** every variable read is
documented"), and Phase 3's ergonomics pass is where that second half lands. It will find this
one. Nothing else reads an undocumented variable.

### 2. The doctor refuses every fixer that reaches outside `FMONERO_STATE_DIR`

**This is the one change in this phase that prevented real damage, and it was found by running
the gate rather than by reading.** The first `--fix` run of gate 3, in a `mktemp -d` sandbox,
wrote `~/.config/systemd/user/fmonero-monerod.service` and `fmonero-wallet-rpc.service` into
the **owner's real home** and would have run `fmonero setup --menu --yes` against the owner's
real menu file. (Both units were removed and `systemctl --user daemon-reload` run; the menu file
was never modified — its content and its pre-existing `.bak` are unchanged, and the gate now
checks the mode, size and mtime of every file under the five relevant `$HOME` trees before and
after.)

The cause: §4.2 makes `units::install` refuse to run when `FMONERO_STATE_DIR` is set, and a
doctor that renders and writes the two unit files *itself* walks straight around that guard.
DESIGN.md states the rule for `units::install` and not for the doctor.

So `mutate::write_scopes` is now test-aware: under a test state directory the scopes are exactly
the three redirected locations (`FMONERO_STATE_DIR`, `FMONERO_CONFIG_DIR`, `FMONERO_CACHE_DIR`),
and `RunArgv` is allowed only for `chmod` with in-scope path arguments — `systemctl --user
disable`, `omarchy plugin enable` and `fmonero setup --menu` reach outside by construction and a
path check cannot see it, because the path is not in the argv. `mutate::refused_here` asks the
question **before** the first mutation so a sandboxed `--fix` reports
`skipped: FMONERO_STATE_DIR is set, so the doctor will not …` rather than a failed fixer, which
would otherwise make every run under a test state dir exit 12 and bury the class-A round trip
the gate measures. A `--dry-run` is exempt: it mutates nothing and disclosing the blast radius is
its whole purpose. `under_a_test_state_dir_no_fixer_may_reach_outside_it` asserts both halves —
the pre-check and the chokepoint itself, which still refuses with exit 13.

**Consequence for DESIGN.md:** §7 should say it, in the same sentence that says the doctor never
runs sudo.

### 3. Redaction keeps the key and redacts the value — because a check has to name the flag

§2b.8 says `probes.rs` rewrites a captured line matching a credential key to
`<key>=<redacted>` before the bytes reach `runs/<id>/probes/`. Implemented literally as "replace
the line with `<key>=<redacted>`", it deleted the very flag `unit_no_password_in_argv` exists to
report: the `unit_no_password_in_argv__rpc_login` and `__password_file` fixtures both went from
`fail` to **`pass`**, on a unit carrying `--rpc-login`. §2b.8's own form is the resolution — the
**key survives** and the value does not — so `redact` now keeps every token and replaces only
the secret, in both the `key=value` and the `--flag value` shapes, and returns a line with no
credential key byte for byte.

Two consequences, both handled:

* `units_present` compares **byte for byte** against `units::render_*`, and comparing a redacted
  copy would report drift on a correct machine the day a unit ever carried such a flag. So
  `probes.rs` makes that comparison on the **unredacted bytes at capture time**
  (`node_unit_matches` / `wallet_unit_matches`) and the detector reads those booleans.
* Gate 8 proves the redactor fires end to end: a unit carrying `--rpc-login fmonero:SECRETVALUE`
  makes `unit_no_password_in_argv` report `fail:credential_in_argv` naming `--rpc-login`, while
  `SECRETVALUE` appears **0 times** in `report.json`, `actions.jsonl`, `facts.json` and every
  probe artifact, which instead hold `--rpc-login <redacted>`.

The **contents** of `monerod.conf` and of the wallet login file are still never carried at all
(§2b.8): only their modes, and the two booleans `prune_configured` and `wallet_daemon_auth` need.

### 4. `settings_valid` parses leniently, and the CLI cannot reach its unparsable case

§7.1's condition has two halves — "`config.toml` parses **and** every key is in the §4 table" —
and `Settings` derives `deny_unknown_fields`, so a strict deserialize of a document with one
stray key answers "does not parse" and hides the real defects behind it. `probes::inspect_settings`
therefore parses to a `toml::Value`, reports unknown keys by name, and validates each known key's
**value** through `Settings::set`. That is what makes the `settings_valid` fixture report all
four of its defects (`node.network`, `wallet.priority`, `timeouts.node_sec`, `wallet.acount`) and
what lets the class-A fixer keep every key that was fine, as `settings_valid__unparsable`'s own
note requires.

**A gap this phase cannot close.** `main.rs` calls `Context::load()?` before dispatch and
`Settings::load` returns `Error::Usage` on an unparsable `config.toml`, so **`fmonero doctor`
exits 2 on the machine whose `config.toml` is broken** — the one command whose job is to repair
it is the one the breakage locks out, which is exactly what
`tests/fixtures/doctor/settings_valid__unparsable/input/config.toml` warns about in its own
header. The detector and the fixer are correct and are proved by the fixture (which drives
`Probes::capture_install` directly); the CLI path is not reachable. Neither `settings.rs` nor
`main.rs` is this phase's file (§2k). **Reported, not worked around.** The fix belongs in one of
them: `Settings::load` degrading to the defaults and recording the parse error, or `main.rs`
special-casing `doctor` — the first is better, because `settings list` has the same problem.

### 5. `fixable` means "this finding can be repaired", and `settings_valid__ok` was changed

§7's check object has two notions of fixable and they are not the same. The per-run `fixable`
key means *this finding can be repaired* — it is what `summary.fixable` counts and what makes the
`fmonero doctor --fix --dry-run` next-step appear — and a check that **passes** has nothing to
repair. The table column ("this check has a class-A fixer") is `auto_fixable` in
`doctor capabilities --json`, where `settings_valid` is fixable in every state.
`tests/fixtures/doctor/settings_valid__ok/expect.json` pinned `fixable: true` on a passing
check; it is now `false`, with the reason written into its `_provenance`. Leaving it would have
made a clean machine advertise a repair for a file that is already correct.

### 6. Four `_provenance`-documented fixture edits (§8 hands this directory to this phase)

* **`_probe.monerod_installed: true` on all seven `node`-group cases.** Every `node` check
  reports `skip:monerod_not_installed` when `/usr/bin/monerod` is absent — §4b's GREEN row for
  this machine and §7.1's "every check whose precondition is missing reports `skip:<reason>`".
  No file under `input/` can carry that fact, so the case states it and the harness applies it
  before capturing. Without it the seven cases test the precondition, not the parser.
* **`_probe.wallet_height` and `_probe.wallet_primary_address` on the three
  `history_cache_fresh` cases**, both read from `tests/fixtures/wallet/{get_height,get_address}.json`
  — the same two files those cases' own notes already compute against. §7.1's condition is "the
  high-water height is within 1 000 of **the wallet's**" and the fingerprint is
  `sha256(primary_address)[..16]`; neither half is decidable from `input/history.bin` alone, and
  with no wallet the check honestly answers `skip:no_wallet_height`.
* **`input/history.bin` grew one byte, 30 → 31: a trailing `0x00`.** §5b's snapshot is *one*
  postcard struct — the header fields followed by `accounts: BTreeMap<u32, Vec<Transfer>>` — and
  postcard encodes a map as a length varint then its entries, so a 30-byte file ends before that
  length and `history::header` (which decodes the whole struct and reports `magic_ok: false` on
  any failure) could not read the header at all: all three cases came out `warn:bad_magic`.
  `0x00` is the empty map, which keeps the file header-only in substance — no `Transfer` bytes,
  whose layout is Phase 1a's — while making it the decodable snapshot the check reads.
* **`settings_valid__ok`'s `fixable`**, above.

Two further notes on the harness, both recorded in `fixture_tests.rs`'s own header:

* **No case sets an environment variable.** The process environment is global and `cargo test`
  runs these in parallel threads, so each `_env` key is applied to the `Context` field it
  resolves to (`paths.proc_root`, `systemctl = Systemctl::Fixture(..)`, `paths.cache_dir`,
  `paths.config_dir`, `paths.systemd_user_dir`). Same seams, reached by the field.
* **`node_data_dir_single_writer__via_config_file`** uses Phase 0's recommendation (a): the copied
  `/proc/<pid>/cmdline` has `/etc/monerod.conf` rewritten to the copied `input/monerod.conf`,
  because no agent on this machine can create `/etc` (§1.1). No product code, one NUL-joined argv.
  The case is therefore **run, not skipped**, and it is the one that catches a detector reading
  argv alone.

### 7. Readings of §7.1 this phase settled, and would settle the same way again

* **`node_layout` `system_unit` is `pass`, not `warn`.** §0 says Arch's node is "detected, used
  read-only, never fought" — a healthy layout, not a defect; the detail says so and names
  `sudo systemctl status monerod`. `remote` is `warn` (Phase 0's reading, kept): §2c makes a
  non-loopback `node.url` an error for *every* client, so a `config.toml` holding one is a
  machine on which nothing that touches the node can work.
* **`pending_send` is class D in every state**, a fully-relayed record included: a fully-relayed
  record is `warn` (the next `send` resolves it by itself, §2a.4) and an unrelayed one is
  `fail`, and neither is ever fixable. Of all the doctor's checks it is the only one whose fixer
  could lose money. Asserted by `a_pending_record_is_reported_and_never_fixed`.
* **`node_data_dir_single_writer` treats our own unit's `MainPID` as `pass`**, any other holder
  as `fail` naming the pid — §2i's distinction, applied to a report-only check.
* **`node_auth` skips rather than warning under the `system_unit` layout**: Arch's unit has no
  `rpc-login` and it is not ours to change (§2c).
* **`ports_owned` derives its port set from `node.network`** (p2p + `node.url`'s port +
  `wallet.port`, deduplicated) rather than hard-coding §7.1's literal `18080 / 18081`. For
  mainnet those are the same three numbers; for the stagenet rehearsal of §4b they are the right
  ones, which a literal pair would not be.
* **`state_perms` excludes the run directory's two daemon-owned names by name** and checks them
  for owner-only instead, exactly as §7.1 requires, and reports them class D with
  `systemctl --user restart` rather than chmod'ing a file the daemon recreates.
* **`history_cache_fresh` with no snapshot is `pass`, not `skip`.** An absent cache is not a
  missing precondition — there is no stale cache to report — and §4b asks the `install` group
  for real verdicts.
* **`doctor undo`'s `WriteFile` inverse quarantines rather than unlinks** when the run created
  the file, which is the question `units_present__absent`'s note asks the Phase 2 agent to
  decide: §7's "never removes a file" is unnarrowed, and a rename is not a removal. The file is
  moved to `runs/<id>/quarantine/<path>`. `CreateDir`'s inverse is the one removal §7 sanctions
  — `rmdir`, and only while the directory is still empty, asserted both ways.

## Not proved by this gate

* **Every `node`, `wallet` and `storage` check against a live daemon** is RED (§4b) and stays
  RED. The gate proves the verdict for a machine with no Monero, and the fixtures prove the
  parsers; neither proves what a real `monerod` answers.
* **`node_data_dir_single_writer`'s `fcntl(F_GETLK)` half** cannot be fixtured — a held POSIX
  record lock is live kernel state. §2i names its own forked-child test for it, which belongs to
  `crates/fmonero-core` (Phase 1b), not here. The fixture covers the `/proc` half only, and
  `tests/fixtures/doctor/node_data_dir_single_writer/expect.json` says so at length.
* **A passing `units_present` fixture cannot exist** (its own note explains why: the rendered
  bytes embed `$HOME`, `wallet.port` and the network). The passing case is the render → write →
  re-read round trip, and under a test state dir this phase's own guard (deviation 2) now
  forbids writing a real unit — so the passing case is proved by
  `the_journal_entry_is_written_before_the_mutation_and_the_round_trip_is_byte_exact` over an
  in-scope file, and the class-B unit fixer's *application* is a **real-machine** operation
  belonging to Phase 4's verifier.
* **`plugin_linked` resolves to the main checkout, never to this worktree** — the blind spot
  §7.1 names. On this machine it happens to `pass`, because the owner's link already points at
  `~/Projects/odcpw/fmonero/plugin/oliver.monero`.
* **`shellcheck` is not installed** (§4b's tooling note): the gate script was checked with
  `bash -n` instead. It is a scratch harness, not a shipped file — `tests/e2e.sh` is Phase 1b's
  and Phase 3 owns folding these lines into it.
* **`tests/e2e.sh` still records the three doctor rows as skipped** ("Phase 2 owns
  crates/fmonero/src/doctor") — that file is Phase 1b's and not this phase's to edit (§2k). Its
  §5c key sets are asserted here instead, in gate 3b, and Phase 3 should replace that skip with
  these assertions.

## Verified: `bash -n` on the gate script

```
$ bash -n decisions/phase2-doctor-gate.sh && echo "bash -n: clean"
bash -n: clean
```

`shellcheck` is absent from this machine, so it was **not** run and is not claimed (§2j).

---

## The gate script

Reproduced in full at `decisions/phase2-doctor-gate.sh`, so the transcript below can be
regenerated.

## The gate transcript, 2026-09-17

One line was removed from the capture: the single 16 KB raw-JSON line that `doctor --json`'s own
payload occupies in gate 1 (the same document is summarised above it, and `report.json` holds it
verbatim). Nothing else was edited.

```text

===== GATE 1: fmonero doctor --json | jq — every §7.1 check with a real verdict, exit 11
exit: 11
jq: valid JSON
GROUP        CHECK                          VERDICT
binaries     monerod_installed              fail:not_installed
binaries     wallet_rpc_installed           fail:not_installed
binaries     version_supported              skip:monerod_not_installed
node         node_layout                    skip:monerod_not_installed
node         node_reachable                 skip:monerod_not_installed
node         node_unrestricted              skip:monerod_not_installed
node         node_synced                    skip:monerod_not_installed
node         node_peers                     skip:monerod_not_installed
node         node_clock                     skip:monerod_not_installed
node         node_data_dir_single_writer    skip:monerod_not_installed
wallet       wallet_rpc_reachable           skip:wallet_rpc_not_installed
wallet       wallet_rpc_auth                skip:wallet_rpc_not_installed
wallet       wallet_daemon_link             skip:wallet_rpc_not_installed
wallet       wallet_daemon_auth             skip:wallet_rpc_not_installed
wallet       wallet_nettype_matches         skip:wallet_rpc_not_installed
wallet       wallet_dir_exists              skip:wallet_rpc_not_installed
wallet       pending_send                   skip:wallet_rpc_not_installed
units        units_present                  warn:unit_drift
units        unit_loopback                  skip:unit_missing
units        unit_no_password_in_argv       skip:unit_missing
units        unit_no_node_wants             skip:unit_missing
units        unit_enabled_state             skip:unit_missing
units        wallet_dir_matches_unit        skip:unit_missing
units        wallet_port_consistent         skip:unit_missing
storage      disk_space                     skip:monerod_not_installed
storage      prune_configured               skip:monerod_not_installed
install      binary_on_path                 warn:not_installed
install      state_valid                    pass
install      settings_valid                 pass
install      history_cache_fresh            pass
install      plugin_linked                  pass
install      plugin_enabled                 pass
install      menu_entries                   warn:missing_ids
security     monerod_conf_mode              skip:no_conf
security     node_auth                      skip:monerod_not_installed
security     state_perms                    pass
security     ports_owned                    pass
-- summary
{"total":37,"pass":7,"warn":3,"fail":2,"skip":25,"fixable":2,"fixed":0,"fix_failed":0,"planned":0,"generated_at":"2026-09-17T02:46:48+02:00","fix":false,"dry_run":false,"only":null,"run_dir":null,"failing":["monerod_installed","wallet_rpc_installed"],"exit_code":11,"next":["fmonero doctor --fix --dry-run","fmonero doctor --json | jq '.checks[] | select(.status != \"pass\")'"]}
-- the four §4b assertions
binaries fail:not_installed is exactly {monerod_installed, wallet_rpc_installed}: true
every node/wallet/storage check is skip:*: true
every install/security check carries a real detail: true
checks reported: 37
-- install and security rows in full
  binary_on_path [warn:not_installed] /home/oliver/.local/bin/fmonero does not exist. The Omarchy shell's PATH includes ~/.local/bin but NOT ~/.cargo/bin (§1.5), so the bar widget cannot find a cargo-installed binary
  state_valid [pass] state.json does not exist yet, which is a valid empty state
  settings_valid [pass] config.toml does not exist yet; every key is at its default
  history_cache_fresh [pass] no history snapshot yet; `wallet history` builds one
  plugin_linked [pass] /home/oliver/.config/omarchy/plugins/oliver.monero → /home/oliver/Projects/odcpw/fmonero/plugin/oliver.monero
  plugin_enabled [pass] oliver.monero is listed by `omarchy plugin list`
  menu_entries [warn:missing_ids] missing from the Omarchy menu: monero, monero.open, monero.send, monero.node, monero.doctor
  monerod_conf_mode [skip:no_conf] /tmp/tmp.Yg5AUb5VIW/g1/config/monerod.conf does not exist yet
  node_auth [skip:monerod_not_installed] monerod is not installed, so there is nothing to ask
  state_perms [pass] 3 files and directories under the state, config and cache directories are owner-only
  ports_owned [pass] nothing listens on 18080 / 18081 / 18083
-- the human table, on a real pty (a non-TTY stdout implies --json, §4), NO_COLOR for the log
[binaries]
  fail:not_installed             monerod_installed             monerod is not on PATH
                                                               fix: omarchy pkg add monero
  fail:not_installed             wallet_rpc_installed          monero-wallet-rpc is not on PATH
                                                               fix: omarchy pkg add monero
  skip:monerod_not_installed     version_supported             monerod is not installed, so there is nothing to ask
[node]
  skip:monerod_not_installed     node_layout                   monerod is not installed, so there is nothing to ask
  skip:monerod_not_installed     node_reachable                monerod is not installed, so there is nothing to ask
  skip:monerod_not_installed     node_unrestricted             monerod is not installed, so there is nothing to ask
  skip:monerod_not_installed     node_synced                   monerod is not installed, so there is nothing to ask
  skip:monerod_not_installed     node_peers                    monerod is not installed, so there is nothing to ask
  skip:monerod_not_installed     node_clock                    monerod is not installed, so there is nothing to ask
  skip:monerod_not_installed     node_data_dir_single_writer   monerod is not installed, so there is nothing to ask
[wallet]
  skip:wallet_rpc_not_installed  wallet_rpc_reachable          monero-wallet-rpc is not installed, so there is nothing to ask
  skip:wallet_rpc_not_installed  wallet_rpc_auth               monero-wallet-rpc is not installed, so there is nothing to ask
  skip:wallet_rpc_not_installed  wallet_daemon_link            monero-wallet-rpc is not installed, so there is nothing to ask
  skip:wallet_rpc_not_installed  wallet_daemon_auth            monero-wallet-rpc is not installed, so there is nothing to ask
  skip:wallet_rpc_not_installed  wallet_nettype_matches        monero-wallet-rpc is not installed, so there is nothing to ask
  skip:wallet_rpc_not_installed  wallet_dir_exists             monero-wallet-rpc is not installed, so there is nothing to ask
  skip:wallet_rpc_not_installed  pending_send                  monero-wallet-rpc is not installed, so there is nothing to ask
[units]
  warn:unit_drift                units_present                 fmonero-monerod.service is absent: run `fmonero setup --units`; fmonero-wallet-rpc.service is absent: run `fmonero setup --units`
                                                               fix: fmonero doctor --fix
  skip:unit_missing              unit_loopback                 fmonero-monerod.service is not written yet
  skip:unit_missing              unit_no_password_in_argv      fmonero-monerod.service is not written yet
  skip:unit_missing              unit_no_node_wants            fmonero-monerod.service is not written yet
  skip:unit_missing              unit_enabled_state            fmonero-monerod.service is not written yet
  skip:unit_missing              wallet_dir_matches_unit       fmonero-monerod.service is not written yet
  skip:unit_missing              wallet_port_consistent        fmonero-monerod.service is not written yet
[storage]
  skip:monerod_not_installed     disk_space                    monerod is not installed, so there is nothing to ask
  skip:monerod_not_installed     prune_configured              monerod is not installed, so there is nothing to ask
[install]
  warn:not_installed             binary_on_path                /home/oliver/.local/bin/fmonero does not exist. The Omarchy shell's PATH includes ~/.local/bin but NOT ~/.cargo/bin (§1.5), so the bar widget cannot find a cargo-installed binary
                                                               fix: scripts/install.sh --yes
  pass                           state_valid                   state.json does not exist yet, which is a valid empty state
  pass                           settings_valid                config.toml does not exist yet; every key is at its default
  pass                           history_cache_fresh           no history snapshot yet; `wallet history` builds one
  pass                           plugin_linked                 /home/oliver/.config/omarchy/plugins/oliver.monero → /home/oliver/Projects/odcpw/fmonero/plugin/oliver.monero
  pass                           plugin_enabled                oliver.monero is listed by `omarchy plugin list`
  warn:missing_ids               menu_entries                  missing from the Omarchy menu: monero, monero.open, monero.send, monero.node, monero.doctor
                                                               fix: fmonero doctor --fix
[security]
  skip:no_conf                   monerod_conf_mode             /tmp/tmp.Yg5AUb5VIW/g1/config/monerod.conf does not exist yet
  skip:monerod_not_installed     node_auth                     monerod is not installed, so there is nothing to ask
  pass                           state_perms                   3 files and directories under the state, config and cache directories are owner-only
  pass                           ports_owned                   nothing listens on 18080 / 18081 / 18083

37 checks: 7 pass, 3 warn, 2 fail, 25 skip

  next: fmonero doctor --fix --dry-run
  next: fmonero doctor --json | jq '.checks[] | select(.status != "pass")'
(pty exit: 11 — script reports its child's status)

===== GATE 2: doctor --fix --dry-run changes nothing (git status + a state-dir checksum)
exit: 11
-- planned mutations, with the paths each would write to
  [B] units_present: write /home/oliver/.config/systemd/user/fmonero-monerod.service (403 bytes, mode 0644)   -> /home/oliver/.config/systemd/user/fmonero-monerod.service
  [B] units_present: write /home/oliver/.config/systemd/user/fmonero-wallet-rpc.service (582 bytes, mode 0644)   -> /home/oliver/.config/systemd/user/fmonero-wallet-rpc.service
  [B] units_present: run systemctl --user daemon-reload   (undo: systemctl --user daemon-reload)   -> 
  [A] state_valid: write /tmp/tmp.Yg5AUb5VIW/g2/state/state.json (125 bytes, mode 0600)   -> /tmp/tmp.Yg5AUb5VIW/g2/state/state.json
  [B] menu_entries: run /home/oliver/.local/bin/fmonero setup --menu --yes   -> 
dry_run: true · planned: 5 · run_dir: null
state dir before: afa2735aee6e1fd56e2c09961c6834ce4b4f70a8b16e9686a8707225e5a120ec  -
state dir after:  afa2735aee6e1fd56e2c09961c6834ce4b4f70a8b16e9686a8707225e5a120ec  -
STATE DIR UNCHANGED
$HOME targets before: 7f76e67f4b2c1d5a75b0ae2b9a14a5815c72ac05f2b57ca802a2f8d95a0b0c50  -
$HOME targets after:  7f76e67f4b2c1d5a75b0ae2b9a14a5815c72ac05f2b57ca802a2f8d95a0b0c50  -
HOME TARGETS UNCHANGED
git status UNCHANGED
git status --porcelain:
   M crates/fmonero/src/doctor/checks.rs
   M crates/fmonero/src/doctor/mod.rs
   M crates/fmonero/src/doctor/mutate.rs
   M crates/fmonero/src/doctor/probes.rs
   M crates/fmonero/src/doctor/report.rs

-- and the same dry run with NO test state dir: the real machine, the real blast radius
exit: 11
  [B] units_present: write /home/oliver/.config/systemd/user/fmonero-monerod.service (403 bytes, mode 0644)   -> /home/oliver/.config/systemd/user/fmonero-monerod.service
  [B] units_present: write /home/oliver/.config/systemd/user/fmonero-wallet-rpc.service (582 bytes, mode 0644)   -> /home/oliver/.config/systemd/user/fmonero-wallet-rpc.service
  [B] units_present: run systemctl --user daemon-reload   (undo: systemctl --user daemon-reload)   -> 
  [B] menu_entries: run /home/oliver/.local/bin/fmonero setup --menu --yes   -> 
dry_run: true · planned: 4 · run_dir: null
$HOME before: 063a40cc926b97045ce2159d61adaef8d401f02c905a895a585c211d3c8b8843  -
$HOME after:  063a40cc926b97045ce2159d61adaef8d401f02c905a895a585c211d3c8b8843  -
REAL $HOME UNCHANGED (mode, size and mtime of every file)
git status UNCHANGED

-- a real --fix under FMONERO_STATE_DIR declines every fixer that reaches outside it
exit: 11
  units_present: skipped: FMONERO_STATE_DIR is set, so the doctor will not touch /home/oliver/.config/systemd/user/fmonero-monerod.service — the same guard §4.2 puts on units::install
  state_valid: fixed
  menu_entries: skipped: FMONERO_STATE_DIR is set, so the doctor will not run `/home/oliver/.local/bin/fmonero setup --menu --yes` — the same guard §4.2 puts on units::install
  $HOME still untouched: yes

===== GATE 3: class-A round trip — corrupt → --fix → assert → undo latest → compare
-- corrupt: both class-A detectors see it
  state_valid [warn:unparsable] /tmp/tmp.Yg5AUb5VIW/g3/state/state.json does not parse: key must be a string at line 1 column 3
  settings_valid [warn:invalid_keys] wallet.priority: '9' is not a number in 0..=4 for wallet.priority
-- --fix
exit: 11
  state_valid [pass] --fix: fixed
  settings_valid [pass] --fix: fixed
run_id: dbe41dc9 · fixed: 2 · fix_failed: 0
-- assert: both files are now valid, and the repaired settings kept nothing bad
  state_valid [pass]
  settings_valid [pass]
  wallet.priority is now: 2
-- the journal
{"seq":1,"check":"state_valid","ok":false,"phase":"intent","kind":"write_file","inverse":"restore_file"}
{"seq":1,"check":"state_valid","ok":true,"phase":"done","kind":"write_file","inverse":"restore_file"}
{"seq":2,"check":"settings_valid","ok":false,"phase":"intent","kind":"write_file","inverse":"restore_file"}
{"seq":2,"check":"settings_valid","ok":true,"phase":"done","kind":"write_file","inverse":"restore_file"}
-- modes of everything the run wrote (§2b.7: files 0600, directories 0700)
-rw------- runs/20260917T004648Z__dbe41dc9/actions.jsonl
-rw------- runs/20260917T004648Z__dbe41dc9/backups/tmp/tmp.Yg5AUb5VIW/g3/config/config.toml
-rw------- runs/20260917T004648Z__dbe41dc9/backups/tmp/tmp.Yg5AUb5VIW/g3/state/state.json
-rw------- runs/20260917T004648Z__dbe41dc9/probes/facts.json
-rw------- runs/20260917T004648Z__dbe41dc9/probes/monero-wallet-rpc-version.txt
-rw------- runs/20260917T004648Z__dbe41dc9/probes/monerod-version.txt
-rw------- runs/20260917T004648Z__dbe41dc9/probes/omarchy-plugin-list.txt
-rw------- runs/20260917T004648Z__dbe41dc9/report.json
drwx------ 
drwx------ runs
drwx------ runs/20260917T004648Z__dbe41dc9
drwx------ runs/20260917T004648Z__dbe41dc9/backups
drwx------ runs/20260917T004648Z__dbe41dc9/backups/tmp
drwx------ runs/20260917T004648Z__dbe41dc9/backups/tmp/tmp.Yg5AUb5VIW
drwx------ runs/20260917T004648Z__dbe41dc9/backups/tmp/tmp.Yg5AUb5VIW/g3
drwx------ runs/20260917T004648Z__dbe41dc9/backups/tmp/tmp.Yg5AUb5VIW/g3/config
drwx------ runs/20260917T004648Z__dbe41dc9/backups/tmp/tmp.Yg5AUb5VIW/g3/state
drwx------ runs/20260917T004648Z__dbe41dc9/probes
drwx------ runs/20260917T004648Z__dbe41dc9/quarantine
lrwxrwxrwx latest
-- undo latest
exit: 0
{"ok":true,"reason":null,"refused":false,"run_id":"dbe41dc9","schema":1,"undone":[{"action":"restore_file","ok":true,"path":"/tmp/tmp.Yg5AUb5VIW/g3/config/config.toml","seq":2},{"action":"restore_file","ok":true,"path":"/tmp/tmp.Yg5AUb5VIW/g3/state/state.json","seq":1}]}
-- compare: byte for byte the corrupt files again
/tmp/tmp.Yg5AUb5VIW/g3/state/state.json: OK
/tmp/tmp.Yg5AUb5VIW/g3/config/config.toml: OK
ROUND TRIP EXACT
-- undo against a run whose backup hash no longer matches: exit 13
exit: 13
{"ok":false,"refused":true,"reason":"the backup of /tmp/tmp.Yg5AUb5VIW/g3/config/config.toml has changed since the run (sha256 92e78d0b032962f47792a9fa95fd981ef63e1e3ef074d536d6304c75eddbe29f != 89dcfe850265c5c03a4dd1659cbcdd3062cb4a3c4b0fe47202ec20347d0418b7); nothing was restored"}

===== GATE 3b: the §5c key sets of the three doctor payloads, asserted exactly
doctor:              checks,ok,run_id,schema,summary
doctor undo:         ok,reason,refused,run_id,schema,undone
doctor undo refused: ok,reason,refused,run_id,schema,undone
doctor keys are exactly §7's object plus the envelope: ok
doctor undo keys are exactly §5c's row plus the envelope: ok
undone[] is exactly {seq, action, path, ok}: ok
checks[] carries §7's row keys (plus reason/verdict/fix_outcome/fix_error/actions): ok

===== GATE 4: doctor capabilities --json is valid
exit: 0
jq: valid JSON
top-level keys: checks, classes, groups, ok, schema
checks: 37 · groups: 7 · classes: 3
  binaries: 3 checks, needs nothing
  node: 7 checks, needs monerod
  wallet: 7 checks, needs monero-wallet-rpc
  units: 7 checks, needs nothing
  storage: 2 checks, needs monerod
  install: 7 checks, needs nothing
  security: 4 checks, needs nothing
  class A: fmonero-owned state; reverts by file backup — 3 member(s)
  class B: fmonero-created integration; reverts by a recorded inverse — 11 member(s)
  class D: never auto-fixed; the detail names the command for a human — 23 member(s)
-- every auto_fixable check and the Action it emits
  wallet_dir_exists (class B) -> create_wallet_dir
  units_present (class B) -> write_both_units
  unit_loopback (class B) -> write_both_units
  unit_no_node_wants (class B) -> write_both_units
  unit_enabled_state (class B) -> disable_units
  wallet_dir_matches_unit (class B) -> write_wallet_unit
  wallet_port_consistent (class B) -> normalise_wallet_port
  state_valid (class A) -> reset_state_json
  settings_valid (class A) -> repair_settings
  plugin_linked (class B) -> link_plugin
  plugin_enabled (class B) -> enable_plugin
  menu_entries (class B) -> merge_menu
  monerod_conf_mode (class B) -> chmod_monerod_conf
  state_perms (class A) -> fix_state_perms
-- no class D row is auto_fixable:
true

===== GATE 5: doctor --only security works (and every other group, and a bad one exits 2)
  --only binaries  exit 11  3 checks (0p 0w 2f 1s), only=binaries
  --only node      exit 0   7 checks (0p 0w 0f 7s), only=node
  --only wallet    exit 0   7 checks (0p 0w 0f 7s), only=wallet
  --only units     exit 0   7 checks (0p 1w 0f 6s), only=units
  --only storage   exit 0   2 checks (0p 0w 0f 2s), only=storage
  --only install   exit 0   7 checks (5p 2w 0f 0s), only=install
  --only security  exit 0   4 checks (2p 0w 0f 2s), only=security
-- --only security in full
  monerod_conf_mode [skip:no_conf] /tmp/tmp.Yg5AUb5VIW/g5/config/monerod.conf does not exist yet
  node_auth [skip:monerod_not_installed] monerod is not installed, so there is nothing to ask
  state_perms [pass] 3 files and directories under the state, config and cache directories are owner-only
  ports_owned [pass] nothing listens on 18080 / 18081 / 18083
-- an unknown group names every legal one and exits 2
{"code":2,"code_name":"usage","error":"'keyring' is not a check group; known: binaries, node, wallet, units, storage, install, security","hint":"fmonero doctor capabilities --json","ok":false,"reason":"bad_args","schema":1}
exit: 2

===== GATE 6: fixture-driven tests for the parsing detectors, and the Verdict round trip
running 31 tests
test doctor::checks::tests::every_check_id_is_unique_and_every_group_has_members ... ok
test doctor::checks::tests::the_verdict_status_round_trips_through_its_documented_spelling ... ok
test doctor::mutate::tests::a_dry_run_records_a_plan_and_touches_nothing ... ok
test doctor::mutate::tests::a_created_directory_is_rmdired_only_while_empty ... ok
test doctor::mutate::tests::a_file_the_run_created_is_quarantined_on_undo_and_never_unlinked ... ok
test doctor::mutate::tests::a_missing_or_altered_backup_refuses_with_thirteen ... ok
test doctor::mutate::tests::a_write_outside_the_scopes_is_refused_and_a_parent_dir_component_cannot_escape ... ok
test doctor::mutate::tests::under_a_test_state_dir_no_fixer_may_reach_outside_it ... ok
test doctor::mutate::tests::the_journal_entry_is_written_before_the_mutation_and_the_round_trip_is_byte_exact ... ok
test doctor::probes::tests::a_proc_net_tcp_address_is_decoded_in_inet_addr_order ... ok
test doctor::probes::tests::a_url_port_is_read_from_the_authority_only ... ok
test doctor::probes::tests::the_exec_start_argv_joins_systemds_line_continuations ... ok
test doctor::probes::tests::the_redactor_fires_on_every_documented_key_and_keeps_the_rest ... ok
test doctor::mutate::tests::undo_refuses_a_target_outside_the_write_scopes ... ok
test doctor::report::tests::exit_codes_follow_the_section_seven_rule ... ok
test doctor::tests::every_group_has_a_name_and_parses_back ... ok
test doctor::tests::the_repo_plugin_dir_points_at_this_checkout_when_there_is_one ... ok
test prompt::tests::a_password_file_must_have_no_group_or_other_bit ... ok
test prompt::tests::one_trailing_newline_is_stripped_and_nothing_else_is ... ok
test prompt::tests::the_file_wins_over_the_environment_and_confirm_does_not_prompt ... ok
test doctor::report::tests::the_capability_document_names_every_check_and_no_class_d_is_fixable ... ok
test setup::tests::a_merge_into_an_empty_document_still_parses ... ok
test setup::tests::comments_are_stripped_only_outside_strings ... ok
test setup::tests::the_five_menu_entries_come_from_the_asset ... ok
test setup::tests::the_merge_inserts_only_the_absent_ids_and_keeps_every_comment ... ok
test doctor::fixture_tests::a_class_a_fixer_round_trips_and_undo_gives_the_corrupt_bytes_back ... ok
test doctor::checks::tests::a_pending_record_is_reported_and_never_fixed ... ok
test doctor::checks::tests::no_class_d_check_can_name_a_plan ... ok
test doctor::fixture_tests::a_detect_only_run_writes_nothing_under_the_state_directory ... ok
test doctor::checks::tests::the_one_line_predicates_answer_as_the_table_says ... ok
test doctor::fixture_tests::every_doctor_fixture_case_answers_as_its_expectation_says ... ok

test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.90s

-- fixture cases on disk
  history_cache_fresh
  history_cache_fresh__bad_magic
  history_cache_fresh__stale
  node_data_dir_single_writer
  node_data_dir_single_writer__free
  node_data_dir_single_writer__via_config_file
  node_layout
  node_layout__none
  node_layout__remote
  node_layout__system_unit
  ports_owned
  ports_owned__external
  ports_owned__foreign
  settings_valid
  settings_valid__ok
  settings_valid__unparsable
  unit_no_password_in_argv
  unit_no_password_in_argv__log_level
  unit_no_password_in_argv__password_file
  unit_no_password_in_argv__rpc_login
  units_present
  units_present__absent

===== GATE 7: cargo fmt --check, clippy -D warnings, and the §2g/§2b greps over the doctor
cargo fmt --check: clean
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.11s
-- §2g: no print macro in the doctor (all output goes through output.rs's Out)
  none
-- §2f: no shell string in the doctor
  none
-- §2b.3: query_key
  none
-- §2b.8: the doctor never reads monerod.conf's or the login file's bytes into a probe
  16://! * the **contents** of `paths.monerod_conf()` and `paths.wallet_login_file(port)` are
  352:    pub wallet_login_file: FileProbe,
  388:    pub monerod_conf: FileProbe,
  389:    pub monerod_conf_prune: Option<bool>,
  390:    pub monerod_conf_has_login: bool,
  532:            wallet_login_file: FileProbe::default(),
  553:            monerod_conf: FileProbe::default(),
  554:            monerod_conf_prune: None,
  555:            monerod_conf_has_login: false,
  670:        self.wallet_login_file =
  671:            FileProbe::stat(&ctx.paths.wallet_login_file(ctx.settings.wallet.port));
  738:        let conf = ctx.paths.monerod_conf();
  739:        self.monerod_conf = FileProbe::stat(&conf);
  740:        if self.monerod_conf.exists {
  742:                self.monerod_conf_has_login = text
  745:                self.monerod_conf_prune = Some(text.lines().any(|l| {
-- §7: no RemoveFile verb anywhere in mutate.rs's Action vocabulary
  41-    WriteFile {
  51-    CreateDir { path: PathBuf, mode: u32 },
  53-    CreateSymlink { link: PathBuf, target: PathBuf },
  55-    RunArgv {
-- §7.1: the check count against the design table
37

===== GATE 8: the probe artifacts a --save run writes, and the §2b known-value grep over them
run artifacts: /tmp/tmp.Yg5AUb5VIW/g8/state/doctor/runs/20260917T004651Z__86c661c3
exit: 11
  -rw------- 0 runs/20260917T004651Z__86c661c3/actions.jsonl
  -rw------- 13294 runs/20260917T004651Z__86c661c3/probes/facts.json
  -rw------- 16573 runs/20260917T004651Z__86c661c3/report.json
  -rw------- 3695 runs/20260917T004651Z__86c661c3/probes/omarchy-plugin-list.txt
  -rw------- 47 runs/20260917T004651Z__86c661c3/probes/monero-wallet-rpc-version.txt
  -rw------- 47 runs/20260917T004651Z__86c661c3/probes/monerod-version.txt
-- every credential key that appears anywhere under the run directory, with what follows it
  login
  login_file
  login_port
  password_in_argv
  (each of those is a FIELD NAME; a value would follow a ':' or '=' — §2b.8)
-- and the same grep over a rendered unit that DOES carry one, to prove the redactor fires
run artifacts: /tmp/tmp.Yg5AUb5VIW/g8/state/doctor/runs/20260917T004651Z__85fa04f7
  doctor --only units exit: 11
  unit_no_password_in_argv [fail:credential_in_argv] fmonero-monerod.service carries --rpc-login; fmonero-wallet-rpc.service carries --rpc-login
  SECRETVALUE anywhere in the run artifacts:
    /tmp/tmp.Yg5AUb5VIW/g8/state/doctor/latest/report.json:0
    /tmp/tmp.Yg5AUb5VIW/g8/state/doctor/latest/actions.jsonl:0
    /tmp/tmp.Yg5AUb5VIW/g8/state/doctor/latest/probes/facts.json:0
    /tmp/tmp.Yg5AUb5VIW/g8/state/doctor/latest/probes/omarchy-plugin-list.txt:0
    /tmp/tmp.Yg5AUb5VIW/g8/state/doctor/latest/probes/monero-wallet-rpc-version.txt:0
    /tmp/tmp.Yg5AUb5VIW/g8/state/doctor/latest/probes/monerod-version.txt:0
    (no match)
        "text": "[Service]\nExecStart=/usr/bin/monero-wallet-rpc --rpc-login <redacted> --log-level 0\n",
        "text": "[Service]\nExecStart=/usr/bin/monero-wallet-rpc --rpc-login <redacted> --log-level 0\n",

===== GATE 9: the check table against §7.1 — every id with its group, severity and class
  binaries/monerod_installed  sev=error  class=D
  binaries/wallet_rpc_installed  sev=error  class=D
  binaries/version_supported  sev=warn  class=D
  node/node_layout  sev=info  class=D
  node/node_reachable  sev=error  class=D
  node/node_unrestricted  sev=warn  class=D
  node/node_synced  sev=warn  class=D
  node/node_peers  sev=warn  class=D
  node/node_clock  sev=error  class=D
  node/node_data_dir_single_writer  sev=error  class=D
  wallet/wallet_rpc_reachable  sev=warn  class=D
  wallet/wallet_rpc_auth  sev=error  class=D
  wallet/wallet_daemon_link  sev=warn  class=D
  wallet/wallet_daemon_auth  sev=error  class=D
  wallet/wallet_nettype_matches  sev=warn  class=D
  wallet/wallet_dir_exists  sev=warn  class=B
  wallet/pending_send  sev=error  class=D
  units/units_present  sev=warn  class=B
  units/unit_loopback  sev=error  class=B
  units/unit_no_password_in_argv  sev=error  class=D
  units/unit_no_node_wants  sev=error  class=B
  units/unit_enabled_state  sev=info  class=B
  units/wallet_dir_matches_unit  sev=error  class=B
  units/wallet_port_consistent  sev=error  class=B
  storage/disk_space  sev=warn  class=D
  storage/prune_configured  sev=info  class=D
  install/binary_on_path  sev=warn  class=D
  install/state_valid  sev=warn  class=A
  install/settings_valid  sev=warn  class=A
  install/history_cache_fresh  sev=info  class=D
  install/plugin_linked  sev=warn  class=B
  install/plugin_enabled  sev=warn  class=B
  install/menu_entries  sev=info  class=B
  security/monerod_conf_mode  sev=error  class=B
  security/node_auth  sev=warn  class=D
  security/state_perms  sev=error  class=A
  security/ports_owned  sev=warn  class=D

===== DONE
```
