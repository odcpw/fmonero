#!/usr/bin/env bash
# Round 5's own captures. Every run is sandboxed: state, config, cache, XDG_DATA_HOME, the systemd
# user dir, the systemctl fixture and FMONERO_NODES_URL (a local --static mock). No sudo, no push,
# nothing of the owner's read or written.
set -uo pipefail
ROOT=/home/oliver/Projects/odcpw/fmonero
OUT="${1:?out dir}"
mkdir -p "$OUT"
FM="$ROOT/target/release/fmonero"
MOCK="$ROOT/target/release/fmonero-mock"
WORK="$(mktemp -d /tmp/claude-1000/r5/work.XXXXXX)"
MOCKS=()
SESSIONS=()
cleanup() {
  for s in "${SESSIONS[@]:-}"; do [ -n "$s" ] && tmux kill-session -t "$s" 2>/dev/null; done
  for p in "${MOCKS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
}
trap cleanup EXIT

NODE_PORT=18591
WALLET_PORT=18593
STATIC_PORT=18595
"$MOCK" --role node   --port $NODE_PORT   --fixtures "$ROOT/tests/fixtures" --log "$WORK/node.jsonl" >/dev/null 2>&1 & MOCKS+=("$!")
"$MOCK" --role wallet --port $WALLET_PORT --fixtures "$ROOT/tests/fixtures" --log "$WORK/wallet.jsonl" --auth fmonero:t3st >/dev/null 2>&1 & MOCKS+=("$!")
"$MOCK" --static "$ROOT/tests/fixtures/nodes" --port $STATIC_PORT --log "$WORK/nodes.jsonl" >/dev/null 2>&1 & MOCKS+=("$!")
for port in $NODE_PORT $WALLET_PORT; do
  for _ in $(seq 60); do curl -s -o /dev/null --connect-timeout 1 -d '{}' "127.0.0.1:$port/get_height" && break; sleep 0.05; done
done

# one sandbox
mksb() { # mksb <name> <mode>
  local n="$1"
  local mode="$2"
  local d="$WORK/$n"
  mkdir -p "$d/state/run" "$d/config" "$d/cache" "$d/data/fmonero/wallets/mainnet" "$d/systemd/user" "$d/wallets/mainnet"
  : > "$d/systemctl.fixture"
  : > "$d/wallets/mainnet/moneroli.keys"
  chmod 700 "$d/state" "$d/config" "$d/cache"
  printf 'fmonero:t3st' > "$d/state/run/monero-wallet-rpc.$WALLET_PORT.login"
  chmod 600 "$d/state/run/monero-wallet-rpc.$WALLET_PORT.login"
  { echo "2026-09-16 14:02:09.114 I Monero 'Fluorine Fermi' (v0.18.5.1-release) started"
    echo "2026-09-16 14:02:10.002 I Synced 3541122/3541122"
    echo "2026-09-16 14:02:11.771 I [127.0.0.1:18080 OUT] NEW BLOCK 3541122"; } > "$d/bitmonero.log"
  sbenv "$n" "$FM" settings set node.url   "http://127.0.0.1:$NODE_PORT" >/dev/null
  sbenv "$n" "$FM" settings set wallet.url "http://127.0.0.1:$WALLET_PORT" >/dev/null
  sbenv "$n" "$FM" settings set wallet.port "$WALLET_PORT" >/dev/null
  sbenv "$n" "$FM" settings set wallet.dir "$d/wallets/mainnet" >/dev/null
  sbenv "$n" "$FM" settings set node.log_file "$d/bitmonero.log" >/dev/null
  if [ "$mode" = remote ]; then
    sbenv "$n" "$FM" settings set node.remote "monero-rpc.cheems.de.box.skhron.com.ua:18089" >/dev/null
    sbenv "$n" "$FM" settings set node.mode remote >/dev/null
  fi
}
sbenv() { local n="$1"; shift; env FMONERO_STATE_DIR="$WORK/$n/state" FMONERO_CONFIG_DIR="$WORK/$n/config" \
  FMONERO_CACHE_DIR="$WORK/$n/cache" XDG_DATA_HOME="$WORK/$n/data" \
  FMONERO_SYSTEMD_USER_DIR="$WORK/$n/systemd/user" FMONERO_SYSTEMCTL_FIXTURE="$WORK/$n/systemctl.fixture" \
  FMONERO_NODES_URL="http://127.0.0.1:$STATIC_PORT/nodes.json" "$@"; }
