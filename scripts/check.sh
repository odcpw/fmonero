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
# The test seam is read in ONE place (§2n.7), and — the revision-3 lesson applied to a second
# check — the name's two DOCUMENTED appearances are exempt by site, not by file: `nodes.rs`'s
# `URL_ENV` const, which is the only reader, and `agent.rs`'s `capabilities.env` row, which §2n.7
# requires. A grep that fired on the documentation would be a grep whose first fix is to delete the
# documentation.
forbid_except "FMONERO_NODES_URL is read only in nodes.rs" 'FMONERO_NODES_URL' \
  '(fmonero-core/src/nodes\.rs):|agent\.rs:[0-9]+:[[:space:]]*name: "FMONERO_NODES_URL"' \
  "${SHIPPED[@]}"
selftest "a second reader of the seam" 'FMONERO_NODES_URL' 'std::env::var("FMONERO_NODES_URL")'
# And the exemption is by SITE: a real read planted in agent.rs is still a hit.
SEAM_PROBE="$SELFTEST/seam-exempt"
mkdir -p "$SEAM_PROBE"
printf 'let base = std::env::var("FMONERO_NODES_URL").unwrap();\n' > "$SEAM_PROBE/agent.rs"
if grep -rnE 'FMONERO_NODES_URL' "$SEAM_PROBE" 2>/dev/null \
   | grep -vE '(fmonero-core/src/nodes\.rs):|agent\.rs:[0-9]+:[[:space:]]*name: "FMONERO_NODES_URL"' \
   | grep -q .; then
  pass "self-test: the agent.rs exemption does not cover a read"
else
  fail "self-test: the agent.rs exemption is too wide — a planted read there would pass"
fi
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

