#!/usr/bin/env bash
# fmonero's end-to-end tests, run against the real binary (DESIGN.md §4b, §5c, §8).
#
#   tests/e2e.sh            --fast (default): every GREEN row, every §5c key set, and the
#                           mock-AMBER rows. Target < 20 s after the build.
#   tests/e2e.sh --full     adds the §2d deadline, breaker and --force paths, the §2a
#                           concurrency tests, and the §2l (a)/(b)/(d) timings. Target < 90 s.
#
# **What these tests prove, and what they do not.** Every row that touches a daemon is served by
# `crates/fmonero-mock` over fixtures DERIVED FROM THE MONERO SOURCE at v0.18.5.1 (§1.1). That
# proves fmonero's wire format, its parsing and its own sequencing; it proves nothing about a real
# `monerod` or `monero-wallet-rpc`, and §4b keeps those rows AMBER for exactly that reason. No
# send has ever been broadcast from this build.
#
# Every case runs in its own FMONERO_STATE_DIR / _CONFIG_DIR / _CACHE_DIR, so nothing here can
# touch the owner's real configuration, units or wallets.

set -uo pipefail

MODE=fast
for arg in "$@"; do
  case "$arg" in
    --fast) MODE=fast ;;
    --full) MODE=full ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "e2e.sh: unknown argument $arg" >&2; exit 2 ;;
  esac
done

cd "$(dirname "$0")/.." || exit 1
ROOT="$PWD"
PASS=0
FAIL=0
SKIP=0
WORK="$(mktemp -d)"
MOCKS=()

# Fixed, unusual ports: the login file monero-wallet-rpc writes is named after its BIND PORT
# (§2c), so `wallet.port` and the mock's port must be the same number.
NODE_PORT=18191
WALLET_PORT=18193

cleanup() {
  for pid in "${MOCKS[@]:-}"; do [ -n "$pid" ] && kill "$pid" 2>/dev/null; done
  rm -rf "$WORK"
}
trap cleanup EXIT

ok()   { PASS=$((PASS+1)); printf '  ok    %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  FAIL  %s\n' "$1"; shift; for l in "$@"; do printf '        %s\n' "$l"; done; }
skip() { SKIP=$((SKIP+1)); printf '  skip  %s\n' "$1"; }
step() { printf '\n== %s\n' "$1"; }

# ---------------------------------------------------------------- build
step "build"
if out="$(cargo build --release 2>&1)"; then
  ok "cargo build --release"
else
  bad "cargo build --release" "$(printf '%s' "$out" | grep -E '^error' | head -5)"
  printf '\ne2e.sh: cannot run without a binary\n'; exit 1
fi
FM="$ROOT/target/release/fmonero"
MOCK="$ROOT/target/release/fmonero-mock"
[ -x "$FM" ] || { echo "e2e.sh: $FM is missing" >&2; exit 1; }
[ -x "$MOCK" ] || { echo "e2e.sh: $MOCK is missing" >&2; exit 1; }

for tool in jq curl; do
  command -v "$tool" >/dev/null 2>&1 || { echo "e2e.sh: $tool is required" >&2; exit 1; }
done

# ---------------------------------------------------------------- harness
#
# `sandbox <name>` points every override at a fresh directory, so no case can see another's
# state, config or cache — and none of them can see the owner's.
#
# **Five directories, not three, and the two added in v5.1 are what make this suite survive its
# own installer.** Until the Phase 4 verifier ran `scripts/install.sh --yes`, `systemd_user_dir`
# and `wallet.dir` were read from the real machine, so five assertions here silently asserted
# "nothing is installed" and failed the moment something was: `node up with no units` found a
# unit, `wallet open` started one, and the `CreateDir wallet.dir` row found the directory already
# there. A test a correct installer breaks is a test that gets deleted, so:
#
#   FMONERO_SYSTEMD_USER_DIR  the §2i seam, now honoured by `Paths::discover` itself (§2i v5.1),
#                             so `detect_layout`, `units::state`, `units::preflight` and the
#                             doctor all look at THIS directory — empty unless a case fills it.
#   XDG_DATA_HOME             `paths.wallet_dir` is `$XDG_DATA_HOME/fmonero/wallets` (§3), and
#                             `setup --units` derives `settings.wallet.dir` from it (§4.2), so
#                             this is the only way to move both together.
#
# No case starts or stops a real systemd unit: every one that could either finds no unit file
# (exit 4 in milliseconds, §2i) or runs under a `FMONERO_SYSTEMCTL_FIXTURE`, and the last step of
# this file asserts both of the owner's units are still inactive afterwards.
CASE=""
sandbox() {
  # The ports are fixed (the login file is named after the bind port, §2c), so a previous
  # case's mock must be gone before this one starts: otherwise the new case talks to the old
  # listener and reads a request log that belongs to another sandbox.
  mocks_down
  CASE="$1"
  export FMONERO_STATE_DIR="$WORK/$CASE/state"
  export FMONERO_CONFIG_DIR="$WORK/$CASE/config"
  export FMONERO_CACHE_DIR="$WORK/$CASE/cache"
  export FMONERO_SYSTEMD_USER_DIR="$WORK/$CASE/systemd"
  export XDG_DATA_HOME="$WORK/$CASE/data"
  unset FMONERO_SYSTEMCTL_FIXTURE FMONERO_PROC_ROOT FMONERO_WALLET_PASSWORD
  mkdir -p "$FMONERO_STATE_DIR/run" "$FMONERO_CONFIG_DIR" "$FMONERO_CACHE_DIR" \
           "$FMONERO_SYSTEMD_USER_DIR" "$XDG_DATA_HOME/fmonero/wallets/mainnet"
  # 0700, including `run/`: that is the mode `Paths::ensure_dirs` creates them at (§2b.7) and the
  # mode the doctor's `state_perms` checks. A bare `mkdir -p` leaves them at the shell's umask —
  # 0755 — and `state_perms` then reports `fail:bad_mode` on a sandbox that is only sloppy, which
  # would make the §4b GREEN row below ("exactly two checks fail") false for the wrong reason.
  chmod 700 "$FMONERO_STATE_DIR" "$FMONERO_STATE_DIR/run" "$FMONERO_CONFIG_DIR" "$FMONERO_CACHE_DIR"
  # The wallet directory exists at 0700 in every case but the `setup` one, which removes it to
  # observe the `CreateDir wallet.dir` action §4.2 promises. The doctor's `wallet_dir_exists`
  # reads its mode, so a umask-mode directory would report `fail` for the wrong reason too.
  chmod 700 "$XDG_DATA_HOME/fmonero" "$XDG_DATA_HOME/fmonero/wallets" \
            "$XDG_DATA_HOME/fmonero/wallets/mainnet"
  # `node.data_dir` is the one remaining path that decides an OUTCOME from the machine: §1.3's
  # 120 GB floor is measured on the filesystem holding it (`fsinfo::free_bytes`, which walks up to
  # the nearest existing ancestor), and `setup --units` refuses below it — correctly. Left at
  # `$HOME/.bitmonero` that makes every `setup` assertion depend on where $HOME happens to live: on
  # a tmpfs scratch HOME the floor refusal fires and six assertions of the `setup` case fail for a
  # reason none of them names. It is pinned to a directory on the filesystem this repo is on, and
  # nothing here creates it; the floor refusal itself gets its own case below, on purpose.
  "$FM" settings set node.data_dir "$ROOT/target/e2e-data/$CASE/bitmonero" >/dev/null
}

OUT=""; ERR=""; RC=0
# Every stdout and stderr of every invocation, appended. §2b's first test is a KEY-NAME grep over
# "the full `--json` and `-v` output of every command run against the mock", and a run that only
# kept the last command's output could not make it. The transcript is read once at the end.
TRANSCRIPT="$WORK/transcript.txt"
: > "$TRANSCRIPT"
# `fm <args…>` runs the binary, capturing stdout, stderr and the exit code separately —
# stdout is data and stderr is diagnostics (§4), and a test that merged them could not tell a
# payload from a hint.
fm() {
  OUT="$("$FM" "$@" 2>"$WORK/stderr")"; RC=$?
  ERR="$(cat "$WORK/stderr")"
  { printf '$ fmonero %s\n' "$*"; printf '%s\n' "$OUT"; printf '%s\n' "$ERR"; } >> "$TRANSCRIPT"
  return 0
}

expect_rc() { # expect_rc <want> <label>
  if [ "$RC" = "$1" ]; then ok "$2 (exit $1)"; else
    bad "$2: expected exit $1, got $RC" "stdout: ${OUT:0:300}" "stderr: ${ERR:0:300}"
  fi
}

expect_reason() { # expect_reason <want> <label>
  local got; got="$(printf '%s' "$OUT" | jq -r '.reason // "null"' 2>/dev/null)"
  if [ "$got" = "$1" ]; then ok "$2 (reason $1)"; else
    bad "$2: expected reason $1, got $got" "stdout: ${OUT:0:300}"
  fi
}

expect_json() { # expect_json <jq filter> <want> <label>
  local got; got="$(printf '%s' "$OUT" | jq -c "$1" 2>/dev/null)"
  if [ "$got" = "$2" ]; then ok "$3"; else
    bad "$3: $1 = $got, expected $2" "stdout: ${OUT:0:400}"
  fi
}

expect_contains() { # expect_contains <needle> <label>  (searches stdout AND stderr)
  case "$OUT$ERR" in
    *"$1"*) ok "$2" ;;
    *) bad "$2: '$1' appears in neither stdout nor stderr" "stdout: ${OUT:0:200}" "stderr: ${ERR:0:200}" ;;
  esac
}

# §5c's gate: the top-level key set must be EXACTLY the documented one — no missing key, no
# extra key. A superset would let a payload grow keys nothing documents; a subset would let one
# quietly lose a key an agent parses.
# Every label is recorded, so the coverage step near the end of this file can COMPARE the set of
# commands asserted against the set of commands that exist. Without the tally that step printed a
# reassuring title and two skips and asserted nothing — a new §4.1 command could arrive with no
# `expect_keys` line at all and the run would still say "0 failed", which is the exact drift §5c
# says it exists to prevent.
ASSERTED="$WORK/asserted.txt"
: > "$ASSERTED"
expect_keys() { # expect_keys <label> <space-separated keys>
  local label="$1"; shift
  local want got
  # The label may carry a parenthesised variant ("wallet send (dry run)"); the command is the
  # part before it.
  printf '%s\n' "${label%% (*}" >> "$ASSERTED"
  # shellcheck disable=SC2048,SC2086
  want="$(printf '%s\n' $* | sort -u | tr '\n' ' ')"
  got="$(printf '%s' "$OUT" | jq -r 'keys[]' 2>/dev/null | sort -u | tr '\n' ' ')"
  if [ "$want" = "$got" ]; then ok "§5c key set: $label"; else
    bad "§5c key set: $label" "want: $want" "got:  $got"
  fi
}

# ---- the mocks ---------------------------------------------------------------
#
# `mock_node_up` and `mock_wallet_up` are the two helpers §2c asks for by name. The wallet one
# writes the login file **before** any fmonero invocation: with no login file the client sends
# unauthenticated requests, the mock answers 401 twice and the command exits 4 `auth_refused`,
# so a test that omitted it would be testing exit 4 rather than the command it named.
NODE_LOG=""; WALLET_LOG=""
mock_node_up() { # mock_node_up [extra argv…]
  NODE_LOG="$WORK/$CASE/node.jsonl"
  mkdir -p "$(dirname "$NODE_LOG")"
  "$MOCK" --role node --port "$NODE_PORT" --fixtures "$ROOT/tests/fixtures" \
          --log "$NODE_LOG" "$@" >/dev/null 2>&1 &
  MOCKS+=("$!")
  "$FM" settings set node.url "http://127.0.0.1:$NODE_PORT" >/dev/null
  wait_port "$NODE_PORT"
}

mock_wallet_up() { # mock_wallet_up [extra argv…]
  WALLET_LOG="$WORK/$CASE/wallet.jsonl"
  mkdir -p "$(dirname "$WALLET_LOG")"
  "$MOCK" --role wallet --port "$WALLET_PORT" --fixtures "$ROOT/tests/fixtures" \
          --log "$WALLET_LOG" --auth fmonero:t3st "$@" >/dev/null 2>&1 &
  MOCKS+=("$!")
  "$FM" settings set wallet.url "http://127.0.0.1:$WALLET_PORT" >/dev/null
  "$FM" settings set wallet.port "$WALLET_PORT" >/dev/null
  printf 'fmonero:t3st' > "$FMONERO_STATE_DIR/run/monero-wallet-rpc.$WALLET_PORT.login"
  chmod 600 "$FMONERO_STATE_DIR/run/monero-wallet-rpc.$WALLET_PORT.login"
  wait_port "$WALLET_PORT"
}

