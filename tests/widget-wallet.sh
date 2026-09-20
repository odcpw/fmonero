#!/usr/bin/env bash
# Exercise actual Service.qml processes: list, private-input open, close, power, and errors.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cp "$ROOT/plugin/oliver.monero/Service.qml" "$ROOT/plugin/oliver.monero/Model.js" "$WORK/"
cat > "$WORK/fake" <<'PY'
#!/usr/bin/env python3
import json, pathlib, sys, os
state = pathlib.Path(__file__).with_name('state')
s = json.loads(state.read_text()) if state.exists() else {'open': False, 'active': True, 'opens': 0}
args = sys.argv[1:]
assert 'synthetic value ' not in str(args)
assert 'FMONERO_WALLET_PASSWORD' not in os.environ
if args[0] == 'status':
 print(json.dumps({'ok': True, 'node': {'layout': 'remote'}, 'wallet': {'open': s['open'], 'unit_active': s['active'], 'reachable': s['active'], 'name': 'Example' if s['open'] else None}}))
elif args[:2] == ['wallet', 'list']:
 print(json.dumps({'ok': True, 'wallets': ['Example', 'Savings']}))
elif args[:2] == ['wallet', 'open']:
 assert args[2] == 'Example' and '--password-stdin' in args
 assert sys.stdin.readline() == 'synthetic value \n'
 s['opens'] += 1
 if s['opens'] == 1:
  state.write_text(json.dumps(s)); print(json.dumps({'ok': False, 'error': 'Incorrect password. Try again.'})); sys.exit(9)
 s['open'] = True; s['active'] = True
 state.write_text(json.dumps(s)); print('{"ok":true}')
elif args[:2] == ['wallet', 'close']:
 assert s['open']; s['open'] = False
 state.write_text(json.dumps(s)); print('{"ok":true}')
elif args[:2] == ['wallet', 'stop']:
 assert '--yes' in args and not s['open'] and s['opens'] == 2
 s['active'] = False; state.write_text(json.dumps(s)); print('{"ok":true}')
else:
 raise AssertionError('unexpected widget action')
PY
chmod +x "$WORK/fake"
cat > "$WORK/shell.qml" <<'QML'
import QtQuick
import Quickshell
ShellRoot {
  property int phase: 0
  Service {
    id: svc
    settings: ({ binary: Qt.resolvedUrl("fake").toString().replace("file://", ""), refreshIntervalSec: 1, idleRefreshIntervalSec: 2 })
    onStatusChanged: {
      if (!alive) return
      if (phase === 0) { phase = 1; listWallets() }
      else if (phase === 2) {
        if (localError.indexOf("Incorrect password") < 0) { console.error("WALLET_FAIL: error lost during poll"); Qt.quit(); return }
        phase = 3
        openWallet("Example", "synthetic value ")
      } else if (phase === 4 && walletOpen && !busy) {
        phase = 5; closeWallet()
      } else if (phase === 5 && !walletOpen && !busy) {
        phase = 6; stopWallet()
      }
    }
    onWalletsChanged: {
      if (phase !== 1) return
      if (wallets.length !== 2) { console.error("WALLET_FAIL: missing list"); Qt.quit(); return }
      openWallet("Example", "synthetic value ")
    }
    onLocalErrorChanged: if (phase === 1 && localError.indexOf("Incorrect password") >= 0) phase = 2
    onWalletOpened: phase = 4
    onWalletStopped: { console.log("WALLET_PASS: list, private input, persistent error, retry, close, and power"); Qt.quit() }
  }
  Timer { interval: 20000; running: true; onTriggered: { console.error("WALLET_FAIL: deadline, phase " + phase); Qt.quit() } }
}
QML
env -u FMONERO_WALLET_PASSWORD QT_QPA_PLATFORM=offscreen timeout 25s quickshell -p "$WORK/shell.qml" > "$WORK/output" 2>&1
cat "$WORK/output"
grep -q 'WALLET_PASS:' "$WORK/output"
! grep -q 'WALLET_FAIL:' "$WORK/output"
