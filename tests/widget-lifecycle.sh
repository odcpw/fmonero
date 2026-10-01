#!/usr/bin/env bash
# The idle widget must never read a wallet or contact a public node.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cp "$ROOT/plugin/oliver.monero/Service.qml" "$ROOT/plugin/oliver.monero/Model.js" "$WORK/"
cat > "$WORK/fake" <<'PY'
#!/usr/bin/env python3
import json, pathlib, sys
path = pathlib.Path(__file__).with_name('state')
s = json.loads(path.read_text()) if path.exists() else {'running': False, 'reads': 0, 'stopped': False, 'bad_reads': 0}
a = sys.argv[1:]
if a[:2] == ['app', 'status']:
 print(json.dumps({'ok': True, 'running': s['running']})); sys.exit(0)
elif a == ['launch']:
 s['running'] = True
elif a[0] == 'status':
 if not s['running']:
  s['bad_reads'] += 1; path.write_text(json.dumps(s)); raise AssertionError('wallet read while app closed')
 s['reads'] += 1
 print('{"ok":true,"node":{"layout":"remote"},"wallet":{"open":false}}')
elif a[:2] == ['app', 'stop']:
 assert s['running'] and '--yes' in a
 s['running'] = False; s['stopped'] = True
 print('{"ok":true}')
else:
 raise AssertionError('unexpected idle widget operation')
path.write_text(json.dumps(s))
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
    onStatusChanged: if (phase === 1 && alive) { phase = 2; stopApp() }
    onAppStopped: {
      if (appRunning || alive) { console.error("LIFECYCLE_FAIL: close left app visible"); Qt.quit() }
      else { phase = 3; settle.start() }
    }
  }
  Timer { interval: 900; running: true; onTriggered: {
      if (svc.appRunning || svc.alive) { console.error("LIFECYCLE_FAIL: active at startup"); Qt.quit(); return }
      phase = 1
      Quickshell.execDetached([Qt.resolvedUrl("fake").toString().replace("file://", ""), "launch"])
  } }
  Timer { id: settle; interval: 3000; onTriggered: {
      if (svc.appRunning || svc.alive) console.error("LIFECYCLE_FAIL: reopened after stop")
      else console.log("LIFECYCLE_PASS: idle at login, launch, full close, idle afterwards")
      Qt.quit()
  } }
  Timer { interval: 12000; running: true; onTriggered: { console.error("LIFECYCLE_FAIL: deadline"); Qt.quit() } }
}
QML
QT_QPA_PLATFORM=offscreen timeout 15s quickshell -p "$WORK/shell.qml" > "$WORK/output" 2>&1
cat "$WORK/output"
grep -q 'LIFECYCLE_PASS:' "$WORK/output"
! grep -q 'LIFECYCLE_FAIL:' "$WORK/output"
python3 - "$WORK/state" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
assert not s['running'] and s['stopped'] and s['reads'] >= 1 and s['bad_reads'] == 0, s
PY