# Is anything LISTENING on this port? Not "does it answer": a mock started with `--delay-ms
# 10000` accepts the connection and then says nothing, and a readiness probe that waited for an
# answer would spend a hundred seconds per case discovering that the delay works.
#   curl 7  = could not connect          -> nothing is listening
#   curl 28 = timed out after connecting -> listening, and deliberately slow
#   curl 0  = answered
port_open() {
  curl -s -o /dev/null --connect-timeout 1 --max-time 1 -d '{}' "127.0.0.1:$1/get_height" >/dev/null 2>&1
  [ "$?" != 7 ]
}

wait_port() {
  for _ in $(seq 100); do
    port_open "$1" && return 0
    sleep 0.05
  done
  return 1
}

mocks_down() {
  for pid in "${MOCKS[@]:-}"; do [ -n "$pid" ] && kill "$pid" 2>/dev/null; done
  MOCKS=()
  # Wait for the listeners to release the FIXED ports before the next case binds them.
  for _ in $(seq 60); do
    port_open "$NODE_PORT" || port_open "$WALLET_PORT" || return 0
    sleep 0.05
  done
}

logged() { # logged <log> <method>  -> count
  [ -f "$1" ] || { echo 0; return; }
  jq -s --arg m "$2" '[.[] | select(.method == $m)] | length' "$1" 2>/dev/null || echo 0
}
log_lines() { [ -f "$1" ] && wc -l < "$1" | tr -d ' ' || echo 0; }

# A systemctl fixture (§2i): `<unit> <is-active> <is-enabled> <exists> <main-pid>`.
systemctl_fixture() { # systemctl_fixture <layout>
  local f="$WORK/$CASE/systemctl.txt"
  mkdir -p "$(dirname "$f")"
  case "$1" in
    user_unit) printf 'fmonero-monerod.service active disabled yes 0\nfmonero-wallet-rpc.service active disabled yes 0\nmonerod.service inactive disabled no -\n' > "$f" ;;
    system_unit) printf 'fmonero-monerod.service inactive disabled no -\nfmonero-wallet-rpc.service inactive disabled no -\nmonerod.service active enabled yes 8421\n' > "$f" ;;
    none) printf 'fmonero-monerod.service inactive disabled no -\nfmonero-wallet-rpc.service inactive disabled no -\nmonerod.service inactive disabled no -\n' > "$f" ;;
  esac
  export FMONERO_SYSTEMCTL_FIXTURE="$f"
}

# A real, checksum-valid mainnet address from upstream's own test vectors, and the one
# `validate_address-standard.json` answers for.
DEST=42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm
BUILT_TXID=a1e7b3c05d928f4617ac0e3b85d2f719c46b0a8e3f512d7409bc6ea18d35047f

SEND_KEYS="ok schema relayed dry_run txids amount_pico amount_xmr fee_pico fee_xmr total_pico total_xmr dest account_index priority weight txs"

# ================================================================ GREEN (§4b)
step "GREEN — both daemons absent, nothing installed (§4b)"
sandbox green

fm status --json
expect_rc 0 "status with both daemons absent"
expect_json '.node.reachable' 'false' "node.reachable is false, not an error (§2e)"
expect_json '.wallet.reachable' 'false' "wallet.reachable is false"
expect_json '.node.height' 'null' "node.height is null, never 0 (§5)"
expect_json '[.warnings[].id] | contains(["wallet_down"])' 'true' "it warns rather than failing"
expect_keys "status" "ok schema generated_at node wallet units pending warnings last_error staleness"

# §2e: `status` never starts anything. The systemctl fixture is the observable proof: a start
# would flip the <is-active> column, and `units::{start,stop}` are the only things that do.
systemctl_fixture none
before="$(cat "$FMONERO_SYSTEMCTL_FIXTURE")"
fm status --json
after="$(cat "$FMONERO_SYSTEMCTL_FIXTURE")"
if [ "$before" = "$after" ]; then ok "§2e: status started nothing"; else
  bad "§2e: status changed the unit state" "$after"
fi
unset FMONERO_SYSTEMCTL_FIXTURE

fm wallet list --json
expect_rc 0 "wallet list on an empty directory"
expect_json '.wallets | length' '0' "no wallets"
expect_keys "wallet list" "ok schema wallets dir"
expect_contains "fmonero wallet create" "the empty list names the command that creates one (§4.1)"
expect_contains "restore-deterministic-wallet" "and the one that brings an existing seed back (§4.1)"

fm wallet validate "$DEST" --json
expect_rc 0 "wallet validate falls back to the structural verdict"
expect_json '.structural_only' 'true' "and says so (§4.1)"
expect_json '.nettype' '"mainnet"' "the decoded prefix names the network"
expect_keys "wallet validate" "ok schema address valid integrated subaddress nettype structural_only"

fm wallet validate "not-an-address" --json
expect_rc 15 "an unparsable address is exit 15"

fm settings list --json
expect_rc 0 "settings list"
expect_json '.settings | length' '20' "the twenty keys of §4"
expect_keys "settings list" "ok schema settings"

fm settings get wallet.priority --json
expect_rc 0 "settings get"
expect_json '.value' '"2"' "the default priority is 2 — normal, never 0 (§1b.4)"
expect_keys "settings get" "ok schema key value default"

fm settings set wallet.account 1 --json
expect_rc 0 "settings set"
expect_keys "settings set" "ok schema key old new"
fm settings set wallet.account 0 --json

fm settings set nope 1 --json
expect_rc 2 "an unknown settings key is exit 2"
expect_reason unknown_key "and it is redirected, not merely refused (§4)"
expect_contains "fmonero settings list" "with the documented fallback when nothing is near (§4)"
for typo in wallet.prioriti timeouts.node node.log; do
  fm settings set "$typo" 2 --json
  expect_contains "did you mean" "'$typo' is redirected to the nearest real key (§4)"
done

fm settings set node.url http://10.0.0.5:18081 --json
expect_rc 2 "a non-loopback node.url is refused (§2c)"
expect_reason non_loopback_url "with the loopback reason"
fm settings set wallet.url http://10.0.0.5:18083 --json
expect_rc 2 "and so is a non-loopback wallet.url — the client that moves money (§2c)"
expect_reason non_loopback_url "with the same reason"

fm capabilities
expect_rc 0 "capabilities"
expect_json '.exit_codes | length' '18' "18 exit codes (§4a)"
expect_json '.reasons | length > 0' 'true' "the reason table is generated"
expect_json '.env | length > 0' 'true' "the environment table"
expect_json '.commands | length > 0' 'true' "the command table"
expect_json '[.commands[] | select(.accepts_dry_run)] | length' '4' "exactly four commands accept --dry-run (§4)"
expect_keys "capabilities" "ok tool version schema contract_version build_stage build_note exit_codes reasons settings warnings commands env"

fm version --json
expect_rc 0 "version"
expect_keys "version" "ok schema version build_stage build_note"
expect_contains "A REAL SEND OR SWEEP OF ANY AMOUNT" "§2j: the note says the send path has never been broadcast"

if "$FM" robot-docs | head -1 | grep -q 'robot docs'; then ok "robot-docs prints plain text"; else
  bad "robot-docs"
fi
if "$FM" robot-docs | grep -q 'pending_send'; then ok "robot-docs names the reasons an agent branches on"; else
  bad "robot-docs does not name the reasons"
fi
fm robot-docs nonsense
expect_rc 2 "an unknown docs section is exit 2"

fm node log --json
expect_rc 0 "node log with no log file (§4b)"
expect_json '.lines | length' '0' "no lines"
expect_keys "node log" "ok schema path lines truncated"
expect_contains "no log yet" "and it says so"

fm node log --follow --json
expect_rc 2 "--follow is refused in the CLI, where a pipe would hang (§2d)"
expect_reason unsupported_flag "with the unsupported-flag reason"

fm wallet clear-pending --yes --json
expect_rc 0 "wallet clear-pending with no record is pure state"
expect_json '.cleared' 'false' "nothing to clear"
expect_keys "wallet clear-pending" "ok schema cleared pending"
fm wallet clear-pending --json
expect_rc 3 "and without --yes it is exit 3"
expect_reason needs_yes "needs_yes"

fm node down --dry-run --json
expect_rc 2 "--dry-run is implemented by exactly four commands (§4)"
expect_reason unsupported_flag "and is never silently ignored"

fm --timeout 0 status --json
expect_rc 2 "--timeout 0 is not a deadline"

fm node up --json
expect_rc 4 "node up with no units is exit 4 (§4b)"
expect_reason unit_missing "unit_missing"

fm wallet open main --json
expect_rc 9 "wallet open with no TTY and no password source is exit 9 (§2b.1)"
expect_contains "FMONERO_WALLET_PASSWORD" "and the hint names the way an agent supplies one"
FMONERO_WALLET_PASSWORD=x fm --timeout 3 wallet open main --json
expect_rc 4 "wallet open with a password but no unit is exit 4 (§4b)"
expect_reason unit_missing "unit_missing, answered from the unit directory — not exit 8 30 s later"

fm tui
expect_rc 2 "tui without a TTY is exit 2 (§4)"

fm nonsense
expect_rc 2 "an unknown command is exit 2"
fm wallet send
expect_rc 2 "a missing argument is exit 2"
expect_reason bad_args "bad_args"
fm wallet send "$DEST" 0.1 --priority 9
expect_rc 2 "priority 9 is out of range"
fm wallet send "$DEST" 1.2345678912345
expect_rc 2 "13 decimal places is not an amount (§2a.5)"
expect_reason bad_amount "bad_amount"
fm wallet send "$DEST" all
expect_rc 2 "'all' is not an amount anywhere in the CLI (§4)"
expect_contains "fmonero wallet sweep" "and the hint names the command that moves everything"

for h in "--help" "wallet --help" "node --help" "settings --help" "setup --help" "doctor --help" "tui --help" "wallet send --help"; do
  # shellcheck disable=SC2086
  if "$FM" $h >/dev/null 2>&1; then ok "fmonero $h"; else bad "fmonero $h"; fi
done

# Bare `fmonero` on a pipe is `status --json` (§4).
if "$FM" | jq -e '.schema == 1' >/dev/null 2>&1; then ok "bare fmonero on a pipe is status --json (§4)"; else
  bad "bare fmonero on a pipe"
fi

# §2a.1's GREEN rows: the refusal order is local first, so a bad address still loses to an
# unreachable daemon and a valid one does not.
fm settings set wallet.url "http://127.0.0.1:$WALLET_PORT" >/dev/null
fm settings set wallet.port "$WALLET_PORT" >/dev/null
fm --timeout 3 wallet send "not-an-address" 0.1 --json
expect_rc 15 "a bad address is exit 15 even with the wallet daemon down (§2a.1)"
fm --timeout 3 wallet send "$DEST" 0.1 --json
expect_rc 4 "a VALID address with the wallet daemon down is exit 4, not the no---yes 3 (§4b)"
expect_reason unreachable "reason unreachable, because nothing is listening (§2c)"
# §4: every hint is a runnable command that HELPS. `unreachable` used to fall through to
# `Error::default_hint`'s `omarchy pkg add monero` — an install command for a daemon that is
# installed and merely not running, and the most frequently hit error path in the whole tool.
HINT="$(printf '%s' "$OUT" | jq -r .hint)"
case "$HINT" in
  *"omarchy pkg add"*) bad "§4: 'unreachable' tells the user to install what they already have" "$HINT" ;;
  *"fmonero wallet open"*|*"fmonero node up"*) ok "§4: and the hint names a command that STARTS it" ;;
  *) bad "§4: the unreachable hint names no start command" "$HINT" ;;
esac
fm --timeout 3 wallet send "$DEST" 0.1 --dry-run --yes --json
expect_rc 4 "--dry-run --yes with no daemon is the same, which is proof the pair cannot relay"

