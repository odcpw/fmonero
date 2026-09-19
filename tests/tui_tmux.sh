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
REMOTE_SESSION="fmonero-tui-remote-$$"
MOCKS=()
cleanup() {
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  tmux kill-session -t "$REMOTE_SESSION" 2>/dev/null || true
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
  # **Our** mocks must own those ports.` bind` failing is silent — the process exits and a
  # listener left over from another run answers in its place, serving the same fixtures from a
  # log this run cannot read. Everything on screen then looks right while every "the mock
  # recorded N requests" assertion reads an empty file, which is the shape of a test that cannot
  # fail. Probe each port and require the answer to land in OUR log.
  for spec in "$NODE_PORT:$WORK/node.jsonl" "$WALLET_PORT:$WORK/wallet.jsonl"; do
    port="${spec%%:*}"; log="${spec#*:}"
    for _ in $(seq 60); do
      curl -s -o /dev/null --connect-timeout 1 --max-time 1 -d '{}' "127.0.0.1:$port/get_height" \
        && break
      sleep 0.05
    done
    if [ ! -s "$log" ]; then
      echo "tui_tmux.sh: port $port is served by a process that is not this run's mock" >&2
      echo "             (nothing reached $log) — stop it and re-run" >&2
      exit 1
    fi
  done
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
#
# **The sandbox is forced into the pane with `env`, and that is not belt-and-braces.** A new
# session created inside an ALREADY-RUNNING tmux server inherits the SERVER's environment, not
# this shell's — so every `export FMONERO_*` above was invisible to the TUI whenever the owner
# (or another agent) had a tmux server up. The TUI then ran against `~/.config/fmonero`,
# 127.0.0.1:18081/18083 and the owner's real wallet: the gate passed vacuously in non-capture
# mode (the words "Node", "Balance", "cheapest", "Log", "Keys" all render in the empty state),
# and in `--capture` mode it typed a mainnet address and `0.01` into the Send form and pressed
# Enter, which is `send::build` taking the owner's real `send.lock` and issuing a real
# `transfer`. Worse, the "nothing was relayed" assertion counts `relay_tx` in the MOCK's log,
# which is empty precisely when nothing reached the mock — passing in exactly the case it exists
# for. Verified on this machine: with a server already running, `tmux new-session -d 'echo
# [$FMONERO_STATE_DIR]'` prints `[]`.
tmux new-session -d -s "$SESSION" -x 120 -y 40 \
  "env FMONERO_STATE_DIR=$FMONERO_STATE_DIR FMONERO_CONFIG_DIR=$FMONERO_CONFIG_DIR FMONERO_CACHE_DIR=$FMONERO_CACHE_DIR $FM tui 2>$WORK/tui.err; echo \$? > $WORK/rc"
sleep 2

# And the sandbox is asserted, not assumed: if the pane is not in it, everything below is a test
# of the owner's own machine.
if [ -n "$CAPTURE" ]; then
  sleep 1
  if grep -q '"method":"get_info"' "$WORK/node.jsonl" 2>/dev/null; then
    ok "the TUI is talking to the sandbox's node mock"
  else
    bad "the TUI reached no mock — it is NOT running in this test's sandbox" \
        "node.jsonl is empty, so every assertion below is about the owner's real configuration"
  fi
fi

if tmux has-session -t "$SESSION" 2>/dev/null; then ok "fmonero tui opened on a pty"; else
  bad "fmonero tui did not stay open" "stderr: $(head -3 "$WORK/tui.err" 2>/dev/null)"
fi

pane() { tmux capture-pane -p -t "$SESSION" 2>/dev/null || true; }
# `visit <key> <label> <needle>[|<needle>…] [capture-file]` — a `|`-separated needle list, because
# one screen has two correct shapes: the Wallet screen shows ` Balance ` with a wallet open and the
# §7a.5 empty-state panel without one, and v5.6.1 deleted the balance pane from the second (V12).
# A needle that matched both would have to be the tab strip, which every screen carries.
visit() { # visit <key> <label> <needle>[|<needle>…] [capture-file]
  tmux send-keys -t "$SESSION" "$1" 2>/dev/null || true
  sleep 1
  local text hit needle; text="$(pane)"; hit=""
  local IFS='|'
  for needle in $3; do
    case "$text" in *"$needle"*) hit="$needle"; break ;; esac
  done
  unset IFS
  if [ -n "$hit" ]; then
    ok "$2 renders ($hit)"
  else
    bad "$2 did not render" "expected one of '$3' in the pane" "$(printf '%s' "$text" | head -4)"
  fi
  [ -n "$CAPTURE" ] && [ -n "${4:-}" ] && printf '%s\n' "$text" > "$CAPTURE/$4"
  return 0
}

