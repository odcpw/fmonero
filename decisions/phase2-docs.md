# Phase 2 · docs — evidence and decisions

Agent: Phase 2 docs (DESIGN.md §8, Phase 2 table, row "docs"). Base: tag `gate-1a`
(`88aee19`). Branch: `phase2/docs`, in its own worktree. Date: 2026-09-17.
Owned files: `README.md`, `plugin/oliver.monero/README.md`, `scripts/install.sh`,
`scripts/uninstall.sh`, and this file. Nothing else was touched.

Phase 3 concatenates this file into `DECISIONS.md` (§8 preamble) and then deletes it.

## 1. What was written

| File | Lines | Shape it follows |
|---|---|---|
| `scripts/install.sh` | 310 | `fproton/scripts/install.sh`, plus the `installer-workmanship` substance named in §8 |
| `scripts/uninstall.sh` | 403 | `fproton/scripts/uninstall.sh`, extended with the two systemd user units fmonero's `setup` creates |
| `README.md` | 424 | `fproton/README.md`, `readme-writing` + `de-slopify` (§9) |
| `plugin/oliver.monero/README.md` | 151 | `fproton/plugin/oliver.protonvpn/README.md`, against §6 |

`plugin/oliver.monero/` did not exist at `gate-1a`; this branch creates the directory with
only the README in it. Every other file under it belongs to the Phase 2 plugin agent (§8).

## 2. Decisions, with reasons

**`cargo build --release -p fmonero`, not a bare `cargo build --release`.** fproton's installer
builds the whole workspace because fproton has no test-only member. fmonero has
`crates/fmonero-mock` (§8 Phase 0), which is a fixture server with `publish = false`, and §8's
own argument for it ends "`scripts/install.sh` builds `-p fmonero` only, so it can never reach
`~/.local/bin`". The `-p` is therefore part of the contract, not a speed optimisation.

**`umask 077`, with two deliberate exceptions.** §8's docs row names the mask and §2b.7 is the
reason: everything fmonero writes under `$HOME` is owner-only. The binary is installed with an
explicit `install -m 0755`, which sets the mode rather than masking it, and `PREFIX` is created
under a local `(umask 022)` subshell **only when it does not already exist**, because a bin
directory at 0700 is a surprise nothing in this design asks for. An existing `PREFIX` is never
chmod'd.

**The installer warns about a missing `monerod` and carries on.** §1.1 says no agent can install
Monero, and §4a maps a missing daemon to exit 4 with `omarchy pkg add monero` as the hint.
Refusing to install fmonero because Monero is absent would make the machine this was developed
on unable to run its own installer; warning once, with the same hint the binary gives, is the
honest version.

**The uninstaller takes the node down through `fmonero node down --yes`, not through
`systemctl --user stop` alone.** §4.1 puts `stop_daemon` over RPC before the unit stop precisely
so LMDB closes cleanly, and §1.2 is about what a dirty LMDB costs. The script falls back to
`systemctl --user stop` when `node down` fails or when the binary is already gone, and it never
touches a node it does not own: under the `system_unit` layout `node down` exits 3 by
construction (§2i), which the script reports rather than working around.

**The wallet unit is stopped with `systemctl --user stop`, deliberately not with a `wallet
close`.** `monero-wallet-rpc` stores the open wallet in its graceful `stop()`
(`wallet_rpc_server.cpp:294-302`), which is what SIGTERM from systemd triggers, so the plain unit
stop already persists the wallet and needs no extra RPC.

**The two unit files are removed by default; `~/.config/fmonero` is not.** The units are
installer-created integration under `$HOME`, the same category as the plugin symlink that
fproton's uninstaller removes by default. `~/.config/fmonero/monerod.conf` is fmonero's own
config directory, so it goes with `--purge`, and the default run prints the paths it is keeping.
A `daemon-reload` runs only when a unit file was actually removed.

**`--purge` refuses while `pending.json` exists, and this is an addition to §8's list rather
than a restatement of it.** §2a.4 makes that file the only thing standing between a crash
mid-relay and a second spend of the same outputs, and its resolution needs the binary the
uninstaller is about to delete. So the check runs first, before any step, and exits 3 with
`fmonero wallet history --limit 5` and `fmonero wallet clear-pending --yes` as the hint. Nothing
in DESIGN.md forbids deleting the record during an uninstall; nothing endorses it either, and an
uninstaller that silently destroys the one money interlock in the project is the §2j failure
applied to a script.