# ---- the unit is there and the DAEMON is not: exit 4 not_installed (§2i, §4b) --------------
#
# Finding 1 of the Phase 4 verification, and the state `scripts/install.sh --yes` leaves on a
# machine `omarchy pkg add monero` has not reached — so it is the first thing an owner meets.
# Before v5.1 both commands waited 30 s, exited **8** ("nothing is broken") and left the unit
# restarting every 5 s on 203/EXEC.
#
# Two deliberate choices make this case safe to run and impossible to fake:
#   * no `FMONERO_SYSTEMCTL_FIXTURE`, because the fixture stands in for the machine's daemon as
#     well (§2i) and would excuse the very check this case exists to prove;
#   * a unit NAME systemd has never heard of, so even a regression that reached
#     `systemctl --user start` could neither start nor leave flapping one of the owner's units.
step "§2i: a unit that is installed with no Monero binary is exit 4 not_installed"
sandbox notinstalled
if command -v monerod >/dev/null 2>&1; then
  skip "monerod is on PATH on this machine, so not_installed is unobservable here"
else
  fm settings set node.unit fmonero-e2e-nosuch-node.service --json
  fm settings set wallet.unit fmonero-e2e-nosuch-wallet.service --json
  printf '[Unit]\n' > "$FMONERO_SYSTEMD_USER_DIR/fmonero-e2e-nosuch-node.service"
  printf '[Unit]\n' > "$FMONERO_SYSTEMD_USER_DIR/fmonero-e2e-nosuch-wallet.service"
  START="$(date +%s%N)"
  fm node up --json
  expect_rc 4 "node up with a unit installed and monerod absent"
  expect_reason not_installed "not_installed, not the exit 8 of a 30 s wait (finding 1)"
  expect_contains "omarchy pkg add monero" "and the hint installs what is missing (§4a)"
  FMONERO_WALLET_PASSWORD=x fm wallet open main --json
  expect_rc 4 "wallet open with a unit installed and monero-wallet-rpc absent"
  expect_reason not_installed "not_installed"
  fm node restart --yes --json
  expect_rc 4 "node restart refuses BEFORE its own systemctl stop"
  expect_reason not_installed "not_installed"
  ELAPSED_MS=$(( ( $(date +%s%N) - START ) / 1000000 ))
  if [ "$ELAPSED_MS" -lt 3000 ]; then
    ok "all three answered in ${ELAPSED_MS}ms — milliseconds, not deadlines (§2i)"
  else
    bad "§2i: the preflight took ${ELAPSED_MS}ms for three commands; it must not wait on anything"
  fi
fi

# ---- every needs_yes hint is the user's OWN command, and it RUNS (§4) ----------------------
#
# V1 of the fix-up verification. `require_yes` built its hint as `"<command> --yes"` and dropped
# the command's required positional, so an agent that did the one thing §4 promises works on
# exit 3 `needs_yes` — append `--yes` and re-run the hint — got exit **2** back:
# `fmonero wallet create --yes` answers "the following required arguments were not provided:
# <NAME>". `send`/`sweep` were gated (`--max-fee`, `--below`, above); the other six were not,
# which is why it survived four review passes. Each row below refuses, compares the hint against
# the user's own command, and then RUNS that hint verbatim: with no daemon and no unit it must
# fail on the machine, never on its own argv.
step "§4: every needs_yes hint is the user's own command, and running it gets past clap"
sandbox hints
hint_runs() { # hint_runs <expected hint> <the hint's own expected exit> <command…>
  local want="$1" want_rc="$2"; shift 2
  fm --timeout 3 "$@" --json
  expect_rc 3 "$* without --yes"
  expect_reason needs_yes "needs_yes"
  local hint; hint="$(printf '%s' "$OUT" | jq -r '.hint // ""')"
  if [ "$hint" = "$want" ]; then ok "§4: the hint is the user's own command — $hint"; else
    bad "§4: the hint is not the user's own command with --yes appended" "want: $want" "got:  $hint"
  fi
  local argv; read -r -a argv <<<"${hint#fmonero }"
  fm --timeout 3 "${argv[@]}" --json
  local reason; reason="$(printf '%s' "$OUT" | jq -r '.reason // "null"')"
  if [ "$RC" = 2 ] || [ "$reason" = bad_args ]; then
    bad "§4: the hint fails on its own argv, not on the machine" "$hint" "exit $RC, reason $reason"
  else
    ok "§4: \`$hint\` gets past clap (exit $RC, reason $reason)"
  fi
  expect_rc "$want_rc" "and the hint's own exit is the machine's answer"
}
hint_runs "fmonero node down --yes"            4 node down
hint_runs "fmonero node restart --yes"         4 node restart
hint_runs "fmonero wallet clear-pending --yes" 0 wallet clear-pending
hint_runs "fmonero wallet book remove 3 --yes" 4 wallet book remove 3
hint_runs "fmonero wallet set-daemon 127.0.0.1:18081 --yes" 4 wallet set-daemon 127.0.0.1:18081
# Last, because the password has to be in the environment for the hint's own run: `wallet
# create` refuses BEFORE it asks for one (§4.1), so only the second invocation needs it.
FMONERO_WALLET_PASSWORD=x hint_runs "fmonero wallet create zzhint --yes" 4 wallet create zzhint
unset FMONERO_WALLET_PASSWORD
# No wallet file was created by either invocation: the refusal is before the prompt and the run
# stops at the missing unit (§2i's preflight).
fm wallet list --json
expect_json '.wallets | length' '0' "and neither invocation created a wallet file"
#
# V2: the `unit_missing` hint and `setup`'s own refusal. `fmonero setup --units` was itself an
# exit 3 `needs_yes`, so the hint cost the agent a third round trip; and that refusal hinted
# `fmonero setup --all --yes`, which WIDENS what the user asked for.
fm node up --json
expect_rc 4 "node up with no unit installed"
expect_reason unit_missing "unit_missing"
expect_json '.hint' '"fmonero setup --units --yes"' "§4: the unit_missing hint runs, rather than refusing in turn"
fm setup --units --json
expect_rc 3 "setup --units without --yes"
expect_reason needs_yes "needs_yes"
expect_json '.hint' '"fmonero setup --units --yes"' "§4: and setup hints the user's own command, not --all"

# §4.2's two gate lines, run against the REAL config dir's renderer but writing nothing.
step "setup --units --dry-run (§4.2)"
sandbox setup
# The one case that needs `wallet.dir` ABSENT: §4.2 promises a `CreateDir` action per missing
# ancestor, and `sandbox` creates the directory for every other case.
rm -rf "$XDG_DATA_HOME/fmonero"
fm setup --units --dry-run --json
expect_rc 0 "setup --units --dry-run"
expect_keys "setup" "ok schema actions dry_run"
expect_json '[.actions[] | select(.what | test("CreateDir wallet.dir"))] | length > 0' 'true' \
  "it lists a CreateDir action for wallet.dir when it is absent (§4.2)"
expect_json '[.actions[] | select(.status == "written")] | length' '0' "a dry run writes nothing"
if printf '%s' "$OUT" | grep -q 'rpc-login=fmonero:'; then
  bad "§4.2: the node credential must be redacted in everything setup prints"
else
  ok "§4.2: the rpc-login value is redacted"
fi
# The %h guard, in the form that can actually fail: the rendered monerod.conf hunk.
CONF_HUNK="$(printf '%s' "$OUT" | jq -r '.actions[] | select(.what == "monerod.conf") | .diff // ""')"
if [ -z "$CONF_HUNK" ]; then
  bad "§4.2: the dry run produced no monerod.conf diff to grep"
elif printf '%s' "$CONF_HUNK" | grep -q '%[hSt]'; then
  bad "§4.2: a systemd specifier reached monerod.conf" "$(printf '%s' "$CONF_HUNK" | grep -n '%[hSt]')"
else
  ok "§4.2: no %h, %S or %t in the rendered monerod.conf"
fi
# And `--wallet-dir` in the RENDERED unit equals `settings get wallet.dir`. The sandbox's
# systemd directory is empty, so the wallet unit is always `planned` and always carries a diff —
# which is what this used to assume and, on a machine `setup --units` had already configured,
# was false: an `unchanged` action carries `"diff": null` by contract (§5b `FileAction`) and the
# grep then read an empty string, reporting a mismatch against a correct machine.
UNIT_DIR="$(printf '%s' "$OUT" | jq -r '.actions[] | select(.what == "wallet unit") | .diff // ""' \
            | grep -o -- '--wallet-dir [^ ]*' | head -1 | cut -d' ' -f2)"
WANT_DIR="$("$FM" settings get wallet.dir --json | jq -r .value)"
if [ -n "$UNIT_DIR" ] && [ "$UNIT_DIR" = "$WANT_DIR" ]; then
  ok "§4.2: the rendered unit's --wallet-dir is settings.wallet.dir byte for byte"
else
  bad "§4.2: --wallet-dir mismatch in the rendered unit" "unit: $UNIT_DIR" "setting: $WANT_DIR"
fi
# And the same equality on the INSTALLED unit — the file the machine actually runs, read from
# disk rather than from an action diff, with the sandbox dropped so both sides are the owner's.
# This is the row the Phase 4 verifier had to check by hand.
INSTALLED_WALLET_UNIT="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/fmonero-wallet-rpc.service"
unsandboxed() { env -u FMONERO_STATE_DIR -u FMONERO_CONFIG_DIR -u FMONERO_CACHE_DIR \
                   -u FMONERO_SYSTEMD_USER_DIR -u XDG_DATA_HOME "$FM" "$@"; }
if [ -f "$INSTALLED_WALLET_UNIT" ]; then
  IU_DIR="$(grep -o -- '--wallet-dir [^ ]*' "$INSTALLED_WALLET_UNIT" | head -1 | cut -d' ' -f2)"
  IW_DIR="$(unsandboxed settings get wallet.dir --json | jq -r .value)"
  if [ -n "$IU_DIR" ] && [ "$IU_DIR" = "$IW_DIR" ]; then
    ok "§4.2: and the INSTALLED unit's --wallet-dir equals this machine's wallet.dir"
  else
    bad "§4.2: the installed unit and the settings disagree" "unit: $IU_DIR" "setting: $IW_DIR"
  fi
else
  skip "§4.2: no wallet unit is installed on this machine, so there is no file to read"
fi
# §4.2 v5.1: the node unit's `--config-file` is the absolute, XDG-resolved `monerod.conf`, never
# `%h/...` — with a non-default XDG_CONFIG_HOME the specifier named a file setup never wrote
# (`secrets-5`), which is why this reads the CONFIG DIR the sandbox set rather than a literal.
NODE_HUNK="$(printf '%s' "$OUT" | jq -r '.actions[] | select(.what == "node unit") | .diff // ""')"
WANT_CONF="${FMONERO_CONFIG_DIR}/monerod.conf"
if printf '%s' "$NODE_HUNK" | grep -q -- "--config-file $WANT_CONF --non-interactive"; then
  ok "§4.2: the rendered node unit names $WANT_CONF absolutely"
else
  bad "§4.2: the node unit's --config-file is not the XDG-resolved conf path" \
      "$(printf '%s' "$NODE_HUNK" | grep -o -- '--config-file [^ ]*' | head -1)" "want: $WANT_CONF"
fi
if printf '%s' "$NODE_HUNK" | grep -q '%'; then
  bad "§4.2 v5.1: a systemd specifier is left in the node unit" \
      "$(printf '%s' "$NODE_HUNK" | grep -n '%')"
else
  ok "§4.2 v5.1: the node unit carries no specifier at all"
fi
# The INSTALLED node unit is deliberately **not** asserted here. Whether the file on disk carries
# the absolute path is a question about whether `setup --units` has been re-run since this build,
# which is the doctor's `units_present` (it compares the installed bytes against `render_*` byte
# for byte) — and an e2e case that failed until the owner re-ran the installer would be a case
# that fails for a reason it does not name.
# §4.2: a test never installs a unit — and `units::install` refusing is exactly the failure
# that used to leave the DERIVED settings rewritten on disk. §4.2 promises "running it twice
# leaves the same machine"; a run that reports failure and has already changed `node.url`,
# `wallet.url`, `wallet.port` and `wallet.dir` is neither idempotent nor honest, and the next
# `status` then talks to the wrong port. Byte-compare config.toml across the failure.
fm settings set node.url "http://127.0.0.1:19999" >/dev/null
fm settings set wallet.port 19998 >/dev/null
cp "$FMONERO_CONFIG_DIR/config.toml" "$WORK/setup/config.before"
fm setup --units --yes --json
expect_rc 3 "setup --units --yes refuses to write under FMONERO_STATE_DIR (§4.2)"
if cmp -s "$WORK/setup/config.before" "$FMONERO_CONFIG_DIR/config.toml"; then
  ok "§4.2: and a failed --units left config.toml byte-identical"