visit 1 "Node screen"   "Node"     "node.txt"
# v5.6.1, V12: with no wallet open there is no ` Balance ` pane to find — it rendered
# `— XMR   unlocked — XMR` above the empty state and is gone. The pane that IS there is the empty
# state's own, and `No wallet is open.` is its first line.
visit 2 "Wallet screen" "No wallet is open.| Balance "  "wallet.txt"
visit 3 "Send screen"   "cheapest" "send.txt"
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
  # **`Down`, not `Tab`.** v5.5 made `Tab` switch SCREENS in every focus state (§7a.1) and the
  # fields move with `↑↓` — this line still sent `Tab`, so the amount was typed into the Log
  # screen and the build never happened. The same script asserts the new rule forty lines below,
  # which is how a capture run came to contradict its own suite.
  tmux send-keys -t "$SESSION" Down 2>/dev/null || true
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
  # POSITIVELY first: the mock must have seen the build, or "zero relay_tx" is the answer an
  # empty log gives and proves nothing (that is exactly how the sandbox leak hid itself).
  BUILT="$(grep -c '"method":"transfer"' "$WORK/wallet.jsonl" 2>/dev/null || true)"
  RELAYED="$(grep -c '"method":"relay_tx"' "$WORK/wallet.jsonl" 2>/dev/null || true)"
  if [ "${BUILT:-0}" -ge 1 ]; then
    ok "the build reached the wallet mock ($BUILT transfer request(s))"
  else
    bad "no transfer reached the mock, so the relay assertion below would be vacuous"
  fi
  [ "${RELAYED:-0}" = "0" ] && ok "nothing was relayed (the capture run never broadcasts)" \
    || bad "a relay_tx reached the mock during a capture run"
  # And the Node capture is the fixture's height, not whatever the owner's node happens to say.
  if grep -q '3541122' "$CAPTURE/node.txt" 2>/dev/null; then
    ok "the Node capture carries the fixture height 3541122"
  else
    bad "the Node capture does not show the fixture height — it is not the mock's data" \
        "$(sed -n '3,6p' "$CAPTURE/node.txt" 2>/dev/null)"
  fi
  tmux send-keys -t "$SESSION" Escape 2>/dev/null || true
  sleep 0.4
fi


# ---------------------------------------------------------------- §7a.1/§7a.4/§7a.5 (v5.5)
#
# The three flows a ProgramSimulator cannot prove, because what is being tested is that the
# TERMINAL delivers the key to us rather than to the emulator, and that the screen a user looks at
# says what the program does.
printf '\n== v5.5: Tab out of a focused field, the picker, and the wallet empty state\n'

# 1. §7a.1, and it is the owner's first complaint on a real pty: type an address into the Send
#    form's To field — which is exactly where `Tab` used to be swallowed — and press Tab. The
#    rendered TAB STRIP must show Log.
tmux send-keys -t "$SESSION" 3 2>/dev/null || true; sleep 0.8
tmux send-keys -t "$SESSION" -l "42ey1afDFnn4886T7196doS9GPMzex" 2>/dev/null || true; sleep 0.4
tmux send-keys -t "$SESSION" Tab 2>/dev/null || true; sleep 1
TABTEXT="$(pane)"
case "$TABTEXT" in
  *"[ 4 Log ]"*) ok "§7a.1: Tab out of a focused Send field reaches Log on a real pty" ;;
  *) bad "Tab was captured by the Send field — the owner's first complaint" \
         "$(printf '%s' "$TABTEXT" | head -2)" ;;
