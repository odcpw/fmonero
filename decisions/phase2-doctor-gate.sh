#!/usr/bin/env bash
# The Phase 2 doctor gate (DESIGN.md §8). Written in the phase2/doctor worktree; Phase 3
# removed that worktree and repointed both `cd`s at the main checkout so this still runs.
set -uo pipefail
cd /home/oliver/Projects/odcpw/fmonero || exit 1
FM="$PWD/target/release/fmonero"
W="$(mktemp -d)"
trap 'rm -rf "$W"' EXIT

say() { printf '\n===== %s\n' "$1"; }

# Every case runs in its own state/config/cache directory, created by fmonero itself so the
# modes are the 0700 §2b.7 requires (a mkdir -p at the shell's umask is 0755, which
# state_perms correctly reports as a failure — that is the check working, not the gate).
sandbox() {
  export FMONERO_STATE_DIR="$W/$1/state"
  export FMONERO_CONFIG_DIR="$W/$1/config"
  export FMONERO_CACHE_DIR="$W/$1/cache"
  mkdir -p "$W/$1"
  # 0700, because that is the mode §2b.7 requires and `state_perms` checks. A bare
  # `mkdir -p` at the shell's umask is 0755, and the check would rightly fail on it.
  ( umask 077
    mkdir -p "$FMONERO_STATE_DIR" "$FMONERO_CONFIG_DIR" "$FMONERO_CACHE_DIR" )
}

# --------------------------------------------------------------- 1
say "GATE 1: fmonero doctor --json | jq — every §7.1 check with a real verdict, exit 11"
sandbox g1
"$FM" doctor --json > "$W/g1.json"
echo "exit: $?"
jq -e . "$W/g1.json" >/dev/null && echo "jq: valid JSON"
printf '%-12s %-30s %s\n' GROUP CHECK VERDICT
jq -r '.checks[] | [.group, .id, .verdict] | @tsv' "$W/g1.json" \
  | while IFS=$'\t' read -r g c v; do printf '%-12s %-30s %s\n' "$g" "$c" "$v"; done
echo "-- summary"
jq -c '.summary' "$W/g1.json"
echo "-- the four §4b assertions"
jq -r '
  (([.checks[] | select(.group=="binaries" and .verdict=="fail:not_installed") | .id] | sort)
     == ["monerod_installed","wallet_rpc_installed"]) as $bins
| ([.checks[] | select(.group=="node" or .group=="wallet" or .group=="storage")
     | select(.status != "skip") | .id] | length == 0) as $skips
| ([.checks[] | select(.group=="install" or .group=="security")
     | select(.detail == "" or .detail == null) | .id] | length == 0) as $real
| ([.checks[] | .id] | length) as $n
| "binaries fail:not_installed is exactly {monerod_installed, wallet_rpc_installed}: \($bins)",
  "every node/wallet/storage check is skip:*: \($skips)",
  "every install/security check carries a real detail: \($real)",
  "checks reported: \($n)"
' "$W/g1.json"
echo "-- install and security rows in full"
jq -r '.checks[] | select(.group=="install" or .group=="security")
       | "  \(.id) [\(.verdict)] \(.detail)"' "$W/g1.json"
echo "-- the human table, on a real pty (a non-TTY stdout implies --json, §4), NO_COLOR for the log"
NO_COLOR=1 script -qec "$FM doctor" /dev/null | sed 's/\r$//' | head -60
echo "(pty exit: ${PIPESTATUS[0]} — script reports its child's status)"

# --------------------------------------------------------------- 2
say "GATE 2: doctor --fix --dry-run changes nothing (git status + a state-dir checksum)"
sandbox g2
# Something for the class-A fixer to plan too, so the assertion is not vacuous.
printf '{ this is not json\n' > "$FMONERO_STATE_DIR/state.json"
chmod 600 "$FMONERO_STATE_DIR/state.json"
BEFORE_STATE="$(find "$FMONERO_STATE_DIR" "$FMONERO_CONFIG_DIR" "$FMONERO_CACHE_DIR" \
  -printf '%M %s %P\n' 2>/dev/null | LC_ALL=C sort | sha256sum)"