else
  bad "§4.2: a failed setup --units kept the settings it had already rewritten" \
      "$(diff "$WORK/setup/config.before" "$FMONERO_CONFIG_DIR/config.toml" | head -6)"
fi
fm setup --json
expect_rc 2 "setup with no target is exit 2"

# §1.3's free-space floor, which §4b listed as RED with "nothing exercises it" — and which is a
# purely local `statvfs` measurement, so nothing about it needs a daemon. It is measured on the
# filesystem holding `node.data_dir`, so pointing that at one with less than 120 GB free is the
# whole test. `$WORK` is `mktemp -d`; when it is roomy this case says so rather than passing.
step "§1.3: setup --units refuses under the 120 GB floor, and --full-chain --yes overrides it"
sandbox diskfloor
FREE_KB="$(df -Pk "$WORK" | awk 'NR==2 {print $4}')"
FLOOR_KB=$((120 * 1024 * 1024))
fm settings set node.data_dir "$WORK/$CASE/bitmonero" --json
if [ "${FREE_KB:-0}" -ge "$FLOOR_KB" ]; then
  skip "§1.3: $(( FREE_KB / 1048576 )) GB free on $WORK — the floor cannot be crossed here"
else
  fm setup --units --dry-run --json
  expect_rc 3 "setup --units below the floor"
  expect_reason full_chain_refused "full_chain_refused"
  expect_contains "ESTIMATE" "and the message says the floor is an estimate, not a measurement (§1.3)"
  # `--dry-run` so the global needs_yes check does not answer first: §4.2's own refusal is the
  # one being tested, and it is the flag PAIR that overrides the floor, never the flag.
  fm setup --units --full-chain --dry-run --json
  expect_rc 3 "--full-chain without --yes is still refused"
  expect_reason full_chain_refused "the flag PAIR is the escape hatch, not the flag"
  fm setup --units --full-chain --yes --dry-run --json
  expect_rc 0 "--full-chain --yes --dry-run overrides the floor and writes nothing"
  expect_json '[.actions[] | select(.status == "written")] | length' '0' "a dry run writes nothing"
  CONF="$(printf '%s' "$OUT" | jq -r '.actions[] | select(.what == "monerod.conf") | .diff // ""')"
  if printf '%s' "$CONF" | grep -q 'prune-blockchain'; then
    bad "§1.3: --full-chain must render a conf with no prune-blockchain" "$(printf '%s' "$CONF" | grep -n prune)"
  else
    ok "§1.3: and the rendered conf carries no prune-blockchain"
  fi
fi

# ================================================================ AMBER (§4b)
step "AMBER — against fmonero-mock over the derived fixtures (§4b)"
sandbox amber
mock_node_up
mock_wallet_up

fm --timeout 3 status --json
expect_rc 0 "status against both mocks"
expect_json '.node.reachable' 'true' "the node answers"
expect_json '.wallet.open' 'true' "and a wallet is open"
expect_json '.wallet.balance_xmr' '"1.234567891234"' "the balance is rendered exactly (§5)"
expect_json '.wallet.subaddress_count' '3' "every GENERATED subaddress, not only the funded one (§5)"
expect_json '.node.authenticated' 'false' "the node accepted us with no credential"
expect_json '.wallet.authenticated' 'true' "the wallet completed a Digest handshake (§2c)"
expect_json '[.warnings[].id] | contains(["auth_missing"])' 'true' "and auth_missing says so"

fm --timeout 3 wallet balance --json
expect_rc 0 "wallet balance"
expect_keys "wallet balance" "ok schema account_index balance_pico balance_xmr unlocked_pico unlocked_xmr blocks_to_unlock time_to_unlock_sec subaddresses"
fm --timeout 3 wallet balance --subaddresses --json
expect_json '.subaddresses | length' '1' "--subaddresses lists only the FUNDED ones (§5)"

fm --timeout 3 wallet address --json
expect_rc 0 "wallet address"
expect_keys "wallet address" "ok schema primary addresses selected"
fm --timeout 3 wallet address --index 1 --json
expect_json '.selected' '"8AsN91rznfkBGTY8psSNkJBg9SZgxxGGRUhGwRptBhgr5XSQ1XzmA9m8QAnoxydecSh5aLJXdrgXwTDMMZ1AuXsN1EX5Mtm"' "--index selects a subaddress"
fm --timeout 3 wallet address --index 99 --json
expect_rc 2 "an index the account does not have is exit 2"

fm --timeout 3 wallet new-address --label test --json
expect_rc 0 "wallet new-address"
expect_keys "wallet new-address" "ok schema primary addresses selected"

fm --timeout 3 wallet history --json
expect_rc 0 "wallet history"
expect_keys "wallet history" "ok schema transfers cache_age_sec high_water cached pool_unavailable"
expect_json '[.transfers[].type] | contains(["block"])' 'true' "a coinbase row deserializes (§1b.4)"
expect_json '[.transfers[] | select(.locked != null)] | length > 0' 'true' "locked is derived, not cached (§5b)"
HIST_BODY="$(jq -s '[.[] | select(.method=="get_transfers")] | last | .body.params' "$WALLET_LOG")"
if printf '%s' "$HIST_BODY" | jq -e '.filter_by_height == true' >/dev/null; then
  ok "§1b.4: the top-up sends filter_by_height together with min_height"
else
  bad "§1b.4: filter_by_height was not sent — the bound is then silently ignored" "$HIST_BODY"
fi
fm --timeout 3 wallet history --cached --json
expect_rc 0 "wallet history --cached"
expect_json '.cached' 'true' "it says it is cached"
expect_json '[.transfers[] | select(.confirmations != null)] | length' '0' "confirmations is null with --cached (§4.1)"
expect_json '[.transfers[] | select(.locked != null)] | length' '0' "and so is locked — never 0 standing in for unknown"
expect_contains "cached: confirmations unknown" "and the footer says which one --cached gives up (§4.1)"

fm --timeout 3 wallet book list --json
expect_rc 0 "wallet book list"
expect_keys "wallet book list" "ok schema entries"
fm --timeout 3 wallet book add "$DEST" friend --json
expect_rc 0 "wallet book add"
expect_keys "wallet book add" "ok schema index address description"
fm --timeout 3 wallet book remove 0 --yes --json
expect_rc 0 "wallet book remove"
expect_keys "wallet book remove" "ok schema index address description"
fm --timeout 3 wallet book remove 0 --json
expect_rc 3 "wallet book remove without --yes is exit 3"

fm --timeout 3 wallet notes get "$BUILT_TXID" --json
expect_rc 0 "wallet notes get"
expect_keys "wallet notes get" "ok schema txids notes"
fm --timeout 3 wallet notes set "$BUILT_TXID" coffee --json
expect_rc 0 "wallet notes set"
expect_keys "wallet notes set" "ok schema txids notes"
fm --timeout 3 wallet notes get "not-a-txid" --json
expect_rc 2 "a malformed txid is exit 2"

fm --timeout 3 wallet refresh --json
expect_rc 0 "wallet refresh"
expect_keys "wallet refresh" "ok schema blocks_fetched received_money"

fm --timeout 3 wallet validate "$DEST" --json
expect_rc 0 "wallet validate against the daemon"
expect_json '.structural_only' 'false' "the daemon answered, so it is not structural-only"
VAL_BODY="$(jq -s '[.[] | select(.method=="validate_address")] | last | .body.params' "$WALLET_LOG")"
if printf '%s' "$VAL_BODY" | jq -e '.any_net_type == true and .allow_openalias == false' >/dev/null; then
  ok "§2a.6: any_net_type is true and allow_openalias is false"
else
  bad "§2a.6: the two validate_address flags are wrong" "$VAL_BODY"
fi

FMONERO_WALLET_PASSWORD=t3stpw fm --timeout 3 wallet open main --json
expect_rc 0 "wallet open"
expect_keys "wallet open" "ok schema name open unit_started nettype"
ORDER="$(jq -rs '[.[] | select(.status==200) | .method] | map(select(. == "open_wallet" or . == "set_daemon" or . == "auto_refresh")) | join(",")' "$WALLET_LOG")"
if [ "$ORDER" = "open_wallet,set_daemon,auto_refresh" ]; then
  ok "§4.2: open_wallet, then set_daemon, then auto_refresh — the order is not free"
else
  bad "§4.2: the open sequence is out of order" "$ORDER"
fi

# §4.2 + upstream: `on_set_daemon` builds the optional login only from a non-empty
# username/password (`wallet_rpc_server.cpp:4754-4757`) and `wallet2::set_daemon` then assigns
# `m_daemon_login = std::move(daemon_login)` UNCONDITIONALLY (`wallet2.cpp:1393`) — so omitting
# the fields does not keep the wallet's credential, it CLEARS it, and every wallet→node call
# 401s from then on with nothing reporting an error.
printf 'rpc-login=fmonero:0123456789abcdef0123456789abcdef\n' > "$FMONERO_CONFIG_DIR/monerod.conf"
chmod 600 "$FMONERO_CONFIG_DIR/monerod.conf"
fm --timeout 3 wallet set-daemon 127.0.0.1:18081 --trusted --yes --json
expect_rc 0 "wallet set-daemon"
expect_keys "wallet set-daemon" "ok schema address trusted authenticated"
SD="$(jq -s '[.[] | select(.method=="set_daemon")] | last | .body.params' "$WALLET_LOG")"
if printf '%s' "$SD" | jq -e '.username == "fmonero" and (.password | length) == 32' >/dev/null; then
  ok "§4.2: a loopback set-daemon carries the node credential"
else
  bad "§4.2: set-daemon sent no credential, which CLEARS the wallet's daemon login" "$SD"
fi
fm --timeout 3 wallet set-daemon 10.0.0.5:18081 --trusted --yes --json
SD="$(jq -s '[.[] | select(.method=="set_daemon")] | last | .body.params' "$WALLET_LOG")"
if printf '%s' "$SD" | jq -e 'has("username") | not' >/dev/null; then
  ok "§2c: and a REMOTE one still gets nothing — a remote node is monero-wallet-rpc's business"
else
  bad "§2c: the node credential was sent to a remote address" "$SD"
fi
fm --timeout 3 wallet set-daemon 127.0.0.1:18081 --json
expect_rc 3 "wallet set-daemon without --yes is exit 3"

FMONERO_WALLET_PASSWORD=t3stpw fm --timeout 3 wallet create newone --yes --json
expect_rc 0 "wallet create (the mock records the call; NO wallet file is ever created)"
expect_keys "wallet create" "ok schema name open unit_started seed_backup_hint"
expect_contains "monero-wallet-cli" "§2b.2: it names the command that shows the seed"
if printf '%s' "$OUT" | grep -qE '"seed"|t3stpw'; then
  bad "§2b.2: wallet create must not print a seed or the password"
else
  ok "§2b.2: no seed and no password in the payload"
fi
FMONERO_WALLET_PASSWORD= fm --timeout 3 wallet create empty --yes --json
expect_rc 2 "an empty password is refused without --allow-empty-password (§2b.2)"
fm --timeout 3 wallet create bad/name --yes --json
expect_rc 2 "a wallet name containing '/' is refused locally (§1b.5)"
fm --timeout 3 wallet create newone --json
expect_rc 3 "wallet create without --yes is exit 3"

fm --timeout 3 wallet close --json
expect_rc 0 "wallet close"
expect_keys "wallet close" "ok schema name open unit_started"

fm --timeout 3 node info --json
expect_rc 0 "node info"
expect_keys "node info" "ok schema node txpool sync hard_fork rpc"
expect_json '.hard_fork.current_version != .hard_fork.voting_on.version or .hard_fork.current_version == 16' 'true' \
  "§1b.3: the two fork versions are kept apart"
expect_json '.rpc.version_major' '3' "the RPC version is unpacked from major<<16|minor"

fm --timeout 3 node peers --json
expect_rc 0 "node peers"
expect_keys "node peers" "ok schema connections white gray"
expect_json '.connections[0].port | type' '"number"' "§5c: connections[].port is rendered as an integer"
expect_json '.white[0].port | type' '"number"' "and so is the peer list's, so both arrays agree"