# ---------------------------------------------------------------- §2o plain language
#
# "Every string a person can read follows this section" (DESIGN.md §2o, v5.6). The rule that can
# be mechanised is rule 7's word list, and this is the grep for it, over the **string literals**
# of the four surfaces a person reads: the TUI, `output.rs`, the warning texts in `status.rs` and
# the bar widget. `--json` is exempt by contract and so are the ids in it, which is why a literal
# that is a single bare token is skipped: prose has spaces in it, and `"wallet_idle"` is a
# contract key, not a sentence.
step "§2o  no banned word in a user-facing string"
# Comments are stripped, then the file is folded into ONE line before the literals are pulled out,
# so a Rust literal broken across lines with a trailing `\` is examined whole rather than slipping
# between two greps. Its one known blind spot, stated rather than hidden: a `//` INSIDE a literal
# on a continuation line is read as the start of a comment, so anything after it on that line is
# not examined.
literals_2o() {
  # `#[cfg(test)]` and everything after it is not a user-facing surface: a test that asserts the
  # OLD words are gone has to name them, and a grep that fires on its own assertions is a grep
  # nobody can keep. These files put their test module last, which is what makes the cut safe.
  #
  # **X2 adds one more cut, and it is the only one that is not about tests: `copy-exempt`.** Two of
  # the seven files the round added format bytes that are not prose at all — the `.service` and
  # `monerod.conf` templates `units.rs` renders, which MUST contain `[Unit]`, `systemd`, `monerod`
  # and `--daemon-address` because that is what the programs reading them parse. A region between
  # `// copy-exempt-begin: <reason>` and `// copy-exempt-end` is skipped, its reason is printed by
  # the step below so every one of them is visible in the gate's own output, and a region with no
  # reason after the colon fails. The lines are blanked rather than deleted so a literal that spans
  # them cannot leave an unbalanced quote behind.
  sed '/^#\[cfg(test)\]/q' "$1" | awk '
    /copy-exempt-begin/ { skip = 1; print ""; next }
    /copy-exempt-end/   { skip = 0; print ""; next }
    skip                { print ""; next }
    { print }
  ' | awk '
    {
      out = ""; inq = 0
      for (i = 1; i <= length($0); i++) {
        c = substr($0, i, 1)
        if (inq) {
          if (c == "\\") { out = out c substr($0, i + 1, 1); i++; continue }
          if (c == "\"") inq = 0
          out = out c
          continue
        }
        if (c == "\"") { inq = 1; out = out c; continue }
        if (c == "/" && substr($0, i + 1, 1) == "/") break
        out = out c
      }
      print out
    }
  ' | tr '\n' ' ' | grep -oE '"([^"\\]|\\.)*"'
}
# §2o rule 7's list, plus `§` (rule 4) and the two multi-word phrases §2o.2 retires.
#
# **v5.6 closes the two holes V17 demonstrated.** (1) The word list omitted four of rule 7's own
# words — `argv`, `breaker`, `snapshot` and `build` — and each is added with word boundaries, so
# `rebuild` and `building` in a legitimate sentence still pass while "the build" does not.
# (2) `monerod ` and `monero-wallet-rpc ` were on the COMMAND allow-list, which exempted the two
# worst prose strings in the program — `monerod did not answer get_info within its deadline` read
# as a runnable command to this grep. Both names are now BANNED words, and the allow-list keeps
# only the shape a real command has: the binary followed immediately by a flag,
# `^"monerod --…` / `^"monero-wallet-rpc --…`, which is what the doctor's two `--version` fix lines
# are and what prose never is (`monerod did not answer …` has a verb in that position). §2o rule 6
# in one regex: an implementation name is allowed inside a command the user runs and nowhere else.
# The self-test below plants one literal per hole, so the closure is demonstrated, not asserted.
# **v5.6.2 adds `txid`** (W20's neighbour, found by reading): §2o.2 says say `id`, and the Wallet
# screen's filter hint read `filter by txid, address or note` through the whole v5.6 round because
# the word was in the vocabulary table and not in this list.
BANNED_2O='(daemons?|units?|RPC|loopback|restricted|prob(e|es|ed|ing)|layouts?|ssl|SSL|systemd|systemctl|relay(ed|ing)?|broadcast|mutator|nettype|txpool|mempool|subaddress|subaddr|priority|in flight|high water|txid|monerod|monero-wallet-rpc|\bargv\b|\bbreakers?\b|\bsnapshots?\b|\b(build|builds|built|building)\b|§)'
# Two exemptions, and each is the reason the grep is honest rather than decorative:
#   * a **runnable command** is a command on purpose — §2o.1's four exceptions and every CLI hint;
#   * a **single bare token** is an id, a key, a JSON value or an argv element, never prose — and
#     v5.6 extends that to `--flag=value`, one whole argv element too, because the widget's
#     `--app-id=fmonerodoctor` contains the string `monerod` and is not a sentence anybody reads.
# **v5.6.3 widens the bare-token shape by one character: a leading `/`.** `"/etc/monerod.conf"` is
# an absolute path, which is the same thing as a bare token and not a sentence anybody reads.
ALLOWED_2O='("(fmonero|omarchy|sudo|monero-wallet-cli|systemctl|which|wl-copy|chmod|df|timedatectl|cd)[ "]|^"(monerod|monero-wallet-rpc) --[A-Za-z0-9-]|In a terminal: |^"/?[A-Za-z0-9][A-Za-z0-9_.:@/+-]*"$|^"--[A-Za-z0-9-]+(=[A-Za-z0-9_.:@/+-]*)?"$)'
# §2o's first paragraph governs "every string a person can read", and v5.5's step read four
# surfaces. v5.6 adds the three V8/V9/V10 found outside them, each one command away from the owner:
# the shared RPC error family (`rpc/jsonrpc.rs`), the settings descriptions `settings list` prints
# (`settings.rs`), and the doctor's forty details and fix lines (`doctor/checks.rs`).
#
# **X2 adds seven more, and they are the reason this list is now "every file that formats a string
# a person can see" rather than a list somebody maintains.** The round-3 verification read `node
# list`, `wallet open` and `settings set` on a real terminal and found seven live violations in
# files this grep had never opened: `main.rs` (the whole command surface, its `--help` prose and a
# dozen runtime refusals), `session.rs` (what `node up` says when it times out), `rpc/mod.rs` (the
# loopback refusal, whose hint carried three banned words in a trailing comment — the exact hole
# v5.6.1 documented fixing, in a file the grep did not read), `rpc/wallet.rs` (two failures on the
# money path), `send.rs` (the line printed immediately before the money moves), `units.rs` and
# `nodes.rs`.
COPY_SURFACES=()
for f in "$BIN"/tui/mod.rs "$BIN"/tui/screens.rs "$BIN"/tui/send.rs "$BIN"/tui/subs.rs \
         "$BIN"/tui/theme.rs "$BIN"/tui/hit.rs "$BIN"/output.rs "$BIN"/doctor/checks.rs \
         "$BIN"/main.rs \
         "$CORE"/status.rs "$CORE"/settings.rs "$CORE"/rpc/jsonrpc.rs "$CORE"/rpc/mod.rs \
         "$CORE"/rpc/wallet.rs "$CORE"/session.rs "$CORE"/send.rs "$CORE"/units.rs \
         "$CORE"/nodes.rs \
         plugin/oliver.monero/Panel.qml plugin/oliver.monero/Service.qml \
         plugin/oliver.monero/MoneroIcon.qml plugin/oliver.monero/Model.js; do
  [ -f "$f" ] && COPY_SURFACES+=("$f")
