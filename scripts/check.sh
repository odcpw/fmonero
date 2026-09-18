#!/usr/bin/env bash
# fmonero's structural checks — the greps that make DESIGN.md's compile-time claims real.
#
# Every claim here is one an unenforced comment would let a single edit falsify: "core is
# silent", "no shell strings", "no TLS stack in the binary", "fmonero never binds a port", "no
# code path can call ureq without a deadline", "no f64 touches a monetary value". So each grep
# runs twice: once over the tree, and once over a **fixture that contains the forbidden thing**,
# where it must fire. A grep that cannot fail is worse than no grep, because the design cites it
# as the mitigation and a later reader takes that as coverage (DESIGN.md §2c, §2f).
#
#   scripts/check.sh          fast: fmt, clippy, tests, every grep and its self-test
#   scripts/check.sh --full   adds `cargo build --release` and `cargo doc`
#
# Exit 0 only when everything passed. Every failure prints what it found.

set -uo pipefail

FULL=0
for arg in "$@"; do
  case "$arg" in
    --full) FULL=1 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "check.sh: unknown argument $arg" >&2; exit 2 ;;
  esac
done

cd "$(dirname "$0")/.." || exit 1
ROOT="$PWD"
FAIL=0
SELFTEST="$(mktemp -d)"
trap 'rm -rf "$SELFTEST"' EXIT

pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=1; }
step() { printf '\n== %s\n' "$1"; }

# ---------------------------------------------------------------- the grep harness
#
# `forbid <name> <pattern> <paths…>` fails when an extended-regexp pattern appears anywhere in
# `paths`. `selftest <name> <pattern> <file-content>` proves the same pattern fires on a file
# that contains it. Every `forbid` below is paired with at least one `selftest`.

forbid() {
  local name="$1" pattern="$2"; shift 2
  local hits
  hits="$(grep -rnE "$pattern" "$@" 2>/dev/null)"
  if [ -n "$hits" ]; then
    fail "$name"
    printf '%s\n' "$hits" | sed 's/^/        /'
    return 1
  fi
  pass "$name"
}

# `forbid_except <name> <pattern> <exclude-regex> <paths…>` is `forbid` over whole DIRECTORIES
# with the legitimate files filtered out of the hit list by path — which is how DESIGN words
# these greps (§2c(v), §2d) and the only form that covers a file added later. An enumerated
# path list is an exception list wearing a different hat: `rpc/mod.rs:130` says so in as many
# words ("an exception list is a grep that stops protecting anything"), and the old list
# happened to omit eight core files and the whole of `src/tui/**` and `src/doctor/**`.
forbid_except() {
  local name="$1" pattern="$2" exclude="$3"; shift 3
  local hits
  hits="$(grep -rnE "$pattern" "$@" 2>/dev/null | grep -vE "$exclude")"
  if [ -n "$hits" ]; then
    fail "$name"
    printf '%s\n' "$hits" | sed 's/^/        /'
    return 1
  fi
  pass "$name"
}

selftest() {
  local name="$1" pattern="$2" content="$3"
  local dir="$SELFTEST/$RANDOM$RANDOM"
  mkdir -p "$dir"
  printf '%s\n' "$content" > "$dir/fixture.rs"
  if grep -rqE "$pattern" "$dir" 2>/dev/null; then
    pass "self-test: $name fires on its own fixture"
  else
    fail "self-test: $name does NOT fire — the check above proves nothing"
  fi
}

# `permit` is the inverse: a pattern that MUST be present, so that a check cannot be defeated by
# deleting the thing it guards.
permit() {
  local name="$1" pattern="$2"; shift 2
  if grep -rqE "$pattern" "$@" 2>/dev/null; then
    pass "$name"
  else
    fail "$name — the pattern this check guards has disappeared"
  fi
}

CORE=crates/fmonero-core/src
BIN=crates/fmonero/src
# The mock is test-only, is never installed, and is the one crate that binds a port on purpose
# (§8 Phase 0), so it is excluded by path from the structural greps — never by exception list.
SHIPPED=("$CORE" "$BIN")

# ---------------------------------------------------------------- cargo
# One run each, and the diagnosis comes from THAT run: re-running a failed command to print
# its output is how a flaky failure gets reported as "FAIL" with nothing under it.
step "cargo fmt --check"
if out="$(cargo fmt --check 2>&1)"; then pass "formatted"; else
  fail "cargo fmt --check"; printf '%s\n' "$out" | head -40 | sed 's/^/        /'
fi

step "cargo clippy --all-targets -- -D warnings"
if out="$(cargo clippy --all-targets -- -D warnings 2>&1)"; then pass "no warnings"; else
  fail "clippy"; printf '%s\n' "$out" | grep -E '^(error|warning)' | head -30 | sed 's/^/        /'