**Wallet files and `~/.bitmonero` are unreachable from either script, under any flag.**
`--purge` names `config_dir`, `state_dir` and `cache_dir` explicitly and never a parent:
`~/.local/share/fmonero` holds `wallets/<network>/`, so purging that directory would delete
wallet files, which with their password are the only copy of money that is not the seed. The
chain is left alone as well: it is re-syncable, but it is 100+ GB and it is not ours.

**`FMONERO_PREFIX` is an installer-only environment variable.** It is read by these two shell
scripts and by nothing under `crates/`, so it is deliberately **not** in
`capabilities.json`'s `env` list. Phase 3's ergonomics pass asserts that round trip over
`crates/` (§5b), and adding a shell-script variable to the binary's contract would make that
assertion fail on correct code.

**README structure: Status sits third, before the CLI.** `readme-writing` says lead with value,
and §2j says claim nothing RED. Putting the verification matrix immediately after Install means
no reader reaches the command reference believing the real-daemon rows have been observed. The
class table is §4b's three classes verbatim in meaning, the RED paragraph is headed "RED, and
therefore unverified" so the Phase 3 honesty grep finds the word adjacent to every RED claim,
and the section hands authority over to `fmonero version --json`'s `build_stage` rather than to
itself, since this README was written at the API-freeze gate.

**No badges, no `curl | bash` one-liner, no FAQ.** `readme-writing`'s hero template asks for
all three. There is no CI workflow in v0.1 (§0 cuts it) so a CI badge would be a lie; there are
no releases and no install URL, so a curl one-liner would point nowhere; and the questions a FAQ
would answer are the ones Status and Limitations answer with evidence. §9 says apply substance,
skip ceremony.

**No em-dash appears as prose punctuation in either README** (`grep -c '—'` is 0 in both), and
the `de-slopify` high-signal tells were grepped for and are absent. The prose keeps DESIGN.md's
own wording where a sentence is a contract (for instance "a crash cannot double-send"), and
labels it AMBER in the same section rather than leaving the absolute unqualified.

## 3. Gate

The §8 docs row's gate, line by line, with real output.

### 3.1 `bash -n` on both scripts — PASS

```
$ bash -n scripts/install.sh; echo "install.sh bash -n exit=$?"
install.sh bash -n exit=0
$ bash -n scripts/uninstall.sh; echo "uninstall.sh bash -n exit=$?"
uninstall.sh bash -n exit=0
```

### 3.2 `shellcheck` — SKIPPED, tool absent

```
$ command -v shellcheck || echo "shellcheck: NOT INSTALLED"
shellcheck: NOT INSTALLED
```