fm --timeout 3 node bans --json
expect_rc 0 "node bans"
expect_keys "node bans" "ok schema bans"

# The mutating node calls need §2i's user_unit layout, and the type gate is what enforces it.
systemctl_fixture user_unit
fm --timeout 3 node ban 1.2.3.4 --json
expect_rc 0 "node ban under the user_unit layout"
expect_keys "node ban" "ok schema host banned seconds"
fm --timeout 3 node unban 1.2.3.4 --json
expect_rc 0 "node unban"
expect_keys "node unban" "ok schema host banned seconds"
fm --timeout 3 node limit --json
expect_rc 0 "node limit with no arguments only reads"
expect_keys "node limit" "ok schema limit_up_kbps limit_down_kbps out_peers in_peers changed"
expect_json '.changed' 'false' "and says it changed nothing"
fm --timeout 3 node limit --up 1024 --json
expect_rc 0 "node limit --up"
expect_json '.changed' 'true' "and says it did"
if jq -se '[.[] | select(.method=="out_peers")] | all(.body.set == false)' "$NODE_LOG" >/dev/null; then
  ok "§1b.3: peers_count always sends set:false"
else
  bad "§1b.3: a read of out_peers wrote instead"
fi

fm --timeout 3 node up --json
expect_rc 0 "node up under the user_unit layout"
expect_keys "node up" "ok schema unit active reachable synchronized waited_sec already_running"
fm --timeout 3 node down --yes --json
expect_rc 0 "node down"
expect_keys "node down" "ok schema unit active reachable synchronized waited_sec already_running"
fm --timeout 3 node restart --yes --json
expect_rc 0 "node restart"
expect_keys "node restart" "ok schema unit active reachable synchronized waited_sec already_running"
fm --timeout 3 node down --json
expect_rc 3 "node down without --yes is exit 3"
unset FMONERO_SYSTEMCTL_FIXTURE

# ---- the money rows, in all three outcomes (§5c) -----------------------------
step "the money path against the mock (§2a, §5c)"
sandbox money
mock_node_up
mock_wallet_up

fm --timeout 3 wallet send "$DEST" 0.1 --dry-run --json
expect_rc 0 "wallet send --dry-run"
expect_keys "wallet send (dry run)" "$SEND_KEYS"
expect_json '.relayed' 'false' "it relayed nothing"
expect_json '.dry_run' 'true' "and says it was a dry run"
expect_json '.fee_xmr' '"0.00003072"' "the fee is the mock's REAL built fee (§2a.2)"
SEND_DRY="$OUT"
if [ "$(logged "$WALLET_LOG" relay_tx)" = "0" ]; then ok "§4: a dry run records zero relay_tx"; else
  bad "§4: a dry run relayed something"
fi
if [ "$(logged "$WALLET_LOG" transfer)" = "1" ]; then ok "and exactly one transfer"; else
  bad "expected one transfer, got $(logged "$WALLET_LOG" transfer)"
fi

fm --timeout 3 wallet send "$DEST" 0.1 --dry-run --yes --json
expect_rc 0 "wallet send --dry-run --yes: --dry-run beats --yes (§4)"
expect_json '.relayed' 'false' "nothing was relayed"

fm --timeout 3 wallet send "$DEST" 0.1 --json
expect_rc 3 "wallet send without --yes is exit 3 (§2a.1 step 10)"
expect_reason needs_yes "needs_yes"
expect_keys "wallet send (the no---yes refusal)" "ok schema code code_name error hint reason $SEND_KEYS"
expect_json '.ok' 'false' "ok is false"
expect_json '.fee_pico' '30720000' "§5c: the refusal carries the BUILT fee, machine-readable"
expect_contains "--yes" "and the hint is the user's own command with --yes appended (§4)"
SEND_REFUSED="$OUT"

fm --timeout 3 wallet send "$DEST" 0.1 --max-fee 0.00000001 --json
expect_rc 3 "a built fee above --max-fee is exit 3 (§2a.2)"
expect_reason fee_above_max "fee_above_max"
# §5c's money exception names BOTH refusals — step 9 `fee_above_max` and step 10 `needs_yes` —
# and the whole point is the BUILT fee being machine-readable: an agent that set --max-fee and
# got only prose has no number to choose a new bound with, and §4 forbids parsing the prose.
expect_keys "wallet send (the --max-fee refusal)" "ok schema code code_name error hint reason $SEND_KEYS"
expect_json '.fee_pico' '30720000' "§5c: the fee_above_max refusal carries the BUILT fee"
expect_json '.relayed' 'false' "and says nothing was relayed"
expect_json '.txids | length' '0' "with no txids"
RELAYS_BEFORE="$(logged "$WALLET_LOG" relay_tx)"

# §4: "the hint is the user's own command with --yes appended VERBATIM". Every bound the user
# gave has to come back: a hint that drops --max-fee prescribes the rebuild §2a.2 warns may
# charge a different fee, with the ceiling removed.
fm --timeout 3 wallet send "$DEST" 0.1 --note coffee --max-fee 1 --json
expect_rc 3 "a send with --max-fee and --note still refuses without --yes"
HINT="$(printf '%s' "$OUT" | jq -r .hint)"
case "$HINT" in
  *"--max-fee 1"*) ok "§4: the hint keeps --max-fee" ;;
  *) bad "§4: the hint dropped --max-fee" "$HINT" ;;
esac
case "$HINT" in
  *"--note coffee"*) ok "§4: and --note" ;;
  *) bad "§4: the hint dropped --note" "$HINT" ;;
esac

fm --timeout 3 wallet send "$DEST" 0.1 --note coffee --yes --json
expect_rc 0 "wallet send --yes"
expect_keys "wallet send (relayed)" "$SEND_KEYS"
expect_json '.relayed' 'true' "it relayed"
expect_json '.txids | length' '1' "one txid"
expect_json ".txids[0]" "\"$BUILT_TXID\"" "and it is the txid that was built (§2a.9)"
SEND_RELAYED="$OUT"
# §2a.3: the relay_tx hex is byte-identical to the metadata the `transfer` response carried.
# The mock served `transfer-do-not-relay.json`, so that fixture's `tx_metadata` IS that value —
# and `tx_blob` is deliberately a different (empty) field, so relaying the wrong one shows up.
RELAY_HEX="$(jq -rs '[.[] | select(.method=="relay_tx")] | last | .body.params.hex' "$WALLET_LOG")"
WANT_HEX="$(jq -r '.tx_metadata' "$ROOT/tests/fixtures/wallet/transfer-do-not-relay.json")"
if [ "$RELAY_HEX" = "$WANT_HEX" ]; then
  ok "§2a.3: relay_tx.hex is byte-identical to the transfer response's tx_metadata"
else
  bad "§2a.3: relay_tx sent the wrong blob" "sent: ${RELAY_HEX:0:60}" "want: ${WANT_HEX:0:60}"
fi
# Six invocations have built so far in this sandbox (--dry-run, --dry-run --yes, no ---yes,
# --max-fee, the --max-fee/--note hint case, --yes) and exactly ONE of them relayed: one build
# per invocation, never two per send, and never a build whose plan is relayed twice (§2a.3).
if [ "$(logged "$WALLET_LOG" transfer)" = "6" ] && [ "$(logged "$WALLET_LOG" relay_tx)" = "1" ]; then
  ok "§2a.3: six builds, one relay — one transfer per invocation and one relay per plan"
else
  bad "§2a.3: expected 6 transfer and 1 relay_tx" \
      "transfer=$(logged "$WALLET_LOG" transfer) relay_tx=$(logged "$WALLET_LOG" relay_tx)"
fi
if [ "$(logged "$WALLET_LOG" store)" -ge 1 ]; then ok "§2a.4: one wallet store after the last relay"; else
  bad "§2a.4: no store was issued, so a SIGKILL would lose the wallet's record"
fi
# §2a.8: no forbidden field in ANY recorded body.
if jq -se 'all(.[] | select(.method=="transfer" or .method=="sweep_all");
              (.body.params | has("unlock_time") or has("payment_id") or has("ring_size")) | not)' \
        "$WALLET_LOG" >/dev/null; then
  ok "§2a.8: no unlock_time, payment_id or ring_size in any recorded body"
else
  bad "§2a.8: a forbidden field was sent"
fi
# The record cleared itself (§2a.4).
fm --timeout 3 status --json
expect_json '.pending' 'null' "§2a.4: a successful send leaves no record behind"

fm --timeout 3 wallet sweep "$DEST" --dry-run --json
expect_rc 0 "wallet sweep --dry-run"
expect_keys "wallet sweep (dry run)" "$SEND_KEYS"
expect_json '.txs | length' '2' "sweep_all.json builds two transactions"
if jq -se 'all(.[] | select(.method=="sweep_all"); .body.params.subaddr_indices_all == true)' "$WALLET_LOG" >/dev/null; then
  ok "§2a.10: every recorded sweep_all carries subaddr_indices_all: true"
else
  bad "§2a.10: a sweep without it moves ONE subaddress chosen at random"
fi
fm --timeout 3 wallet sweep "$DEST" --json
expect_rc 3 "wallet sweep without --yes is exit 3"
fm --timeout 3 wallet sweep "$DEST" --below 0.5 --json
expect_rc 3 "a bounded sweep refuses without --yes too"
HINT="$(printf '%s' "$OUT" | jq -r .hint)"
case "$HINT" in
  # Without it the hint sweeps the WHOLE account — a different amount and a different fee
  # from the one the refusal just described (§4).
  *"--below 0.5"*) ok "§4: the sweep hint keeps --below" ;;
  *) bad "§4: the sweep hint dropped --below, so running it empties the account" "$HINT" ;;
esac
fm --timeout 3 wallet sweep "$DEST" --yes --json
expect_rc 0 "wallet sweep --yes"
expect_json '.txids | length' '2' "one relay_tx per tx_metadata_list entry, in order (§2a.4)"
expect_keys "wallet sweep (relayed)" "$SEND_KEYS"
SWEEP_RELAYED="$OUT"

# §2a.4: an unrelayed record BLOCKS, and --force does not widen it.
printf '{"kind":"send","account_index":0,"dest":"%s","built_at":1,"txs":[{"txid":"%s","amount":1,"fee":2,"relayed":false,"relayed_at":null}]}\n' \
  "$DEST" "$(printf 'f%.0s' $(seq 64))" > "$FMONERO_STATE_DIR/pending.json"
chmod 600 "$FMONERO_STATE_DIR/pending.json"
BEFORE="$(log_lines "$WALLET_LOG")"
fm --timeout 3 wallet send "$DEST" 0.1 --yes --force --json
expect_rc 3 "§2a.4: an unrelayed record blocks even with --yes --force"
expect_reason pending_send "pending_send"
if [ "$(log_lines "$WALLET_LOG")" = "$BEFORE" ]; then ok "§2a: and the mock recorded ZERO new requests"; else
  bad "§2a: --force reached the daemon" "before $BEFORE, after $(log_lines "$WALLET_LOG")"
fi
fm --timeout 3 status --json
expect_json '[.warnings[].id] | contains(["send_may_be_in_flight"])' 'true' "§5: status raises send_may_be_in_flight"
expect_json '.pending.total_count' '1' "and surfaces the record"
expect_json '.pending.relayed_count' '0' "with its counts pre-rendered in Rust (§5b)"
# §2a.11: `clear-pending` is the record's third writer and must hold the same lock. Without it
# a clear between two relay_tx calls of a sweep removes the record under the relay, mark_relayed
# fails for a transaction that really went out, and NOTHING is left on disk to block the next
# send — the lost update §2a.4 put the record in its own file to prevent.
python3 - "$FMONERO_STATE_DIR/send.lock" <<'LOCK' &
import fcntl, os, sys, time
fd = os.open(sys.argv[1], os.O_CREAT | os.O_RDWR, 0o600)
fcntl.flock(fd, fcntl.LOCK_EX)
time.sleep(3)
LOCK
LOCKER=$!
sleep 0.6
fm --timeout 3 wallet clear-pending --yes --json
expect_rc 3 "§2a.11: clear-pending refuses while a send holds send.lock"
expect_reason send_in_progress "send_in_progress"
if [ -s "$FMONERO_STATE_DIR/pending.json" ]; then ok "and the record is still there"; else
  bad "§2a.11: the record was removed under a held lock"