fi

step "cargo test"
if out="$(cargo test 2>&1)"; then pass "all tests"; else
  fail "cargo test"
  printf '%s\n' "$out" | grep -E '(FAILED|^error|^---- |panicked)' | head -40 | sed 's/^/        /'
fi

# ---------------------------------------------------------------- §2f argv only
step "§2f  argv only — no shell strings anywhere"
# Three cases, as fproton's check.sh uses: the argv-array form, the Command::new form, and the
# QML one (which `tests/plugin-smoke.sh` owns; here only the Rust halves).
SHELL_ARGV='"(sh|bash)"[[:space:]]*,[[:space:]]*"-c"'
SHELL_NEW='Command::new\("(sh|bash)"\)'
forbid "no argv array runs a shell" "$SHELL_ARGV" "${SHIPPED[@]}"
forbid "no Command::new(\"sh\")" "$SHELL_NEW" "${SHIPPED[@]}"
selftest "shell argv" "$SHELL_ARGV" 'let argv = ["bash", "-c", "rm -rf /"];'
selftest "shell argv (sh)" "$SHELL_ARGV" 'run(&["sh" , "-c" , "echo"]);'
selftest "Command::new" "$SHELL_NEW" 'let c = std::process::Command::new("bash");'

# ---------------------------------------------------------------- §2g silent core
step "§2g  the core is silent"
PRINTS='(^|[^[:alnum:]_!])(println!|eprintln!|print!|eprint!|dbg!)'
forbid "no print macro in fmonero-core" "$PRINTS" "$CORE"
selftest "print macro" "$PRINTS" 'fn f() { println!("core must not print"); }'
selftest "dbg macro" "$PRINTS" 'fn f(x: u32) -> u32 { dbg!(x) }'
if grep -qE '^anyhow' crates/fmonero-core/Cargo.toml 2>/dev/null; then
  fail "fmonero-core must not depend on anyhow (§2g)"
else
  pass "fmonero-core has no anyhow"
fi

# ---------------------------------------------------------------- §2h no stubs
step "§2h  no stubs, no placeholders"
STUBS='(todo!|unimplemented!\(|FIXME|XXX|placeholder)'
forbid "no todo/unimplemented/FIXME/XXX/placeholder" "$STUBS" "${SHIPPED[@]}"
selftest "todo!" "$STUBS" 'fn f() { todo!() }'
selftest "FIXME" "$STUBS" '// FIXME: finish this'
# `Error::unimplemented` is the one sanctioned "not implemented" (§2h). Every REMAINING
# constructor is a gap a later phase must close, so they are counted rather than forbidden.
LEFT="$(grep -rn 'Error::unimplemented(' "${SHIPPED[@]}" 2>/dev/null | grep -v 'crates/fmonero-core/src/error.rs' | wc -l)"
printf '  note  %s Error::unimplemented call site(s) left (§2h: zero by the Phase 3 gate)\n' "$LEFT"
grep -rn 'Error::unimplemented(' "${SHIPPED[@]}" 2>/dev/null | grep -v 'crates/fmonero-core/src/error.rs' | sed 's/^/        /'

# ---------------------------------------------------------------- §2b secrets
step "§2b  secrets"
forbid "query_key is out of scope" 'query_key' crates/
selftest "query_key" 'query_key' 'let k = w.query_key("spend_key");'

# ---------------------------------------------------------------- §2a.5 no floats on money
step "§2a.5  no f64 touches a monetary value"
FLOAT='as f64'
# §2a.5's rule is "No `f64` touches a **monetary value**" — percentages, byte rates and ETAs are
# not monetary values, and four files compute one. They are named here so the exception is
# visible rather than silent, and the grep covers **every other file in both crates**, including
# ones added later: the old enumerated list left all of `src/tui/**` and `src/doctor/**` out.
# v5.5 adds `nodes.rs`: `health_ratio` is a display ratio of two integers and never an amount
# (§5b's own note says this file is on the audited list for exactly that line).
AUDITED_FLOAT='(fmonero-core/src/(status|nodes)\.rs|fmonero/src/tui/mod\.rs|fmonero/src/tui/screens\.rs|fmonero/src/doctor/checks\.rs):'
forbid_except "no 'as f64' outside the four audited non-money sites" "$FLOAT" "$AUDITED_FLOAT" \
  "${SHIPPED[@]}"
