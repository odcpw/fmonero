#!/usr/bin/env bash
# Real Quickshell event-loop regression: a slow status must complete, then poll again.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cp "$ROOT/plugin/oliver.monero/Service.qml" "$ROOT/plugin/oliver.monero/Model.js" "$WORK/"
cat > "$WORK/fake-status" <<'PY'
#!/usr/bin/env python3
import json, pathlib, sys, time
assert '--check-node' in sys.argv, 'closed panels must keep the node fresh'
count = pathlib.Path(__file__).with_name('count')
n = int(count.read_text()) + 1 if count.exists() else 1
count.write_text(str(n))
# The first poll used to be terminated at eight seconds, before any data reached the bar.
if n == 1:
    time.sleep(9)
print(json.dumps({'ok': True, 'node': {'layout': 'remote'}, 'wallet': {'open': True, 'height': n}}))
PY
chmod +x "$WORK/fake-status"
cat > "$WORK/shell.qml" <<'QML'
import QtQuick
import Quickshell
ShellRoot {
  Service {
    id: service
    settings: ({binary: Qt.resolvedUrl("fake-status").toString().replace("file://", ""), idleRefreshIntervalSec: 2})
    property int seen: 0
    onStatusChanged: {
      if (!alive || !wallet) return
      if (wallet.height !== seen + 1) { console.error("POLL_FAIL: skipped response"); Qt.quit(); return }
      seen = wallet.height
      if (seen === 2) { console.log("POLL_PASS: slow status completed and updated twice"); Qt.quit() }
    }
  }
  Timer { interval: 20000; running: true; onTriggered: { console.error("POLL_FAIL: deadline"); Qt.quit() } }
}
QML
QT_QPA_PLATFORM=offscreen timeout 25s quickshell -p "$WORK/shell.qml" > "$WORK/output" 2>&1
cat "$WORK/output"
grep -q 'POLL_PASS:' "$WORK/output"
! grep -q 'POLL_FAIL:' "$WORK/output"
