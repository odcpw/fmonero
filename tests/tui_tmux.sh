#!/usr/bin/env bash
# tests/tui_tmux.sh — the TUI on a REAL pty (DESIGN.md §7a, §8 Phase 2 "tui").
#
# `fmonero tui` refuses a non-TTY stdout (exit 2, main.rs), so the only honest smoke test
# drives it through a pty. Two modes:
#
#   tests/tui_tmux.sh                 # the gate: open, visit 1 2 3 4, ?, quit cleanly
#   tests/tui_tmux.sh --capture DIR   # the same, against fmonero-mock, writing one
#                                     # capture-pane text file per screen into DIR
#
# The capture mode is what fills decisions/phase2-tui.md, and it runs against
# `fmonero-mock` serving tests/fixtures/ ONLY — never a real wallet (§8's tui gate: the
# Wallet screen renders a balance, a primary address and counterparty addresses, and §2b.2
# keeps real ones out of the decisions log).
#
# Requires tmux. `script -qc` would also give a pty, but tmux can send keys and read the
# screen back, which is what turns "it started" into "every screen rendered".
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CAPTURE=""
[ "${1:-}" = "--capture" ] && { CAPTURE="${2:?--capture needs a directory}"; mkdir -p "$CAPTURE"; }

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  ok    %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL  %s\n' "$1"; shift; for l in "$@"; do printf '        %s\n' "$l"; done; }

command -v tmux >/dev/null || { echo "tui_tmux.sh: tmux is required" >&2; exit 1; }

printf '\n== build\n'
( cd "$ROOT" && cargo build --release >/dev/null 2>&1 ) || { echo "cargo build --release failed" >&2; exit 1; }
FM="$ROOT/target/release/fmonero"
MOCK="$ROOT/target/release/fmonero-mock"
ok "cargo build --release"

WORK="$(mktemp -d)"
SESSION="fmonero-tui-$$"
MOCKS=()
cleanup() {
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  for pid in "${MOCKS[@]:-}"; do [ -n "$pid" ] && kill "$pid" 2>/dev/null || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT

export FMONERO_STATE_DIR="$WORK/state"
export FMONERO_CONFIG_DIR="$WORK/config"
export FMONERO_CACHE_DIR="$WORK/cache"
mkdir -p "$FMONERO_STATE_DIR/run" "$FMONERO_CONFIG_DIR" "$FMONERO_CACHE_DIR"
chmod 700 "$FMONERO_STATE_DIR" "$FMONERO_CONFIG_DIR" "$FMONERO_CACHE_DIR"

NODE_PORT=18291
WALLET_PORT=18293
SCENARIO="both daemons absent"

# The mocks, and the login file §2c requires BEFORE the first fmonero invocation: with none the
# client sends unauthenticated requests, the mock answers 401 twice and every wallet row is
# exit 4 `auth_refused` instead of the thing it names.
if [ -n "$CAPTURE" ]; then
  [ -x "$MOCK" ] || { echo "tui_tmux.sh: $MOCK is missing" >&2; exit 1; }
  "$MOCK" --role node   --port "$NODE_PORT"   --fixtures "$ROOT/tests/fixtures" \
          --log "$WORK/node.jsonl" >/dev/null 2>&1 & MOCKS+=("$!")
  "$MOCK" --role wallet --port "$WALLET_PORT" --fixtures "$ROOT/tests/fixtures" \
          --log "$WORK/wallet.jsonl" --auth fmonero:t3st >/dev/null 2>&1 & MOCKS+=("$!")
  printf 'fmonero:t3st' > "$FMONERO_STATE_DIR/run/monero-wallet-rpc.$WALLET_PORT.login"
  chmod 600 "$FMONERO_STATE_DIR/run/monero-wallet-rpc.$WALLET_PORT.login"
  "$FM" settings set node.url   "http://127.0.0.1:$NODE_PORT"   >/dev/null
  "$FM" settings set wallet.url "http://127.0.0.1:$WALLET_PORT" >/dev/null
  "$FM" settings set wallet.port "$WALLET_PORT" >/dev/null
  # A synthetic daemon log for the Log screen, and a wallet name for the open modal.
  mkdir -p "$WORK/wallets/mainnet"
  : > "$WORK/wallets/mainnet/main.keys"
  "$FM" settings set wallet.dir "$WORK/wallets/mainnet" >/dev/null
  { echo "2026-09-16 14:02:09.114 I Monero 'Fluorine Fermi' (v0.18.5.1-release) started";
    echo "2026-09-16 14:02:10.002 I Synced 3541122/3541122";
    echo "2026-09-16 14:02:11.771 I [127.0.0.1:18080 OUT] NEW BLOCK 3541122"; } > "$WORK/bitmonero.log"
  "$FM" settings set node.log_file "$WORK/bitmonero.log" >/dev/null
  SCENARIO="fmonero-mock --role node/--role wallet --auth over tests/fixtures/ (no real daemon, no real wallet)"
  for _ in $(seq 60); do
    curl -s -o /dev/null --connect-timeout 1 --max-time 1 -d '{}' \
      "127.0.0.1:$WALLET_PORT/get_height" && break
    sleep 0.05
  done
fi

printf '\n== the TUI on a pty (%s)\n' "$SCENARIO"
# stdout MUST be the pty: `fmonero tui` refuses a redirected stdout with exit 2 (main.rs), so
# only stderr is captured and the screen is read back with capture-pane.
tmux new-session -d -s "$SESSION" -x 120 -y 40 \
  "$FM tui 2>$WORK/tui.err; echo \$? > $WORK/rc"
sleep 2

if tmux has-session -t "$SESSION" 2>/dev/null; then ok "fmonero tui opened on a pty"; else
  bad "fmonero tui did not stay open" "stderr: $(head -3 "$WORK/tui.err" 2>/dev/null)"
fi

pane() { tmux capture-pane -p -t "$SESSION" 2>/dev/null || true; }
visit() { # visit <key> <label> <needle> [capture-file]
  tmux send-keys -t "$SESSION" "$1" 2>/dev/null || true
  sleep 1
  local text; text="$(pane)"
  case "$text" in
    *"$3"*) ok "$2 renders ($3)" ;;
    *) bad "$2 did not render" "expected '$3' in the pane" "$(printf '%s' "$text" | head -4)" ;;
  esac
  [ -n "$CAPTURE" ] && [ -n "${4:-}" ] && printf '%s\n' "$text" > "$CAPTURE/$4"
  return 0
}