done
# Every `copy-exempt` region in the tree, printed, so a skipped region is visible in the gate's
# output rather than only in a diff. A region whose marker carries no reason after the colon is a
# failure: an exemption nobody has to justify is an exemption that spreads.
EXEMPT_LINES="$(grep -rn 'copy-exempt-begin' "${COPY_SURFACES[@]}" 2>/dev/null || true)"
EXEMPT_BAD="$(printf '%s\n' "$EXEMPT_LINES" | grep -v 'copy-exempt-begin: *[^ ]' || true)"
if [ -n "${EXEMPT_LINES// /}" ]; then
  printf '        copy-exempt regions (file content, never prose):\n'
  printf '%s\n' "$EXEMPT_LINES" | sed 's/^/          /'
fi
if [ -n "${EXEMPT_BAD// /}" ]; then
  fail "§2o: a copy-exempt region carries no reason"
  printf '%s\n' "$EXEMPT_BAD" | sed 's/^/        /'
fi
# **A `{field}` is a substitution, not prose** (v5.6.2): the name inside the braces is a Rust
# variable and no reader ever sees it, so `"… · id {txid} · …"` is a sentence that says `id`. The
# names are stripped before the word match; the text around them is checked exactly as before.
strip_fields() { printf '%s' "$1" | sed 's/{[^}]*}/{}/g'; }
# **Three more things a reader never reads as prose** (X2), each narrow enough to name:
#   * a `--flag` is an element of a command the user types (rule 6's own exception), so
#     `--probe`, `--pool` and `--priority` in `--help` are not the words rule 7 bans;
#   * an `FMONERO_*` environment variable is a name the caller sets — rule 5's carve-out for a
#     settings key, in the other namespace;
#   * the four settings keys whose own names contain a banned word. Rule 5 carves them out **by
#     name**, so they are listed by name here rather than matched by a shape that would also
#     swallow `monero-wallet-rpc.service`.
strip_flags() { printf '%s' "$1" | sed 's/--[A-Za-z0-9][A-Za-z0-9-]*/--/g'; }
strip_envs() { printf '%s' "$1" | sed 's/FMONERO_[A-Z0-9_]*/ENV/g'; }
strip_keys() {
  printf '%s' "$1" | sed -e 's/node\.unit/KEY/g' -e 's/wallet\.unit/KEY/g' \
    -e 's/node\.remote_probe_sec/KEY/g' -e 's/timeouts\.transfer_sec/KEY/g'
}
strip_all() { strip_keys "$(strip_envs "$(strip_flags "$(strip_fields "$1")")")"; }
# The §5c `setup.actions[].what` values, exempt **by name**. They are payload values an agent
# branches on — `tests/e2e.sh` selects rows with `select(.what == "wallet unit")` — and `setup`'s
# human rendering puts them through `units::action_words`, which is where the sentence a person
# reads comes from. Four literals, listed, so a fifth has to be added deliberately.
ALLOWED_LABELS='^"(node unit|wallet unit|wallet set-daemon|CreateDir wallet\.dir)"$'
HITS_2O=""
for f in "${COPY_SURFACES[@]}"; do
  while IFS= read -r lit; do
    [ -n "$lit" ] || continue
    printf '%s\n' "$lit" | grep -qE "$ALLOWED_LABELS" && continue
    # **The allow-list reads the literal as written; the strippers only feed the banned match.**
    # `--version` and `--app-id=fmonerodoctor` ARE the shapes the allow-list recognises, so
    # stripping their flags first turned `"monerod --version"` into `"monerod --"` and un-exempted
    # the doctor's own two fix lines.
    raw="$lit"
    lit="$(strip_all "$lit")"
    if printf '%s\n' "$raw" | grep -qE "$ALLOWED_2O"; then
      # **An allow-listed command's trailing comment is still prose** (v5.6.1). `fmonero …   # …`
      # and `sudo …   # …` are printed whole, comment included, and the allow-list exempted the
      # whole literal — which is how `# a restricted node is not ours to reconfigure` sat in a
      # doctor fix line through the v5.6 round. The command half is exempt; the half after the
      # `#` is checked like any other sentence.
      case "$lit" in
        *'#'*) tail_2o="${lit#*#}" ;;
        *) continue ;;
      esac
      printf '%s\n' "$tail_2o" | grep -qE "$BANNED_2O" || continue
      HITS_2O="$HITS_2O$f: (comment of a command) #$tail_2o"$'\n'
      continue
    fi
    printf '%s\n' "$lit" | grep -qE "$BANNED_2O" || continue
    HITS_2O="$HITS_2O$f: $lit"$'\n'
  done < <(literals_2o "$f")