BEFORE_HOME="$(find "$HOME/.config/systemd/user" "$HOME/.config/fmonero" \
  "$HOME/.config/omarchy/plugins" "$HOME/.config/omarchy/extensions" \
  -printf '%M %s %P\n' 2>/dev/null | LC_ALL=C sort | sha256sum)"
BEFORE_GIT="$(git status --porcelain)"
"$FM" doctor --fix --dry-run --json > "$W/g2.json"
echo "exit: $?"
echo "-- planned mutations, with the paths each would write to"
jq -r '.checks[] | select(.actions | length > 0)
       | .id as $i | .actions[] | "  [\(.class)] \($i): \(.action)   -> \(.writes_to | join(", "))"' \
  "$W/g2.json"
jq -r '"dry_run: \(.summary.dry_run) · planned: \(.summary.planned) · run_dir: \(.summary.run_dir)"' "$W/g2.json"
AFTER_STATE="$(find "$FMONERO_STATE_DIR" "$FMONERO_CONFIG_DIR" "$FMONERO_CACHE_DIR" \
  -printf '%M %s %P\n' 2>/dev/null | LC_ALL=C sort | sha256sum)"
AFTER_HOME="$(find "$HOME/.config/systemd/user" "$HOME/.config/fmonero" \
  "$HOME/.config/omarchy/plugins" "$HOME/.config/omarchy/extensions" \
  -printf '%M %s %P\n' 2>/dev/null | LC_ALL=C sort | sha256sum)"
AFTER_GIT="$(git status --porcelain)"
echo "state dir before: $BEFORE_STATE"
echo "state dir after:  $AFTER_STATE"
[ "$BEFORE_STATE" = "$AFTER_STATE" ] && echo "STATE DIR UNCHANGED" || echo "STATE DIR CHANGED -- FAIL"
echo "\$HOME targets before: $BEFORE_HOME"
echo "\$HOME targets after:  $AFTER_HOME"
[ "$BEFORE_HOME" = "$AFTER_HOME" ] && echo "HOME TARGETS UNCHANGED" || echo "HOME TARGETS CHANGED -- FAIL"
[ "$BEFORE_GIT" = "$AFTER_GIT" ] && echo "git status UNCHANGED" || echo "git status CHANGED -- FAIL"
echo "git status --porcelain:"; git status --porcelain | sed 's/^/  /'

echo
echo "-- and the same dry run with NO test state dir: the real machine, the real blast radius"
BH2="$(find "$HOME/.config/systemd/user" "$HOME/.config/fmonero" "$HOME/.config/omarchy" \
  "$HOME/.local/state/fmonero" "$HOME/.local/share/fmonero" \
  -printf '%M %s %T@ %P\n' 2>/dev/null | LC_ALL=C sort | sha256sum)"
BG2="$(git status --porcelain)"
( unset FMONERO_STATE_DIR FMONERO_CONFIG_DIR FMONERO_CACHE_DIR
  "$FM" doctor --fix --dry-run --json > "$W/g2real.json"; echo "exit: $?" )
jq -r '.checks[] | select(.actions | length > 0)
       | .id as $i | .actions[] | "  [\(.class)] \($i): \(.action)   -> \(.writes_to | join(", "))"' \
  "$W/g2real.json"
jq -r '"dry_run: \(.summary.dry_run) · planned: \(.summary.planned) · run_dir: \(.summary.run_dir)"' "$W/g2real.json"
AH2="$(find "$HOME/.config/systemd/user" "$HOME/.config/fmonero" "$HOME/.config/omarchy" \
  "$HOME/.local/state/fmonero" "$HOME/.local/share/fmonero" \
  -printf '%M %s %T@ %P\n' 2>/dev/null | LC_ALL=C sort | sha256sum)"