esac
# And the status bar states the rule from wherever the user is.
case "$TABTEXT" in
  *"⇥ screens"*) ok "and the status bar says so on every screen" ;;
  *) bad "the status bar does not carry '⇥ screens'" "$(printf '%s' "$TABTEXT" | tail -2)" ;;
esac
# Shift+Tab goes back, which is the other half of the rule.
tmux send-keys -t "$SESSION" BTab 2>/dev/null || true; sleep 1
case "$(pane)" in
  *"[ 3 Send ]"*) ok "and ⇧Tab goes back" ;;
  *) bad "⇧Tab did not return to Send" "$(printf '%s' "$(pane)" | head -2)" ;;
esac
# `Esc` then unfocuses the form. **`1 2 3 4` are legal base58 characters**, so a focused To field
# keeps them — which is why §7a.1's answer to "I cannot change screens" is `Tab` and not the
# digits: `Tab` is the one key that works without leaving the field first, and from here on this
# script navigates with it.
tmux send-keys -t "$SESSION" Escape 2>/dev/null || true; sleep 0.6

# 2. §7a.4's picker, with NO cache: the panel, the chip and the disclosure — not an empty table
#    under a footer rendered against a `fetched_at` that does not exist (revision 8).
tmux send-keys -t "$SESSION" 1 2>/dev/null || true; sleep 1.2
PTEXT="$(pane)"
[ -n "$CAPTURE" ] && printf '%s\n' "$PTEXT" > "$CAPTURE/picker-empty.txt"
case "$PTEXT" in
  *"No list of public nodes yet."*) ok "§7a.4: the no-cache panel renders" ;;
  *) bad "the picker's no-cache panel is missing" "$(printf '%s' "$PTEXT" | tail -14)" ;;
esac
case "$PTEXT" in
  *"[f] Get the list"*) ok "with the chip that fetches it" ;;
  *) bad "the [f] chip is missing" "$(printf '%s' "$PTEXT" | tail -14)" ;;
esac
case "$PTEXT" in
  *"tells monero.fail your IP address"*) ok "and the one line saying what that discloses" ;;
  *) bad "the disclosure line is missing from the panel" "$(printf '%s' "$PTEXT" | tail -14)" ;;
esac
case "$PTEXT" in
  *"list  old"*|*"0 nodes ·"*) bad "the footer must be SUPPRESSED with no cache, not rendered empty" ;;
  *) ok "and no footer is rendered against a fetched_at that does not exist" ;;
esac

# 3. The picker WITH a cache: two column groups, a health ratio beside its sample size, and the
#    footer that surfaces `skipped_no_port`. The cache is written here, so nothing is fetched.
cat > "$FMONERO_CACHE_DIR/public-nodes.json" <<JSON
{"schema":1,"fetched_at":$(date +%s),"source":"https://monero.fail","skipped_no_port":37,
 "skipped_invalid":0,"notice_ack":$(date +%s),
 "nodes":[{"url":"http://pty.example:18081","address":"pty.example:18081","scheme":"http",
           "web_compatible":false,"custom":false,"health_available":true,"health_ratio":0.72,
           "health_checks":18,"health_last_height":3765172,
           "health_checked_at_text":"Fri, 18 Sep 2026 05:12:02 GMT",
           "health_checked_at":$(( $(date +%s) - 600 )),
           "probe":{"reachable":true,"height":3765172,"synchronized":true,"restricted":true,
                    "nettype":"mainnet","version":null,"latency_ms":100,"reason":null,
                    "probed_at":$(date +%s)}}]}