done
if [ -n "$HITS_2O" ]; then
  fail "§2o rule 7: a banned word reaches a user-facing string"
  printf '%s' "$HITS_2O" | sed 's/^/        /'
else
  pass "§2o rule 7: no banned word in the TUI, the CLI, the doctor, core or the widget (${#COPY_SURFACES[@]} files)"
fi
# ---------------------------------------------------------------- S6: the raw OS error
#
# **The durable half of round 4's S6, and it closes the blind spot X1 had.** `{e}` is a runtime
# substitution: the banned-word grep above sees `"cannot read {}: {e}"`, finds no banned word in it
# and passes — while what a person read was `cannot read /path: Permission denied (os error 13)`.
# Thirty-odd sites had that shape, plus `impl From<std::io::Error>`, which put the operating
# system's own words in the MESSAGE slot that `Error::with_detail`'s own comment says they must
# never reach.
#
# The rule is a convention a grep can check: **a binding named `e`, `err` or `error` holds technical
# text, and technical text never appears in a string a person reads.** `Error::io` / `io_words` build
# the sentence and `with_detail(e.to_string())` carries the rest. Where what is interpolated IS
# already a plain sentence — another `Error`'s own message, a probe's recorded text — the binding is
# named `why`, which costs one word and makes the grep exact.
#
# Two exemptions, both narrow and both on the line itself: `with_detail(` is the slot the technical
# text belongs in, and `RpcError::` is the transport carrier whose every value `jsonrpc.rs` renders
# into a `detail` (one `match`, three arms, all of them `with_detail`).
step "S6  no operating-system error text in a string a person reads"
OS_SUBST='"[^"]*\{(e|err|error)(:[^}]*)?\}'
OS_EXEMPT='with_detail\(|RpcError::'
HITS_OS=""
for f in "${COPY_SURFACES[@]}"; do
  case "$f" in *.rs) ;; *) continue ;; esac
  found="$(sed '/^#\[cfg(test)\]/q' "$f" | grep -n '"' | grep -vE "$OS_EXEMPT" | grep -E "$OS_SUBST" || true)"
  [ -z "$found" ] && continue
  HITS_OS="$HITS_OS$(printf '%s\n' "$found" | sed "s|^|$f:|")"$'\n'