AG2="$(git status --porcelain)"
echo "\$HOME before: $BH2"
echo "\$HOME after:  $AH2"
[ "$BH2" = "$AH2" ] && echo "REAL \$HOME UNCHANGED (mode, size and mtime of every file)" || echo "REAL HOME CHANGED -- FAIL"
[ "$BG2" = "$AG2" ] && echo "git status UNCHANGED" || echo "git status CHANGED -- FAIL"

echo
echo "-- a real --fix under FMONERO_STATE_DIR declines every fixer that reaches outside it"
"$FM" doctor --fix --json > "$W/g2fix.json"; echo "exit: $?"
jq -r '.checks[] | select(.fix_outcome != null) | "  \(.id): \(.fix_outcome)"' "$W/g2fix.json"
echo "  \$HOME still untouched: $([ "$AH2" = "$(find "$HOME/.config/systemd/user" "$HOME/.config/fmonero" "$HOME/.config/omarchy" "$HOME/.local/state/fmonero" "$HOME/.local/share/fmonero" -printf '%M %s %T@ %P\n' 2>/dev/null | LC_ALL=C sort | sha256sum)" ] && echo yes || echo NO)"

# --------------------------------------------------------------- 3
say "GATE 3: class-A round trip — corrupt → --fix → assert → undo latest → compare"
sandbox g3

printf '{ this is not json\n'  > "$FMONERO_STATE_DIR/state.json"
printf '[wallet]\npriority = 9\n' > "$FMONERO_CONFIG_DIR/config.toml"
chmod 600 "$FMONERO_STATE_DIR/state.json" "$FMONERO_CONFIG_DIR/config.toml"
sha256sum "$FMONERO_STATE_DIR/state.json" "$FMONERO_CONFIG_DIR/config.toml" > "$W/g3.before"
echo "-- corrupt: both class-A detectors see it"
"$FM" doctor --only install --json | jq -r '.checks[] | select(.id=="state_valid" or .id=="settings_valid") | "  \(.id) [\(.verdict)] \(.detail)"'
echo "-- --fix"
"$FM" doctor --fix --json > "$W/g3.fix.json"
echo "exit: $?"
jq -r '.checks[] | select(.id=="state_valid" or .id=="settings_valid")
       | "  \(.id) [\(.verdict)] --fix: \(.fix_outcome)"' "$W/g3.fix.json"
jq -r '"run_id: \(.run_id) · fixed: \(.summary.fixed) · fix_failed: \(.summary.fix_failed)"' "$W/g3.fix.json"
echo "-- assert: both files are now valid, and the repaired settings kept nothing bad"
"$FM" doctor --only install --json | jq -r '.checks[] | select(.id=="state_valid" or .id=="settings_valid") | "  \(.id) [\(.verdict)]"'
echo "  wallet.priority is now: $("$FM" settings get wallet.priority --json | jq -r .value)"
echo "-- the journal"
jq -c '{seq, check, ok, phase: (.phase // "-"), kind: .action.kind, inverse: .inverse.kind}' \
  "$FMONERO_STATE_DIR/doctor/latest/actions.jsonl"
echo "-- modes of everything the run wrote (§2b.7: files 0600, directories 0700)"
find "$FMONERO_STATE_DIR/doctor" -printf '%M %P\n' | LC_ALL=C sort | head -20
echo "-- undo latest"
"$FM" doctor undo latest --json > "$W/g3.undo.json"
echo "exit: $?"
jq -c . "$W/g3.undo.json"
echo "-- compare: byte for byte the corrupt files again"
sha256sum -c "$W/g3.before" && echo "ROUND TRIP EXACT"
echo "-- undo against a run whose backup hash no longer matches: exit 13"
"$FM" doctor --fix --json >/dev/null
printf 'tampered\n' > "$(readlink -f "$FMONERO_STATE_DIR/doctor/latest")/backups$FMONERO_CONFIG_DIR/config.toml"
"$FM" doctor undo latest --json > "$W/g3.refuse.json"
echo "exit: $?"
jq -c '{ok, refused, reason}' "$W/g3.refuse.json"