envstr() { local n="$1"; printf 'FMONERO_STATE_DIR=%s FMONERO_CONFIG_DIR=%s FMONERO_CACHE_DIR=%s XDG_DATA_HOME=%s FMONERO_SYSTEMD_USER_DIR=%s FMONERO_SYSTEMCTL_FIXTURE=%s FMONERO_NODES_URL=http://127.0.0.1:%s/nodes.json' \
  "$WORK/$n/state" "$WORK/$n/config" "$WORK/$n/cache" "$WORK/$n/data" "$WORK/$n/systemd/user" "$WORK/$n/systemctl.fixture" "$STATIC_PORT"; }

open_tui() { # open_tui <sb> <session> <w> <h>
  local n="$1"
  local s="$2-$$"
  local w="$3"
  local h="$4"
  tmux kill-session -t "$s" 2>/dev/null
  tmux new-session -d -s "$s" -x "$w" -y "$h" "env $(envstr "$n") $FM tui 2>$WORK/$s.err; echo \$? > $WORK/$s.rc"
  SESSIONS+=("$s")
  sleep 2.5
  tmux has-session -t "$s" 2>/dev/null || { echo "open_tui: $s did not stay open: $(head -2 "$WORK/$s.err")" >&2; return 1; }
  SESSION_NOW="$s"
}
snap() { tmux capture-pane -p -t "$1-$$" 2>/dev/null; }
send() { tmux send-keys -t "$1-$$" "${@:2}" 2>/dev/null || true; }
grab() { # grab <session> <file>
  local text
  text="$(snap "$1")"
  if [ -z "$text" ]; then
    echo "grab: $1 gave an EMPTY pane for $2 — the session is gone: $(head -2 "$WORK/$1-$$.err" 2>/dev/null)" >&2
  fi
  printf '%s\n' "$text" > "$OUT/$2"
}

# ---------------------------------------------------------------- sandboxes
mksb A local      # your own node (the mock answers), a wallet open
mksb C remote     # using a public node, a list cached
mksb D remote     # using a public node, no list at all
mksb P local      # a wallet open with an unresolved pending.json

# C's cache: the fixture's own widest address, plus a 43-character one and a short one
NOW=$(date +%s)
cat > "$WORK/C/cache/public-nodes.json" <<JSON
{"schema":1,"fetched_at":$NOW,"source":"https://monero.fail","skipped_no_port":37,
 "skipped_invalid":0,"notice_ack":$NOW,
 "nodes":[
  {"url":"https://monero-rpc.cheems.de.box.skhron.com.ua:18089","address":"monero-rpc.cheems.de.box.skhron.com.ua:18089","scheme":"https","web_compatible":false,"custom":false,"health_available":true,"health_ratio":0.94,"health_checks":18,"health_last_height":3764877,"health_checked_at_text":"Fri, 18 Sep 2026 05:12:02 GMT","health_checked_at":$((NOW-600)),"probe":{"reachable":true,"height":3765100,"synchronized":true,"restricted":true,"nettype":"mainnet","version":null,"latency_ms":42,"reason":null,"probed_at":$NOW}},
  {"url":"https://monero-rpc.cheems.de.box.skhron.com.u:18089","address":"monero-rpc.cheems.de.box.skhron.com.u:18089","scheme":"https","web_compatible":false,"custom":false,"health_available":true,"health_ratio":0.72,"health_checks":18,"health_last_height":3765172,"health_checked_at_text":"Fri, 18 Sep 2026 05:12:02 GMT","health_checked_at":$((NOW-600)),"probe":null},
  {"url":"http://node.xmr.pub:443","address":"node.xmr.pub:443","scheme":"http","web_compatible":false,"custom":false,"health_available":true,"health_ratio":1.0,"health_checks":18,"health_last_height":3765209,"health_checked_at_text":"Fri, 18 Sep 2026 05:12:02 GMT","health_checked_at":$((NOW-600)),"probe":{"reachable":true,"height":3765209,"synchronized":true,"restricted":false,"nettype":"mainnet","version":"0.18.5.1","latency_ms":34,"reason":null,"probed_at":$NOW}}]}
JSON