fi
wait "$LOCKER" 2>/dev/null

fm --timeout 3 wallet clear-pending --yes --json
expect_rc 0 "wallet clear-pending clears it"
expect_json '.cleared' 'true' "cleared"

# §2a.4 + §4: the escape hatch the `pending_send` refusal's own hint names must work on a record
# fmonero cannot parse — otherwise a hand-edited or truncated file blocks every send for good
# and the printed hint fails the same way.
printf '{"kind":"send","account_index":0}' > "$FMONERO_STATE_DIR/pending.json"
chmod 600 "$FMONERO_STATE_DIR/pending.json"
fm --timeout 3 wallet send "$DEST" 0.1 --json
expect_rc 3 "an unparsable record blocks the send (§2a.4)"
expect_reason pending_send "pending_send"
fm --timeout 3 wallet clear-pending --yes --json
expect_rc 0 "and clear-pending — the command that hint names — still clears it"
expect_json '.cleared' 'true' "cleared"
expect_json '.pending' 'null' "with no record to render"
if [ -e "$FMONERO_STATE_DIR/pending.json" ]; then
  bad "the unparsable record is still on disk"
else
  ok "the file is gone, so the next send is not blocked"
fi

# §2a's --force list: it never bypasses the §2i layout gate.
systemctl_fixture system_unit
BEFORE="$(log_lines "$NODE_LOG")"
fm --timeout 3 node down --yes --force --json
expect_rc 3 "§2i: node down under the system_unit layout is exit 3, even with --force"
expect_reason foreign_node "foreign_node"
for cmd in "node restart --yes" "node ban 1.2.3.4" "node unban 1.2.3.4" "node limit --up 1024"; do
  # shellcheck disable=SC2086
  fm --timeout 3 $cmd --json
  expect_rc 3 "§2i: '$cmd' is refused under the system_unit layout"
done
if [ "$(log_lines "$NODE_LOG")" = "$BEFORE" ]; then ok "§2i: and the mock recorded ZERO requests"; else
  bad "§2i: a mutating call reached the node" "before $BEFORE, after $(log_lines "$NODE_LOG")"
fi
unset FMONERO_SYSTEMCTL_FIXTURE

# §2a's third --force case.
fm settings set node.data_dir "$WORK/money/bitmonero" >/dev/null
cp "$FMONERO_CONFIG_DIR/config.toml" "$WORK/money/config.bak"
sed -i 's|^url = "http://127.0.0.1:18191"|url = "http://10.0.0.5:18081"|' "$FMONERO_CONFIG_DIR/config.toml"
fm --timeout 3 status --force --json
expect_rc 2 "§2a: status --force with a non-loopback node.url exits 2, not 5"
expect_reason non_loopback_url "because require_loopback runs before any breaker"
# §2c: the money path answers the same way. It used to discard the constructor's error and
# report exit 4 `unreachable` with `fmonero node up` as the hint — a command that itself exits 2
# on this configuration, so the diagnosis pointed away from the cause and an agent never
# converged.
fm --timeout 3 wallet send "$DEST" 0.1 --json
expect_rc 2 "§2c: a non-loopback node.url on the send path is exit 2, not 4 unreachable"
expect_reason non_loopback_url "non_loopback_url"
fm --timeout 3 wallet sweep "$DEST" --json
expect_rc 2 "§2c: and on the sweep path"
expect_reason non_loopback_url "non_loopback_url"
cp "$WORK/money/config.bak" "$FMONERO_CONFIG_DIR/config.toml"

# ---- the §1b.4 error map, end to end ----------------------------------------
step "the §1b.4 wallet-RPC error map, end to end"
mocks_down
sandbox errmap
mock_node_up
mock_wallet_up --fail "get_balance=-17:not enough money"
fm --timeout 3 wallet balance --json
expect_rc 14 "--fail get_balance=-17 makes wallet balance exit 14 (§1b.4)"
if [ -s "$FMONERO_STATE_DIR/run/monero-wallet-rpc.$WALLET_PORT.login" ]; then
  ok "and the matching login file was written first (§2c)"
else
  bad "the login file is missing, so this tested exit 4 instead"
fi
mocks_down
sandbox errmap2
mock_node_up
mock_wallet_up --fail "get_balance=-13:No wallet file"
fm --timeout 3 wallet balance --json
expect_rc 9 "-13 NOT_OPEN is exit 9"
expect_reason not_open "not_open"
mocks_down
sandbox errmap3
mock_node_up
mock_wallet_up --fail "transfer=-18:Transaction would be too large.  try /transfer_split."
fm --timeout 3 wallet send "$DEST" 0.1 --json
expect_rc 16 "-18 TX_TOO_LARGE is exit 16"
expect_reason too_large "too_large"
expect_contains "fmonero wallet sweep" "with its OWN hint naming sweep (§1b.4)"
mocks_down
sandbox errmap4
mock_node_up
mock_wallet_up --fail "transfer=-2:WRONG_ADDRESS"
fm --timeout 3 wallet send "$DEST" 0.1 --json
expect_rc 15 "-2 WRONG_ADDRESS is exit 15"
mocks_down
sandbox errmap5
mock_node_up
mock_wallet_up --fail "get_transfers=0:"
fm --timeout 3 wallet history --pool --json
expect_rc 5 "epee's code:0 catch-all is exit 5"
expect_reason rpc_error "rpc_error"
expect_contains "cannot reach the daemon" "with a SYNTHESISED message, never an empty one (§2j)"

# §4.2 exempts exactly ONE `set_daemon` failure from `wallet open`: `-38 NO_DAEMON_CONNECTION`,
# where the wallet is open and simply has no daemon yet. Exit 4 is a whole CLASS, and the rest of
# it — a 401, `-7 DENIED`, `-48 DISABLED`, `-23 NO_WALLET_DIR` — leaves the wallet open with NO
# daemon credential, which §4.2 calls the step "whose omission means the wallet can never sync":
# reporting that as a successful open shows a stale balance on the money surface for ever.
mocks_down
sandbox setdaemon
mock_node_up
mock_wallet_up --fail "set_daemon=-7:DENIED"
mkdir -p "$FMONERO_STATE_DIR/wallets"
fm settings set wallet.dir "$FMONERO_STATE_DIR/wallets" >/dev/null
: > "$FMONERO_STATE_DIR/wallets/main.keys"
FMONERO_WALLET_PASSWORD=t3stpw fm --timeout 3 wallet open main --json
expect_rc 4 "a -7 DENIED from set_daemon FAILS wallet open (§4.2)"
mocks_down
sandbox setdaemon38
mock_node_up
mock_wallet_up --fail "set_daemon=-38:no connection to daemon"
mkdir -p "$FMONERO_STATE_DIR/wallets"
fm settings set wallet.dir "$FMONERO_STATE_DIR/wallets" >/dev/null
: > "$FMONERO_STATE_DIR/wallets/main.keys"
FMONERO_WALLET_PASSWORD=t3stpw fm --timeout 3 wallet open main --json
expect_rc 0 "…while the one code §4.2 exempts, -38, does not"
expect_json '.open' 'true' "the wallet is open, it simply has no daemon yet"

# ---- the restricted node, and BUSY ------------------------------------------
step "the restricted node and the BUSY degradation (§1b.2, §1b.3)"
mocks_down
sandbox restricted
mock_node_up --restricted --scenario restricted
fm --timeout 3 status --json
expect_rc 0 "status against a restricted node still exits 0"
expect_json '.node.restricted' 'true' "and says it is restricted"
expect_json '.node.uptime_sec' 'null' "a poisoned start_time renders null, not 1970 (§5)"
expect_json '.node.free_space' 'null' "and 2^64-1 free space renders null, not 16 exabytes"
expect_json '.node.outgoing_connections_count' 'null' "and the peer counts render null, not a false alarm"
expect_json '.node.database_size_rounded' 'true' "database_size carries its sibling flag"
expect_json '[.warnings[].id] | contains(["node_restricted"])' 'true' "node_restricted is raised"
expect_json '[.warnings[].id] | contains(["node_no_peers"])' 'false' "and node_no_peers is NOT (§5)"
fm --timeout 3 node peers --json
expect_rc 5 "node peers on a restricted node is exit 5"
expect_reason restricted "restricted"
mocks_down
sandbox busy
mock_node_up --scenario syncing
fm --timeout 3 status --json
expect_rc 0 "status against a syncing node exits 0 (§1b.3)"
expect_json '.node.top_block_age_sec' 'null' "a BUSY all-zero header renders null, not a 56-year-old block"
expect_json '[.warnings[].id] | contains(["node_syncing"])' 'true' "node_syncing is raised"
fm --timeout 3 wallet send "$DEST" 0.1 --yes --json
expect_rc 4 "and a send with no wallet daemon still fails at the probe"