done
if [ -n "${HITS_OS//[$'\n' ]/}" ]; then
  fail "S6: a {e} substitution reaches a string a person reads (use Error::io / with_detail, or name the binding why)"
  printf '%s' "$HITS_OS" | sed 's/^/        /'
else
  pass "S6: no {e} substitution in a user-facing string (${#COPY_SURFACES[@]} files)"
fi
# Its self-test: the shape that must fire, and the two that must not.
SELF_OS="$SELFTEST/os-error"
mkdir -p "$SELF_OS"
{
  printf '%s\n' 'let a = Error::failure(format!("cannot read {}: {e}", dir.display()));'
  printf '%s\n' 'let b = Error::failure("fmonero could not read it.").with_detail(format!("{e}"));'
  printf '%s\n' 'let c = format!("{why} fmonero asked it on {}.", url);'
} > "$SELF_OS/probe.rs"
OS_FIRED="$(grep -n '"' "$SELF_OS/probe.rs" | grep -vE "$OS_EXEMPT" | grep -cE "$OS_SUBST" || true)"
if [ "$OS_FIRED" = 1 ]; then
  pass "self-test: S6 fires on the {e}-in-a-message line and on neither the with_detail nor the {why} one"
else
  fail "self-test: S6 matched $OS_FIRED of the three planted lines; exactly the first must match"
fi

# ---------------------------------------------------------------- §2o rule 12, the em-dash
#
# **W4 mechanises rule 12**, which `docs/COPY.md` §7 said was "enforced by reading" — and reading
# missed four, three of them strings that round had just written. An em-dash is prose punctuation
# and §2o has none; the one use the design keeps is the **`—` that stands for an unknown value**
# ("The `—` for an unknown value, everywhere", COPY §4), which is a cell glyph and not punctuation.
# So a literal passes when every em-dash in it is that glyph: the literal is the glyph alone
# (padding aside), or the glyph is the first thing in it and there is only one. Everything else —
# ` — ` inside a sentence, a modal title's ` — `, a row that joins two values with one — fires.
HITS_DASH=""
for f in "${COPY_SURFACES[@]}"; do
  while IFS= read -r lit; do
    case "$lit" in
      *—*) ;;
      *) continue ;;
    esac
    # The literal's content, without the quotes.
    body="${lit%\"}"; body="${body#\"}"
    stripped="$(printf '%s' "$body" | tr -d ' ')"
    lead="$(printf '%s' "$body" | sed 's/^ *//')"
    count="$(printf '%s' "$body" | grep -o '—' | wc -l)"
    if [ "$stripped" = "—" ]; then continue; fi
    case "$lead" in
      —*) [ "$count" = 1 ] && continue ;;
    esac
    HITS_DASH="$HITS_DASH$f: $lit"$'\n'
  done < <(literals_2o "$f")
done
if [ -n "$HITS_DASH" ]; then
  fail "§2o rule 12: an em-dash is in a user-facing string"
  printf '%s' "$HITS_DASH" | sed 's/^/        /'
else
  pass "§2o rule 12: no em-dash in prose, in any of the ${#COPY_SURFACES[@]} surfaces"
