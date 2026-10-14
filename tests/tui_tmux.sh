#!/usr/bin/env bash
# Actual PTY qualification against scratch fixtures only. Never builds.
# Root first builds fmonero + fmonero-mock with scripts/cargo-serial.sh.
# Run: tests/tui_tmux.sh [--bin PATH] [--mock PATH] [--capture NEW_DIRECTORY]
# SGR mouse injection exercises the real application parser and hit regions.
# It does not certify desktop terminal mouse delivery or native Monero behavior.
set -euo pipefail
umask 077
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FM="$ROOT/target/release/fmonero"
MOCK="$ROOT/target/release/fmonero-mock"
CAPTURE=""
while (($#)); do
  case "$1" in
    --bin) FM="${2:?--bin requires a prebuilt binary}"; shift 2 ;;
    --mock) MOCK="${2:?--mock requires a prebuilt mock}"; shift 2 ;;
    --capture) CAPTURE="${2:?--capture requires a new directory}"; shift 2 ;;
    -h|--help) sed -n '2,6p' "$0"; exit 0 ;;
    *) printf 'Unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
done
for program in tmux python3 timeout realpath; do
  command -v "$program" >/dev/null || { printf 'Required: %s\n' "$program" >&2; exit 2; }
done
[[ -x "$FM" && -x "$MOCK" ]] || {
  echo "Prebuilt fmonero and fmonero-mock required; this harness never builds." >&2; exit 2;
}
FM="$(realpath "$FM")"
MOCK="$(realpath "$MOCK")"
if [[ -n "$CAPTURE" ]]; then
  mkdir -m 700 -- "$CAPTURE" # Refuse overwrite of previous evidence or arbitrary directories.
  CAPTURE="$(cd "$CAPTURE" && pwd)"
