#!/usr/bin/env bash
# Smoke test for the Omarchy bar widget `oliver.monero` (DESIGN.md §6.7). It runs against this
# machine: it validates the repository copy of the plugin, checks the manifest and the QML for the
# traps §6 names, then asks the running shell to reload its plugins and reads the journal for a
# load failure.
#
# Every forbidding grep is paired with a **positive fixture** it must fire on. A grep that cannot
# fail is worse than no grep, because §6.7 cites it as the mitigation and a later reader takes that
# as coverage — the same rule scripts/check.sh applies to the Rust half.
#
# Exits non-zero on the first failing check. Under ~10 s.
#
#   tests/plugin-smoke.sh            # full run (needs a running omarchy-shell)
#   tests/plugin-smoke.sh --static   # skip the reload + journal + IPC half

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN="$REPO/plugin/oliver.monero"
ID="oliver.monero"
STATIC_ONLY=0
case "${1:-}" in
  --static) STATIC_ONLY=1 ;;
  -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
  "") ;;
  *) printf 'plugin-smoke: unknown argument %s\n' "$1" >&2; exit 2 ;;
esac

PASS=0
FAIL=0
SELFTEST="$(mktemp -d)"
trap 'rm -rf "$SELFTEST"' EXIT

ok()  { printf '  ok    %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '  FAIL  %s\n' "$1"; FAIL=$((FAIL + 1)); }
skip(){ printf '  skip  %s\n' "$1"; }
die() { printf 'plugin-smoke: %s\n' "$1" >&2; exit 2; }

command -v jq >/dev/null || die "jq is required"
[[ -d $PLUGIN ]] || die "plugin directory not found: $PLUGIN"

# `forbid <name> <extended-regexp>` fails when the pattern appears in any QML or JS file of the
# plugin. `fires <name> <pattern> <content>` proves the same pattern matches a file that contains
# the forbidden thing; `silent` proves it does NOT match a file that merely looks similar.
forbid() {
  local name="$1" pattern="$2" hits
  hits="$(grep -rnE "$pattern" "$PLUGIN"/*.qml "$PLUGIN"/*.js 2>/dev/null)"
  if [[ -n $hits ]]; then
    bad "$name"
    printf '%s\n' "$hits" | sed 's/^/        /'
  else
    ok "$name"
  fi
}
# The other half of `forbid`: a line that MUST be there. Used by the §2m structural checks,
# where the property being asserted is that a branch exists and is reachable.
present() {
  local name="$1" file="$2" pattern="$3"
  if grep -qF "$pattern" "$PLUGIN/$file" 2>/dev/null; then
    ok "$name"
  else
    bad "$name" "$PLUGIN/$file does not contain: $pattern"
  fi
}
# The other half of `present`: a pattern that must NOT be there. One string a round deleted comes
# back the moment nothing asserts its absence (round 4 iteration 3).
absent() {
  local name="$1" file="$2" pattern="$3"
  if grep -qF "$pattern" "$PLUGIN/$file" 2>/dev/null; then
    bad "$name" "$PLUGIN/$file still contains: $pattern"
  else
    ok "$name"
  fi
}
fires() {
  local name="$1" pattern="$2" content="$3" dir="$SELFTEST/$RANDOM$RANDOM"
  mkdir -p "$dir"
  printf '%s\n' "$content" > "$dir/fixture.qml"
  if grep -rqE "$pattern" "$dir" 2>/dev/null; then
    ok "self-test: $name fires on its own fixture"
  else
    bad "self-test: $name does NOT fire — the check above proves nothing"
  fi
}
silent() {
  local name="$1" pattern="$2" content="$3" dir="$SELFTEST/$RANDOM$RANDOM"
  mkdir -p "$dir"
  printf '%s\n' "$content" > "$dir/fixture.qml"
  if grep -rqE "$pattern" "$dir" 2>/dev/null; then
    bad "self-test: $name fires on correct code — it would be relaxed the first time it hit"
  else
    ok "self-test: $name leaves its negative fixture alone"
  fi
}

echo "plugin-smoke: $PLUGIN"

# --- 1. the validator the shell itself mirrors --------------------------------
# It refuses symlinks, so this must be the repository directory and never the link installed under
# ~/.config/omarchy/plugins (DESIGN.md §1.5).
if command -v omarchy-plugin-validate >/dev/null; then
  if out=$(omarchy-plugin-validate "$PLUGIN" 2>&1); then
    ok "omarchy-plugin-validate"
  else
    bad "omarchy-plugin-validate: $out"
  fi
else
  bad "omarchy-plugin-validate not on PATH"
fi

# --- 2. manifest --------------------------------------------------------------
M="$PLUGIN/manifest.json"
check_jq() { # <description> <filter>
  if jq -e "$2" "$M" >/dev/null 2>&1; then ok "$1"; else bad "$1"; fi
}
check_jq "manifest id is $ID"                 ".id == \"$ID\""
check_jq "schemaVersion is 1"                 '.schemaVersion == 1'
check_jq "kinds contains bar-widget"          '(.kinds | index("bar-widget")) != null'
check_jq "entryPoints.barWidget = Panel.qml"  '.entryPoints.barWidget == "Panel.qml"'
check_jq "defaultSection = right"             '.barWidget.defaultSection == "right"'
check_jq "allowMultiple = false"              '.barWidget.allowMultiple == false'

# --- 3. files -----------------------------------------------------------------
for f in manifest.json Panel.qml Service.qml MoneroIcon.qml Model.js; do
  if [[ -f $PLUGIN/$f ]]; then ok "$f exists"; else bad "$f is missing"; fi
done
# README.md belongs to the docs agent (DESIGN.md §6.1), which lands on a different branch. It is
# not loaded by the shell, so its absence is reported rather than failed.
if [[ -f $PLUGIN/README.md ]]; then
  ok "README.md exists"
else
  skip "README.md is not here yet (owned by the docs agent, DESIGN.md §6.1)"
fi

# --- 4. QML syntax ------------------------------------------------------------
# `omarchy-shell shell rescanPlugins` does not re-read the QML of an already loaded symlinked
# plugin, so a syntax check here is what actually catches a broken edit; the journal and IPC checks
# below only catch a plugin that fails on a cold load.
QMLLINT="$(command -v qmllint 2>/dev/null || true)"
[[ -n $QMLLINT ]] || QMLLINT=/usr/lib/qt6/bin/qmllint
if [[ -x $QMLLINT ]]; then
  # Syntax only: qmllint cannot resolve qs.Ui / qs.Commons outside the shell's import path, and
  # those import warnings are expected noise.
  syntax=$("$QMLLINT" "$PLUGIN"/*.qml "$PLUGIN"/*.js 2>&1 | grep -E 'Syntax error|\[syntax\]' || true)
  if [[ -z $syntax ]]; then ok "qmllint: no syntax errors"; else bad "qmllint: $syntax"; fi
else
  skip "qmllint is not installed (looked on PATH and in /usr/lib/qt6/bin) — QML syntax unchecked"
fi

# --- 5. the §6 traps ----------------------------------------------------------
if grep -q 'manageIpc: false' "$PLUGIN/Panel.qml"; then
  ok "Panel.qml sets manageIpc: false"
else
  bad "Panel.qml must set manageIpc: false (it declares its own IpcHandler)"
fi

# `bar.run(cmd)` is `bash -lc <string>`; `execDetached(["bash", …])` is a shell string in argv
# clothing. Both are DESIGN.md §1.5/§2f.
SHELL_ARGV='execDetached\(\["(ba)?sh'
BAR_RUN='bar\.run\('
forbid "no execDetached([\"bash\" …])" "$SHELL_ARGV"
fires  "execDetached bash" "$SHELL_ARGV" 'Quickshell.execDetached(["bash", "-lc", "fmonero status"])'
forbid "no bar.run()" "$BAR_RUN"
fires  "bar.run" "$BAR_RUN" 'bar.run("fmonero node up")'

# Manifest defaults are never merged into `settings` (DESIGN.md §1.5), so every default key must be
# read through setting()/intSetting() with the same value duplicated in code.
missing_fallback=0
while IFS= read -r key; do
  grep -qE "(setting|intSetting)\(\"$key\"" "$PLUGIN"/*.qml && continue
  bad "manifest default '$key' has no in-code fallback"
  missing_fallback=1
done < <(jq -r '.barWidget.defaults | keys[]' "$M")
[[ $missing_fallback -eq 0 ]] && ok "every manifest default has an in-code fallback"

# --- 6. the money grep (DESIGN.md §6.7, §2b.5) --------------------------------
# No argv *element* may be one of the mutating wallet verbs, and no QML file may carry the word
# that names a wallet secret. The match is on a whole quoted argv element, not a substring, so the
# constant literal "--send" that opens the TUI's Send *screen* passes while a real
# ["fmonero","wallet","send"] cannot — a substring grep would fail on this design's own widget, and
# a grep that fails on correct code is a grep the first agent to hit it relaxes.
MONEY_ARGV='"(send|sweep|create|restore|transfer|relay)"'
forbid "no money argv element" "$MONEY_ARGV"
fires  "money argv" "$MONEY_ARGV" 'Quickshell.execDetached(["fmonero","wallet","send"])'
silent "money argv" "$MONEY_ARGV" 'Quickshell.execDetached(["fmonero","tui","--send"])'
# v5.5's two new buttons are the case a lazier substring grep would break, so both are asserted
# silent: `--wallet` and `--node` are FLAGS, and the forbidden elements are the bare words.
silent "money argv" "$MONEY_ARGV" 'Quickshell.execDetached(["fmonero","tui","--wallet"])'
silent "money argv" "$MONEY_ARGV" 'Quickshell.execDetached(["fmonero","tui","--node"])'

# v5.5, §6.3 and §6.7: **the widget reaches systemd only through fmonero.** There is deliberately
# no "start the wallet daemon" button, because fmonero has no `wallet up` — the unit is started by
# `units::ensure_running` as the first step of `wallet open`/`wallet create`, and a widget button
# would have to be `systemctl --user start …`: the widget reaching around fmonero, skipping §2i's
# preflight and reporting success for a unit that failed to start.
SYSTEMCTL='systemctl'
forbid "the widget never names systemctl" "$SYSTEMCTL"
fires  "a planted systemctl argv" "$SYSTEMCTL" 'Quickshell.execDetached(["systemctl","--user","start","fmonero-wallet-rpc.service"])'

# Owner-requested inline opening: a secret may exist transiently in a masked field and
# private input pipe, never in an environment variable, file, command, or diagnostic log.
SECRET='FMONERO_WALLET_PASSWORD|--password-file|console\.(log|warn|error).*secret'
forbid "no password environment, file, or logging path" "$SECRET"
fires "secret logging" "$SECRET" 'console.log(secret)'
present "wallet opens through a private input pipe" Service.qml '"--password-stdin"'
present "input is masked" Panel.qml 'password: true'
present "secret buffer is cleared after writing" Service.qml 'secret = ""'
present "power uses the guarded app command" Service.qml '["wallet", "stop", "--yes", "--json"]'

# --- 7. no arithmetic on money in QML (DESIGN.md §2a.5, the QML half) ---------
# Status JSON carries every amount as a pre-rendered `*_xmr` string, so the widget never needs a
# float. `grep -rn 'as f64'` covers the Rust half in scripts/check.sh.
FLOATS='parseFloat|Number\(|/ 1e'
forbid "no float arithmetic on money" "$FLOATS"
fires  "parseFloat" "$FLOATS" 'var x = parseFloat(status.pending.txs[0].amount_xmr)'
fires  "Number()"   "$FLOATS" 'var x = Number(status.wallet.balance_xmr) + 1'
fires  "pico / 1e12" "$FLOATS" 'var xmr = status.wallet.balance_pico / 1e12'


# --- 7c. the rendered rows, against REAL Status fixtures (§6.7) ---------------
#
# `Model.js` is `.pragma library` plus plain JavaScript — no QML imports, no I/O — so `node` can
# load it with that one line stripped and its formatters can be driven against a fixture document.
# That is the half the structural greps above cannot give: it proves what the panel will actually
# RENDER for a state the owner met, rather than which branch comes first.
if command -v node >/dev/null 2>&1; then
  MODEL_TMP="$(mktemp -d)"
  sed '/^\.pragma library/d' "$PLUGIN/Model.js" > "$MODEL_TMP/model.js"
  printf 'module.exports = { headerLine, checkedLabel, walletStateLabel, nodeLabel, worstSeverity, age, blocksLeft, walletSyncPct, grouped, pendingLabel, shortAddr, waitingLabel };\n' \
    >> "$MODEL_TMP/model.js"
  # Fixture 1: the owner's machine as of 2026-09-18 — a public node, no wallet open, `wallet_idle`,
  # and a populated `node.remote_probe`.
  cat > "$MODEL_TMP/closed.json" <<'JSON'
{ "ok": true, "schema": 1,
  "node": { "layout": "remote", "url": "xmr-node.cakewallet.com:18081", "trusted": false,
            "reachable": null, "synchronized": null, "sync_percent": null, "height": null,
            "remote_probe": { "reachable": true, "height": 3765172, "synchronized": true,
                              "restricted": true, "nettype": "mainnet", "version": null,
                              "latency_ms": 100, "reason": null,
                              "probed_at": 1789716001, "age_sec": 42 } },
  "wallet": { "unit_active": false, "open": false, "reachable": false, "name": null,
              "height": null, "balance_xmr": null },
  "pending": null,
  "warnings": [ { "id": "remote_node", "severity": "info", "text": "public node", "hint": "",
                  "action": "Press f on the Node screen to choose another." },
                { "id": "wallet_idle", "severity": "info",
                  "text": "No wallet is open. The wallet service starts when you open or create one.",
                  "hint": "fmonero wallet open <name>",
                  "action": "Press o to open a wallet, or c to create one." } ] }
JSON
  # Fixture 2 (v5.6, §6.7): **a public node with a wallet open and 240 000 blocks to go.** The two
  # heights are chosen so the percentage and the remainder are both exact — 408 648 of 648 648 is
  # 63 % with exactly 240 000 left — because a fixture whose own arithmetic nobody can check is a fixture
  # that proves the formatter agrees with itself.
  cat > "$MODEL_TMP/behind.json" <<'JSON'
{ "ok": true, "schema": 1,
  "node": { "layout": "remote", "url": "xmr-node.cakewallet.com:18081", "trusted": false,
            "reachable": null, "synchronized": null, "sync_percent": null, "height": null,
            "remote_probe": { "reachable": true, "height": 650000, "synchronized": true,
                              "restricted": false, "nettype": "mainnet", "version": null,
                              "latency_ms": 240, "reason": null,
                              "probed_at": 1789716001, "age_sec": 120 } },
  "wallet": { "unit_active": true, "open": true, "reachable": true, "name": "moneroli",
              "height": 410000, "blocks_behind": 240000,
              "balance_xmr": "1.234567", "unlocked_xmr": "1.204567",
              "last_refresh_at": null },
  "pending": null,
  "warnings": [ { "id": "wallet_behind", "severity": "warn",
                  "text": "This wallet has 240 000 blocks to go.",
                  "hint": "fmonero wallet refresh",
                  "action": "This keeps going in the background, even after you close this window." },
                { "id": "wallet_stalled", "severity": "error",
                  "text": "The wallet service took too long to answer. fmonero will try again in 30s.",
                  "hint": "fmonero doctor   # or re-run the command with --force",
                  "action": null } ] }
JSON
  cat > "$MODEL_TMP/run.js" <<'JS'
const M = require('./model.js');
const s = JSON.parse(require('fs').readFileSync(process.argv[2], 'utf8'));
console.log(JSON.stringify({
  header: M.headerLine(s, true, false),
  headerHidden: M.headerLine(s, false, false),
  node: M.nodeLabel(s.node),
  checked: M.checkedLabel(s.node),
  checkedNoData: M.checkedLabel({}),
  wallet: M.walletStateLabel(s.wallet, s.warnings),
  sync: M.blocksLeft(s) > 0 ? M.grouped(M.blocksLeft(s)) + " blocks left" : "",
  left: M.blocksLeft(s),
  pct: M.walletSyncPct(s),
  worst: M.worstSeverity(s.warnings),
  // Round 5, M4: the same shortening the TUI's `screens::short_addr` does — eight and eight.
  shortAddr: M.shortAddr("42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm"),
  // Round 5, M8: whose payments are waiting.
  waiting: M.waitingLabel(14, false),
  waitingOne: M.waitingLabel(1, true),
  // Round 5, M6: a wallet answering on a service fmonero did not write is running.
  walletForeign: M.walletStateLabel({ reachable: true, unit_active: false }, []),
  walletSilent: M.walletStateLabel({ reachable: false, unit_active: true }, []),
}));
JS
  CLOSED_OUT="$(cd "$MODEL_TMP" && node run.js closed.json 2>&1)" || CLOSED_OUT="node failed: $CLOSED_OUT"
  BEHIND_OUT="$(cd "$MODEL_TMP" && node run.js behind.json 2>&1)" || BEHIND_OUT="node failed: $BEHIND_OUT"
  expect() {
    if printf '%s' "$2" | grep -qF "$3"; then ok "$1"; else bad "$1 — got: $2"; fi
  }
  refuse() {
    if printf '%s' "$2" | grep -qF "$3"; then bad "$1 — it rendered '$3': $2"; else ok "$1"; fi
  }
  # 1. the bar icon is NOT urgent: `wallet_idle` is an *info*, and §6.4's warning row matches
  #    `error` only. That was the owner's experience and it is the whole bug — the bar was
  #    shouting about a service that is supposed to be down until a wallet is opened.
  expect "wallet_idle keeps the bar out of the alarm channel (§6.4)" "$CLOSED_OUT" '"worst":"info"'
  # 2. §6.4's header, which is the one string the hero renders.
  expect "a public node with nothing open reads 'Public node · synced · wallet closed'" \
    "$CLOSED_OUT" '"header":"Public node · synced · wallet closed"'
  expect "the node row names the node and nothing else" "$CLOSED_OUT" \
    '"node":"public node xmr-node'
  # 3. **v5.6 retires v5.5's `idle — starts when you open a wallet`**, which is one of the strings
  #    §2o deletes, and asserts its replacement instead.
  expect "the wallet row reads 'starts when you open a wallet' (§6.5)" "$CLOSED_OUT" \
    '"wallet":"starts when you open a wallet"'
  refuse "no 'idle' anywhere in the rendered rows" "$CLOSED_OUT" 'idle —'
  # 4. the checked row carries a block number AND an age. **A height with no age fails this**,
  #    because that is the one untruth §6.4 guards against: the measurement came from an earlier
  #    invocation, and a four-hour-old `synced` says nothing about now.
  expect "the checked row carries both a block number and its age (§6.4)" "$CLOSED_OUT" \
    '"checked":"block 3 765 172 · synced · checked 42s ago"'
  expect "an unchecked public node reads 'not checked yet'" "$CLOSED_OUT" \
    '"checkedNoData":"not checked yet"'
  # 5. **v5.6's wallet-sync fixture** (§6.7): the header is the wallet's, and the supporting line
  #    carries the remainder — **not the percentage again** (round 4 iteration 3): the two together
  #    put `Wallet syncing 63%` over `Syncing 63% · 240 000 blocks left` on a two-line hero.
  expect "a wallet 240 000 blocks behind reads 'Wallet syncing 63%'" "$BEHIND_OUT" \
    '"header":"Wallet syncing 63%"'
  expect "its supporting line is the remainder alone, not the percent again" "$BEHIND_OUT" \
    '"sync":"240 000 blocks left"'
  refuse "the old supporting line is gone" "$BEHIND_OUT" '"sync":"Syncing'
  expect "the remainder is exact" "$BEHIND_OUT" '"left":240000'
  # 6. §2o rule 7 over the RENDERED rows, which is the half a source grep cannot prove.
  for word in daemon unit probe restricted trusted ssl "not available"; do
    refuse "no '$word' in any rendered row (§2o)" "$CLOSED_OUT$BEHIND_OUT" "$word"
  done
  # **Round 5, M4: one shortening for one address.** This took eight from the head and SIX from the
  # tail while `screens::short_addr` took eight — so the same payment read `42ey1afD…fJJQAWDm` in the
  # TUI's pending banner and `42ey1afD…JQAWDm` on the bar, one fact in two shapes (§2o rule 8). The
  # expected string is the Rust function's own output for the upstream test vector.
  expect "an address is shortened the way the TUI shortens it (M4)" "$CLOSED_OUT" \
    '"shortAddr":"42ey1afD…fJJQAWDm"'
  # **Round 5, M8: whose payments are waiting to be mined.** `14 payments` on a panel whose every
  # other figure is this machine's read as the owner's own.
  expect "the waiting row says whose payments those are (M8)" "$CLOSED_OUT" \
    '"waiting":"14 payments across the network"'
  expect "and it pluralises and keeps the restricted floor" "$CLOSED_OUT" \
    '"waitingOne":"1 payment across the network or more"'
  # **Round 5, M6: `unit_active` alone was the wrong predicate.** With a wallet open and answering on
  # a service fmonero did not write, the wallet row said `not running` under a header reading
  # `Wallet syncing 99%` — the V13 contradiction, in the direction the TUI fixed and this did not.
  expect "a wallet answering on somebody else's service is not 'not running' (M6)" "$CLOSED_OUT" \
    '"walletForeign":"running (not fmonero'"'"'s)"'
  expect "and a unit that is up and silent is 'not answering', as the TUI's badge says" "$CLOSED_OUT" \
    '"walletSilent":"not answering"'
  # 7. showBalance off keeps the amount off the bar, header included (§6.2).
  refuse "showBalance off keeps the balance out of the header" "$BEHIND_OUT" \
    '"headerHidden":"Wallet open · 1.234567 XMR"'
  rm -rf "$MODEL_TMP"
else
  printf '  skip  Model.js fixture test: node is not installed\n'
fi

# --- 7d. the problems section is not the warnings list (§6.5, v5.6) ----------
#
# The gate lives in `Panel.qml`'s `problems` property, which `node` cannot evaluate, so it is
# asserted on the source — and on the one binding that carries the whole of §2o.1: the row renders
# the warning's **action**, never its `hint`, which is where `fmonero node local --yes` reached the
# bar. A `wallet_closed` or `wallet_idle` (`info`) therefore produces no row at all, while a
# `wallet_stalled` (`error`) produces one.
if grep -q 'sev !== "warn" && sev !== "error"' "$PLUGIN/Panel.qml"; then
  ok "the problems section keeps only warn and error (§6.5)"
else
  bad "the problems section must exclude info warnings (§6.5)"
fi
# **Round 5, S7: `panel_action` first.** `action` is written for the TUI's keymap, and seven of the
# nine warnings that reach this panel named a key it does not bind — two of them naming `d`, which
# opens the doctor here and stops your node there. §5 carries both sentences; `action` stays as the
# fallback for a document an older binary wrote, and `hint` is still never rendered.
if grep -q 'String(modelData.panel_action || modelData.action || "")' "$PLUGIN/Panel.qml" &&
   ! grep -q 'modelData.hint' "$PLUGIN/Panel.qml"; then
  ok "a problem row renders the warning's panel_action and never its hint (§2o.1, S7)"
else
  bad "a problem row must render \`panel_action\`, not \`hint\` (§2o.1, S7)"
fi
if grep -q 'send_may_be_in_flight' "$PLUGIN/Panel.qml"; then
  ok "send_may_be_in_flight is excluded while the pending row renders it (§6.5)"
else
  bad "the pending row's own warning must be excluded from the problems list (§6.5)"
fi

# --- 7b. remote node mode (DESIGN.md §2m, §6.4, §6.5; v5.2) -------------------
# This script has no way to feed the widget a Status document — the cursor IPC is a read-only
# hook and a setter that injected one would let anything spoof what the bar displays — so the
# remote rows are asserted STRUCTURALLY, on the one property that is load-bearing: the two
# remote cases must come BEFORE the three node cases in `iconCase`. Below them, `!svc.nodeUp`
# and `!svc.synced` read fields that are null in remote mode, and the icon would draw a crossed
# glyph and a 0 % ring for a wallet that is working perfectly (§6.4). The live half below then
# proves the file still loads, and `cursor` now reports `remote` so a human can check it.
ICON_EXPR="$(sed -n '/readonly property int iconCase:/,/svc.walletOpen ? 4 : 5/p' "$PLUGIN/Panel.qml")"
REMOTE_AT="$(printf '%s\n' "$ICON_EXPR" | grep -n 'svc.remote' | head -1 | cut -d: -f1)"
NODEUP_AT="$(printf '%s\n' "$ICON_EXPR" | grep -n '!svc.nodeUp' | head -1 | cut -d: -f1)"
if [[ -n $REMOTE_AT && -n $NODEUP_AT && $REMOTE_AT -lt $NODEUP_AT ]]; then
  ok "svc.remote is matched before !svc.nodeUp (§6.4)"
else
  bad "§6.4: the remote rows must be matched BEFORE !svc.nodeUp" "$ICON_EXPR"
fi
# v5.6 renamed both, and §2o.2 deletes the word the old names carried.
present "Model.nodeLabel exists" Model.js 'function nodeLabel'
present "Model.checkedLabel exists" Model.js 'function checkedLabel'
present "Panel.qml renders the node row from it" Panel.qml 'Model.nodeLabel'
present "Panel.qml renders the check with its age" Panel.qml 'Model.checkedLabel'
present "Model.headerLine is the hero's one string (§6.4)" Panel.qml 'Model.headerLine'
# **The hero's second line carries the COUNT, and the header carries the percent** (round 4
# iteration 3). `Model.walletSyncLabel` composed both into one string and the header already had the
# percent, so a catching-up wallet read `Wallet syncing 99%` over `Syncing 99% · 6 122 blocks left`.
# The function is gone; these two assertions are what stops it coming back.
present "the hero's second line is the blocks-left count" Panel.qml 'Model.grouped(svc.blocksLeft) + " blocks left"'
absent  "no walletSyncLabel: the percent is the header's alone" Model.js 'function walletSyncLabel'
# W10: decision 7 is ONE sentence, and this is the assertion that keeps the widget's copy of it
# from drifting from `fmonero_core::SYNC_KEEPS_GOING`. The Rust const is read out of the source
# rather than restated here, so the two cannot be edited apart.
SYNC_SENTENCE="$(sed -n '/pub const SYNC_KEEPS_GOING/,/;$/p' \
   "$REPO/crates/fmonero-core/src/lib.rs" | grep -oE '"[^"]+"' | tr -d '"')"
if [ -n "$SYNC_SENTENCE" ] && grep -qF "$SYNC_SENTENCE" "$PLUGIN/Panel.qml"; then
  ok "the widget quotes decision 7's one sentence, byte for byte (W10)"
else
  bad "Panel.qml must carry fmonero_core::SYNC_KEEPS_GOING verbatim: $SYNC_SENTENCE"
fi
present "the node toggle is inert in remote mode" Service.qml 'if (remote) { localError'
present "cursor reports the mode" Panel.qml 'remote: svc.remote'
# V16: the header takes the SHORT balance from Status, because §2a.5 forbids the QML from
# shortening the exact one itself.
present "the header takes the pre-rendered short balance (§6.4, V16)" Model.js 'balance_xmr_short'
# §6.7's `parseFloat` / `Number(` / `/ 1e` greps cover the arithmetic; `toFixed` is the one that
# would shorten an amount, which is why V16 put the short form in Rust. A real `forbid`, with the
# fixture below proving it fires — the first draft of this check was a broken pipeline that could
# only ever pass.
# ...and the grep is on a line that ALSO names an amount, which is the same shape
# `scripts/check.sh` uses for its `as f64` half. A blanket `toFixed` ban would fire on `bytes()`,
# where rounding a byte count is correct — and a grep that fails on correct code is a grep the next
# agent relaxes (§6.7's own rule).
forbid "no QML or JS shortens an amount itself (§2a.5)" \
  '(balance|unlocked|amount|_xmr|pico)[A-Za-z_]*\)?\.?[^;]*(toFixed|\.slice\(|substring\()'
fires "the amount-shortening grep" \
  '(balance|unlocked|amount|_xmr|pico)[A-Za-z_]*\)?\.?[^;]*(toFixed|\.slice\(|substring\()' \
  'var s = String(w.balance_xmr).toFixed(4)'
silent "the amount-shortening grep" \
  '(balance|unlocked|amount|_xmr|pico)[A-Za-z_]*\)?\.?[^;]*(toFixed|\.slice\(|substring\()' \
  'return (i === 0 ? v.toFixed(0) : v.toFixed(1)) + " " + u[i]'


# --- 7c. WHICH QML THE SHELL IS RUNNING (DECISIONS V1) ------------------------
#
# The v5.6 redesign had never rendered on the owner's bar and no gate could tell: Quickshell holds
# the loaded QML, a symlink does not hot-reload (§1.5), and `omarchy restart shell` is refused while
# the session is locked. The live half below asserted only that `cursor` returned a JSON object —
# and the proof that the panel was the PREVIOUS one, a section named `warnings` that the current
# file does not build, was inside that assertion's own passing message, twice, unread.
#
# So: `Panel.qml` carries a version token, `cursor` reports it, and this is the token the checked-out
# tree should have — the first eight hex digits of the sha256 of the four widget files with the
# token's own line removed. An edit to any of them changes it, so the token cannot be left behind,
# and the failure message is the value to paste.
QML_WANT="qml-$(cat "$PLUGIN/Panel.qml" "$PLUGIN/Service.qml" "$PLUGIN/MoneroIcon.qml" \
                   "$PLUGIN/Model.js" | grep -v 'property string panelVersion' \
                | sha256sum | cut -c1-8)"
QML_HAVE="$(grep -oE 'property string panelVersion: "[^"]+"' "$PLUGIN/Panel.qml" \
            | grep -oE 'qml-[0-9a-f]+' | head -1)"
if [[ $QML_HAVE == "$QML_WANT" ]]; then
  ok "Panel.qml's version token matches its own contents ($QML_WANT)"
else
  bad "Panel.qml says $QML_HAVE; this tree is $QML_WANT — bump panelVersion"
fi
present "cursor carries the version token" Panel.qml 'version: root.panelVersion'
# And the sections the cursor reports must be the ones THIS file builds. `warnings` was the name of
# the pre-v5.6 panel's alarm section, so a shell still running it fails here by name.
QML_SECTIONS="$(grep -oE '(s = \["header"\]|s\.push\("[a-z]+"\))' "$PLUGIN/Panel.qml" \
                | grep -oE '"[a-z]+"' | tr -d '"' | sort -u | tr '\n' ' ')"
if [[ -n ${QML_SECTIONS// /} ]]; then
  ok "Panel.qml builds the sections: $QML_SECTIONS"
else
  bad "could not read navSections out of Panel.qml"
fi

# --- 8. reload the shell, read the journal, ask the widget ---------------------
if [[ $STATIC_ONLY -eq 1 ]]; then
  echo "plugin-smoke: --static, skipping the reload half"
elif ! command -v omarchy-shell >/dev/null; then
  bad "omarchy-shell not on PATH"
elif ! omarchy-shell shell ping >/dev/null 2>&1; then
  bad "omarchy-shell is not answering (is the shell running?)"
else
  SINCE=$(date '+%Y-%m-%d %H:%M:%S')
  if omarchy-shell shell rescanPlugins >/dev/null 2>&1; then ok "rescanPlugins"; else bad "rescanPlugins failed"; fi
  sleep 2
  # Two lines are benign and appear on every reload: the registry's own reload notice, and the
  # IpcHandler warning every panel plugin on this box logs (DESIGN.md §1.5).
  noise='Local plugin changed|Handler was registered but will not be used'
  bads='failed:|Expected token|unavailable|is not a type|ReferenceError|TypeError'
  lines=$(journalctl --user -t omarchy-shell --since "$SINCE" --no-pager 2>/dev/null \
            | grep -F "$ID" | grep -Ev "$noise" | grep -E "$bads" || true)
  if [[ -z $lines ]]; then
    ok "journal carries no load failure for $ID"
  else
    bad "journal: $lines"
  fi

  # The widget answers its own IPC only when Panel.qml actually instantiated.
  if cursor=$(omarchy-shell "$ID" cursor 2>/dev/null) && [[ $cursor == \{* ]]; then
    ok "widget is loaded (cursor: $cursor)"
    # V1, the assertion this gate was missing: the shell is running THIS tree's panel.
    LIVE_VERSION="$(printf '%s' "$cursor" | jq -r '.version // "none"')"
    if [[ $LIVE_VERSION == "$QML_WANT" ]]; then
      ok "the running shell is this tree's panel ($LIVE_VERSION)"
    else
      bad "the shell is running $LIVE_VERSION, this tree is $QML_WANT — it has not reloaded (§1.5: \
a symlink does not hot-reload; run \`omarchy restart shell\`, which is refused while the session is \
locked)"
    fi
    # And its sections are the ones Panel.qml builds — `warnings` belongs to the pre-v5.6 panel.
    LIVE_SECTIONS="$(printf '%s' "$cursor" | jq -r '.sections // [] | .[]' 2>/dev/null)"
    UNKNOWN=""
    for sec in $LIVE_SECTIONS; do
      case " $QML_SECTIONS " in *" $sec "*) ;; *) UNKNOWN="$UNKNOWN $sec" ;; esac
    done
    if [[ -z ${UNKNOWN// /} ]]; then
      ok "every section the shell reports is one Panel.qml builds ($(printf '%s' "$LIVE_SECTIONS" | tr '\n' ' '))"
    else
      bad "the shell reports section(s)$UNKNOWN, which Panel.qml does not build: it is a different panel"
    fi
  else
    bad "widget did not answer IPC — it is not on the bar, or it failed to load"
  fi
fi

echo
printf 'plugin-smoke: %d passed, %d failed\n' "$PASS" "$FAIL"
[[ $FAIL -eq 0 ]]