JSON
tmux send-keys -t "$SESSION" 4 2>/dev/null || true; sleep 0.5
tmux send-keys -t "$SESSION" 1 2>/dev/null || true; sleep 1.2
CTEXT="$(pane)"
[ -n "$CAPTURE" ] && printf '%s\n' "$CTEXT" > "$CAPTURE/picker.txt"
case "$CTEXT" in
  *"pty.example:18081"*) ok "§7a.4: the cached row renders, address first and never elided" ;;
  *) bad "the picker did not render the cached row" "$(printf '%s' "$CTEXT" | tail -14)" ;;
esac
case "$CTEXT" in
  *"72% of 18"*) ok "with the health ratio BESIDE its sample size (§2n.1)" ;;
  *) bad "the ratio must carry its sample size" "$(printf '%s' "$CTEXT" | tail -14)" ;;
esac
case "$CTEXT" in
  *"37 skipped"*) ok "and the footer surfaces the skip count" ;;
  *) bad "the footer does not surface the skip count" "$(printf '%s' "$CTEXT" | tail -4)" ;;
esac

# 4. §7a.5's wallet empty state: three chips and a service line, with no wallet open.
#
# **Under `--capture` this screen is not reachable, and the block says so instead of failing.** The
# mock serves an open wallet and is *stateless by design* (see its own header): `x` really issues
# `close_wallet` and the fixture really answers, but `get_address` and `get_balance` keep answering
# too, so `status` goes on reporting a wallet that is open and the empty state never renders. The
# close is asserted on what IS observable — the toast — and the four empty-state needles are
# skipped with the reason, because a test that fails on a correct program is a test that gets
# deleted.
tmux send-keys -t "$SESSION" 2 2>/dev/null || true; sleep 1.2
if [ -n "$CAPTURE" ]; then
  tmux send-keys -t "$SESSION" x 2>/dev/null || true; sleep 1.5
  case "$(pane)" in
    *"wallet closed"*) ok "§7a.5: x closes the open wallet on a real pty (the toast says so)" ;;
    *) bad "x did not report closing the wallet" "$(printf '%s' "$(pane)" | tail -3)" ;;
  esac
  printf '%s\n' "$(pane)" > "$CAPTURE/wallet-empty.txt"
  printf '  skip  §7a.5 the empty state: the mock is stateless, so a closed wallet still answers\n'
else
  WTEXT="$(pane)"
  for needle in "[o] Open a wallet" "[c] Create wallet" "[W] Start the wallet service" "wallet service:"; do
    case "$WTEXT" in
      *"$needle"*) ok "§7a.5: the empty state carries '$needle'" ;;
      *) bad "the empty state is missing '$needle'" "$(printf '%s' "$WTEXT" | sed -n '5,12p')" ;;
    esac
  done
fi

# 5. §7a.1 revision 1, the blocker's own flow: fill ALL FOUR create-wallet controls from the
#    keyboard alone and reach the seed-backup modal. It is refused before anything is sent — the
#    name already exists in `wallet.dir` under --capture — so this asserts the modal is REACHABLE
#    and that its refusal is inline, which is the half a trap would fail.
tmux send-keys -t "$SESSION" c 2>/dev/null || true; sleep 0.8
CRTEXT="$(pane)"
case "$CRTEXT" in
  *"Create a wallet"*) ok "§7a.5: c opens the create modal (it was close, not create, before v5.5)" ;;
  *) bad "c did not open the create modal" "$(printf '%s' "$CRTEXT" | sed -n '10,18p')" ;;
esac
tmux send-keys -t "$SESSION" -l "ptywallet" 2>/dev/null || true
tmux send-keys -t "$SESSION" Down 2>/dev/null || true
tmux send-keys -t "$SESSION" -l "pw1" 2>/dev/null || true
tmux send-keys -t "$SESSION" Down 2>/dev/null || true
tmux send-keys -t "$SESSION" -l "pw2" 2>/dev/null || true
tmux send-keys -t "$SESSION" Down 2>/dev/null || true
sleep 0.8
FILLED="$(pane)"
[ -n "$CAPTURE" ] && printf '%s\n' "$FILLED" > "$CAPTURE/create-filled.txt"
case "$FILLED" in
  *"ptywallet"*) ok "and ↑↓ reach every control: the name survived three Downs" ;;
  *) bad "the name field lost its text while moving between controls" \
         "$(printf '%s' "$FILLED" | sed -n '10,18p')" ;;