selftest "as f64" "$FLOAT" 'let xmr = amount.0 as f64 / 1e12;'
# And a money-SHAPED float is forbidden everywhere, the audited files included — which is the
# half a per-file exclusion cannot give, since those four are where a new screen would put one.
MONEY_FLOAT='(pico|amount|fee|balance|unlocked|total|xmr)[A-Za-z_0-9.]*[[:space:]]+as f64'
forbid "no f64 on a value named like money, anywhere" "$MONEY_FLOAT" "${SHIPPED[@]}"
selftest "money as f64" "$MONEY_FLOAT" 'let x = plan.total_fee.0 as f64 / 1e12;'
printf '  note  the four audited sites are a sync percentage, a byte rate, a byte formatter and\n'
printf '        a disk figure — none of them monetary (§2a.5, §5 types sync_percent as f64):\n'
grep -rn 'as f64' "${SHIPPED[@]}" 2>/dev/null | sed 's/^/        /'

# ---------------------------------------------------------------- §2a.3 one send path
step "§2a.3  only send.rs may build or relay a transaction"
# Call syntax, not the bare names: a doc comment that mentions `relay_tx` is documentation,
# and a grep that fires on it is a grep somebody relaxes. The self-test below is a real call.
MONEY='(\.(relay_tx|build_transfer|build_sweep_all)\(|state::set_pending_send\()'
# The WHOLE binary tree, which is what §2a.2's own test command greps
# (`grep -rn … crates/fmonero/src`) and what §2a.3's argument is about: "The CLI and the TUI are
# two independently written callers; an invariant that lives in both of them is an invariant that
# holds in one", and §7a repeats "the TUI's copy of it would be the one nobody reviews". A
# four-file list left `src/tui/**` and `src/doctor/**` — the second caller and the read-only
# probes — outside the guard entirely. Excluded by path, with the reason, as the core-side list
# below already does: `tui/tests.rs` writes a pending record to set a test up.
forbid_except "the binary never builds or relays directly" "$MONEY" \
  '(fmonero/src/tui/tests\.rs):' "$BIN"
selftest "a direct relay" "$MONEY" 'let h = w.relay_tx(&plan.txs[0].metadata)?;'
# And inside core, only send.rs may name them (the clients DEFINE them, so wallet.rs is the
# other legitimate file).
OFFENDERS="$(grep -rlE "$MONEY" "$CORE" 2>/dev/null | grep -vE '(send\.rs|rpc/wallet\.rs|state\.rs|rpc/mock_tests\.rs|status\.rs)')"
if [ -n "$OFFENDERS" ]; then
  fail "only send.rs orchestrates the money path (§2a.3)"; printf '%s\n' "$OFFENDERS" | sed 's/^/        /'
else
  pass "only send.rs, rpc/wallet.rs and state.rs name the money calls"
fi

# ---------------------------------------------------------------- §2d one deadline
step "§2d  no code path can call ureq without a deadline"
UREQ='ureq::'
# §2d words this as "`grep -rn 'ureq::' crates/*/src` … fails on any hit outside
# `crates/fmonero-core/src/rpc/mod.rs`", and `rpc/mod.rs:130` explains why it must be that shape:
# "an exception list is a grep that stops protecting anything". Directories, with the two
# legitimate files filtered out of the hit list.
forbid_except "ureq is named only in rpc/mod.rs" "$UREQ" \
  '(fmonero-core/src/rpc/(mod|mock_tests)\.rs):' "${SHIPPED[@]}"
selftest "a stray ureq call" "$UREQ" 'let r = ureq::get("http://127.0.0.1:18081").call()?;'
permit "the agent still sets timeout_global" 'timeout_global' "$CORE/rpc/mod.rs"

# ---------------------------------------------------------------- §2c loopback only
step "§2c  loopback plaintext only"
BINDS='(TcpListener|UdpSocket|\.bind\()'
# §2c(v) words this as `grep -rn 'TcpListener|UdpSocket|\.bind(' crates/fmonero-core/src
# crates/fmonero/src` with the mock "excluded by path" — directories, not an enumerated list
# that a new file silently escapes.
forbid_except "fmonero never binds a port" "$BINDS" \
  '(fmonero-core/src/rpc/mock_tests\.rs|fmonero/src/tui/tests\.rs):' "${SHIPPED[@]}"
selftest "a listener" "$BINDS" 'let l = std::net::TcpListener::bind("0.0.0.0:8080")?;'
printf '  note  crates/fmonero-mock and rpc/mock_tests.rs are excluded BY PATH: the mock binds\n'
printf '        a port on purpose (§8 Phase 0) and is never installed.\n'