fi
# Its self-test: the three shapes that must fire and the two that must not.
SELF_DASH="$SELFTEST/em-dash"
mkdir -p "$SELF_DASH"
{
  printf '%s\n' 'let a = "Switching will not start it — press u after this.";'
  printf '%s\n' 'let b = " Open a wallet — Enter opens · Esc cancels ";'
  printf '%s\n' 'let c = "block 3 541 122 — checked 2s ago";'
} > "$SELF_DASH/bad.rs"
{
  printf '%s\n' 'let d = "—";'
  printf '%s\n' 'let e = "—           ";'
  printf '%s\n' 'let f = "— this node does not answer that";'
} > "$SELF_DASH/good.rs"
DASH_FIRED=0
DASH_WRONG=0
for kind in bad good; do
  while IFS= read -r lit; do
    case "$lit" in *—*) ;; *) continue ;; esac
    body="${lit%\"}"; body="${body#\"}"
    stripped="$(printf '%s' "$body" | tr -d ' ')"
    lead="$(printf '%s' "$body" | sed 's/^ *//')"
    count="$(printf '%s' "$body" | grep -o '—' | wc -l)"
    ok_lit=0
    [ "$stripped" = "—" ] && ok_lit=1
    case "$lead" in —*) [ "$count" = 1 ] && ok_lit=1 ;; esac
    if [ "$kind" = bad ] && [ "$ok_lit" = 0 ]; then DASH_FIRED=$((DASH_FIRED + 1)); fi
    if [ "$kind" = good ] && [ "$ok_lit" = 0 ]; then DASH_WRONG=$((DASH_WRONG + 1)); fi
  done < <(literals_2o "$SELF_DASH/$kind.rs")
done
if [ "$DASH_FIRED" = 3 ] && [ "$DASH_WRONG" = 0 ]; then
  pass "self-test: §2o rule 12 fires on all three planted em-dashes and on none of the three glyphs"
else
  fail "self-test: rule 12 caught $DASH_FIRED of 3 planted dashes and rejected $DASH_WRONG of 3 glyphs"
fi

# The self-test §2o asks for by name: the planted literal must fire, and the allowed command
# must not. A grep that cannot be shown to fire is a grep the first agent to hit it relaxes.
SELF_2O="$SELFTEST/plain-language"
mkdir -p "$SELF_2O"
printf '%s\n' 'let s = "the wallet daemon is not running";' > "$SELF_2O/bad.rs"
printf '%s\n' 'let s = "In a terminal: omarchy pkg add monero";' > "$SELF_2O/good.rs"
# v5.6, V17: the two literals the OLD allow-list exempted, and the four words the old list omitted.
# Each planted line must fire; a grep whose closure is asserted rather than shown is a grep the
# next agent re-opens.
{
  printf '%s\n' 'let a = "monero-wallet-rpc is not answering: io: Connection refused (os error 111)";'
  printf '%s\n' 'let b = "monerod did not answer get_info within its deadline";'
  printf '%s\n' 'let c = "the argv carries a breaker snapshot and one build";'
} > "$SELF_2O/holes.rs"
# And the field-name rule, both ways: the name inside the braces is invisible, the prose is not.
printf '%s\n' 'let s = "the payment id {txid} has gone out";' > "$SELF_2O/field.rs"
printf '%s\n' 'let s = "the daemon {name} has gone out";' > "$SELF_2O/field-bad.rs"
FIELD_OK="$(strip_fields "$(literals_2o "$SELF_2O/field.rs")")"
FIELD_BAD="$(strip_fields "$(literals_2o "$SELF_2O/field-bad.rs")")"
if ! printf '%s\n' "$FIELD_OK" | grep -qE "$BANNED_2O" &&
   printf '%s\n' "$FIELD_BAD" | grep -qE "$BANNED_2O"; then
  pass "self-test: §2o reads a {field} as a substitution and the prose around it as prose"
else
  fail "self-test: the {field} rule hides prose, or fires on a field name"
fi
BAD_LIT="$(literals_2o "$SELF_2O/bad.rs")"
GOOD_LIT="$(literals_2o "$SELF_2O/good.rs")"
HOLES=0
while IFS= read -r lit; do
  [ -n "$lit" ] || continue
  if printf '%s\n' "$lit" | grep -qE "$ALLOWED_2O"; then continue; fi
  printf '%s\n' "$lit" | grep -qE "$BANNED_2O" && HOLES=$((HOLES + 1))
done < <(literals_2o "$SELF_2O/holes.rs")
if [ "$HOLES" = 3 ]; then
  pass "self-test: §2o fires on all three of V17's holes (two binary names, four missing words)"
else
  fail "self-test: §2o caught $HOLES of V17's 3 planted literals — the holes are still open"