# --------------------------------------------------------------- 4
say "GATE 3b: the §5c key sets of the three doctor payloads, asserted exactly"
echo "doctor:              $(jq -r '[keys[]] | join(",")' "$W/g1.json")"
echo "doctor undo:         $(jq -r '[keys[]] | join(",")' "$W/g3.undo.json")"
echo "doctor undo refused: $(jq -r '[keys[]] | join(",")' "$W/g3.refuse.json")"
jq -e '([keys[]] | sort) == ["checks","ok","run_id","schema","summary"]' "$W/g1.json" >/dev/null \
  && echo "doctor keys are exactly §7's object plus the envelope: ok"
jq -e '([keys[]] | sort) == ["ok","reason","refused","run_id","schema","undone"]' "$W/g3.undo.json" >/dev/null \
  && echo "doctor undo keys are exactly §5c's row plus the envelope: ok"
jq -e '(.undone | length == 0) or (([.undone[0] | keys[]] | sort) == ["action","ok","path","seq"])' \
  "$W/g3.undo.json" >/dev/null && echo "undone[] is exactly {seq, action, path, ok}: ok"
jq -e '([.checks[0] | keys[]] | sort) == ["actions","class","detail","fix_cmd","fix_error","fix_outcome","fixable","group","id","reason","severity","status","verdict"]' \
  "$W/g1.json" >/dev/null && echo "checks[] carries §7's row keys (plus reason/verdict/fix_outcome/fix_error/actions): ok"

say "GATE 4: doctor capabilities --json is valid"
sandbox g4
"$FM" doctor capabilities --json > "$W/g4.json"
echo "exit: $?"
jq -e . "$W/g4.json" >/dev/null && echo "jq: valid JSON"
jq -r '"top-level keys: \(keys | join(", "))"' "$W/g4.json"
jq -r '"checks: \(.checks | length) · groups: \(.groups | length) · classes: \(.classes | length)"' "$W/g4.json"
jq -r '.groups[] | "  \(.group): \(.checks) checks, needs \(.needs)"' "$W/g4.json"
jq -r '.classes[] | "  class \(.class): \(.meaning) — \(.members | length) member(s)"' "$W/g4.json"
echo "-- every auto_fixable check and the Action it emits"
jq -r '.checks[] | select(.auto_fixable) | "  \(.id) (class \(.class)) -> \(.plan)"' "$W/g4.json"
echo "-- no class D row is auto_fixable:"
jq -r '[.checks[] | select(.class=="D" and .auto_fixable)] | length == 0' "$W/g4.json"

# --------------------------------------------------------------- 5
say "GATE 5: doctor --only security works (and every other group, and a bad one exits 2)"
sandbox g5
for g in binaries node wallet units storage install security; do
  OUT="$("$FM" doctor --only "$g" --json)"; RC=$?
  printf '  --only %-9s exit %-3s %s\n' "$g" "$RC" \
    "$(printf '%s' "$OUT" | jq -r '"\(.summary.total) checks (\(.summary.pass)p \(.summary.warn)w \(.summary.fail)f \(.summary.skip)s), only=\(.summary.only)"')"
done
echo "-- --only security in full"
"$FM" doctor --only security --json | jq -r '.checks[] | "  \(.id) [\(.verdict)] \(.detail)"'
echo "-- an unknown group names every legal one and exits 2"
"$FM" doctor --only keyring --json; echo "exit: $?"

# --------------------------------------------------------------- 6
say "GATE 6: fixture-driven tests for the parsing detectors, and the Verdict round trip"
cd /home/oliver/Projects/odcpw/fmonero
cargo test -p fmonero 2>&1 | sed -n '/running /,$p'
echo "-- fixture cases on disk"
ls -1 tests/fixtures/doctor | sed 's/^/  /'