# ================================================================ --full
if [ "$MODE" = full ]; then
  step "--full: the §2d deadline, breaker and --force paths"
  mocks_down
  sandbox deadline
  mock_node_up --delay-ms 10000
  mock_wallet_up
  # `node bans`, not `node info`: `node info` renders the §5 document first, and `status`'s own
  # client is never blocked by a breaker (§2d), so it would wait the full deadline once before
  # the breaker path was even reached — the command would be slow for a correct reason and the
  # timing assertion would be measuring the wrong thing.
  START=$(date +%s%N)
  fm --timeout 2 node bans --json
  ELAPSED=$(( ($(date +%s%N) - START) / 1000000 ))
  expect_rc 5 "a call killed at its deadline is exit 5"
  expect_reason killed_at_deadline "killed_at_deadline"
  if [ "$ELAPSED" -ge 1500 ] && [ "$ELAPSED" -le 5000 ]; then
    ok "and it took ${ELAPSED}ms — between 1.5 s and 5 s (§2d)"
  else
    bad "the deadline did not hold: ${ELAPSED}ms"
  fi
  START=$(date +%s%N)
  fm --timeout 2 node bans --json
  ELAPSED=$(( ($(date +%s%N) - START) / 1000000 ))
  expect_rc 5 "the second invocation takes the breaker path"
  expect_reason stalled "stalled, which is a different reflex from rpc_error (§4)"
  if [ "$ELAPSED" -lt 1000 ]; then ok "and returns in ${ELAPSED}ms without waiting again (§2d)"; else
    bad "the breaker did not short-circuit: ${ELAPSED}ms"
  fi
  fm --timeout 2 node bans --force --json
  expect_reason killed_at_deadline "--force attempts the call anyway (§2d)"
  # And `node info` shows why the breaker cannot make `status` fast: it is never blocked by one.
  fm --timeout 2 node info --json
  expect_rc 5 "node info under an armed breaker is still exit 5"
  fm --timeout 2 status --json
  expect_rc 0 "status is never blocked by a breaker: it reports it (§2d)"
  expect_json '[.warnings[].id] | contains(["node_stalled"])' 'true' "node_stalled"

  # §2d: a wallet timeout while the wallet is far behind is wallet_syncing, NOT a stall.
  mocks_down
  sandbox syncing-wallet
  mock_node_up
  mock_wallet_up --delay-ms 10000
  fm --timeout 3 status --json
  expect_rc 0 "status with a slow wallet still exits 0 and degrades its fields (§2d)"
  expect_json '.wallet.reachable' 'false' "the wallet probe was killed"
  expect_json '.wallet.balance_pico' 'null' "so the balance is null, never 0"
  fm --timeout 2 wallet balance --json
  expect_rc 5 "a foreground wallet call is exit 5"
  # `status` never arms a breaker (§2d), so the only armed one can come from the command above.
  fm --timeout 2 status --json
  expect_rc 0 "and status still answers"

  step "--full: §2a.4's concurrency — status in a loop while a send relays"
  mocks_down
  sandbox concurrent
  mock_node_up
  mock_wallet_up --delay-ms 400
  ( for _ in $(seq 40); do "$FM" --timeout 5 status --json >/dev/null 2>&1; done ) &
  POLLER=$!
  fm --timeout 20 wallet sweep "$DEST" --yes --json
  SWEEP_RC=$RC
  wait "$POLLER" 2>/dev/null
  if [ "$SWEEP_RC" = 0 ]; then ok "§2a.4: the sweep succeeded while status polled throughout"; else
    bad "§2a.4: the sweep failed under a concurrent status loop (exit $SWEEP_RC)" "${OUT:0:300}"
  fi
  if [ "$(logged "$WALLET_LOG" relay_tx)" = "2" ]; then ok "and both transactions were relayed exactly once"; else
    bad "expected two relay_tx, got $(logged "$WALLET_LOG" relay_tx)"
  fi

  step "--full: §2a.11's interlock — two send processes at once"
  mocks_down
  sandbox interlock
  mock_node_up
  # --delay-ms 1500, so the first process is PROVABLY still holding send.lock when the second
  # reaches it. Against an instant mock the first can finish build → relay → resolve before the
  # second gets there, both legitimately succeed, and the only test of the one interlock that
  # can double-spend becomes a coin flip.
  mock_wallet_up --delay-ms 1500
  "$FM" --timeout 30 wallet send "$DEST" 0.1 --yes --json > "$WORK/interlock/a.json" 2>"$WORK/interlock/a.err" &
  A=$!
  sleep 0.4
  "$FM" --timeout 30 wallet send "$DEST" 0.1 --yes --json > "$WORK/interlock/b.json" 2>"$WORK/interlock/b.err" &
  B=$!
  wait "$A"; RC_A=$?
  wait "$B"; RC_B=$?
  ZEROES=0; THREES=0; BUSY_REASON=""
  for pair in "$RC_A:a" "$RC_B:b"; do
    rc="${pair%%:*}"; which="${pair##*:}"
    case "$rc" in
      0) ZEROES=$((ZEROES+1)) ;;
      3) THREES=$((THREES+1)); BUSY_REASON="$(jq -r '.reason // "null"' "$WORK/interlock/$which.json" 2>/dev/null)" ;;
    esac
  done
  if [ "$ZEROES" = 1 ] && [ "$THREES" = 1 ]; then
    ok "§2a.11: exactly one send succeeded and one was refused"
  else
    bad "§2a.11: expected one exit 0 and one exit 3, got $RC_A and $RC_B" \
        "a: $(head -c 200 "$WORK/interlock/a.json")" "b: $(head -c 200 "$WORK/interlock/b.json")"
  fi
  if [ "$BUSY_REASON" = send_in_progress ]; then ok "and the refusal is reason send_in_progress"; else
    bad "§2a.11: the refusal's reason was '$BUSY_REASON'"
  fi
  if [ "$(logged "$WALLET_LOG" transfer)" = "1" ]; then
    ok "and the mock recorded exactly ONE transfer: nothing was built twice"
  else
    bad "§2a.11: $(logged "$WALLET_LOG" transfer) transfer calls — two builds select from one unspent set"
  fi

  step "--full: §2a.4's write order — a send killed mid-relay"
  mocks_down
  sandbox killed
  mock_node_up
  mock_wallet_up --delay-ms 1500
  "$FM" --timeout 60 wallet sweep "$DEST" --yes --json >/dev/null 2>&1 &
  VICTIM=$!
  # Wait until the record shows the first entry relayed and the second not, then kill.
  SEEN=no
  for _ in $(seq 200); do
    if [ -f "$FMONERO_STATE_DIR/pending.json" ]; then
      R="$(jq '[.txs[] | select(.relayed)] | length' "$FMONERO_STATE_DIR/pending.json" 2>/dev/null || echo x)"
      T="$(jq '.txs | length' "$FMONERO_STATE_DIR/pending.json" 2>/dev/null || echo x)"
      if [ "$R" = 1 ] && [ "$T" = 2 ]; then SEEN=yes; break; fi
    fi
    sleep 0.05
  done
  kill -9 "$VICTIM" 2>/dev/null
  wait "$VICTIM" 2>/dev/null
  if [ "$SEEN" = yes ]; then
    R="$(jq '[.txs[] | select(.relayed)] | length' "$FMONERO_STATE_DIR/pending.json")"
    if [ "$R" = 1 ]; then
      ok "§2a.4: a SIGKILL mid-sweep leaves exactly one relayed: true and one false"
    else
      bad "§2a.4: after the kill the record shows $R relayed"
    fi
  else
    bad "§2a.4: the record never passed through the one-relayed state"
  fi
  # And the next send BLOCKS on it.
  fm --timeout 5 wallet send "$DEST" 0.1 --yes --json
  expect_rc 3 "and the next send is refused"
  expect_reason pending_send "pending_send"

  step "--full: §2l timings, measured with the date loop of §1.4"
  mocks_down
  sandbox timing-a
  S=$(date +%s%N); for _ in $(seq 20); do "$FM" status --json >/dev/null 2>&1; done; E=$(date +%s%N)
  A_MS=$(( (E-S)/20000000 ))
  if [ "$A_MS" -le 60 ]; then ok "§2l (a) status, both daemons absent: ${A_MS}ms mean (budget 60 ms, GREEN)"; else
    bad "§2l (a) status, both daemons absent: ${A_MS}ms mean, over the 60 ms budget"
  fi
  sandbox timing-b
  mock_node_up
  mock_wallet_up
  "$FM" --timeout 3 status --json >/dev/null 2>&1   # one cold pass first
  S=$(date +%s%N); for _ in $(seq 20); do "$FM" --timeout 3 status --json >/dev/null 2>&1; done; E=$(date +%s%N)
  B_MS=$(( (E-S)/20000000 ))
  if [ "$B_MS" -le 80 ]; then ok "§2l (b) status against the mock: ${B_MS}ms mean (budget 80 ms, AMBER)"; else
    bad "§2l (b) status against the mock: ${B_MS}ms mean, over the 80 ms budget"
  fi
  "$FM" --timeout 5 wallet history --limit 50 --json >/dev/null 2>&1   # build the snapshot
  S=$(date +%s%N); for _ in $(seq 20); do "$FM" wallet history --limit 50 --json --cached >/dev/null 2>&1; done; E=$(date +%s%N)
  D_MS=$(( (E-S)/20000000 ))
  if [ "$D_MS" -le 50 ]; then ok "§2l (d) wallet history --cached: ${D_MS}ms mean (budget 50 ms, AMBER)"; else
    bad "§2l (d) wallet history --cached: ${D_MS}ms mean, over the 50 ms budget"
  fi
  printf '  note  §2l (c) — warm, against a REAL monerod and monero-wallet-rpc — is RED and is\n'
  printf '        neither measured nor claimed here (§4b, §2j).\n'
fi

# ================================================================ the doctor (§4b GREEN, §5c)
#
# The three doctor rows were a skip at the Phase 1b gate — `doctor::run` was still the §8
# handover stub — and Phase 3 turns them into assertions, because §4b's GREEN list names THIS
# script for one of them in as many words: "`tests/e2e.sh` asserts exit 11 and that exact
# failure set". The set is asserted positively (which ids fail) rather than by a count, so it
# strengthens when the owner installs Monero: the two `fail:not_installed` rows become passes
# and this case fails until it is updated, which is the point.
step "§4b GREEN: doctor on this machine — exit 11 and that exact failure set"
sandbox doctor
fm doctor --json
expect_rc 11 "doctor with no Monero installed"
expect_keys "doctor" ok schema run_id checks summary
expect_json '[.checks[] | select(.status=="fail") | .id] | sort' \
  '["monerod_installed","wallet_rpc_installed"]' \
  "exactly monerod_installed and wallet_rpc_installed fail (§4b)"
expect_json '[.checks[] | select(.status=="fail") | .reason] | unique' '["not_installed"]' \
  "and both say fail:not_installed, not a bare fail"
expect_json '[.checks[] | select(.group=="node" or .group=="wallet" or .group=="storage")
              | select(.status != "skip") | .id] | length' '0' \
  "every node/wallet/storage check is a skip:<reason> (§7.1)"
expect_json '[.checks[] | select(.group=="node" or .group=="wallet" or .group=="storage")
              | select(.reason == null) | .id] | length' '0' \
  "and every one of those skips names its reason, never a bare skip"
expect_json '[.checks[] | select(.group=="install") | select(.status=="skip") | .id] | length' '0' \
  "the install group has real verdicts, not skips (§4b)"
expect_json '[.checks[] | select(.group=="security") | select(.status=="pass") | .id] | sort' \
  '["ports_owned","state_perms"]' \
  "and the security group's own two locally-answerable checks pass"
expect_json '.summary.exit_code' '11' "the summary agrees with the process exit code (§2j)"

fm doctor capabilities --json
expect_rc 0 "doctor capabilities"
expect_keys "doctor capabilities" ok schema checks groups classes
expect_json '[.groups[].group] | sort' \
  '["binaries","install","node","security","storage","units","wallet"]' \
  "the seven §7 groups"
expect_json '[.classes[].class] | sort' '["A","B","D"]' \
  "three safety classes — there is no class C (§7)"

fm doctor --only security --json
expect_rc 0 "doctor --only security"
expect_json '[.checks[].group] | unique' '["security"]' "and it ran only that group"

# The class-A round trip, which is what makes `doctor undo` a GREEN row rather than a claim:
# corrupt two fmonero-owned files, fix them, undo, and compare the bytes. `config.toml` is
# corrupted by an INVALID VALUE rather than by unparsable TOML, because unparsable TOML fails
# `Context::load` before the doctor can run at all (exit 2) — the doctor repairs a key it can
# read, and there is no path on which it repairs a file the CLI cannot load.
step "§4b GREEN: doctor --fix and doctor undo latest, byte for byte"
sandbox doctor-undo
printf '{ this is not json\n'          > "$FMONERO_STATE_DIR/state.json"
printf '[wallet]\npriority = 9\n'      > "$FMONERO_CONFIG_DIR/config.toml"
chmod 600 "$FMONERO_STATE_DIR/state.json" "$FMONERO_CONFIG_DIR/config.toml"
sha256sum "$FMONERO_STATE_DIR/state.json" "$FMONERO_CONFIG_DIR/config.toml" > "$WORK/doctor.before"

fm doctor --only install --json
expect_json '[.checks[] | select(.id=="state_valid" or .id=="settings_valid") | .status] | unique' \
  '["warn"]' "both class-A detectors see the corruption"

fm doctor --fix --dry-run --json
expect_json '[.checks[] | select(.fix_outcome=="planned") | .id] | length > 0' 'true' \
  "--fix --dry-run plans the class-A repairs"
if sha256sum -c --status "$WORK/doctor.before" 2>/dev/null; then
  ok "--fix --dry-run changed neither file (§7)"
else
  bad "--fix --dry-run modified a file"
fi

fm doctor --fix --json
case "$RC" in
  11|12) ok "doctor --fix (exit $RC: fail rows remain — monerod is still not installed)" ;;
  *) bad "doctor --fix: expected 11 or 12, got $RC" "stdout: ${OUT:0:300}" ;;
esac
expect_json '[.checks[] | select(.id=="state_valid" or .id=="settings_valid") | .fix_outcome] | unique' \
  '["fixed"]' "both class-A fixers report fixed"
RUN_ID="$(printf '%s' "$OUT" | jq -r '.run_id')"
fm doctor --only install --json
expect_json '[.checks[] | select(.id=="state_valid" or .id=="settings_valid") | .status] | unique' \
  '["pass"]' "and both files are valid afterwards"
fm settings get wallet.priority --json
expect_json '.value' '"2"' "the repaired key is at its documented default, not at 9"

fm doctor undo latest --json
expect_rc 0 "doctor undo latest"
expect_keys "doctor undo" ok schema run_id undone refused reason
expect_json '.refused' 'false' "it was not refused"
expect_json '.run_id' "\"$RUN_ID\"" "and it undid the run --fix had just written"
expect_json '[.undone[] | select(.ok != true) | .path] | length' '0' "every recorded action reversed"
if sha256sum -c --status "$WORK/doctor.before" 2>/dev/null; then
  ok "the two files are byte-for-byte the corrupt originals again (§7 axiom 3)"
else
  bad "doctor undo did not restore the original bytes"
fi

# And the integrity refusal: a backup whose hash no longer matches must stop the replay.
fm doctor --fix --json
printf 'tampered\n' > "$(readlink -f "$FMONERO_STATE_DIR/doctor/latest")/backups$FMONERO_CONFIG_DIR/config.toml"
fm doctor undo latest --json
expect_rc 13 "doctor undo against a tampered backup"
expect_json '.refused' 'true' "refused, with the integrity failure named"

