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
MONEY_ARGV='"(send|sweep|open|create|restore|transfer|relay)"'
forbid "no money argv element" "$MONEY_ARGV"
fires  "money argv" "$MONEY_ARGV" 'Quickshell.execDetached(["fmonero","wallet","send"])'
silent "money argv" "$MONEY_ARGV" 'Quickshell.execDetached(["fmonero","tui","--send"])'

SECRET='password'
forbid "no wallet secret named in QML" "$SECRET"
fires  "secret name" "$SECRET" 'property string password: ""   // the widget must never have one'

# --- 7. no arithmetic on money in QML (DESIGN.md §2a.5, the QML half) ---------
# Status JSON carries every amount as a pre-rendered `*_xmr` string, so the widget never needs a
# float. `grep -rn 'as f64'` covers the Rust half in scripts/check.sh.
FLOATS='parseFloat|Number\(|/ 1e'
forbid "no float arithmetic on money" "$FLOATS"
fires  "parseFloat" "$FLOATS" 'var x = parseFloat(status.pending.txs[0].amount_xmr)'
fires  "Number()"   "$FLOATS" 'var x = Number(status.wallet.balance_xmr) + 1'
fires  "pico / 1e12" "$FLOATS" 'var xmr = status.wallet.balance_pico / 1e12'

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
present "Model.remoteLabel exists" Model.js 'function remoteLabel'
present "and Panel.qml uses it instead of a sync gauge" Panel.qml 'Model.remoteLabel'
present "the node toggle is inert in remote mode" Service.qml 'if (remote) { localError'
present "cursor reports the mode" Panel.qml 'remote: svc.remote'

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
  else
    bad "widget did not answer IPC — it is not on the bar, or it failed to load"
  fi
fi

echo
printf 'plugin-smoke: %d passed, %d failed\n' "$PASS" "$FAIL"
[[ $FAIL -eq 0 ]]