fi
WORK="$(mktemp -d -t fmonero-pty.XXXXXXXX)"
SOCKET="$WORK/tmux.sock"
MOCKS=()
PASS=0
PHASE=setup
PANE=""
FRAME=""
tm() { timeout -k 1s 3s tmux -S "$SOCKET" -f /dev/null "$@"; }
cleanup() {
  # This socket belongs to this fresh private directory, never the owner's server.
  tm kill-session -t gate >/dev/null 2>&1 || true
  for pid in "${MOCKS[@]}"; do kill "$pid" 2>/dev/null || true; done
  for ((i=0; i<20; i++)); do
    live=0
    for pid in "${MOCKS[@]}"; do kill -0 "$pid" 2>/dev/null && live=1; done
    ((live)) || break
    sleep 0.05
  done
  for pid in "${MOCKS[@]}"; do
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf -- "$WORK"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Unicode terminal-cell coordinates and structured request counts. No request
# bodies are printed, and exported screens redact even synthetic secret canaries.
cat > "$WORK/read.py" <<'PY'
import json, pathlib, re, sys, unicodedata
mode, *args = sys.argv[1:]
if mode == "count":
    path, method, *status = args
    rows = pathlib.Path(path).read_text().splitlines() if pathlib.Path(path).exists() else []
    calls = [json.loads(row) for row in rows]
    print(sum(r.get("method") == method and
              (not status or r.get("status") == int(status[0])) for r in calls))
elif mode == "prepared":
    rows = [json.loads(row) for row in pathlib.Path(args[0]).read_text().splitlines()]
    calls = [r for r in rows if r.get("method") == "transfer_split" and r.get("status") == 200]
    assert len(calls) == 1, "exactly one answered preparation must reach our mock"
    p = calls[0]["body"]["params"]
    assert p["do_not_relay"] is True and p["get_tx_metadata"] is True
    assert p["destinations"] == [{"address": args[1], "amount": 10_000_000_000}]
elif mode == "locate":
    needle, row = args
    matches = []
    for y, line in enumerate(sys.stdin.read().splitlines()):
        if row not in ("any", "first") and y != int(row):
            continue
        at = line.find(needle)
        if at >= 0:
            x = sum(0 if unicodedata.combining(c) else
                    2 if unicodedata.east_asian_width(c) in ("W", "F") else 1
                    for c in line[:at])
            matches.append((x + max(0, len(needle)//2), y))
    if not matches or (row != "first" and len(matches) != 1):
        sys.exit(1)
    print(*matches[0])
elif mode == "masked":
    lines = sys.stdin.read().splitlines()
    for label in ("Password", "Again"):
        values = [m.group(1) for line in lines
                  if (m := re.search(r"(?:^|[ >])" + label + r"\s+([^│]*)", line))]
        if len(values) != 1 or "•" not in values[0] or any(c.isalnum() for c in values[0]):
            sys.exit(1)
elif mode == "safe":
    text = sys.stdin.read()
    for token in ("PTY_SECRET_qvorn123456_A", "PTY_SECRET_qvorn123456_B", "PTY_LOG_SECRET_8372"):
        text = text.replace(token, "[synthetic secret redacted]")
    # A broken masked editor might expose only a horizontally clipped suffix.
    text = re.sub(r"((?:^|[ >])(?:Password|Again)\s+)[^│\n]*",
                  r"\1[protected field]", text, flags=re.M)
    print(text, end="")
PY
safe() { python3 "$WORK/read.py" safe; }
ok() { PASS=$((PASS+1)); printf '  ok  [%s] %s\n' "$PHASE" "$1"; }
fail() {
  printf '  FAIL [%s] %s\n' "$PHASE" "$1" >&2
  if [[ -n "$PANE" ]]; then
    tm capture-pane -p -t "$PANE" 2>/dev/null | safe >&2 || true
  fi
  [[ ! -s "$WORK/$PHASE.err" ]] || safe < "$WORK/$PHASE.err" >&2
  exit 1
}
frame() { FRAME="$(tm capture-pane -p -t "$PANE")" || fail "PTY disappeared"; }
wait_text() {
  local label="$1" needle="$2"
  for ((attempt=0; attempt<80; attempt++)); do
    frame
    if [[ "$FRAME" == *"$needle"* ]]; then ok "$label"; return; fi
    [[ ! -f "$WORK/$PHASE.rc" ]] || fail "$label: TUI exited early"
    sleep 0.1
  done
  fail "$label: expected visible '$needle' within 8 seconds"
}
absent() {
  frame
  [[ "$FRAME" != *"$2"* ]] || fail "$1"
  ok "$1"
}
wait_absent() {
  local label="$1" needle="$2"
  for ((attempt=0; attempt<80; attempt++)); do
    frame
    if [[ "$FRAME" != *"$needle"* ]]; then ok "$label"; return; fi
    sleep 0.1
  done
  fail "$label: '$needle' remained visible"
}
masked() {
  for ((attempt=0; attempt<80; attempt++)); do
    frame
    for sentinel in PTY_SECRET_qvorn123456_A PTY_SECRET_qvorn123456_B 123456_A 123456_B; do
      [[ "$FRAME" != *"$sentinel"* ]] || fail "protected input appeared in the PTY"
    done
    if printf '%s\n' "$FRAME" | python3 "$WORK/read.py" masked; then
      ok "both protected editors render masks and no plaintext"
      return
    fi
    sleep 0.1
  done
  fail "both visible protected fields must contain masks, never letters/digits"
}
save_frame() {
  frame
  [[ -z "$CAPTURE" ]] || printf '%s\n' "$FRAME" | safe > "$CAPTURE/$PHASE-$1.txt"
}
keys() {
  local key
  for key in "$@"; do
    tm send-keys -t "$PANE" -- "$key"
    # ESC immediately followed by a digit is an Alt-digit terminal sequence.
    # Separate an intentional Escape key before sending the next gesture.
    [[ "$key" != Escape ]] || sleep 0.15
  done
}
literal() { tm send-keys -t "$PANE" -l -- "$1"; }
click_text() {
  local needle="$1" row="${2:-any}" point x y bytes
  frame
  point="$(printf '%s\n' "$FRAME" | python3 "$WORK/read.py" locate "$needle" "$row")" \
    || fail "mouse target '$needle' missing or ambiguous"
  read -r x y <<< "$point"
  printf -v bytes '\033[<0;%d;%dM\033[<0;%d;%dm' "$((x+1))" "$((y+1))" "$((x+1))" "$((y+1))"
  literal "$bytes"
}
resize() {
  local width="$1" height="$2"
  tm resize-window -t gate:0 -x "$width" -y "$height"
  for ((attempt=0; attempt<40; attempt++)); do
    [[ "$(tm display-message -p -t "$PANE" '#{pane_width}x#{pane_height}')" == "$width"x"$height" ]] && break
    sleep 0.05
  done
  [[ "$(tm display-message -p -t "$PANE" '#{pane_width}x#{pane_height}')" == "$width"x"$height" ]] \
    || fail "tmux did not deliver requested ${width}x${height}"
  sleep 0.2 # Allow SIGWINCH through the real event loop; callers assert redraw content.
  ok "actual pane resized to ${width}x${height}"
}
count() { python3 "$WORK/read.py" count "$@"; }
zero_calls() {
  local log="$1"; shift
  for method in "$@"; do
    [[ "$(count "$log" "$method")" == 0 ]] || fail "unexpected $method RPC attempt"
  done
}
start_mock() {
  local name="$1"; shift
  "$MOCK" "$@" --port 0 --log "$WORK/$name.jsonl" >"$WORK/$name.out" 2>"$WORK/$name.boot" &
  MOCKS+=("$!")
  MOCK_PORT=""
  for ((attempt=0; attempt<80; attempt++)); do
    kill -0 "${MOCKS[-1]}" 2>/dev/null || fail "scratch mock $name exited before binding"
    MOCK_PORT="$(sed -nE 's/.* on 127\.0\.0\.1:([0-9]+)$/\1/p' "$WORK/$name.boot")"
    [[ "$MOCK_PORT" =~ ^[0-9]+$ ]] && return
    sleep 0.05
  done
  fail "scratch mock $name did not bind within 4 seconds"
}
start_mock node --role node --fixtures "$ROOT/tests/fixtures"
NODE_PORT="$MOCK_PORT"
start_mock wallet --role wallet --fixtures "$ROOT/tests/fixtures" --auth fmonero:t3st
WALLET_PORT="$MOCK_PORT"
start_mock closed --role wallet --fixtures "$ROOT/tests/fixtures" --auth fmonero:t3st \
  --fail 'get_height=-13:No wallet file' --fail 'get_address=-13:No wallet file'
CLOSED_PORT="$MOCK_PORT"
start_mock nodes --static "$ROOT/tests/fixtures/nodes"
NODES_PORT="$MOCK_PORT"

configure() {
  PHASE="$1"
  local port="$2"
  BASE="$WORK/$PHASE"
  SANDBOX=(
    "FMONERO_STATE_DIR=$BASE/state"
    "FMONERO_CONFIG_DIR=$BASE/config"
    "FMONERO_CACHE_DIR=$BASE/cache"
    "XDG_DATA_HOME=$BASE/data"
    "FMONERO_SYSTEMD_USER_DIR=$BASE/systemd/user"
    "FMONERO_SYSTEMCTL_FIXTURE=$BASE/systemctl.fixture"
    "FMONERO_NODES_URL=http://127.0.0.1:$NODES_PORT/nodes.json"
  )
  mkdir -p "$BASE/state/run" "$BASE/config" "$BASE/cache" "$BASE/data" "$BASE/systemd/user" "$BASE/wallets"
  : > "$BASE/systemctl.fixture"
  printf 'fmonero:t3st' > "$BASE/state/run/monero-wallet-rpc.$port.login"
  cli settings set node.url "http://127.0.0.1:$NODE_PORT" >/dev/null
  cli settings set wallet.url "http://127.0.0.1:$port" >/dev/null
  cli settings set wallet.port "$port" >/dev/null
  cli settings set wallet.dir "$BASE/wallets" >/dev/null
  cli settings set node.data_dir "$BASE/chain" >/dev/null
  cli settings set node.log_file "$BASE/node.log" >/dev/null
  cli settings set tui.privacy false >/dev/null
  cli settings set tui.notifications false >/dev/null
  printf 'PTY_NODE_LOG ordinary fixture event\n' > "$BASE/node.log"
  printf 'PTY_WALLET_LOG ordinary fixture event\npassword: PTY_LOG_SECRET_8372\n' > "$BASE/state/run/monero-wallet-rpc.log"
}
cli() { timeout -k 2s 20s env "${SANDBOX[@]}" "$FM" "$@"; }
cat > "$WORK/pane.sh" <<'SH'
#!/usr/bin/env bash
set -uo pipefail
fm="$1"; result="$2"; errors="$3"; proof="$4"; shift 4
for key in FMONERO_STATE_DIR FMONERO_CONFIG_DIR FMONERO_CACHE_DIR XDG_DATA_HOME \
           FMONERO_SYSTEMD_USER_DIR FMONERO_SYSTEMCTL_FIXTURE FMONERO_NODES_URL; do
  [[ -n "${!key:-}" ]] || exit 98
  printf '%s=%s\n' "$key" "${!key}"
done > "$proof"
"$fm" tui "$@" 2>"$errors"
rc=$?
printf '%s\n' "$rc" > "$result"
exit "$rc"
SH
launch() {
  PANE="$(tm new-session -d -P -F '#{pane_id}' -s gate -x 120 -y 40 \
    env "${SANDBOX[@]}" TERM=xterm-256color LC_ALL=C.UTF-8 \
    bash "$WORK/pane.sh" "$FM" "$WORK/$PHASE.rc" "$WORK/$PHASE.err" "$WORK/$PHASE.env" "$@")"
  tm set-option -t gate status off
  tm set-window-option -t gate:0 window-size manual
  tm set-option -t gate history-limit 0
  for ((attempt=0; attempt<50; attempt++)); do
    [[ -f "$WORK/$PHASE.env" ]] && break
    sleep 0.05
  done
  printf '%s\n' "${SANDBOX[@]}" > "$WORK/expected.env"
  cmp -s "$WORK/expected.env" "$WORK/$PHASE.env" || fail "pane did not inherit all seven exact sandbox variables"
  resize 120 40
  ok "all seven sandbox variables verified inside the private PTY"
}
finish() {
  keys q
  for ((attempt=0; attempt<80; attempt++)); do
    [[ -f "$WORK/$PHASE.rc" ]] && break
    sleep 0.1
  done
  [[ -f "$WORK/$PHASE.rc" && "$(cat "$WORK/$PHASE.rc")" == 0 ]] || fail "q did not exit cleanly within 8 seconds"
  [[ ! -s "$WORK/$PHASE.err" ]] || fail "TUI wrote to stderr"
  [[ ! -s "$BASE/systemctl.fixture" ]] || fail "service fixture was mutated"
  PANE=""
  tm kill-session -t gate >/dev/null 2>&1 || true
  ok "clean exit, empty stderr, unchanged service fixture"
}

# Both input modalities reach the same local refusal. The deliberately closed
# mock returns -13, so a stateless success fixture cannot fake the empty state.
configure closed "$CLOSED_PORT"
launch
wait_text "Wallet is the default screen" "No wallet is open."
[[ "$(count "$WORK/closed.jsonl" get_height 200)" -gt 0 ]] || fail "no positive scratch-wallet contact"
[[ "$(count "$WORK/node.jsonl" get_info 200)" -gt 0 ]] || fail "no positive scratch-node contact"
for size in '40 10' '80 24' '120 40'; do
  read -r width height <<< "$size"
  resize "$width" "$height"
  for mode in keyboard mouse; do
    wait_text "create control survives resize" "(C)reate"
    if [[ "$mode" == keyboard ]]; then keys c; else click_text "(C)reate"; fi
    wait_text "$mode opens create form at ${width}x${height}" "Create encrypted wallet"
    literal PTY_FORM_nrocqv123456
    keys Tab
    literal PTY_SECRET_qvorn123456_A
    keys Tab
    literal PTY_SECRET_qvorn123456_B
    keys Tab
    wait_text "Tab remains in the modal" "Language"
    masked
    if [[ "$mode" == keyboard ]]; then keys Enter; else click_text "(Enter)"; fi
    wait_text "mismatched secrets are refused inline" "passwords do not match"
    masked
    save_frame "create-$width-$mode"
    if [[ "$mode" == keyboard ]]; then keys Escape; else click_text "(Esc) Cancel"; fi
    wait_absent "$mode cancel dismisses the create modal" "Create encrypted wallet"
    wait_text "$mode cancel returns to wallet" "No wallet is open."
    keys c
    wait_text "reopening creates an empty form" "Create encrypted wallet"
    absent "cancel discarded the prior form name" "PTY_FORM_nrocqv123456"
    keys Escape
    wait_absent "empty form dismissed" "Create encrypted wallet"
  done
done
zero_calls "$WORK/closed.jsonl" create_wallet open_wallet restore_deterministic_wallet query_key \
  transfer transfer_split sweep_all relay_tx submit_transfer
ok "invalid/cancelled forms sent no wallet mutation or secret-query RPC"
finish

# Record identity via the real guarded opener, against a scratch stateless mock.
configure open "$WALLET_PORT"
: > "$BASE/wallets/main.keys"
printf '\n' | cli wallet open main --password-stdin --json > "$WORK/open-result.json"
[[ "$(count "$WORK/wallet.jsonl" open_wallet 200)" == 1 ]] || fail "guarded mock open was not observed"
launch
wait_text "open wallet fixture rendered" "Balance"
declare -a names=("" Node Wallet Send Logs Receive Tools)
declare -a bodies=("" Synced Balance "Send · prepare" "log ·" "(N)ew address" "Restore wallet")
for size in '40 10' '80 24' '120 40'; do
  read -r width height <<< "$size"
  resize "$width" "$height"
  for key in 1 2 3 4 5 6; do
    keys Escape "$key"
    wait_text "key $key reaches ${names[$key]} at ${width}x${height}" "${bodies[$key]}"
    save_frame "${names[$key]}-$width"
    # Leave the target first: an inert mouse must not pass by coincidence.
    keys Escape
    if [[ "$key" == 1 ]]; then keys 2; else keys 1; fi
    wait_text "leave target before mouse parity" "$([[ "$key" == 1 ]] && echo Balance || echo Synced)"
    click_text " $key " 0
    wait_text "mouse tab reaches ${names[$key]} at ${width}x${height}" "${bodies[$key]}"
  done
done
resize 80 24
keys Escape 3
wait_text "Send field active" "Recipient"
literal "nrocqv123456"
keys Tab
literal "0.01"
wait_text "Tab advances to amount without navigating away" "0.01"
absent "typed v did not toggle shared privacy" "Privacy on"
keys BTab C-a
literal "EDITED"
wait_text "Shift+Tab returns to recipient" "EDITED"
keys Escape 4
wait_text "Escape releases input for screen navigation" "Wallet log"
wait_text "own synthetic wallet log is visible" "PTY_WALLET_LOG"
absent "secret-bearing log line is redacted" "PTY_LOG_SECRET_8372"
keys /
literal "qv123456"
wait_text "log search owns navigation and quit keys" "qv123456"
keys C-a BSpace Escape s
wait_text "log source changes to local node" "Local node log"
wait_text "own node log is visible" "PTY_NODE_LOG"
save_frame logs-source

keys Escape 6
wait_text "Tools groups are discoverable" "Offline"
keys Enter
wait_text "keyboard Tools workflow opens restore fields" "Seed words"
keys Escape
wait_absent "restore cancelled by keyboard" "Seed words"
click_text "Restore wallet"
wait_text "mouse Tools workflow opens same restore fields" "Seed words"
keys Escape
wait_absent "second restore cancelled without backend work" "Seed words"
for size in '40 10' '80 24' '120 40'; do
  read -r width height <<< "$size"
  resize "$width" "$height"
  keys '?'
  wait_text "help opens at ${width}x${height}" "Keys ·"
  before="$(tm capture-pane -p -t "$PANE" | sed '1d;$d')"
  # The modal is above the inert Tools footer, which uses the same label.
  click_text "(↓) More" first
  sleep 0.2
  frame
  if [[ "$width" == 40 ]]; then
    [[ "$(printf '%s\n' "$FRAME" | sed '1d;$d')" != "$before" ]] || fail "short help More did not scroll"
  fi
  # Reach a late binding through actual scrolling; a changed footer alone is insufficient.
  for ((step=0; step<160; step++)); do
    frame
    [[ "$FRAME" == *"Mouse"* ]] && break
    keys Down
    sleep 0.05
  done
  wait_text "help retains late mouse/accessibility binding" "Mouse"
  click_text "(Esc) Close"
  wait_absent "help close dismisses overlay" "Keys ·"
  wait_text "help mouse close returns to Tools" "Offline"
done

# Exact fixture-backed prepare/review/cancel. Never type the confirmation word.
resize 120 40
keys 3
wait_text "Send form before preparation" "Recipient"
click_text "Recipient"
keys C-a
literal "42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm"
keys Tab C-a
literal "0.01"
keys Enter
wait_text "prepared payment requires immutable review" "Review payment"
wait_text "review requires typed confirmation" "Type send to confirm"
wait_text "review exposes exact amount" "0.010000000000 XMR"
save_frame payment-review
python3 "$WORK/read.py" prepared "$WORK/wallet.jsonl" \
  "42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm" \
  || fail "preparation did not preserve do_not_relay, exact amount and recipient"
for size in '40 10' '80 24' '120 40'; do
  read -r width height <<< "$size"
  resize "$width" "$height"
  wait_text "payment review survives resize" "Review payment"
  wait_text "payment Cancel remains visible" "(Esc) Cancel"
done
keys Enter # Empty confirmation must not submit, even after a complete review was visible.
wait_text "Enter alone cannot approve a payment" "Review payment"
click_text "(Esc) Cancel"
wait_absent "cancel discards immutable review" "Review payment"
wait_text "mouse Cancel returns to Send form" "Send · prepare"
zero_calls "$WORK/wallet.jsonl" relay_tx submit_transfer create_wallet restore_deterministic_wallet query_key
ok "one answered mock preparation, zero relay/submit attempts"
keys Escape 2
cli settings set tui.privacy true >/dev/null
wait_text "external privacy policy masks active TUI" "Privacy on"
absent "privacy hides fixture address" "42ey1afD"
keys 5
wait_text "navigation keeps shared privacy enabled" "Privacy on"
keys v
wait_text "explicit privacy toggle restores Receive" "(N)ew address"
keys Escape
finish

# The remote address is our own mock: accidental probes are observable locally.
configure remote "$CLOSED_PORT"
cli settings set node.remote "127.0.0.1:$NODE_PORT" >/dev/null
cli settings set node.mode remote >/dev/null
before_entry="$(count "$WORK/node.jsonl" get_info 200)"
before_set="$(count "$WORK/closed.jsonl" set_daemon)"
launch --node
wait_text "remote facts are shown" "This node"
wait_text "configured public node is shown" "127.0.0.1:$NODE_PORT"
# Entering an active Node page deliberately refreshes its configured node once.
# The later cancelled confirmations must not add requests or fetch a node list.
for ((attempt=0; attempt<50; attempt++)); do
  [[ "$(count "$WORK/node.jsonl" get_info 200)" -gt "$before_entry" ]] && break
  sleep 0.1
done
[[ "$(count "$WORK/node.jsonl" get_info 200)" == "$((before_entry+1))" ]] \
  || fail "active remote entry did not issue exactly one configured-node query"
before_node="$(wc -l < "$WORK/node.jsonl")"
ok "active remote entry queried only its configured node once"
keys l
wait_text "connection change requires confirmation" "Use your own node"
for size in '40 10' '80 24' '120 40'; do
  read -r width height <<< "$size"
  resize "$width" "$height"
  wait_text "connection Cancel remains visible" "(Esc) Cancel"
done
click_text "(Esc) Cancel"
wait_absent "cancel dismisses connection review" "Use your own node"
wait_text "cancelling keeps remote mode" "This node"
keys f
wait_text "fetch asks before contacting the list" "Before contacting anyone"
click_text "(Esc) Cancel"
wait_absent "cancel dismisses fetch consent" "Before contacting anyone"
wait_text "cancelled fetch returns to remote view" "This node"
[[ ! -s "$WORK/nodes.jsonl" ]] || fail "cancelled consent contacted node-list mock"
[[ "$(wc -l < "$WORK/node.jsonl")" == "$before_node" ]] || fail "cancelled remote confirmation made an additional node query"
[[ "$(count "$WORK/closed.jsonl" set_daemon)" == "$before_set" ]] || fail "cancelled connection changed daemon"
ok "cancelled confirmations issued no additional node/list/connection requests"
save_frame node-remote
finish

if [[ -n "$CAPTURE" ]]; then
  printf '%s\n' \
    "Fixture-only actual PTY run; no native wallet, funded payment or device qualification." \
    "SGR injection tests the app parser/hit regions, not terminal-emulator mouse delivery." \
    "Seven sandbox variables checked per pane; private tmux socket and ephemeral mock ports." \
    "Only synthetic/redacted screen captures retained; no secret or owner-state goldens." \
    > "$CAPTURE/scenario.txt"
fi
printf '\nPTY CHECKS: %s passed. Fixture-backed terminal evidence only.\n' "$PASS"