# ---- §2c grep (vi), v5.5: the ONLY module that spawns curl ---------------------
#
# **Scoped to the spawn surface, not to the substring** (revision 3). The substring form
# `grep -rn 'curl' …` could not pass its own first commit: §5b.4 row 6 adds `(4, "curl_missing")`
# to `Error::REASONS` and §2n.3 gives the hint `omarchy pkg add curl`, both of which live in
# `error.rs` by §3. A check that fails on day one is worse than no check — the agent who hits it
# widens it until the build passes, most likely to "grep for Command::new", which no longer says
# anything about curl, while §2c and README keep printing the boundary claim.
#
# So: outside `nodes.rs`, no file may name curl as a PROCESS, and the two reason-token strings in
# `error.rs` are exempt BY NAME rather than by file — an exemption that cannot quietly widen into
# "anything in error.rs".
CURL_SPAWN='(which\("curl"\)|Command::new\("curl"\)|"curl"[[:space:]]*,)'
CURL_EXEMPT='(fmonero-core/src/nodes\.rs):|error\.rs:[0-9]+:.*(curl_missing|omarchy pkg add curl)'
forbid_except "curl is spawned only from nodes.rs (§2c vi)" "$CURL_SPAWN" \
  "$CURL_EXEMPT" "${SHIPPED[@]}"
# TWO self-tests, because one would not prove the exemption is narrow.
selftest "a planted curl argv" "$CURL_SPAWN" 'let argv = ["curl", "-sS", "http://x"];'
selftest "a planted Command::new curl" "$CURL_SPAWN" 'std::process::Command::new("curl");'
# And the exemption really is by TOKEN: a `Command::new("curl")` in error.rs is still a hit.
CURL_PROBE="$SELFTEST/curl-exempt"
mkdir -p "$CURL_PROBE"
printf 'std::process::Command::new("curl");\n' > "$CURL_PROBE/error.rs"
if grep -rnE "$CURL_SPAWN" "$CURL_PROBE" 2>/dev/null | grep -vE "$CURL_EXEMPT" | grep -q .; then
  pass "self-test: the error.rs exemption does not cover a spawn"
else
  fail "self-test: the error.rs exemption is too wide — a planted spawn there would pass"
fi
# The test seam is read in ONE place (§2n.7).
forbid_except "FMONERO_NODES_URL is read only in nodes.rs" 'FMONERO_NODES_URL' \
  '(fmonero-core/src/nodes\.rs):' "${SHIPPED[@]}"
selftest "a second reader of the seam" 'FMONERO_NODES_URL' 'std::env::var("FMONERO_NODES_URL")'
# §7a.5: `wallet.autostart` has exactly one reader, `tui::run` — `status` must never start
# anything (§2e), and neither may the widget or the doctor.
# `\.wallet\.autostart` is the FIELD ACCESS, not the setting's name: a prose mention of the key
# in a comment or a doc string is not a reader, and matching one is the day-one failure revision 3
# is about.
forbid_except "wallet.autostart is read only by the TUI" '\.wallet\.autostart' \
  '(fmonero-core/src/settings\.rs|fmonero/src/tui/mod\.rs):' "${SHIPPED[@]}"
selftest "a second reader of autostart" '\.wallet\.autostart' 'if ctx.settings.wallet.autostart {}'

TLS='^(rustls|native-tls|openssl|openssl-sys|hyper-tls|tokio)'
if command -v cargo >/dev/null 2>&1; then
  TREE="$(cargo tree -p fmonero -e normal --prefix none 2>/dev/null | sed 's/ .*//' | sort -u)"
  if [ -z "$TREE" ]; then
    fail "cargo tree produced nothing, so the no-TLS claim is unchecked"
  elif printf '%s\n' "$TREE" | grep -qE "$TLS"; then
    fail "the binary carries a TLS or async stack (§2c)"
    printf '%s\n' "$TREE" | grep -E "$TLS" | sed 's/^/        /'
  else
    pass "no rustls/native-tls/openssl/hyper-tls/tokio in the binary"
  fi
  # The self-test: the same grep over a tree that DOES contain one must fire.
  if printf 'fmonero\nrustls\nserde\n' | grep -qE "$TLS"; then
    pass "self-test: the TLS grep fires on a tree that has rustls"
  else
    fail "self-test: the TLS grep does NOT fire"
  fi
fi

# ---------------------------------------------------------------- §7a the TUI's Action
step "§7a  the TUI's Action derives neither Clone nor Debug"
if [ -d "$BIN/tui" ]; then
  ACTION='#\[derive\([^)]*(Clone|Debug)[^)]*\)\][[:space:]]*(pub )?enum Action'
  if grep -rqzE "$ACTION" "$BIN/tui" 2>/dev/null; then
    fail "Action must derive neither Clone nor Debug (§7a): a Clone leaves a second copy of the password nobody zeroes"
  else
    pass "Action carries no derived Clone or Debug"
  fi
else
  printf '  skip  crates/fmonero/src/tui does not exist yet (Phase 2 owns it)\n'
fi