fi
if printf '%s\n' "$BAD_LIT" | grep -qE "$BANNED_2O" &&
   ! printf '%s\n' "$BAD_LIT" | grep -qE "$ALLOWED_2O"; then
  pass "self-test: §2o fires on a planted \"the wallet daemon is not running\""
else
  fail "self-test: §2o does NOT fire on its own fixture — the check above proves nothing"
fi
if printf '%s\n' "$GOOD_LIT" | grep -qE "$ALLOWED_2O"; then
  pass "self-test: §2o exempts \"In a terminal: omarchy pkg add monero\""
else
  fail "self-test: §2o would reject its own documented exception"
fi
# **X2's own four self-tests**, one per mechanism the round added, each planted so its closure is
# shown rather than asserted.
#
# 1. the `copy-exempt` region: a banned literal inside one is skipped, the same literal outside one
#    fires. Without both halves the mechanism is a blanket nobody can see the edge of.
SELF_EX="$SELFTEST/copy-exempt"
mkdir -p "$SELF_EX"
{
  printf '%s\n' '// copy-exempt-begin: file content, not prose'
  printf '%s\n' 'let unit = "[Unit]\nDescription=Monero daemon (fmonero)\n";'
  printf '%s\n' '// copy-exempt-end'
  printf '%s\n' 'let s = "the wallet daemon is not running";'
} > "$SELF_EX/mixed.rs"
EX_HITS=0
while IFS= read -r lit; do
  [ -n "$lit" ] || continue
  printf '%s\n' "$(strip_all "$lit")" | grep -qE "$BANNED_2O" && EX_HITS=$((EX_HITS + 1))
done < <(literals_2o "$SELF_EX/mixed.rs")
if [ "$EX_HITS" = 1 ]; then
  pass "self-test: copy-exempt hides the unit template and hides nothing else"
else
  fail "self-test: copy-exempt saw $EX_HITS banned literals where exactly 1 is outside the region"
fi
# 2. the flag stripper, both ways: `--probe` is an element of a command, `probing` is prose.
printf '%s\n' 'let s = "--probe then checks the top --limit candidates from here";' > "$SELF_EX/flag.rs"
printf '%s\n' 'let s = "probing up to 20 nodes";' > "$SELF_EX/flag-bad.rs"
if ! printf '%s\n' "$(strip_all "$(literals_2o "$SELF_EX/flag.rs")")" | grep -qE "$BANNED_2O" &&
   printf '%s\n' "$(strip_all "$(literals_2o "$SELF_EX/flag-bad.rs")")" | grep -qE "$BANNED_2O"; then
  pass "self-test: §2o reads --probe as a flag and 'probing' as prose"
else
  fail "self-test: the --flag rule hides prose, or fires on a flag the user types"
fi
# 3. the settings-key and environment-variable carve-outs, and their prose neighbours.
printf '%s\n' 'let s = "older than node.remote_probe_sec, and FMONERO_SYSTEMCTL_FIXTURE is set";' \
  > "$SELF_EX/key.rs"
printf '%s\n' 'let s = "older than the last probe, and systemctl was called";' > "$SELF_EX/key-bad.rs"
if ! printf '%s\n' "$(strip_all "$(literals_2o "$SELF_EX/key.rs")")" | grep -qE "$BANNED_2O" &&
   printf '%s\n' "$(strip_all "$(literals_2o "$SELF_EX/key-bad.rs")")" | grep -qE "$BANNED_2O"; then
  pass "self-test: §2o exempts a settings key and an FMONERO_ variable by name, and nothing else"
else
  fail "self-test: the key/environment carve-out hides prose, or fires on a name the user types"
fi
# 4. the §5c label exemption is exactly four literals wide: `"node unit"` passes, a sentence that
#    merely contains it does not.
printf '%s\n' 'let a = "node unit"; let b = "the node unit is not running";' > "$SELF_EX/label.rs"
LABEL_HITS=0
while IFS= read -r lit; do
  [ -n "$lit" ] || continue
  printf '%s\n' "$lit" | grep -qE "$ALLOWED_LABELS" && continue
  printf '%s\n' "$(strip_all "$lit")" | grep -qE "$BANNED_2O" && LABEL_HITS=$((LABEL_HITS + 1))