# **C and D must not contact anything.** In remote mode the Node screen checks the CONFIGURED node
# on the §2n.4 schedule, and a cold cache means that check runs — which on the first pass of this
# round sent a DNS query for `monero-rpc.cheems.de.box.skhron.com.ua` and rendered `a web error`.
# Seeding a FRESH probe into `state.json` is what `nodes::spawn_check` reads to decide it has nothing
# to do (bound 1 of §5's four, `the_check_flag_starts_nothing_when_the_last_check_is_fresh`), so the
# screen renders a real measurement and starts nothing. D is left cold on purpose — the notice modal
# is what stands in front of its `f`, which is the whole of S6 — and its own configured address is
# this machine, so even a check that did start could not leave it.
cat > "$WORK/C/state/state.json" <<JSON
{"remote_probe":["monero-rpc.cheems.de.box.skhron.com.ua:18089",
  {"reachable":true,"height":3765100,"synchronized":true,"restricted":true,"nettype":"mainnet",
   "version":null,"latency_ms":42,"reason":null,"probed_at":$NOW}]}
JSON
chmod 600 "$WORK/C/state/state.json"
sbenv D "$FM" settings set node.remote "127.0.0.1:$STATIC_PORT" >/dev/null
# And belt as well as braces: `node.remote_probe_sec = 0` turns the schedule off outright
# (`nodes::spawn_check`'s first branch), so no check of any node can start in either sandbox.
sbenv C "$FM" settings set node.remote_probe_sec 0 >/dev/null
sbenv D "$FM" settings set node.remote_probe_sec 0 >/dev/null

# P's unresolved record
cat > "$WORK/P/state/pending.json" <<JSON
{"kind":"send","account_index":0,
 "dest":"42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm",
 "built_at":1789000000,
 "txs":[{"txid":"5c2e0b8391f7a6c4d2e0b8391f7a6c4d2e0b8391f7a6c4d2e0b8391f729d74b1",
         "amount":500000000000,"fee":30720000,"relayed":false,"relayed_at":null}]}
JSON
chmod 600 "$WORK/P/state/pending.json"

# ---------------------------------------------------------------- the TUI, both sizes
for size in 120x32 150x40; do
  W="${size%x*}"; H="${size#*x}"
  # --- A: your own node, a wallet open
  S="r5A$W"
  open_tui A "$S" "$W" "$H"
  grab "$S" "A-$size-01-node.txt"
  send "$S" 2; sleep 1.2; grab "$S" "A-$size-02-wallet.txt"
  send "$S" Enter; sleep 1.0; grab "$S" "A-$size-03-payment-detail.txt"; send "$S" Escape; sleep 0.5
  send "$S" n; sleep 0.9; grab "$S" "A-$size-04-note-modal.txt"; send "$S" Escape; sleep 0.5
  send "$S" Q; sleep 1.0; grab "$S" "A-$size-05-qr.txt"; send "$S" Escape; sleep 0.5
  send "$S" t; sleep 0.8; grab "$S" "A-$size-06-kind-filter.txt"
  send "$S" o; sleep 1.0; grab "$S" "A-$size-07-open-wallet.txt"; send "$S" Escape; sleep 0.5
  send "$S" c; sleep 1.0; grab "$S" "A-$size-08-create-wallet.txt"; send "$S" Tab; sleep 0.8
  grab "$S" "A-$size-09-create-tab.txt"; send "$S" Escape; sleep 0.5
  send "$S" 3; sleep 1.0; grab "$S" "A-$size-10-send-empty.txt"
  send "$S" -l "42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm"
  send "$S" Down; send "$S" -l "0.1"; sleep 0.8; grab "$S" "A-$size-11-send-filled.txt"
  send "$S" Enter; sleep 3.5; grab "$S" "A-$size-12-send-confirm.txt"
  send "$S" Tab; sleep 1.0; grab "$S" "A-$size-13-send-confirm-tab.txt"
  send "$S" Escape; sleep 0.8
  send "$S" Escape; sleep 0.4
  send "$S" 4; sleep 1.0; grab "$S" "A-$size-14-log.txt"
  send "$S" "?"; sleep 1.0; grab "$S" "A-$size-15-help.txt"; send "$S" Escape; sleep 0.5
  send "$S" 1; sleep 1.0; send "$S" b; sleep 1.0; grab "$S" "A-$size-16-node-bans.txt"
  send "$S" b; sleep 0.8
  send "$S" L; sleep 1.0; grab "$S" "A-$size-17-log-level.txt"; send "$S" Escape; sleep 0.5
  send "$S" u; sleep 1.0; grab "$S" "A-$size-18-node-up-refusal.txt"
  send "$S" q; sleep 1.0
  # --- C: a public node with a list cached
  S="r5C$W"
  open_tui C "$S" "$W" "$H"
  grab "$S" "C-$size-01-node.txt"
  send "$S" Down; sleep 1.0; grab "$S" "C-$size-02-picker-sel.txt"
  send "$S" Enter; sleep 1.0; grab "$S" "C-$size-03-use-node.txt"; send "$S" Escape; sleep 0.5
  send "$S" a; sleep 0.9; grab "$S" "C-$size-04-add-node.txt"; send "$S" Escape; sleep 0.5
  send "$S" "/"; sleep 0.7; grab "$S" "C-$size-05-picker-filter.txt"; send "$S" Escape; sleep 0.5
  send "$S" l; sleep 1.0; grab "$S" "C-$size-06-use-local.txt"; send "$S" Escape; sleep 0.5
  send "$S" 2; sleep 1.2; grab "$S" "C-$size-07-wallet.txt"
  send "$S" 4; sleep 1.0; grab "$S" "C-$size-08-log-remote.txt"
  send "$S" q; sleep 1.0
  # --- D: a public node, no list at all
  S="r5D$W"
  open_tui D "$S" "$W" "$H"
  grab "$S" "D-$size-01-node.txt"
  send "$S" f; sleep 1.2; grab "$S" "D-$size-02-notice.txt"; send "$S" Escape; sleep 0.6
  send "$S" q; sleep 1.0
  # --- P: an unresolved payment record
  S="r5P$W"
  open_tui P "$S" "$W" "$H"
  grab "$S" "P-$size-01-node-banner.txt"
  send "$S" 2; sleep 1.2; grab "$S" "P-$size-02-wallet-banner.txt"
  send "$S" P; sleep 1.0; grab "$S" "P-$size-03-clear-confirm.txt"; send "$S" Escape; sleep 0.6
  send "$S" 3; sleep 1.0
  send "$S" -l "42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm"
  send "$S" Down; send "$S" -l "0.1"; sleep 0.8
  send "$S" Enter; sleep 3.0; grab "$S" "P-$size-04-send-refusal.txt"
  send "$S" q; sleep 1.0