visit 1 "Node screen"   "Node"     "node.txt"
visit 2 "Wallet screen" "Balance"  "wallet.txt"
visit 3 "Send screen"   "Priority" "send.txt"
# The Send form owns printable keys while it is focused (otherwise an address typed into the
# To field would switch screens), so `Esc` leaves it first — which is what its own hint line
# and the help overlay say. This sequence is the documented rule, asserted.
tmux send-keys -t "$SESSION" Escape 2>/dev/null || true
sleep 0.4
visit 4 "Log screen"    "Log"      "log.txt"
visit "?" "help overlay" "Keys"    "help.txt"
tmux send-keys -t "$SESSION" Escape 2>/dev/null || true
sleep 0.4

# The two overlays that matter most, against the mock only: the receive QR and the send
# confirm modal — the whole safety surface. Nothing is broadcast: the modal wants the word
# "send" typed and Esc discards the built transaction (do_not_relay never committed it, §2a.3).
if [ -n "$CAPTURE" ]; then
  DEST=42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm
  tmux send-keys -t "$SESSION" 2 2>/dev/null || true; sleep 0.6
  visit "Q" "receive QR" "Receive" "qr.txt"
  tmux send-keys -t "$SESSION" Escape 2>/dev/null || true; sleep 0.4
  tmux send-keys -t "$SESSION" 3 2>/dev/null || true; sleep 0.6
  tmux send-keys -t "$SESSION" -l "$DEST" 2>/dev/null || true
  tmux send-keys -t "$SESSION" Tab 2>/dev/null || true
  tmux send-keys -t "$SESSION" -l "0.01" 2>/dev/null || true
  sleep 0.4
  tmux send-keys -t "$SESSION" Enter 2>/dev/null || true   # validate, then build
  sleep 3
  text="$(pane)"
  case "$text" in
    *'Type "send"'*) ok "the send confirm modal shows the built fee and wants the word"
                     printf '%s\n' "$text" > "$CAPTURE/send-confirm.txt" ;;
    *) bad "the confirm modal did not appear" "$(printf '%s' "$text" | sed -n '10,14p')" ;;
  esac
  # Esc discards: the built transaction is dropped, which releases the §2a.11 send.lock.
  tmux send-keys -t "$SESSION" Escape 2>/dev/null || true
  sleep 0.6
  RELAYED="$(grep -c '"method":"relay_tx"' "$WORK/wallet.jsonl" 2>/dev/null || true)"
  [ "${RELAYED:-0}" = "0" ] && ok "nothing was relayed (the capture run never broadcasts)" \
    || bad "a relay_tx reached the mock during a capture run"
  tmux send-keys -t "$SESSION" Escape 2>/dev/null || true
  sleep 0.4
fi

# q quits, and the exit code is the TUI's own (0 on a clean quit, §7a).
tmux send-keys -t "$SESSION" q 2>/dev/null || true
for _ in $(seq 50); do [ -f "$WORK/rc" ] && break; sleep 0.1; done
if [ -f "$WORK/rc" ]; then
  RC="$(cat "$WORK/rc")"
  [ "$RC" = "0" ] && ok "q quits cleanly (exit 0)" || bad "q quit with exit $RC"
else
  bad "the TUI did not exit within 5 s of q"
fi
# §7a: no print macro anywhere under src/tui — the runtime owns the terminal, and a stray
# `println!` corrupts the frame rather than appearing anywhere a test could read it. Asserted
# on the source, because on a pty the runtime's own writes are indistinguishable from a print.
if grep -rnE '\b(println!|print!|eprintln!|eprint!|dbg!)' "$ROOT/crates/fmonero/src/tui"; then
  bad "a print macro reached src/tui (§7a)"
else
  ok "no print macro under src/tui"
fi
# And stderr stays empty on a clean run: a panic or an anyhow message would land there.
if [ -s "$WORK/tui.err" ]; then
  bad "the TUI wrote to stderr" "$(head -3 "$WORK/tui.err")"
else
  ok "the TUI wrote nothing to stderr"
fi

if [ -n "$CAPTURE" ]; then
  printf '%s\n' "$SCENARIO" > "$CAPTURE/scenario.txt"
  printf '\n  captures in %s\n' "$CAPTURE"
fi
printf '\n== %s passed, %s failed\n' "$PASS" "$FAIL"
[ "$FAIL" = "0" ]