# --------------------------------------------------------------- 7
say "GATE 7: cargo fmt --check, clippy -D warnings, and the §2g/§2b greps over the doctor"
cargo fmt --check && echo "cargo fmt --check: clean"
cargo clippy --all-targets -- -D warnings 2>&1 | tail -2
echo "-- §2g: no print macro in the doctor (all output goes through output.rs's Out)"
grep -rn 'println!\|eprintln!\|print!\|dbg!' crates/fmonero/src/doctor/ || echo "  none"
echo "-- §2f: no shell string in the doctor"
grep -rnE '"(sh|bash)"\s*,\s*"-c"|Command::new\("(sh|bash)"\)' crates/fmonero/src/doctor/ || echo "  none"
echo "-- §2b.3: query_key"
grep -rn 'query_key' crates/fmonero/src/doctor/ || echo "  none"
echo "-- §2b.8: the doctor never reads monerod.conf's or the login file's bytes into a probe"
grep -n 'monerod_conf\|wallet_login_file' crates/fmonero/src/doctor/probes.rs | sed 's/^/  /'
echo "-- §7: no RemoveFile verb anywhere in mutate.rs's Action vocabulary"
grep -n 'enum Action' -A 30 crates/fmonero/src/doctor/mutate.rs | grep -E '^\s*[0-9]+-\s+(WriteFile|CreateDir|CreateSymlink|RunArgv|Remove)' | sed 's/^/  /'
echo "-- §7.1: the check count against the design table"
grep -c '^        id: "' crates/fmonero/src/doctor/checks.rs

say "GATE 8: the probe artifacts a --save run writes, and the §2b known-value grep over them"
sandbox g8
"$FM" doctor --save --json >/dev/null; echo "exit: $?"
find "$FMONERO_STATE_DIR/doctor" -type f -printf '%M %s %P\n' | LC_ALL=C sort | sed 's/^/  /'
echo "-- every credential key that appears anywhere under the run directory, with what follows it"
grep -rhoE '(rpc-login|daemon-login|password|login)[^"]{0,24}' \
  "$FMONERO_STATE_DIR/doctor/latest/" | sort -u | sed 's/^/  /' || echo "  none"
echo "  (each of those is a FIELD NAME; a value would follow a ':' or '=' — §2b.8)"
echo "-- and the same grep over a rendered unit that DOES carry one, to prove the redactor fires"
mkdir -p "$W/g8unit"
printf '[Service]\nExecStart=/usr/bin/monero-wallet-rpc --rpc-login fmonero:SECRETVALUE --log-level 0\n' \
  > "$W/g8unit/fmonero-wallet-rpc.service"
cp "$W/g8unit/fmonero-wallet-rpc.service" "$W/g8unit/fmonero-monerod.service"
FMONERO_SYSTEMD_USER_DIR="$W/g8unit" "$FM" doctor --only units --save --json > "$W/g8unit.json"
echo "  doctor --only units exit: $?"
jq -r '.checks[] | select(.id=="unit_no_password_in_argv") | "  \(.id) [\(.verdict)] \(.detail)"' "$W/g8unit.json"
echo "  SECRETVALUE anywhere in the run artifacts:"
grep -rc SECRETVALUE "$FMONERO_STATE_DIR/doctor/latest/" 2>/dev/null | sed 's/^/    /' || echo "    (no match)"
grep -rh 'rpc-login' "$FMONERO_STATE_DIR/doctor/latest/probes/facts.json" | head -2 | sed 's/^/    /'
say "GATE 9: the check table against §7.1 — every id with its group, severity and class"
jq -r '.checks[] | "  \(.group)/\(.id)  sev=\(.severity)  class=\(.class)"' "$W/g4.json"

say "DONE"
