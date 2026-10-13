import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

Item {
  id: root

  property var settings: ({})
  property bool opened: false
  property bool appRunning: false

  property var status: ({ ok: false })
  property string pollError: ""
  readonly property bool alive:      status.ok === true
  readonly property var  node:       status.node || null
  readonly property var  wallet:     status.wallet || null
  readonly property var  pending:    status.pending || null
  readonly property var  units:      status.units || null
  readonly property var  warnings:   Array.isArray(status.warnings) ? status.warnings : []
  readonly property string lastError: pollError || String((status.last_error && status.last_error.text) || "")
  readonly property bool remote:     !!(node && node.layout === "remote")
  readonly property bool nodeUp:     !!(node && node.reachable === true)
  readonly property bool synced:     !!(node && node.synchronized === true)
  readonly property int  syncPct:    node && node.sync_percent !== null && node.sync_percent !== undefined
                                       ? Math.floor(node.sync_percent) : 0
  readonly property bool walletBusy: !!(wallet && wallet.busy === true)
  readonly property bool walletOpen: !!(wallet && wallet.open === true)
  readonly property bool walletUnitActive: !!(wallet && wallet.unit_active === true)
  readonly property bool walletReachable:  !!(wallet && wallet.reachable === true)
  readonly property bool walletUp:   !!(wallet && wallet.unit_active === true)
  readonly property var  probe:      (node && node.remote_probe) || null
  readonly property int  walletHeight: Model.walletHeight(status)
  readonly property int  nodeHeight:   Model.nodeHeight(status)
  readonly property int  blocksLeft:   Model.blocksLeft(status)
  readonly property bool walletBehind: Model.walletBehind(status)
  readonly property int  walletSyncPct: Model.walletSyncPct(status)

  property string actionStatus: ""     // transient label, cleared 2.2 s after an action exits
  property string localError: ""       // stderr (or the JSON error) of the last failed action
  property int _desired: -1            // optimism bridge: -1 follow, 0 want down, 1 want up
  readonly property bool busy: actionProc.running || walletOpenProc.running
  property var wallets: []
  property string walletListError: ""
  readonly property bool listingWallets: walletListProc.running
  property int _statusEpoch: 0
  signal walletOpened()
  signal walletOpenFailed()
  signal appStopped()

  readonly property bool desiredOn: _desired === -1 ? nodeUp : _desired === 1

  property bool binaryChecked: false
  property bool binaryMissing: false

  readonly property string bin: String(setting("binary", "fmonero"))
  readonly property int refreshMs: 1000 * intSetting("refreshIntervalSec", 3, 1, 60)
  readonly property int idleRefreshMs: 1000 * intSetting("idleRefreshIntervalSec", 8, 2, 600)
  readonly property bool showBalance: String(setting("showBalance", "on")) === "on"
  readonly property string tuiCommand:
    String(setting("tuiCommand", "omarchy-launch-or-focus-tui --app-id=org.odcpw.fmonero fmonero tui"))

  function setting(name, fallback) {
    var v = settings ? settings[name] : undefined
    return v === undefined || v === null ? fallback : v
  }
  function intSetting(name, fallback, min, max) {
    var n = parseInt(String(setting(name, fallback)), 10)
    if (!isFinite(n)) n = fallback
    return Math.max(min, Math.min(max, n))
  }

  function refresh() {
    if (binaryMissing) return
    if (!binaryChecked) { checkBinary(); return }
    if (appProc.running || statusProc.running || busy) return
    appProc.epoch = _statusEpoch
    appProc.running = true
  }
  function pollStatus() {
    if (!appRunning || statusProc.running || busy) return
    statusProc.epoch = _statusEpoch
    statusProc.command = [root.bin, "status", "--json", "--check-node", "--timeout", "2"]
    statusProc.running = true
    if (!pollWatchdog.running) pollWatchdog.start()
  }

  function checkBinary() {
    if (whichProc.running) return
    whichProc.command = ["which", root.bin]
    whichProc.running = true
  }

  function runAction(args, label) {
    if (binaryMissing || busy) return
    actionStatus = String(label || "")
    actionStatusTimer.stop()
    localError = ""
    _statusEpoch++
    actionProc.command = [root.bin].concat(args)
    actionProc.running = true
  }

  function startNode() {
    _desired = 1
    desiredTimer.restart()
    runAction(["node", "up", "--json"], "Starting your node…")
  }
  function stopNode() {
    _desired = 0
    desiredTimer.restart()
    runAction(["node", "down", "--yes", "--json"], "Stopping your node…")
  }
  function toggleNode() {
    if (remote) { localError = "You are using a public node."; return }
    desiredOn ? stopNode() : startNode()
  }

  function refreshWallet() {
    if (!walletOpen) return
    runAction(["wallet", "refresh", "--json"], "Checking for payments…")
  }

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
    if (argv.length === 0) argv = ["omarchy-launch-or-focus-tui", "--app-id=org.odcpw.fmonero", "fmonero", "tui"]
    Quickshell.execDetached(argv)
  }
  function openSendScreen() {
    Quickshell.execDetached(["omarchy-launch-or-focus-tui", "--app-id=org.odcpw.fmonero", "fmonero", "tui", "--send"])
  }
  function listWallets() {
    if (walletListProc.running || binaryMissing) return
    walletListError = ""
    walletListProc.command = [bin, "wallet", "list", "--json"]
    walletListProc.running = true
  }
  function openWallet(name, secret) {
    if (busy || binaryMissing || wallets.indexOf(name) < 0) return false
    localError = ""
    actionStatusTimer.stop()
    actionStatus = "Opening " + name + "…"
    _statusEpoch++
    walletOpenProc.secret = secret
    walletOpenProc.command = [bin, "wallet", "open", name, "--password-stdin", "--json"]
    walletOpenProc.running = true
    return true
  }
  function closeWallet() { runAction(["wallet", "close", "--json"], "Closing wallet…") }
  function stopApp() { runAction(["app", "stop", "--yes", "--json"], "Saving and closing fmonero…") }
  function actionError(raw) {
    var doc = Model.parseJson(raw)
    return doc && doc.error ? String(doc.error) : "That did not work. Please try again."
  }

  function openNodeScreen() {
    Quickshell.execDetached(["omarchy-launch-or-focus-tui", "--app-id=org.odcpw.fmonero", "fmonero", "tui", "--node"])
  }
  function openDoctor() {
    Quickshell.execDetached(["omarchy-launch-or-focus-tui", "--app-id=org.odcpw.fmonerodoctor", "fmonero", "doctor"])
  }

  function applyStatus(text, exitCode, stderrText) {
    var parsed = Model.parseStatus(text)
    if (parsed.ok === true) {
      root.status = parsed
      root.pollError = ""
    } else {
      root.status = { ok: false, node: root.node, wallet: root.wallet, pending: root.pending }
      var _unused = String(stderrText || "")
      root.pollError = "fmonero could not say how things are. Press d for the doctor."
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
    id: appProc
    property int epoch: 0
    command: [root.bin, "app", "status", "--json", "--timeout", "2"]
    stdout: StdioCollector { id: appOut; waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(code) {
      if (epoch !== root._statusEpoch) { delayedRefresh.restart(); return }
      var doc = Model.parseJson(String(appOut.text || ""))
      if (code !== 0 || !doc || doc.ok !== true) { root.pollError = "Could not check whether fmonero is running."; return }
      root.appRunning = doc.running === true
      if (root.appRunning) root.pollStatus()
      else root.status = { ok: false }
    }
  }

  Process {
    id: statusProc
    property int epoch: 0
    running: false
    command: [root.bin, "status", "--json"]   // replaced per poll by refresh() (§6.3)
    stdout: StdioCollector { id: statusOut; waitForEnd: true }
    stderr: StdioCollector { id: statusErr; waitForEnd: true }
    onExited: function(exitCode) {
      pollWatchdog.stop()
      if (epoch !== root._statusEpoch) { delayedRefresh.restart(); return }
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
        root._desired = -1
        var doc = Model.parseJson(String(actionOut.text || ""))
        var msg = doc && doc.error ? String(doc.error) : String(actionErr.text || "").trim().split("\n")[0]
        var act = doc && doc.action ? String(doc.action) : ""
        var said = msg !== "" && msg.indexOf("os error") === -1
          ? msg
          : "That did not work."
        root.localError = act !== "" ? said + " " + act : said
        root.actionStatus = root.localError
      } else {
        root.localError = ""
        root.actionStatus = ""
        if (actionProc.command[1] === "app" && actionProc.command[2] === "stop") {
          root.appRunning = false
          root.status = { ok: false }
          root.appStopped()
        }
      }
      actionStatusTimer.restart()
      delayedRefresh.restart()
    }
  }

  Process {
    id: walletListProc
    stdout: StdioCollector { id: walletListOut; waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(code) {
      var doc = Model.parseJson(String(walletListOut.text || ""))
      if (code === 0 && doc && Array.isArray(doc.wallets)) root.wallets = doc.wallets
      else { root.wallets = []; root.walletListError = "Could not read your wallet list. Try refreshing." }
    }
  }
  Process {
    id: walletOpenProc
    property string secret: ""
    stdinEnabled: true
    stdout: StdioCollector { id: walletOpenOut; waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onStarted: {
      write(secret + "\n")
      secret = ""
    }
    onExited: function(code) {
      secret = ""
      root.actionStatus = ""
      if (code === 0) { root.localError = ""; root.walletOpened() }
      else { root.localError = root.actionError(String(walletOpenOut.text || "")); root.walletOpenFailed() }
      delayedRefresh.restart()
    }
  }

  Timer {
    id: pollTimer
    interval: root.opened ? root.refreshMs : root.idleRefreshMs
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: root.refresh()
  }

  Timer {
    id: pollWatchdog
    interval: 60000
    repeat: false
    onTriggered: if (statusProc.running) statusProc.running = false
  }

  Timer { id: delayedRefresh; interval: 600; repeat: false; onTriggered: root.refresh() }
  Timer { id: actionStatusTimer; interval: 2200; repeat: false; onTriggered: root.actionStatus = "" }

  Timer {
    id: desiredTimer
    interval: 5000
    repeat: false
    onTriggered: root._desired = -1
  }

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