# ================================================================ §5c coverage
step "§5c: every command in §4.1 has been asserted, or is named as not covered"
# `tui` and `robot-docs` are the two rows §5c marks exempt — the only two. Every other command of
# §4.1 must have had its top-level key set asserted above, and this COMPARES the two sets rather
# than asserting it in a step title: the command list comes from `capabilities`, which is
# generated from the same table §4.1 defines, so a new command arrives here on its own.
"$FM" capabilities --json | jq -r '.commands[].name' | grep -vE '^(tui|robot-docs)$' \
  | sort -u > "$WORK/expected-commands.txt"
sort -u "$ASSERTED" > "$WORK/asserted-sorted.txt"
# A label may name a SUBcommand of a §4.1 row (`wallet book list` covers `wallet book`), so the
# match is "equal, or the row followed by a space".
MISSING=""
while read -r want; do
  [ -n "$want" ] || continue
  grep -qE "^$want( |\$)" "$WORK/asserted-sorted.txt" || MISSING="$MISSING $want,"
done < "$WORK/expected-commands.txt"
if [ -z "${MISSING// /}" ]; then
  ok "every §4.1 command has its top-level key set asserted ($(grep -c '' "$WORK/expected-commands.txt") of them)"
else
  bad "§5c: a command of §4.1 has NO key-set assertion anywhere in this file" "missing: $MISSING"
fi
skip "tui — exempt: interactive, and it exits 2 on a non-TTY stdout (§5c)"
skip "robot-docs — exempt: plain text by contract (§5c)"

# ================================================================ docs/ROBOT.md
step 'docs/ROBOT.md is `fmonero robot-docs` output, not a hand-edited copy of it (§3)'
# §3 says the file is "generated by `fmonero robot-docs > docs/ROBOT.md` in tests/e2e.sh; never
# hand-edited". This compares instead of overwriting, and fails with that exact command as the
# hint — a test that silently rewrote a tracked file would make `git status` useless as the
# "--dry-run changed nothing" oracle two gates above depend on, and it could not fail on a hand
# edit, which is the thing §3 forbids.
"$FM" robot-docs > "$WORK/ROBOT.generated" 2>/dev/null
if [ ! -f "$ROOT/docs/ROBOT.md" ]; then
  bad "docs/ROBOT.md does not exist" "run: fmonero robot-docs > docs/ROBOT.md"
elif cmp -s "$WORK/ROBOT.generated" "$ROOT/docs/ROBOT.md"; then
  ok "docs/ROBOT.md is byte-identical to this binary's robot-docs ($(grep -c '' "$ROOT/docs/ROBOT.md") lines)"
else
  bad "docs/ROBOT.md is stale or hand-edited" \
      "run: fmonero robot-docs > docs/ROBOT.md" \
      "$(diff "$ROOT/docs/ROBOT.md" "$WORK/ROBOT.generated" | head -8)"
fi

# ================================================================ §2b secrets
#
# §2b names two tests and they catch different things. Test (1) is a KEY-NAME grep over the full
# `--json` and `-v` output of every command; test (2) is the known-value grep below. A run that
# had only the second would pass a payload that faithfully printed a `tx_key` the mock invented.
step "§2b test (1): the key-name grep over every command's --json and -v output"
# The transcript already holds every `fm` invocation's stdout and stderr. The verbose channel is
# added here explicitly, against both mocks, because §2b names `-v` and no case above uses it.
sandbox secrets-verbose
mock_node_up
mock_wallet_up
FMONERO_WALLET_PASSWORD=t3stpw fm -vv --timeout 3 wallet open main --json
for verbose_case in \
  "status" \
  "node info" "node peers" "node bans" "node limit" \
  "wallet balance" "wallet address" "wallet history --limit 5" "wallet book list" \
  "wallet refresh" "wallet new-address" "wallet set-daemon 127.0.0.1:18191 --yes"
do
  # shellcheck disable=SC2086
  fm -vv --timeout 3 $verbose_case --json
done
fm -vv --timeout 3 wallet notes get "$BUILT_TXID" --json
fm -vv --timeout 3 wallet send "$DEST" 0.01 --json          # the no---yes refusal: it carries a fee
fm -vv --timeout 3 wallet send "$DEST" 0.01 --dry-run --json
fm -vv --timeout 3 wallet sweep "$DEST" --dry-run --json
fm -vv --timeout 3 capabilities
fm -vv --timeout 3 settings list --json
fm -vv --timeout 3 wallet close --json

# The keys §2b forbids, matched as JSON keys rather than as substrings: `seed_backup_hint` is a
# documented key of `wallet create` (§2b.2) and `pruning_seed` is a field of every peer row, so a
# substring grep on `seed` would fail on correct output — and a grep that fails on correct output
# is the grep the next agent relaxes. Both negative cases are self-tested below.
FORBIDDEN_KEYS='"(seed|spend_key|view_key|tx_key|tx_metadata|password|login)"[[:space:]]*:'
if grep -nEq "$FORBIDDEN_KEYS" "$TRANSCRIPT"; then
  bad "§2b: a forbidden key name appears in this run's output" \
      "$(grep -nEo "$FORBIDDEN_KEYS" "$TRANSCRIPT" | sort -u | head -5)"
else
  ok "§2b: no seed/spend_key/view_key/tx_key/tx_metadata/password/login key in any output ($(wc -l < "$TRANSCRIPT") lines)"
fi
printf '{"tx_key":"deadbeef","password":"hunter2","tx_metadata" : "0100"}\n' > "$WORK/2b-positive"
if grep -Eq "$FORBIDDEN_KEYS" "$WORK/2b-positive"; then
  ok "self-test: the key-name grep fires on its own fixture"
else
  bad "self-test: the key-name grep does NOT fire on a payload carrying tx_key — it is vacuous"
fi
printf '{"seed_backup_hint":"run monero-wallet-cli","pruning_seed":384,"logins":2}\n' > "$WORK/2b-negative"
if grep -Eq "$FORBIDDEN_KEYS" "$WORK/2b-negative"; then
  bad "self-test: the key-name grep fires on seed_backup_hint/pruning_seed, which are honest keys"
else
  ok "self-test: and it leaves seed_backup_hint and pruning_seed alone"
fi

step "§2b test (2): the known-value grep over everything this run wrote"
# The test's own wallet password and the mock's login-file contents must appear in none of:
# stdout, stderr (the transcript above is both), state.json, history.bin, any doctor run, any
# fixture, DECISIONS.md. The shape test ("any 64-hex run") is deliberately NOT used: fmonero's
# own honest output is full of them (`top_block_hash`, every txid).
#
# Three exclusions, each by name and each with a reason, because a grep that fires on correct
# behaviour is the grep the next agent deletes:
#   *.err                                  — a case's expected-stderr capture
#   */run/monero-wallet-rpc.<port>.login   — the login file THIS script writes (§2c)
#   *.jsonl                                — the MOCK's request log. `open_wallet` and
#                                            `create_wallet` carry the password in the request
#                                            body by construction (§1b.4) and the mock logs
#                                            every body verbatim, so the wire is not fmonero's
#                                            output. What matters there is asserted separately,
#                                            just below: no OTHER method may carry it.
leak_scan() { # leak_scan <needle> -> the files that hold it, after the three exclusions
  grep -rl "$1" "$WORK" 2>/dev/null \
    | grep -v '\.err$' \
    | grep -v '\.jsonl$' \
    | grep -v '/run/monero-wallet-rpc\.[0-9]*\.login$' || true
}
# Self-test FIRST, then the file is removed: the Phase 1b form of this grep was
# `grep -rql … | head -1 | grep -q .`, where `-q` silences the `-l` and the pipeline could never
# print a filename — a grep that could not fail. It is worth one fixture to know this one can.
printf 'leaked: t3stpw\n' > "$WORK/2b-leak-selftest.txt"
if [ -n "$(leak_scan t3stpw)" ]; then
  ok "self-test: the known-value scan fires on a planted leak"
else
  bad "self-test: the known-value scan does NOT fire on a planted leak — it is vacuous"
fi
rm -f "$WORK/2b-leak-selftest.txt"
LEAKS=0
for needle in t3stpw fmonero:t3st; do
  hits="$(leak_scan "$needle")"
  if [ -n "$hits" ]; then
    LEAKS=1
    bad "§2b: '$needle' appears in a file this run wrote" "$(printf '%s' "$hits" | head -5)"
  fi
done
[ "$LEAKS" = 0 ] && ok "§2b: the wallet password and the mock's credential appear in nothing this run wrote"
# DECISIONS.md is scanned for the wallet PASSWORD only, and that is a deviation from §2b's
# "the exact contents of the mock's login file … in none of … DECISIONS.md" with its reason:
# DESIGN.md §8's own Phase 1b gate line is
# `printf 'fmonero:t3st' > $FMONERO_STATE_DIR/run/monero-wallet-rpc.<port>.login`, required to be
# pasted into DECISIONS.md as real output — and DECISIONS.md is append-only (§2j), so the
# contradiction cannot be resolved by editing the log. `fmonero:t3st` is a harness credential
# this document publishes on purpose; `t3stpw` stands in for a real wallet password and is what
# the rule is about. Fixtures are scanned for both.
DOC_LEAK=0
if grep -q 't3stpw' "$ROOT/DECISIONS.md" 2>/dev/null; then
  DOC_LEAK=1
  bad "§2b: the wallet password appears in DECISIONS.md"
fi
if [ -n "$(grep -rl 't3stpw\|fmonero:t3st' "$ROOT/tests/fixtures" 2>/dev/null)" ]; then
  DOC_LEAK=1
  bad "§2b: a test credential appears in a fixture"
fi
[ "$DOC_LEAK" = 0 ] && ok "§2b: and in neither DECISIONS.md nor any fixture"
# The wire: the password reaches monero-wallet-rpc in exactly the two request bodies that need
# it, and in no other. A `set_daemon` or `refresh` body carrying it would be a real defect that
# the exclusion above would otherwise hide.
PW_METHODS="$(jq -rs '[.[] | select((.body | tostring) | contains("t3stpw")) | .method] | unique | sort | join(",")' \
              "$WORK"/*/wallet.jsonl 2>/dev/null)"
case "$PW_METHODS" in
  ""|"create_wallet"|"open_wallet"|"create_wallet,open_wallet"|"open_wallet,create_wallet")
    ok "§2b: on the wire the password appears only in open_wallet/create_wallet (${PW_METHODS:-none})" ;;
  *) bad "§2b: the password was sent in a request that does not need it" "methods: $PW_METHODS" ;;
esac
if grep -rq 'rpc-login=fmonero:[0-9a-f]' "$WORK"/*/state 2>/dev/null; then
  bad "§2b: the node credential leaked into a state directory"
else
  ok "§2b: the node credential is nowhere outside monerod.conf"
fi

# ================================================================ no side effect on the machine
#
# The Phase 4 verifier's runs left both of the owner's systemd user units flapping in a
# `Restart=on-failure` loop, and had to stop them by hand. That is a test-suite defect, not an
# accident of the machine: a suite that mutates the system it is measuring cannot be trusted about
# anything else it reports. Every case above either finds no unit file (exit 4 in milliseconds,
# §2i) or runs under a `FMONERO_SYSTEMCTL_FIXTURE`, so nothing here can reach
# `systemctl --user start` on a real unit — and this step is what makes that claim falsifiable
# rather than an argument.
step "no side effect: the owner's two units are untouched by this run"
if command -v systemctl >/dev/null 2>&1; then
  LEFT_RUNNING=""
  for unit in fmonero-monerod.service fmonero-wallet-rpc.service; do
    ACTIVE="$(systemctl --user show -p ActiveState --value "$unit" 2>/dev/null || echo unknown)"
    case "$ACTIVE" in
      active|activating|reloading|deactivating) LEFT_RUNNING="$LEFT_RUNNING $unit=$ACTIVE" ;;
    esac
  done
  if [ -z "${LEFT_RUNNING// /}" ]; then
    ok "both fmonero units are inactive — no case started one"
  else
    bad "a case started one of the owner's units and left it running" "$LEFT_RUNNING" \
        "stop it with: systemctl --user stop$LEFT_RUNNING"
  fi
else
  skip "systemctl is not on PATH, so there are no user units to leave behind"
fi

printf '\n'
printf 'e2e.sh (%s): %s passed, %s failed, %s skipped\n' "$MODE" "$PASS" "$FAIL" "$SKIP"
[ "$FAIL" = 0 ] || exit 1
exit 0