Recorded as a skip, not a pass (§2j, §4b's rule about tools this machine does not have). Both
scripts were written to the style shellcheck enforces: every expansion quoted, `local` on every
function variable, `[ ... ]` tests with quoted operands, `rm`/`mv`/`cp`/`install` given `--`
before paths, `$*` used only inside the `plan`/`info` printers, and no unquoted array or glob.
The one construct shellcheck would flag on sight, a backtick inside a double-quoted `skip`
message, was found by reading and removed before the first commit; it would have run
`node down` as a command substitution.

### 3.3 `scripts/install.sh --dry-run` prints every step and touches nothing — PASS

```
$ git status --porcelain
[empty]
$ /tmp/claude-1000/fmonero-docs-probe.sh <worktree> > gate-home-before.txt
$ ./scripts/install.sh --dry-run
warn monerod is not on PATH; fmonero installs, but every command that needs the node or the wallet will exit 4
hint: omarchy pkg add monero   # then: fmonero setup --units --yes
would cargo build --release -p fmonero   (in /home/oliver/Projects/odcpw/fmonero/.claude/worktrees/wf_233da421-45c-4)
would install -m 0755 /home/oliver/Projects/odcpw/fmonero/.claude/worktrees/wf_233da421-45c-4/target/release/fmonero /home/oliver/.local/bin/fmonero
would fmonero setup --all --yes   (units written disabled, plugin link, enable, bar placement, menu entries)

dry run: nothing above was executed.
install.sh --dry-run exit=0
```

Every step the `--yes` path would take is printed: build, install, setup. The lock, the
`mkdir -p` of the prefix and the version probe are the three things the dry run skips rather
than prints, because none of them changes the machine's contents in a way a reader of the plan
needs, and printing a lock the dry run does not take would be the misleading half.

The proof that it touched nothing is a checksum of the state of every path the real run would
write, taken before and after:

```
$ sha256sum gate-home-before.txt gate-home-after.txt
1261f55dc1e3b969c054ba3b8e0b28908424c1bbe3f52c8664b8b6642bcfdb8a  gate-home-before.txt
1261f55dc1e3b969c054ba3b8e0b28908424c1bbe3f52c8664b8b6642bcfdb8a  gate-home-after.txt
$ diff -u gate-home-before.txt gate-home-after.txt; echo "diff exit=$?"
diff exit=0
$ git status --porcelain
[empty]
```

The 16 paths in that snapshot, each recorded as a symlink target, a
`mode size mtime sha256[0:16]`, a directory `mode mtime entry-count`, or `absent`:

```
/home/oliver/.local/bin/fmonero                                  absent
/home/oliver/.local/bin                                          dir 755 1789444173 entries=34
/run/user/1000/fmonero-install.lock                              absent
<worktree>/target                                                absent
/home/oliver/.config/systemd/user/fmonero-monerod.service        absent
/home/oliver/.config/systemd/user/fmonero-wallet-rpc.service     absent
/home/oliver/.config/fmonero                                     absent
/home/oliver/.config/fmonero/monerod.conf                        absent
/home/oliver/.config/omarchy/plugins/oliver.monero               absent
/home/oliver/.config/omarchy/plugins                             dir 755 1789423054 entries=4
/home/oliver/.config/omarchy/extensions/omarchy-menu.jsonc        file 644 2701 1789444173 601d6420feacf985
/home/oliver/.config/omarchy/shell.json                          file 644 1423 1789591329 2d00f1755e995bb7
/home/oliver/.local/share/fmonero                                absent
/home/oliver/.local/state/fmonero                                absent
/home/oliver/.cache/fmonero                                      absent
/home/oliver/.bitmonero                                          absent
```

The `setup --all --dry-run` probe branch was exercised separately, with `--prefix` pointing at a
copy of the `gate-1a` binary so that `DEST` was executable, and the `$HOME` snapshot was
unchanged afterwards as well:

```
$ ./scripts/install.sh --dry-run --prefix /tmp/claude-1000/fakeprefix
…
-> /tmp/claude-1000/fakeprefix/fmonero setup --all --dry-run
{"code":1,"code_name":"failure","error":"fmonero setup is not implemented in this build","hint":"fmonero capabilities   # what this build actually does","ok":false,"reason":null,"schema":1}
warn that fmonero exited 1 for the setup plan (see above); the real run may differ

dry run: nothing above was executed.
EXIT=0
```

That is `setup::run`'s 1a stub answering, and it is the reason the branch degrades to a warning
instead of failing: at `gate-1a` there is no setup plan to print, and after Phase 1b there will
be one. The installer's own exit code stays 0 either way, because a dry run that cannot obtain a
plan has still printed one.

Exit codes, checked against §4a:

```
$ ./scripts/install.sh --bogus            -> 2   (usage; hint: scripts/install.sh --help)
$ ./scripts/install.sh </dev/null         -> 3   (safety; needs --yes without a terminal)
$ ./scripts/install.sh --help             -> 0
$ ./scripts/uninstall.sh --bogus          -> 2
$ ./scripts/uninstall.sh </dev/null       -> 3
```

### 3.4 `scripts/uninstall.sh --dry-run` — PASS (not a named gate line; run anyway)

On this machine, where nothing is installed, every step correctly reports a skip rather than
planning a removal:

```
$ ./scripts/uninstall.sh --dry-run --menu --purge
--   fmonero-monerod.service is not active
--   fmonero-wallet-rpc.service is not active
--   no /home/oliver/.config/systemd/user/fmonero-monerod.service
--   no /home/oliver/.config/systemd/user/fmonero-wallet-rpc.service
--   oliver.monero is not registered with omarchy
--   no plugin link at /home/oliver/.config/omarchy/plugins/oliver.monero
--   no binary at /home/oliver/.local/bin/fmonero
--   no fmonero entries in /home/oliver/.config/omarchy/extensions/omarchy-menu.jsonc
--   no /home/oliver/.config/fmonero
--   no /home/oliver/.local/state/fmonero
--   no /home/oliver/.cache/fmonero

dry run: nothing above was executed.
uninstall.sh --dry-run exit=0
```

The `pending.json` refusal was driven with `FMONERO_STATE_DIR` pointed at a scratch directory
holding a record with one unrelayed entry:

```
$ FMONERO_STATE_DIR=/tmp/claude-1000/fmsb/state ./scripts/uninstall.sh --dry-run --purge
error /tmp/claude-1000/fmsb/state/pending.json exists: a send may be in flight, and --purge would delete the record of it
hint: fmonero wallet history --limit 5   # confirm whether it landed, then: fmonero wallet clear-pending --yes
EXIT=3
```

The menu editing chain was exercised on a synthetic `omarchy-menu.jsonc` carrying the owner's
own entries plus the five `monero*` ids and the `// fmonero` marker. Running the script's own
`awk` programs against it, then its `python3` JSONC parser over the result:

```
$ awk '<menu_strip>' menu.jsonc            # removes the marker and the five monero lines
{
  // the owner's own entries
  "learn": {"icon":"","label":"Learn","aliases":["learn"]},
  "style": {"icon":"","label":"Style"},
}
$ awk '<menu_fix_trailing_comma>' stripped.jsonc
{
  // the owner's own entries
  "learn": {"icon":"","label":"Learn","aliases":["learn"]},
  "style": {"icon":"","label":"Style"}
}
$ python3 - fixed.jsonc <<'PY' … PY
parses; keys = ['learn', 'style']
EXIT=0
```

The owner's comments and entries survive byte for byte and the result parses. What is **not**
proved here: the full `--menu` write path (verbatim backup, then restore when the result stops
parsing) was not executed end to end, because `MENU_FILE` derives from `XDG_CONFIG_HOME`, which
this agent's sandbox refuses to override, and editing the owner's real menu file is not a docs
gate. The backup-and-restore code is fproton's, unchanged except for the id pattern.

### 3.5 The `--yes` path: read for idempotence, not executed — as §8 requires

§8 is explicit that this agent must not run `install.sh --yes`: at `gate-1a` `setup::run` is
still a stub (see the transcript in 3.3), so the run would exit 1 on its first real step, and
§4b reserves the one real `setup` for the Phase 4 verifier. What the reading establishes, step
by step:

1. **Preflight** does `command -v` probes and `[ -w ]` tests; the only write is
   `mkdir -p "$PREFIX"`, guarded by `[ ! -d "$PREFIX" ]`. Repeatable.
2. **Consent** is skipped with `--yes`; nothing is written.
3. **Lock**: `mkdir "$XDG_RUNTIME_DIR/fmonero-install.lock"`, removed by the `EXIT` trap, so a
   second run finds no lock. A concurrent second run exits 1 rather than racing.
4. **Build**: `cargo build --release -p fmonero` is a no-op when nothing changed.
5. **Install**: `cmp -s` short-circuits to `--  <dest> is already this exact binary`, so the
   second run does not even rename. When it does write, it writes `.fmonero.install.$$` beside
   the target and `mv -f`s it over, which is atomic and safe while an fmonero is running
   (in-place replacement of a running binary gives ETXTBSY).
6. **Setup**: `fmonero setup --all --yes`. §4.2 makes it idempotent by contract, each part
   skipped when already done, and §4.2's `rpc-login` rule is what keeps the second run's diff
   empty: an existing valid credential line is carried through and no new randomness is drawn.
7. **Summary** prints only.

Nothing in the script appends to a file, edits a dotfile, or writes outside `$PREFIX`, the
repo's `target/` and the lock. **The idempotence claim itself is therefore unproven by this
phase**, and §8's Phase 4 verifier is where it is settled: it runs `scripts/install.sh --yes`
twice and asserts the second run reports every action unchanged.

The same reading for `uninstall.sh --yes`: every one of the seven steps is guarded by an
existence or state test and prints a skip when there is nothing to do, which the dry run in 3.4
demonstrates for all of them at once on a machine where nothing is installed. A second
`uninstall.sh --yes` is therefore all skips and exit 0. `daemon-reload` runs only when a unit
file was removed, and `--menu` is guarded by the same grep that decides whether there is
anything to strip.

### 3.6 README: Status mirrors §4b, claims nothing RED — PASS by inspection plus a grep

The Status section carries §4b's three classes with the same meanings and the same command
lists, folded to the "lowest class any of its scenarios reaches" rule: `wallet send`, for
instance, appears under GREEN for its refusal order with no daemon, under AMBER for the mocked
build and relay, and under RED for a real broadcast, and the RED sentence is the one that
carries the word "unverified".

An approximation of the Phase 3 honesty grep over both READMEs:

```
$ grep -nEi 'sent |broadcast|synchronized|synced|sync progress|peer list|live peer' README.md plugin/oliver.monero/README.md
plugin/oliver.monero/README.md:97:  … a node that is not synchronized gets a percent ring …     (icon logic, not a claim)
README.md:34:  … the value-initialised zero the daemon sent …                                    (about a daemon response)
README.md:75:  fmonero node up --wait   # start monerod and wait for it to report itself synced   (a command; line 78 says "Those three commands are unverified")
README.md:133: Real sync progress, a real peer list, a real balance …                             (inside the paragraph headed "RED, and therefore unverified")
README.md:227: **A crash cannot double-send.** … what is about to be broadcast …                  (mechanism; line 253 labels the whole section AMBER and the real broadcast unverified)
README.md:393: Cuprate … `get_connections`, the peer list … return `not_available`                (a limitation)
```

No sentence asserts that a send, a sync or a peer list has been observed working. Three separate
places say the opposite: the header paragraph ("Read Status before you believe anything here"),
the note under the Monero install block, and the RED paragraph.

### 3.7 Every README claim findable in code or in the matrix

Each claim was checked against a DESIGN.md section or against real `gate-1a` output. The
non-obvious ones and where they come from:

| Claim | Source |
|---|---|
| 109 QML files, 26 869 lines of QML, `monerod sync_info` every 2 s | DESIGN.md header, `docs/FEASIBILITY.md` §2 |
| wallet-rpc serves RPC on one thread, yields 200-300 ms per period | §2d (`wallet_rpc_server.cpp:291-292`, `:211-280`) |
| `"BUSY"` for the whole initial sync; `top_block_age_sec` null | §1b.3, §5 |
| restricted node poisons uptime, free space, four peer counts | §1b.3, §5 |
| three hot RPCs closed / five open, two `systemctl` spawns, 60 ms | §5, §2l (a) |
| no TLS stack in the binary; loopback literal only | §2c |
| MD5 Digest, and the browser-POST argument for it | §1b.7, §2c |
| 60 s breaker, 100-blocks-behind exemption | §2d |
| `node up` checks `lmdb/lock.mdb` and scans `/proc` | §2i |
| units written disabled, `rpc-login` 0600 and not rotated, redacted diffs | §4.2 |
| exit codes, `reason` values for exit 3, `--dry-run` on four commands | §4, §4a, and `fmonero --help` at `gate-1a` |
| settings keys and defaults | `fmonero settings list` at `gate-1a` |
| every command and flag in the CLI block | `fmonero {--help, wallet --help, node --help, wallet send --help, wallet sweep --help, node up --help}` at `gate-1a` |
| the whole Money safety section | §2a.1 through §2a.11 |
| `wallet create` does not print the seed; the two-prompt rule | §2b.2 |
| TUI screens, the typed-word confirm, relay ignores cancellation | §7a |
| doctor chokepoint, `actions.jsonl`, undo exit 13, never deletes | §7 |
| widget icon states, panel order, keys, settings, greps, budgets | §6.1 through §6.7 |
| the v0.1 cut list, 100-150 GB estimate, fork table ends at v16 | §0, §1.3, §1b.1 |

`fmonero version --json` at `gate-1a`, which is what the README points readers at instead of
trusting its own Status section:

```
$ fmonero version --json
{"build_note":"API freeze only (DESIGN.md §8, Phase 1a). The command surface, the exit codes,
the `reason` discriminators, the settings table, amount parsing and address parsing are
implemented and tested. Every path that talks to monerod or monero-wallet-rpc still exits 1
'not implemented in this build'. No Monero binary is installed on the build machine, so nothing
that needs a live daemon has ever been observed working (§1.1, §4b RED).",
"build_stage":"phase-1a-api-freeze","ok":true,"schema":1,"version":"0.1.0"}
```

## 4. Notes for other phases

**For the plugin agent (§1.5 / §6.6, the reload loop).** DESIGN.md says to run
`omarchy-shell shell rescanPlugins` after every QML edit. fproton's plugin README records a
measurement on this same machine that contradicts the useful half of that: a rescan is the
cold-load path and did **not** re-read the QML of an already-loaded symlinked plugin, because
the registry's reload is guarded by `Qt.clearComponentCache`; there, `omarchy restart shell` was
what picked up an edit, and a deliberate syntax error went unnoticed by both the rescan and the
smoke test. `plugin/oliver.monero/README.md` documents DESIGN's instruction first and this
caveat immediately after. The plugin agent should verify which one is true here before trusting
a green smoke test, since the failure mode is three hours of QML that never loaded.

**For Phase 3.** This file is the docs half of the concatenation in §8's preamble. The README
says `DECISIONS.md` carries the §2l (a) measurement and the §4b evidence; those come from the 1b
and 4 gates, so if either is missing at merge time the README's pointer is the thing to fix, not
the claim.

**For Phase 4.** Two claims this phase could not close, both named above: the idempotence of
`install.sh --yes` (3.5) and the full `--menu` write-and-restore path (3.4). The verifier's
double `install.sh --yes` run settles the first.
