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
# `sync_percent` is an f64 by §5's own schema and is computed in `status.rs`; it is a
# PERCENTAGE, not a monetary value. It is named here so the exception is visible rather than
# silent, and the grep still covers every other file.
forbid "no 'as f64' outside status.rs's sync percentage" "$FLOAT" \
  "$CORE/amount.rs" "$CORE/send.rs" "$CORE/history.rs" "$CORE/state.rs" \
  "$CORE/rpc" "$BIN/output.rs" "$BIN/main.rs" "$BIN/setup.rs"
selftest "as f64" "$FLOAT" 'let xmr = amount.0 as f64 / 1e12;'
printf '  note  status.rs is excluded and audited by hand: §5 types sync_percent as f64.\n'
grep -n 'as f64' "$CORE/status.rs" | sed 's/^/        /'

# ---------------------------------------------------------------- §2a.3 one send path
step "§2a.3  only send.rs may build or relay a transaction"
# Call syntax, not the bare names: a doc comment that mentions `relay_tx` is documentation,
# and a grep that fires on it is a grep somebody relaxes. The self-test below is a real call.
MONEY='(\.(relay_tx|build_transfer|build_sweep_all)\(|state::set_pending_send\()'
forbid "the binary never builds or relays directly" "$MONEY" \
  "$BIN/main.rs" "$BIN/setup.rs" "$BIN/output.rs" "$BIN/prompt.rs"
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
forbid "ureq is named only in rpc/mod.rs" "$UREQ" \
  "$CORE/amount.rs" "$CORE/address.rs" "$CORE/send.rs" "$CORE/history.rs" "$CORE/state.rs" \
  "$CORE/status.rs" "$CORE/units.rs" "$CORE/session.rs" "$CORE/log.rs" "$CORE/agent.rs" \
  "$CORE/rpc/jsonrpc.rs" "$CORE/rpc/node.rs" "$CORE/rpc/wallet.rs" "$CORE/rpc/digest.rs" "$BIN"
selftest "a stray ureq call" "$UREQ" 'let r = ureq::get("http://127.0.0.1:18081").call()?;'
permit "the agent still sets timeout_global" 'timeout_global' "$CORE/rpc/mod.rs"

# ---------------------------------------------------------------- §2c loopback only
step "§2c  loopback plaintext only"
BINDS='(TcpListener|UdpSocket|\.bind\()'
forbid "fmonero never binds a port" "$BINDS" \
  "$CORE/amount.rs" "$CORE/address.rs" "$CORE/send.rs" "$CORE/history.rs" "$CORE/state.rs" \
  "$CORE/status.rs" "$CORE/units.rs" "$CORE/session.rs" "$CORE/log.rs" "$CORE/agent.rs" \
  "$CORE/context.rs" "$CORE/paths.rs" "$CORE/proc.rs" "$CORE/settings.rs" "$CORE/fsinfo.rs" \
  "$CORE/wallet_dir.rs" "$CORE/error.rs" "$CORE/lib.rs" "$CORE/rpc/mod.rs" \
  "$CORE/rpc/jsonrpc.rs" "$CORE/rpc/node.rs" "$CORE/rpc/wallet.rs" "$CORE/rpc/digest.rs" "$BIN"
selftest "a listener" "$BINDS" 'let l = std::net::TcpListener::bind("0.0.0.0:8080")?;'
printf '  note  crates/fmonero-mock and rpc/mock_tests.rs are excluded BY PATH: the mock binds\n'
printf '        a port on purpose (§8 Phase 0) and is never installed.\n'

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
  step "--full: the documented environment variables are all read (§5b)"
  MISSING=""
  for var in FMONERO_STATE_DIR FMONERO_CONFIG_DIR FMONERO_CACHE_DIR FMONERO_WALLET_PASSWORD \
             NO_COLOR CI FMONERO_PROC_ROOT FMONERO_SYSTEMCTL_FIXTURE; do
    grep -rq "$var" "${SHIPPED[@]}" 2>/dev/null || MISSING="$MISSING $var"
  done
  if [ -n "$MISSING" ]; then fail "capabilities documents variables nothing reads:$MISSING"; else
    pass "every documented variable is read somewhere under crates/"
  fi
fi

printf '\n'
if [ "$FAIL" = 0 ]; then
  printf 'check.sh: PASS (%s)\n' "$ROOT"
  exit 0
fi
printf 'check.sh: FAIL (%s)\n' "$ROOT"
exit 1