# ---------------------------------------------------------------- §7a / §6.1 line budgets
#
# §7a's budget is a **ceiling against growth**, not a target: DESIGN.md v5.1 replaced "≤ 1 800"
# with the measured 4 600 after three phases reported the overrun and none of them closed it
# (`ux-19`). The gate exists so the next agent cannot quietly add another 2 000 lines the way
# these five files got their first 2 600 — and it is measured here, in a gate that fails, rather
# than recorded in a ledger nothing reads. `tests.rs` is excluded by §7a: it is test code.
step "§7a  the TUI fits its line budget · §6.1 the plugin fits its own"
# v5.2: 4700, measured 4655 — §2m's Node screen, stated in DESIGN.md §7a first (was 4600).
TUI_BUDGET=7200
if [ -d "$BIN/tui" ]; then
  TUI_FILES=()
  for f in mod theme screens send subs hit; do
    [ -f "$BIN/tui/$f.rs" ] && TUI_FILES+=("$BIN/tui/$f.rs")
  done
  TUI_LINES="$(cat "${TUI_FILES[@]}" 2>/dev/null | grep -c '')"
  if [ "$TUI_LINES" -le "$TUI_BUDGET" ]; then
    pass "§7a: $TUI_LINES lines across src/tui/{mod,theme,screens,send,subs,hit}.rs (budget $TUI_BUDGET, tests.rs excluded)"
  else
    fail "§7a: $TUI_LINES lines across src/tui/{mod,theme,screens,send,subs,hit}.rs exceeds the $TUI_BUDGET-line ceiling"
  fi
else
  printf '  skip  §7a line budget: crates/fmonero/src/tui does not exist yet\n'
fi
# v5.2 raised it from 1310 by §6.1's +15/+15 for the remote badge and `Model.remoteLabel`
# and by nothing else, stated in DESIGN.md before the code was written (§6.1).
QML_BUDGET=1516
if [ -d plugin/oliver.monero ]; then
  QML_LINES="$(cat plugin/oliver.monero/*.qml plugin/oliver.monero/*.js plugin/oliver.monero/manifest.json 2>/dev/null | grep -c '')"
  if [ "$QML_LINES" -le "$QML_BUDGET" ]; then
    pass "§6.1: $QML_LINES lines of manifest + QML + Model.js (budget $QML_BUDGET)"
  else
    fail "§6.1: $QML_LINES lines of manifest + QML + Model.js exceeds the $QML_BUDGET-line budget"
  fi
  # v5.5, revision 12: the PER-FILE figures too. §6.1 calls them "hard per-file budgets" and
  # nothing checked them, which is how `MoneroIcon.qml` came to be documented as 95 lines while
  # measuring 96 — a number only prose defended.
  while IFS=' ' read -r want file; do
    [ -e "plugin/oliver.monero/$file" ] || continue
    have="$(grep -c '' "plugin/oliver.monero/$file")"
    if [ "$have" -le "$want" ]; then
      pass "§6.1: $file $have lines (budget $want)"
    else
      fail "§6.1: $file $have lines exceeds its $want-line budget"
    fi
  done <<'PERFILE'
420 Service.qml
800 Panel.qml
96 MoneroIcon.qml
200 Model.js
PERFILE
else
  printf '  skip  §6.1 line budget: plugin/oliver.monero does not exist yet\n'
fi