done

# A with NO wallets at all, for the empty state
mksb E local
rm -f "$WORK/E/wallets/mainnet/moneroli.keys"
for size in 120x32 150x40; do
  W="${size%x*}"; H="${size#*x}"
  S="r5E$W"
  open_tui E "$S" "$W" "$H"
  send "$S" 2; sleep 1.5; grab "$S" "E-$size-01-wallet-empty.txt"
  send "$S" o; sleep 1.0; grab "$S" "E-$size-02-open-none.txt"; send "$S" Escape; sleep 0.5
  send "$S" q; sleep 1.0
done
printf 'captures in %s\n' "$OUT"

# ---------------------------------------------------------------- the CLI, on a 120-column pty
# `script -q` gives a real pty so stdout is a TTY and the human renderer runs; COLUMNS is what
# `output::term_width` reads first (§4, X3), so the width is exactly 120.
cli() { # cli <sandbox> <file> <args…>
  local n="$1"
  local f="$2"
  shift 2
  local cmd
  cmd="env $(envstr "$n") COLUMNS=120 $FM $*"
  printf '$ fmonero %s\n' "$*" > "$OUT/cli-$f.txt"
  COLUMNS=120 script -q -e -c "$cmd" /dev/null 2>&1 | sed 's/\r$//' >> "$OUT/cli-$f.txt"
  printf '[exit %s]\n' "${PIPESTATUS[0]}" >> "$OUT/cli-$f.txt"
}
# A's cache, so `node list` has rows to print, with probe results (S2's own state).
cp "$WORK/C/cache/public-nodes.json" "$WORK/A/cache/public-nodes.json"
cli A status        status
cli A doctor        doctor
cli A settings-list settings list
cli A node-list     node list
cli A node-list-probed node list
cli A node-info     node info
cli A node-peers    node peers
cli A wallet-balance wallet balance
cli A wallet-history wallet history
cli A wallet-list   wallet list
cli A version       version
cli A send-refusal  wallet send 42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm 0.1
cli A sweep-refusal wallet sweep 42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm
cli A sweep-dryrun  wallet sweep 42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm --dry-run --yes
cli P send-pending  wallet send 42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm 0.1 --yes
cli P clear-pending wallet clear-pending
# two error paths
cli A err-address   wallet validate notanaddress
cli A err-subcmd    note
cli A err-settings  settings get nope
cli A err-amount    wallet send 42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm 1.2.3
cli D status-remote status
cli D node-list-empty node list
cli A help-wallet   wallet --help
cli A help-node     node --help
printf 'cli captures in %s\n' "$OUT"