esac
case "$FILLED" in
  *pw1*|*pw2*) bad "a password field is NOT masked (§2b.4)" ;;
  *) ok "and both password fields are masked" ;;
esac
tmux send-keys -t "$SESSION" Enter 2>/dev/null || true; sleep 1
case "$(pane)" in
  *"do not match"*) ok "a mismatched pair is refused INLINE, with nothing sent" ;;
  *) bad "the mismatch refusal did not render" "$(printf '%s' "$(pane)" | sed -n '10,20p')" ;;
esac
# And `Tab` is held here, which is the one documented exception — said in the modal itself.
tmux send-keys -t "$SESSION" Tab 2>/dev/null || true; sleep 0.8
case "$(pane)" in
  *"Esc to discard first"*) ok "§7a.1: the create modal holds Tab and says so in the modal" ;;
  *) bad "the create modal must consume Tab" "$(printf '%s' "$(pane)" | tail -4)" ;;
esac
tmux send-keys -t "$SESSION" Escape 2>/dev/null || true; sleep 0.5

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

# ---------------------------------------------------------------- §2m remote node mode
#
# The verifier's finding: the Node screen's remote rendering and the status bar's remote marker
# were covered by unit tests and by one hand-made pty capture pasted into DECISIONS.md, and by
# nothing that runs in a gate. This is that case. It needs **no daemon of any kind** — in remote
# mode `status` constructs no node client and makes no node call (§2m.3) — so it runs in the
# default mode as well as under `--capture`, which is the point: the screen a remote-mode owner
# looks at is now asserted on every run.
printf '\n== the TUI in remote mode (no daemon, §2m.3)\n'
R_STATE="$WORK/remote/state"; R_CONFIG="$WORK/remote/config"; R_CACHE="$WORK/remote/cache"
mkdir -p "$R_STATE/run" "$R_CONFIG" "$R_CACHE"
chmod 700 "$R_STATE" "$R_STATE/run" "$R_CONFIG" "$R_CACHE"
REMOTE_ADDR="node.example.com:18089"
# Written with the CLI, so the settings the TUI reads are the ones §2m.1's invariant accepts —
# and the order is the invariant's: the address first, then the mode.
env FMONERO_STATE_DIR="$R_STATE" FMONERO_CONFIG_DIR="$R_CONFIG" FMONERO_CACHE_DIR="$R_CACHE" \
  "$FM" settings set node.remote "$REMOTE_ADDR" >/dev/null
env FMONERO_STATE_DIR="$R_STATE" FMONERO_CONFIG_DIR="$R_CONFIG" FMONERO_CACHE_DIR="$R_CACHE" \
  "$FM" settings set node.mode remote >/dev/null
tmux new-session -d -s "$REMOTE_SESSION" -x 120 -y 40 \
  "env FMONERO_STATE_DIR=$R_STATE FMONERO_CONFIG_DIR=$R_CONFIG FMONERO_CACHE_DIR=$R_CACHE $FM tui 2>$WORK/tui-remote.err; echo \$? > $WORK/rc-remote"
sleep 2
tmux send-keys -t "$REMOTE_SESSION" 1 2>/dev/null || true
sleep 1.5
RTEXT="$(tmux capture-pane -p -t "$REMOTE_SESSION" 2>/dev/null || true)"
[ -n "$CAPTURE" ] && printf '%s\n' "$RTEXT" > "$CAPTURE/node-remote.txt"
# **v5.6 (§7a.3): the screen shows what IS known instead of two panes of what is not.** The two
# panes that read `not available with a remote node` are gone, and one ` This node ` pane carries
# the cached check — here in its honest empty state, because nothing has checked this address yet.
case "$RTEXT" in
  *"This node"*) ok "the Node screen renders the public-node facts pane (§7a.3)" ;;
  *) bad "the Node screen did not render ' This node '" \
         "$(printf '%s' "$RTEXT" | sed -n '1,8p')" ;;
