#!/usr/bin/env bash
# The automated half of the friend's read, over every capture.
set -uo pipefail
OUT="${1:?dir}"
fail=0
say() { printf '  %-6s %s\n' "$1" "$2"; }
strip() { sed -E 's/\x1b\[[0-9;]*m//g' "$1"; }

# The verifier's own twenty-two, at word boundaries and case-sensitively where the word IS a case
# (`RPC` is the banned one; `monero-rpc.cheems.de…` is a hostname). Two line-level carve-outs, the
# same two §2o.3's own grep makes: a line that carries a **runnable command** is a command on
# purpose, and a line that carries a **settings key** or an absolute path carries a name the user
# types (rule 5's own exemption). Everything else is prose.
WORDS='\b(daemons?|units?|loopback|restricted|probes?|probed|layouts?|ssl|systemd|systemctl|argv|breakers?|snapshots?|monerod|monero-wallet-rpc|txpool|mempool|nettype|mutator|relay|relayed|broadcast|transactions?|txid|code_name)\b|RPC|§|os error|high water|error\[|reason:'
ALLOW='fmonero |omarchy |In a terminal: |monero-wallet-cli |/[A-Za-z0-9_.-]*monero|/usr/bin/|/etc/|--[a-z]|[a-z]+\.(unit|url|mode|dir|network|remote|log_file|port|autostart|mouse|priority|account|data_dir|remote_probe_sec|remote_trusted|level|service)'
hits=""
for f in "$OUT"/*.txt; do
  case "$f" in *scenario.txt) continue ;; esac
  h="$(strip "$f" | grep -nE "$WORDS" | grep -vE "$ALLOW" | head -4)"
  [ -n "$h" ] && hits="$hits$(basename "$f"): $(printf '%s' "$h" | tr '\n' '/')"$'\n'
done
if [ -z "$hits" ]; then say ok "no banned word in any capture"; else say FAIL "banned words:"; printf '%s' "$hits" | sed 's/^/        /'; fail=1; fi

# an id: 8+ hex characters carrying at least one a-f, and not inside a base58 address
hits=""
for f in "$OUT"/*.txt; do
  case "$f" in *scenario.txt) continue ;; esac
  h="$(strip "$f" | grep -oE '[0-9a-f]{8,}' | grep -E '[a-f]' | sort -u | head -3)"
  [ -n "$h" ] && hits="$hits$(basename "$f"): $(printf '%s' "$h" | tr '\n' ' ')"$'\n'
done
if [ -z "$hits" ]; then say ok "no id-shaped hex run in any capture"; else say FAIL "hex runs:"; printf '%s' "$hits" | sed 's/^/        /'; fail=1; fi

# an em-dash used as prose punctuation: a `—` with a letter on both sides of a space
hits=""
for f in "$OUT"/*.txt; do
  # Prose punctuation is `word — word` with ONE space each side. A missing-value cell is a whole
  # cell — `right now   — down · — up` — and is padded away from its label by the row's own columns.
  h="$(strip "$f" | grep -oE '[A-Za-z] — [A-Za-z]' | head -2)"
  [ -n "$h" ] && hits="$hits$(basename "$f"): $(printf '%s' "$h" | tr '\n' '/')"$'\n'
done
if [ -z "$hits" ]; then say ok "every em-dash is a missing-value cell, not prose"; else say FAIL "prose em-dashes:"; printf '%s' "$hits" | sed 's/^/        /'; fail=1; fi

# every CLI line fits 120 columns (the `$ fmonero …` echo lines this script writes are excluded)
hits=""
for f in "$OUT"/cli-*.txt; do
  h="$(strip "$f" | awk 'NR>1 && length($0)>120 {print NR": "length($0)}' | head -2)"
  [ -n "$h" ] && hits="$hits$(basename "$f"): $(printf '%s' "$h" | tr '\n' '/')"$'\n'
done
if [ -z "$hits" ]; then say ok "every CLI output line fits 120 columns"; else say FAIL "over 120:"; printf '%s' "$hits" | sed 's/^/        /'; fail=1; fi

# the year is in every payments row of every wallet capture
n=0
for f in "$OUT"/*wallet*.txt "$OUT"/cli-wallet-history.txt; do
  [ -e "$f" ] || continue
  grep -qE '20[0-9]{2}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}' "$f" && n=$((n+1))
done
say ok "the year is on $n wallet captures"

# the two addresses S1 is about, whole
for a in monero-rpc.cheems.de.box.skhron.com.ua:18089 monero-rpc.cheems.de.box.skhron.com.u:18089; do
  n=$(grep -lF "$a" "$OUT"/C-*.txt 2>/dev/null | wc -l)
  if [ "$n" -ge 2 ]; then say ok "$a renders whole in $n picker captures"; else say FAIL "$a whole in only $n"; fail=1; fi
done
# the notice's own keys
n=$(grep -lF "Enter continues · Esc cancels" "$OUT"/D-*.txt 2>/dev/null | wc -l)
if [ "$n" -ge 2 ]; then say ok "the consent modal's keys are on screen at both sizes"; else say FAIL "consent keys on $n"; fail=1; fi
# S12
n=$(grep -lF "Esc first, then Tab." "$OUT"/A-*.txt 2>/dev/null | wc -l)
if [ "$n" -ge 2 ]; then say ok "Tab in the send confirm says which key comes first, at both sizes"; else say FAIL "Tab notice on $n"; fail=1; fi
# S8
n=$(grep -lF "[P] Clear that record" "$OUT"/P-*.txt 2>/dev/null | wc -l)
if [ "$n" -ge 2 ]; then say ok "the clear-that-record chip is on the Wallet screen at both sizes"; else say FAIL "P chip on $n"; fail=1; fi
printf '\n  %s\n' "$([ "$fail" = 0 ] && echo 'the read found nothing' || echo 'the read found something')"
exit "$fail"