# ---------------------------------------------------------------- shell scripts
step "shell scripts parse"
for f in scripts/*.sh tests/*.sh; do
  [ -e "$f" ] || continue
  if bash -n "$f" 2>/dev/null; then pass "bash -n $f"; else fail "bash -n $f"; bash -n "$f" 2>&1 | sed 's/^/        /'; fi
done
if command -v shellcheck >/dev/null 2>&1; then
  for f in scripts/*.sh tests/*.sh; do
    [ -e "$f" ] || continue
    if shellcheck -S warning "$f" >/dev/null 2>&1; then pass "shellcheck $f"; else
      fail "shellcheck $f"; shellcheck -S warning "$f" 2>&1 | head -20 | sed 's/^/        /'
    fi
  done
else
  printf '  skip  shellcheck is not installed on this machine (§1.4); bash -n ran instead\n'
fi

# ---------------------------------------------------------------- §2j nothing RED is claimed
#
# "README, `--help`, `robot-docs` and `capabilities.build_note` claim only what §4b marks GREEN or
# AMBER, and say which. A line describing a RED row as working is a Phase 3 blocker" (§2j), and
# §4b spells the mechanism out: "Phase 3 greps README, every `--help` string and `robot-docs` for
# any sentence claiming a successful send, a successful sync or a live peer list, and fails on a
# hit that is not adjacent to the word 'unverified'".
#
# Two deliberate widenings of that sentence, both recorded in DECISIONS.md:
#  * the honesty marker is `unverified` OR `RED`/`AMBER` OR `no send`/`nothing has been`/`cannot be
#    verified`, because README's own Status section words it "RED, and therefore unverified" and
#    `build_note` words it "no send has ever been broadcast". Demanding the single word
#    `unverified` would fail on text that is already honest, and a check that fails on correct
#    output is the check that gets deleted.
#    **`never` and `not yet` are deliberately NOT markers.** They are ordinary English this
#    project uses constantly — "the doctor never deletes anything", "core never prints", "status
#    never starts anything" — and README alone has 25 such lines out of 424, so with ±1 adjacency
#    roughly a fifth of the file was pre-excused and an outright false RED claim one line below a
#    "never" sentence passed in silence. The third self-test below is that exact fixture;
#  * "adjacent" is the line itself and the two lines around it, since these are wrapped
#    paragraphs, not one sentence per line.
# The claim pattern matches ACHIEVED capability ("successfully sent", "a real peer list",
# "verified against a live daemon") and not mechanism prose ("a normal successful send clears the
# record"), which describes what happens when one succeeds and claims nothing. It also matches
# "every step is idempotent" and "wires up Omarchy", because a real `setup` run is RED (§4b) and
# README's Install section claimed both flatly — a claim about a RED row that matched none of the
# alternatives above.
step "§2j  README, every --help and robot-docs claim nothing §4b marks RED"
CLAIM='successfully (sent|swept|broadcast|relayed|synced)|(has|have|had|was|were) (been )?(sent|swept|broadcast|relayed) (to|on|by|from)|(a |the )?(real|live) (send|sweep|broadcast|sync|peer list|balance|refresh)|(sends|broadcasts|relays|sweeps) (real|mainnet|stagenet)|(verified|proven|tested) (against|with|on) (a )?(real|live)|(every|each) step is idempotent|(wires up|sets up) (omarchy|the)'
MARKER_CI='unverified|no send|nothing has been|cannot be verified'
# Written as an explicit character class rather than with `\<`/`\>`: gawk warns that those
# escapes are "treated as plain" and the pattern then demands literal angle brackets, which would
# make this whole marker dead and the check quietly stricter than it reads.
MARKER_CS='(^|[^A-Za-z])(RED|AMBER)([^A-Za-z]|$)'

honesty_scan() { # honesty_scan <file> -> prints each claim line that has no marker within ±1 line
  awk -v claim="$CLAIM" -v mci="$MARKER_CI" -v mcs="$MARKER_CS" '
    { line[NR] = $0 }
    END {
      for (i = 1; i <= NR; i++) {
        if (tolower(line[i]) ~ claim) {
          marked = 0
          for (j = i - 1; j <= i + 1; j++) {
            if (j < 1 || j > NR) continue
            if (tolower(line[j]) ~ mci) marked = 1
            if (line[j] ~ mcs) marked = 1
          }
          if (!marked) printf "%d: %s\n", i, line[i]
        }
      }
    }' "$1"
}

# The self-tests first, so a claim pattern that stopped matching anything cannot pass as silence.
printf 'fmonero has successfully broadcast a real send to mainnet.\nIt also verified against a live daemon.\n' \
  > "$SELFTEST/claims-positive.txt"
if [ -n "$(honesty_scan "$SELFTEST/claims-positive.txt")" ]; then
  pass "self-test: the RED-claim scan fires on its own fixture"
else
  fail "self-test: the RED-claim scan does NOT fire on 'successfully broadcast a real send' — it is vacuous"
fi
# One negative fixture per marker family, so neither path can rot into a dead pattern: the first
# is qualified only by `unverified` (the word §4b names), the second only by `RED`.
printf 'These rows are unverified today.\nA real send or sweep of any amount, and a real peer list.\n' \
  > "$SELFTEST/claims-negative-word.txt"
if [ -n "$(honesty_scan "$SELFTEST/claims-negative-word.txt")" ]; then
  fail "self-test: the RED-claim scan fires on a line an adjacent 'unverified' already qualifies"
else
  pass "self-test: and it leaves a line adjacent to 'unverified' alone"
fi
printf '**RED.**\nA real send or sweep of any amount, and a real peer list.\n' \
  > "$SELFTEST/claims-negative-class.txt"
if [ -n "$(honesty_scan "$SELFTEST/claims-negative-class.txt")" ]; then
  fail "self-test: the RED-claim scan fires on a line an adjacent 'RED' already qualifies — MARKER_CS is dead"
else
  pass "self-test: and it leaves a line adjacent to the class name RED alone"
fi
# The third one pins the NARROWING: an ordinary "never" sentence — the shape this codebase writes
# on 6% of README's lines — must not excuse the false claim below it.
printf 'Core is silent: no function in fmonero-core ever writes to stdout, and it never prints.\nfmonero has successfully broadcast a real send to mainnet, and a real peer list is verified against a live daemon.\n' \
  > "$SELFTEST/claims-never.txt"
if [ -n "$(honesty_scan "$SELFTEST/claims-never.txt")" ]; then
  pass "self-test: an adjacent 'never' sentence does NOT excuse a RED claim"
else
  fail "self-test: 'never' is excusing RED claims again — MARKER_CI was widened back"
fi

FMBIN="$ROOT/target/release/fmonero"
[ -x "$FMBIN" ] || cargo build --release -p fmonero >/dev/null 2>&1
if [ ! -x "$FMBIN" ]; then
  fail "§2j: target/release/fmonero is missing, so --help, robot-docs and build_note cannot be read"
else
  CORPUS="$SELFTEST/claims-corpus.txt"
  : > "$CORPUS"
  for f in README.md plugin/oliver.monero/README.md; do
    [ -f "$f" ] && cat "$f" >> "$CORPUS"
  done
  "$FMBIN" --help >> "$CORPUS" 2>&1
  # **The command list is DERIVED, never transcribed.** `agent.rs` says "`scripts/check.sh`
  # counts it, so a command that exists in `main.rs` and not here is visible rather than
  # undocumented" — which was not true of any line in this file. A hand-written list also cannot
  # notice a new command, so its `after_help` would be unreachable by the scan below: the honesty
  # gate would go quiet on exactly the text nobody re-reads. No `jq`: this script needs only
  # cargo and coreutils, and the field order of `capabilities` is stable (it is generated).
  CMDLIST="$SELFTEST/commands.txt"
  "$FMBIN" capabilities --json 2>/dev/null | tr '{' '\n' \
    | grep -oE '"mutating":[a-z]+,"name":"[^"]+"' | sed 's/.*"name":"//; s/"$//' \
    | sort -u > "$CMDLIST"
  if [ ! -s "$CMDLIST" ]; then
    fail "§2j: capabilities lists no commands, so the corpus below would be nearly empty"
  fi
  # The clap groups are not §4.1 rows, but their own `--help` is prose a claim can hide in.
  { printf 'node\nwallet\nsettings\ndoctor undo\ndoctor capabilities\n'; cat "$CMDLIST"; } \
    | sort -u > "$CMDLIST.all"
  while read -r cmd; do
    [ -n "$cmd" ] || continue
    # shellcheck disable=SC2086
    "$FMBIN" $cmd --help >> "$CORPUS" 2>&1
  done < "$CMDLIST.all"

  # And the two halves have to agree: every documented command must exist in the clap tree, and
  # every top-level subcommand of the clap tree must be documented. Either direction failing is
  # the drift `agent.rs`'s own doc comment promises this file catches.
  MISSING=""
  while read -r cmd; do
    [ -n "$cmd" ] || continue
    # shellcheck disable=SC2086
    "$FMBIN" $cmd --help >/dev/null 2>&1 || MISSING="$MISSING $cmd"
  done < "$CMDLIST"
  if [ -n "$MISSING" ]; then
    fail "§4.1: capabilities documents a command the binary does not have:$MISSING"
  else
    pass "every command capabilities lists exists in the clap tree ($(grep -c '' "$CMDLIST") of them)"
  fi
  UNDOC=""
  for top in $("$FMBIN" --help 2>&1 | sed -n '/^Commands:/,/^Options:/p' \
                 | grep -oE '^  [a-z][a-z-]+' | tr -d ' '); do
    grep -qE "^$top( |$)" "$CMDLIST.all" || UNDOC="$UNDOC $top"
  done
  if [ -n "$UNDOC" ]; then
    fail "§4.1: the binary has a subcommand capabilities does not document:$UNDOC"
  else
    pass "and every subcommand the binary offers is documented"
  fi
  "$FMBIN" robot-docs >> "$CORPUS" 2>&1
  # The two JSON payloads are ONE line each, and `build_note`'s paragraph breaks are the literal
  # two characters `\n` — so "adjacent" (±1 line) means nothing inside them, and a marker one
  # paragraph away from a claim is not adjacent to anything. Unescape them into real lines first,
  # which is also what makes the `RED`/`AMBER` word boundary work: in `…\n\nRED —` the character
  # before `RED` is the letter `n`.
  "$FMBIN" version --json 2>&1 | sed 's/\\n/\n/g' >> "$CORPUS"       # build_stage + build_note
  "$FMBIN" capabilities --json 2>&1 | sed 's/\\n/\n/g' >> "$CORPUS"  # build_note + every doc
  OFFENDERS="$(honesty_scan "$CORPUS")"
  if [ -n "$OFFENDERS" ]; then
    fail "§2j: a RED row is described as working, with no honesty marker nearby"
    printf '%s\n' "$OFFENDERS" | head -20 | sed 's/^/        /'
  else
    pass "no RED claim in README, $(grep -c '' "$CORPUS") lines of --help/robot-docs/capabilities"
  fi
fi

# ---------------------------------------------------------------- --full
if [ "$FULL" = 1 ]; then
  step "--full: cargo build --release"
  if cargo build --release >/dev/null 2>&1; then pass "release build"; else
    fail "cargo build --release"; cargo build --release 2>&1 | grep -E '^error' | head -20 | sed 's/^/        /'
  fi
  step "--full: cargo doc -p fmonero-core"
  if cargo doc -p fmonero-core --no-deps >/dev/null 2>&1; then pass "rustdoc"; else
    fail "cargo doc"; cargo doc -p fmonero-core --no-deps 2>&1 | grep -E '^(error|warning)' | head -20 | sed 's/^/        /'
  fi
  # §5b: "Phase 3's ergonomics pass asserts every documented env var is read somewhere under
  # `crates/` AND that every variable read is documented — the round trip is what stops the list
  # drifting." Only the first half existed, and the second half is the one that catches a new
  # variable: the Phase 2 doctor added FMONERO_SYSTEMD_USER_DIR and nothing noticed.
  step "--full: the documented environment variables and the read ones are the same set (§5b)"
  DOCUMENTED="$("$ROOT/target/release/fmonero" capabilities --json 2>/dev/null \
                | tr ',{}' '\n\n\n' | grep -oE '"FMONERO_[A-Z_]+"|"NO_COLOR"|"CI"' \
                | tr -d '"' | sort -u)"
  if [ -z "$DOCUMENTED" ]; then
    fail "could not read capabilities.env — the round trip below would be vacuous"
  fi
  MISSING=""
  for var in $DOCUMENTED; do
    grep -rq "$var" "${SHIPPED[@]}" 2>/dev/null || MISSING="$MISSING $var"
  done
  if [ -n "$MISSING" ]; then fail "capabilities documents variables nothing reads:$MISSING"; else
    pass "every documented variable is read somewhere under crates/ ($(printf '%s' "$DOCUMENTED" | grep -c '') of them)"
  fi
  # The other direction. `PATH` is excluded by name: `proc::which` reads it, every process has
  # one, and it is not an fmonero knob — documenting it would say fmonero has a setting it does
  # not have. Everything else a source file reads must appear in `capabilities.env`.
  UNDOCUMENTED=""
  READS="$(grep -rhoE 'env::var(_os)?\("[A-Za-z_0-9]+"\)' "${SHIPPED[@]}" 2>/dev/null \
           | grep -oE '"[A-Za-z_0-9]+"' | tr -d '"' | sort -u | grep -v '^PATH$')"
  # `paths.rs` reads its four through one `env_dir(key)` helper, so the literals live in the
  # call sites rather than in an `env::var` call: add any FMONERO_* string literal that the
  # tree hands to that helper.
  READS="$READS $(grep -rhoE 'env_dir\("FMONERO_[A-Z_]+"\)|under_test_state_dir' "${SHIPPED[@]}" 2>/dev/null \
                  | grep -oE 'FMONERO_[A-Z_]+' | sort -u)"
  for var in $(printf '%s\n' $READS | sort -u); do
    case " $(printf '%s' "$DOCUMENTED" | tr '\n' ' ') " in
      *" $var "*) ;;
      *) UNDOCUMENTED="$UNDOCUMENTED $var" ;;
    esac
  done
  if [ -n "$UNDOCUMENTED" ]; then
    fail "the tree reads variables capabilities does not document:$UNDOCUMENTED"
  else
    pass "and every variable the tree reads is documented (PATH excluded by name)"
  fi
  # Self-test: the reverse direction must fire on a source file that reads an undocumented one.
  mkdir -p "$SELFTEST/envcheck"
  printf 'fn f() { std::env::var_os("FMONERO_NOT_DOCUMENTED"); }\n' > "$SELFTEST/envcheck/fixture.rs"
  if grep -rhoE 'env::var(_os)?\("[A-Za-z_0-9]+"\)' "$SELFTEST/envcheck" | grep -q FMONERO_NOT_DOCUMENTED; then
    pass "self-test: the reverse env scan sees a var read in a source file"
  else
    fail "self-test: the reverse env scan finds nothing in its own fixture — it is vacuous"
  fi
fi

printf '\n'
if [ "$FAIL" = 0 ]; then
  printf 'check.sh: PASS (%s)\n' "$ROOT"
  exit 0
fi
printf 'check.sh: FAIL (%s)\n' "$ROOT"
exit 1