esac
case "$RTEXT" in
  *"not available"*) bad "a pane still says 'not available'" \
         "$(printf '%s' "$RTEXT" | sed -n '1,12p')" ;;
  *) ok "no pane says 'not available' any more (§2o)" ;;
esac
case "$RTEXT" in
  *"public node"*) ok "the status bar names the public node" ;;
  *) bad "the status bar does not say 'public node'" \
         "$(printf '%s' "$RTEXT" | tail -3)" ;;
esac
case "$RTEXT" in
  *"$REMOTE_ADDR"*) ok "and the address in force is on screen ($REMOTE_ADDR)" ;;
  *) bad "the remote address is not shown anywhere" "$(printf '%s' "$RTEXT" | sed -n '1,8p')" ;;
esac
# `u` says so and names the two keys that do something about it; it must never run systemctl
# (§2m.3) and — from v5.6 — **must never print a shell command** (§2o.1). That refusal was the
# single worst instance of the rule the owner asked for: *"I don't want to manage the CLI."*
tmux send-keys -t "$REMOTE_SESSION" u 2>/dev/null || true
sleep 1
RTEXT2="$(tmux capture-pane -p -t "$REMOTE_SESSION" 2>/dev/null || true)"
case "$RTEXT2" in
  # The status line is trimmed to the pane's width, so the assertion is on the half that fits —
  # which still has to be the part naming a key, not a command.
  *"You are using a public node. Press f"*)
    ok "u names the in-app way out (§2o.1)" ;;
  *) bad "u did not name the in-app way out" "$(printf '%s' "$RTEXT2" | tail -3)" ;;
esac
case "$RTEXT2" in
  *"fmonero node local"*) bad "u printed a shell command at a user who cannot type it (§2o.1)" \
         "$(printf '%s' "$RTEXT2" | tail -3)" ;;
  *) ok "and prints no shell command" ;;
esac
# v5.6, §7a.4: `l` is the in-app way back, and it asks first.
tmux send-keys -t "$REMOTE_SESSION" l 2>/dev/null || true
sleep 1
RTEXT3="$(tmux capture-pane -p -t "$REMOTE_SESSION" 2>/dev/null || true)"
[ -n "$CAPTURE" ] && printf '%s\n' "$RTEXT3" > "$CAPTURE/node-remote-local.txt"
case "$RTEXT3" in
  *"Use your own node"*) ok "l opens the confirm that switches back (§7a.4)" ;;
  *) bad "l did not open the 'Use your own node' confirm" "$(printf '%s' "$RTEXT3" | sed -n '1,12p')" ;;
esac
tmux send-keys -t "$REMOTE_SESSION" Escape 2>/dev/null || true
sleep 0.5
tmux send-keys -t "$REMOTE_SESSION" q 2>/dev/null || true
for _ in $(seq 50); do [ -f "$WORK/rc-remote" ] && break; sleep 0.1; done
if [ -f "$WORK/rc-remote" ]; then
  RC="$(cat "$WORK/rc-remote")"
  [ "$RC" = "0" ] && ok "q quits cleanly in remote mode (exit 0)" || bad "remote-mode q quit with exit $RC"
else
  bad "the remote-mode TUI did not exit within 5 s of q"
fi
if [ -s "$WORK/tui-remote.err" ]; then
  bad "the remote-mode TUI wrote to stderr" "$(head -3 "$WORK/tui-remote.err")"
else
  ok "the remote-mode TUI wrote nothing to stderr"
fi

if [ -n "$CAPTURE" ]; then
  printf '%s\n' "$SCENARIO" > "$CAPTURE/scenario.txt"
  printf '\n  captures in %s\n' "$CAPTURE"
fi
printf '\n== %s passed, %s failed\n' "$PASS" "$FAIL"
[ "$FAIL" = "0" ]