done < <(literals_2o "$SELF_EX/label.rs")
if [ "$LABEL_HITS" = 1 ]; then
  pass "self-test: the §5c label exemption covers the value and not the sentence around it"
else
  fail "self-test: the label exemption matched $LABEL_HITS literals where exactly 1 must fire"
fi

# v5.6.1: and the comment half of an allow-listed command is not exempt with it.
printf '%s\n' 'let s = "fmonero doctor   # a restricted node is not ours to reconfigure";' \
  > "$SELF_2O/comment.rs"
COMMENT_LIT="$(literals_2o "$SELF_2O/comment.rs")"
COMMENT_TAIL="${COMMENT_LIT#*#}"
if printf '%s\n' "$COMMENT_LIT" | grep -qE "$ALLOWED_2O" &&
   printf '%s\n' "$COMMENT_TAIL" | grep -qE "$BANNED_2O"; then
  pass "self-test: §2o checks the comment of an allow-listed command"
else
  fail "self-test: §2o exempts a command's comment as well as its command"
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
# v5.6: 7600, and §7a says so before the code was written — the plain-language round's own
# arithmetic is the ` This node ` pane, the `Context` reload and its three cases, and the fee-level
# helpers (was 7200).
# v5.6.3: 8600, itemised in §7a (was 8300) — ten items before the code was written and four more
# from reading the round's own captures, which is the step the whole effort exists for. Four of the
# fourteen items
# DELETE code, and the two largest are the two that stop a frame saying one thing six times.
# v5.6.2: 8300, itemised in §7a before the code was written (was 7900). Three of the nine items
# are **room** rather than text — the toast's own row, the picker footer's wrap, the `?` overlay's
# — because a sentence cut to fit is a sentence the reader does not have.
# v5.6.4: **9200**, itemised in §7a before this line moved (was 8600) — twenty-three items from a
# fresh reader's round (+541, measured 9 076) and nine more from this round's own read of its own
# captures (+74, measured 9 150). Seven of those nine are ROOM rather than text: a sentence cut to
# fit is a sentence the reader does not have, and four of the nine were exactly that.
TUI_BUDGET=9200
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
# v5.6: 1529, and §6.1 writes the derivation out — the four per-file budgets (340 + 820 + 112 +
# 225 = 1 497) plus manifest.json's 32 real lines, which this gate counts.
# v5.6.1: **1561**, and the arithmetic is in §6.1 before the code was written — the four per-file
# budgets (340 + 845 + 112 + 232 = 1 529) plus the same 32. The two items are V1's version token,
# which is what lets this suite tell which QML the shell is running, and V16's short balance field.
# v5.6.3: **1604**, itemised in §6.1 before the code was written — 340 + 850 + 112 + 270 = 1 572 plus
# the same 32. `Panel.qml` +10 for X8 (`14 transactions` on the ` Waiting ` row) and `Model.js` +38
# for X6 (one decimal byte formatter), X11 (one rounding, down), X13 (the public node's verdict on the
# hero) and the notes for X10 and X30.
# v5.6.4: **1649**, itemised in §6.1 before this line moved — 360 + 850 + 112 + 295 = 1 617 plus
# the same 32. `Service.qml` +12 for S8 (an exit code and the program's own stderr at a person) and
# M26 (the in-app `action` a failed button never showed); `Model.js` +21 for M2 (one duration shape),
# M27 (a row starting with a bare separator, and a header with no verdict) and M28 (the peers row
# blaming a node that is off).
QML_BUDGET=1649
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
360 Service.qml
850 Panel.qml
112 MoneroIcon.qml
295 Model.js
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
                | tr ',{}' '\n\n\n' | grep -oE '"FMONERO_[A-Z_]+"|"NO_COLOR"|"CI"|"COLUMNS"' \
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
