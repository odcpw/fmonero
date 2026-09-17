import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// The widget's whole state model: one `fmonero status --json` poll and one serialized action
// queue. Nothing is computed about Monero here — Status JSON is the single source of truth
// (DESIGN.md §5, §6.3) — and every action is an argv array handed to fmonero. No shell strings,
// ever (§2f), and no arithmetic on money (§2a.5): the panel renders the `*_xmr` strings Status
// already carries.
//
// The widget never opens, creates or restores a wallet, never sends and never sweeps (§2b.5,
// §6.3). There is no argv below that can move money; `wallet open` is a terminal action and the
// panel's entry point for it is the TUI button.
Item {
  id: root

  property var settings: ({})
  property bool opened: false

  // ---- the last Status document (DESIGN.md §6.3) ---------------------------
  property var status: ({ ok: false })
  readonly property bool alive:      status.ok === true
  readonly property var  node:       status.node || null
  readonly property var  wallet:     status.wallet || null
  readonly property var  pending:    status.pending || null
  readonly property var  units:      status.units || null
  readonly property var  warnings:   Array.isArray(status.warnings) ? status.warnings : []
  readonly property string lastError: String((status.last_error && status.last_error.text) || "")
  readonly property bool nodeUp:     !!(node && node.reachable === true)
  readonly property bool synced:     !!(node && node.synchronized === true)
  readonly property int  syncPct:    node && node.sync_percent !== null && node.sync_percent !== undefined
                                       ? Math.floor(node.sync_percent) : 0
  readonly property bool walletOpen: !!(wallet && wallet.open === true)
  readonly property bool walletUnitActive: !!(wallet && wallet.unit_active === true)
  readonly property bool walletReachable:  !!(wallet && wallet.reachable === true)

  property string actionStatus: ""     // transient label, cleared 2.2 s after an action exits
  property string localError: ""       // stderr (or the JSON error) of the last failed action
  property int _desired: -1            // optimism bridge: -1 follow, 0 want down, 1 want up
  readonly property bool busy: actionProc.running

  // What the node ToggleSwitch shows. `_desired` is set on a toggle and cleared as soon as a
  // Status agrees or an action fails, so a refusal (exit 3 under the §2i system_unit layout)
  // puts the switch straight back on the real `node.reachable` — the widget never appears to
  // have done something it did not (§6.3).
  readonly property bool desiredOn: _desired === -1 ? nodeUp : _desired === 1

  property bool binaryChecked: false
  property bool binaryMissing: false

  readonly property string bin: String(setting("binary", "fmonero"))
  readonly property int refreshMs: 1000 * intSetting("refreshIntervalSec", 3, 1, 60)
  readonly property int idleRefreshMs: 1000 * intSetting("idleRefreshIntervalSec", 8, 2, 600)
  readonly property bool showBalance: String(setting("showBalance", "on")) === "on"
  readonly property string tuiCommand:
    String(setting("tuiCommand", "omarchy-launch-or-focus-tui --app-id=fmonero fmonero tui"))

  // Manifest `barWidget.defaults` are never merged into `settings` (DESIGN.md §1.5): enabling the
  // widget writes only `{"id":"oliver.monero"}` into shell.json, so every default above repeats
  // its manifest value verbatim. tests/plugin-smoke.sh asserts that pairing.
  function setting(name, fallback) {
    var v = settings ? settings[name] : undefined
    return v === undefined || v === null ? fallback : v
  }
  function intSetting(name, fallback, min, max) {
    var n = parseInt(String(setting(name, fallback)), 10)
    if (!isFinite(n)) n = fallback
    return Math.max(min, Math.min(max, n))
  }

  // ---- the poll ------------------------------------------------------------
  // `status` never starts a daemon and never calls a wallet mutator (DESIGN.md §2e), which is why
  // it is safe to run it every few seconds from a bar widget.
  function refresh() {
    // A missing binary pauses polling until the panel is opened again: re-running `which` on
    // every tick would be a process spawn per tick, forever.
    if (binaryMissing) return
    if (!binaryChecked) { checkBinary(); return }
    if (statusProc.running) return
    statusProc.running = true
    if (!pollWatchdog.running) pollWatchdog.start()
  }

  function checkBinary() {
    if (whichProc.running) return
    whichProc.command = ["which", root.bin]
    whichProc.running = true
  }

  // ---- actions (DESIGN.md §6.3) -------------------------------------------
  // One at a time, in order, each followed by a 600 ms refresh. `label` is the transient line
  // under the hero; an empty label shows nothing.
  property var _queue: []
  function runAction(args, label) {
    if (binaryMissing) return
    _queue.push({ args: args, label: String(label || "") })
    pump()
  }
  function pump() {
    if (actionProc.running || _queue.length === 0) return
    var next = _queue.shift()
    actionStatus = next.label
    actionStatusTimer.stop()
    localError = ""
    actionProc.command = [root.bin].concat(next.args)
    actionProc.running = true
  }

  function startNode() {
    _desired = 1
    desiredTimer.restart()
    runAction(["node", "up", "--json"], "Starting the node…")
  }
  function stopNode() {
    _desired = 0
    desiredTimer.restart()
    runAction(["node", "down", "--yes", "--json"], "Stopping the node…")
  }
  function toggleNode() { desiredOn ? stopNode() : startNode() }

  // Shown only while a wallet is open, so it can never be the call that starts a wallet daemon.
  function refreshWallet() {
    if (!walletOpen) return
    runAction(["wallet", "refresh", "--json"], "Refreshing the wallet…")
  }

  // ---- detached launches ---------------------------------------------------
  // Only constant literals reach `omarchy-launch-*`: all three of those scripts end in `eval` or
  // `bash -c` on a joined string (DESIGN.md §1.5). `wl-copy` is genuinely argv-shaped, and the
  // address it carries came out of our own Status JSON.
  // `flash` is the one way to set the transient label: a caller that assigns `actionStatus`
  // directly would leave it on screen for ever, because actionStatusTimer is private to this file.
  function flash(text) {
    actionStatus = String(text)
    actionStatusTimer.restart()
  }
  function copyAddress() {
    var a = wallet && wallet.primary_address ? String(wallet.primary_address) : ""
    if (a === "") return
    Quickshell.execDetached(["wl-copy", "--", a])
    flash("Copied " + Model.shortAddr(a))
  }
  function copyText(text) {
    var t = String(text || "")
    if (t === "") return
    Quickshell.execDetached(["wl-copy", "--", t])
    flash("Copied: " + t)
  }
  function openTui() {
    var argv = Model.safeArgv(root.tuiCommand)
    if (argv.length === 0) argv = ["omarchy-launch-or-focus-tui", "--app-id=fmonero", "fmonero", "tui"]
    Quickshell.execDetached(argv)
  }
  function openSendScreen() {
    // `--send` is the TUI's Send *screen* flag (§7a), not a money argv: the widget has no send
    // path at all (§2b.5). Constant literals only.
    Quickshell.execDetached(["omarchy-launch-or-focus-tui", "--app-id=fmonero", "fmonero", "tui", "--send"])
  }
  function openDoctor() {
    Quickshell.execDetached(["omarchy-launch-or-focus-tui", "--app-id=fmonerodoctor", "fmonero", "doctor"])
  }

  // ---- status --------------------------------------------------------------
  function applyStatus(text, exitCode, stderrText) {
    var parsed = Model.parseStatus(text)
    if (parsed.ok === true) {
      root.status = parsed
      root.localError = ""
    } else {
      // Keep the last good node/wallet rows for display — the panel dims them rather than
      // blanking out mid-poll — but `ok` is false, so nothing is presented as current (§6.3).
      root.status = { ok: false, node: root.node, wallet: root.wallet }
      var msg = String(stderrText || "").trim()
      root.localError = msg !== "" ? msg.split("\n")[0] : "fmonero status failed (exit " + exitCode + ")"
    }
    if (root._desired !== -1) {
      if (root.alive && (root._desired === 1) === root.nodeUp) { root._desired = -1; desiredTimer.stop() }
    }
  }

  Process {
    id: whichProc
    running: false
    command: []
    stdout: StdioCollector {}
    stderr: StdioCollector {}
    onExited: function(exitCode) {
      // The name may have changed while this check was in flight (shell.json hot-reloads); an
      // answer about the old name says nothing about the new.
      if (whichProc.command.length > 1 && String(whichProc.command[1]) !== root.bin) {
        root.checkBinary()
        return
      }
      root.binaryChecked = true
      root.binaryMissing = exitCode !== 0
      if (!root.binaryMissing) root.refresh()
      else root.status = { ok: false }
    }
  }

  Process {
    id: statusProc
    running: false
    command: [root.bin, "status", "--json"]
    stdout: StdioCollector { id: statusOut; waitForEnd: true }
    stderr: StdioCollector { id: statusErr; waitForEnd: true }
    onExited: function(exitCode) {
      pollWatchdog.stop()
      root.applyStatus(String(statusOut.text || ""), exitCode, String(statusErr.text || ""))
    }
  }

  Process {
    id: actionProc
    running: false
    command: []
    stdout: StdioCollector { id: actionOut; waitForEnd: true }
    stderr: StdioCollector { id: actionErr; waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode !== 0) {
        // Includes the §2i refusal: under a `system_unit` layout `node up`/`node down` exit 3
        // without issuing an RPC, and its hint is what the panel shows.
        root._desired = -1
        var doc = Model.parseJson(String(actionOut.text || ""))
        var msg = doc && doc.error ? String(doc.error) : String(actionErr.text || "").trim().split("\n")[0]
        if (doc && doc.hint) msg += " — try: " + doc.hint
        root.localError = msg !== "" ? msg : "fmonero exited " + exitCode
        root.actionStatus = root.localError
      } else {
        root.localError = ""
        root.actionStatus = ""
      }
      actionStatusTimer.restart()
      delayedRefresh.restart()
      root.pump()
    }
  }

  Timer {
    // 3 s while the panel is open, when the user is watching a sync percentage tick; 8 s while it
    // is closed, when a bar widget needs only enough to pick an icon state. Each poll is a
    // process spawn, which is the expensive part of the §2l budget.
    id: pollTimer
    interval: root.opened ? root.refreshMs : root.idleRefreshMs
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: root.refresh()
  }

  Timer {
    // Every tick is skipped while the poll is still running (the §2d poll_inflight guard), so a
    // status call that never exits would stop the panel refreshing for good. Armed on the launch
    // that needs watching and never restarted while armed.
    id: pollWatchdog
    interval: 8000
    repeat: false
    onTriggered: if (statusProc.running) statusProc.running = false
  }

  Timer { id: delayedRefresh; interval: 600; repeat: false; onTriggered: root.refresh() }
  Timer { id: actionStatusTimer; interval: 2200; repeat: false; onTriggered: root.actionStatus = "" }

  Timer {
    // Last resort for the optimism bridge: a node that neither came up nor went down within 5 s
    // stops being claimed either way.
    id: desiredTimer
    interval: 5000
    repeat: false
    onTriggered: root._desired = -1
  }

  // The binary name can change under us: shell.json hot-reloads, so someone editing the setting in
  // Setup > Plugins must not leave the panel polling a name that no longer exists.
  onBinChanged: {
    binaryChecked = false
    binaryMissing = false
    checkBinary()
  }

  onOpenedChanged: {
    if (!opened) return
    if (binaryMissing) checkBinary()
    else refresh()
  }
}
