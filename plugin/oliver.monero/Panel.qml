import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

// Bar button plus keyboard-driven panel for Monero. Everything shown here comes from
// `fmonero status --json` (DESIGN.md §5) and every action is an argv array; the QML holds no
// Monero logic of its own (§6). Amounts are the pre-rendered `*_xmr` strings Status already
// carries — the panel never does arithmetic on money (§2a.5, §6.5).
Panel {
  id: root
  moduleName: "oliver.monero"
  ipcTarget: "oliver.monero"
  manageIpc: false

  // **Which QML the shell is actually running** (v5.6.1, DECISIONS V1).
  //
  // The v5.6 redesign — the whole answer to the owner's *"the text in the widget isn't clear or
  // useful"* — had never rendered on his bar: Quickshell holds the loaded QML, a symlink does not
  // hot-reload (§1.5), `omarchy restart shell` is refused while the session is locked, and nothing
  // could tell. `tests/plugin-smoke.sh` asserted only that `cursor` returned a JSON object, and the
  // evidence that the panel was the PREVIOUS one — a section named `warnings`, which this file does
  // not build — sat unread inside the passing assertion's own message.
  //
  // So the panel states its own version, the `cursor` payload carries it, and the gate compares it
  // with the checked-out tree: a stale shell now fails the gate instead of decorating it. The token
  // is the first eight hex digits of the sha256 of the four widget files with this line removed, so
  // it cannot be left behind by an edit — the gate computes what it should be and says so.
  readonly property string panelVersion: "qml-6550d06c"

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  // ---- bar icon state (DESIGN.md §6.4, first match wins) -------------------
  readonly property bool anyError: Model.worstSeverity(svc.warnings) === "error"
  // Cases 6 and 7 are v5.2's remote rows, and they sit ABOVE the three node rows deliberately
  // (§6.4): `!nodeUp`, `!synced` and the percent ring all read fields that are null in remote
  // mode, so left below they would draw a crossed glyph and a 0 % ring for a wallet that is
  // working perfectly — a bar icon claiming the node is down.
  // Case 8 is v5.6's `walletBehind`, and it sits ABOVE both remote rows deliberately (§6.4): a
  // wallet catching up is the one long-running thing this product does that the user waits on, it
  // happens in both modes, and with a public node v5.5 had no ring at all — the icon said "public
  // node, wallet open" for twenty minutes while the wallet scanned a million blocks.
  readonly property int iconCase: !svc.alive ? 0
    : anyError ? 1
    : svc.walletBehind ? 8
    : (svc.remote && svc.walletOpen) ? 6
    : svc.remote ? 7
    : !svc.nodeUp ? 2
    : !svc.synced ? 3
    : svc.walletOpen ? 4 : 5
  readonly property color barIconColor: iconCase === 1 ? urgent
    : (iconCase === 0 || iconCase === 2) ? Qt.darker(barForeground, 1.55) : barForeground
  readonly property bool iconCrossed: iconCase === 0 || iconCase === 2
  readonly property bool iconWarning: iconCase <= 1
  readonly property bool iconFilled: iconCase === 4 || iconCase === 6 || iconCase === 8
  // A small chevron in the same badge slot the warning badge uses, in `barForeground` rather
  // than `urgent`: remote mode is a configuration, not a fault (§6.4).
  readonly property bool iconRemote: iconCase === 6 || iconCase === 7 || (iconCase === 8 && svc.remote)
  // The ring draws the **wallet's** catch-up in case 8 and the node's own sync in case 3. The
  // wallet's percent is available with a public node precisely because the cached check supplies a
  // node height (§6.3); when it is not, case 8 does not match and no ring is drawn from a guess.
  readonly property real iconRing: iconCase === 8 ? svc.walletSyncPct / 100
    : iconCase === 3 ? svc.syncPct / 100 : -1
  // The number only when the bar is tall enough to draw two legible digits inside the ring.
  readonly property string iconPercent: !(bar && bar.barSize >= 30) ? ""
    : iconCase === 8 ? String(svc.walletSyncPct)
    : iconCase === 3 ? String(svc.syncPct) : ""

  // **The header line, plus at most two supporting facts** (§6.4, v5.6). It never contains a
  // block number, a peer count or a sync phrase it does not have, and with a public node the
  // check's age travels with its height in one string, for §5's reason.
  readonly property string headerLine:
    Model.headerLine(svc.status, svc.showBalance, svc.binaryMissing)
  readonly property string tooltipText: {
    var parts = [root.headerLine]
    if (svc.alive && svc.remote && svc.probe) parts.push(Model.checkedLabel(svc.node))
    if (svc.alive && svc.walletBehind) parts.push(Model.grouped(svc.blocksLeft) + " blocks left")
    return parts.join(" · ")
  }

  // ---- panel state ---------------------------------------------------------
  property bool cursorActive: false
  property string focusSection: "header"
  property int selectedIndex: 0

  readonly property bool walletHidden: !svc.showBalance
  // 0 = nothing to act on, 2 = copy + check for payments. A **closed wallet is a state, never a
  // warning** (§6.5), and the `Open wallet…` button below is its action — so it has no row here.
  readonly property int walletTargets: svc.walletOpen ? 2 : 0
  readonly property bool lockedFunds:
    svc.walletOpen && Model.int(svc.wallet.blocks_to_unlock, 0) > 0
  // Minutes from `time_to_unlock_sec`, or two per block, which is Monero's block time. **The
  // amount is absent here and present in the TUI's otherwise identical line**: it is
  // `balance - unlocked`, arithmetic on money, which §2a.5 forbids in QML outright, and Status
  // carries no pre-rendered locked amount to display instead (DECISIONS).
  readonly property int lockedMinutes: {
    if (!svc.walletOpen) return 0
    var sec = svc.wallet.time_to_unlock_sec
    if (sec !== null && sec !== undefined) return Math.ceil(Model.int(sec, 0) / 60)
    return Model.int(svc.wallet.blocks_to_unlock, 0) * 2
  }

  // §6.5: **the one fact that explains the header**, and nothing when a row below would repeat
  // it — which is why a `Public node…` header has no second line: the `Checked` row already says
  // `checked 2m ago`, and the same sentence twice in a 380-pixel panel is what this removes.
  readonly property string heroMeta: {
    if (svc.binaryMissing || !svc.alive || svc.pending) return ""
    if (svc.walletBehind) return Model.walletSyncLabel(svc.status)
    if (svc.walletOpen) return refreshedAgo()
    if (svc.remote || !svc.node) return ""
    if (svc.nodeUp && !svc.synced && svc.node.target_height)
      return Model.grouped(svc.node.height) + " / " + Model.grouped(svc.node.target_height)
    return ""
  }

  // `wallet.last_refresh_at` is the last refresh **fmonero itself issued** (§5) — not the wallet
  // service's own 20 s auto-refresh, which fmonero cannot observe. Date arithmetic on a timestamp
  // is not arithmetic on money.
  function refreshedAgo() {
    var raw = svc.walletOpen ? svc.wallet.last_refresh_at : null
    if (!raw) return ""
    var t = Date.parse(String(raw))
    if (!isFinite(t)) return ""
    return "refreshed " + Model.age(Math.floor((Date.now() - t) / 1000)) + " ago"
  }

  // **The problems section, which is not the warnings list** (§6.5, v5.6). `info` warnings are
  // the state the rows above already show, and v5.5 put them in the alarm channel — on the
  // owner's correctly configured machine that meant an alarm about a wallet service designed to
  // be down. The one exclusion is `send_may_be_in_flight` while the pending row renders it:
  // together they are one alarm printed twice.
  readonly property var problems: {
    var out = []
    for (var i = 0; i < svc.warnings.length; i++) {
      var w = svc.warnings[i]
      var sev = String(w.severity || "")
      if (sev !== "warn" && sev !== "error") continue
      if (svc.pending && String(w.id || "") === "send_may_be_in_flight") continue
      out.push(w)
    }
    return out
  }

  // Two or three: `Start your node` / `Stop your node` only with your own node (§6.5). Five
  // buttons in a 380-pixel panel is a menu, and the owner's complaint was that the widget did not
  // answer a question — not that it was missing a sixth control.
  readonly property int footerCount: svc.remote || !svc.alive ? 2 : 3

  readonly property var navSections: {
    var s = ["header"]
    if (problems.length > 0) s.push("problems")
    if (walletTargets > 0) s.push("wallet")
    if (svc.pending) s.push("pending")
    s.push("footer")
    return s
  }

  function sectionCount(name) {
    if (name === "problems") return problems.length
    if (name === "wallet") return walletTargets
    if (name === "footer") return footerCount
    return 1
  }
  function isHorizontal(name) { return name === "footer" }

  function ensureCursor() {
    if (navSections.indexOf(focusSection) < 0) { focusSection = "header"; selectedIndex = 0 }
    var n = sectionCount(focusSection)
    if (selectedIndex >= n) selectedIndex = Math.max(0, n - 1)
    if (selectedIndex < 0) selectedIndex = 0
  }

  function gotoSection(position, direction) {
    if (position < 0 || position >= navSections.length) return
    focusSection = navSections[position]
    var n = sectionCount(focusSection)
    selectedIndex = direction > 0 || isHorizontal(focusSection) ? 0 : Math.max(0, n - 1)
    scrollCursorIntoView()
  }

  function moveCursor(dx, dy) {
    cursorActive = true
    ensureCursor()
    var here = navSections.indexOf(focusSection)
    if (here < 0) return
    if (dy !== 0) {
      if (!isHorizontal(focusSection)) {
        var next = selectedIndex + dy
        if (next >= 0 && next < sectionCount(focusSection)) {
          selectedIndex = next
          scrollCursorIntoView()
          return
        }
      }
      gotoSection(here + dy, dy)
      return
    }
    if (dx !== 0 && isHorizontal(focusSection)) {
      selectedIndex = Math.max(0, Math.min(sectionCount(focusSection) - 1, selectedIndex + dx))
      scrollCursorIntoView()
    }
  }

  function setCursor(section, index) {
    cursorActive = true
    focusSection = section
    selectedIndex = index
    ensureCursor()
  }

  function activateCursor() {
    ensureCursor()
    if (focusSection === "header") { svc.openTui(); root.close() }
    else if (focusSection === "problems") activateProblem(selectedIndex)
    else if (focusSection === "wallet") activateWallet(selectedIndex)
    else if (focusSection === "pending") { svc.openTui(); root.close() }
    else if (focusSection === "footer") activateFooter(selectedIndex)
  }

  // **§2o.1: a problem row's action is something the user can do here**, never a command to copy
  // into a terminal. The doctor is where a machine-shaped problem is diagnosed and it is the one
  // thing this panel can run itself; everything else opens the TUI screen the action names, which
  // is where the keys the action mentions actually are.
  function activateProblem(index) {
    var w = problems[index]
    if (!w) return
    var id = String(w.id || "")
    if (id === "node_stalled" || id === "wallet_stalled" || id === "monero_not_installed") {
      svc.openDoctor(); root.close(); return
    }
    if (id === "remote_node" || id === "remote_node_trusted" || id === "node_down") {
      svc.openNodeScreen(); root.close(); return
    }
    svc.openWalletScreen(); root.close()
  }

  function activateWallet(index) {
    if (index === 0) svc.copyAddress()
    else svc.refreshWallet()
  }

  function activateFooter(index) {
    // Both of the first two open the TUI on a screen — the affordance that LEADS to the thing the
    // owner could not find, not a shortcut past the prompt (§6.3).
    if (index === 0) {
      if (svc.walletOpen) svc.refreshWallet()
      else { svc.openWalletScreen(); root.close() }
    } else if (index === 1) { svc.openNodeScreen(); root.close() }
    else svc.toggleNode()
  }

  // ---- scrolling -----------------------------------------------------------
  function sectionColumn(name) {
    if (name === "problems") return warningColumn
    if (name === "wallet") return walletActionColumn
    return null
  }
  function sectionAnchor(name) {
    if (name === "header") return heroBlock
    if (name === "pending") return pendingRow
    if (name === "footer") return footerBlock
    return null
  }
  function scrollCursorIntoView() {
    var col = sectionColumn(focusSection)
    var item = col && selectedIndex >= 0 && selectedIndex < col.children.length
      ? col.children[selectedIndex] : sectionAnchor(focusSection)
    scrollItemIntoView(item)
  }
  function scrollItemIntoView(item) {
    if (!panelFlick || !item) return
    Qt.callLater(function() {
      if (!item) return
      var margin = Style.space(6)
      var point = item.mapToItem(panelFlick.contentItem, 0, 0)
      var top = point.y
      var bottom = top + item.height
      var viewTop = panelFlick.contentY
      var viewBottom = viewTop + panelFlick.height
      var maxY = Math.max(0, panelFlick.contentHeight - panelFlick.height)
      if (top < viewTop + margin) panelFlick.contentY = Math.max(0, top - margin)
      else if (bottom > viewBottom - margin) panelFlick.contentY = Math.min(maxY, bottom + margin - panelFlick.height)
    })
  }

  implicitWidth: button.implicitWidth; implicitHeight: button.implicitHeight

  onOpenedChanged: {
    svc.opened = opened
    if (!opened) return
    cursorActive = false
    focusSection = "header"
    selectedIndex = 0
    if (panelFlick) panelFlick.contentY = 0
    svc.refresh()
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }
  onNavSectionsChanged: ensureCursor()

  Service {
    id: svc
    settings: root.settings
  }

  IpcHandler {
    target: root.ipcTarget
    function open(): void { root.open() }
    function close(): void { root.close() }
    function show(): void { root.open() }
    function hide(): void { root.close() }
    function toggle(): void { root.toggle() }
    function refresh(): string { svc.refresh(); return "ok" }
    function nodeUp(): string { svc.startNode(); return "ok" }
    function nodeDown(): string { svc.stopNode(); return "ok" }
    // Test hook: where the keyboard cursor is, without a screenshot.
    function cursor(): string {
      return JSON.stringify({ version: root.panelVersion,
                              section: root.focusSection, index: root.selectedIndex,
                              active: root.cursorActive, opened: root.opened,
                              sections: root.navSections, alive: svc.alive,
                              binaryMissing: svc.binaryMissing, nodeUp: svc.nodeUp,
                              synced: svc.synced, walletOpen: svc.walletOpen,
                              remote: svc.remote,
                              iconCase: root.iconCase, iconColor: String(root.barIconColor) })
    }
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    tooltipText: root.tooltipText
    iconComponent: Component {
      MoneroIcon {
        anchors.centerIn: parent
        iconSize: Style.space(14)
        color: root.barIconColor
        badgeColor: root.urgent
        crossed: root.iconCrossed
        warning: root.iconWarning
        filled: root.iconFilled
        remote: root.iconRemote
        ringProgress: root.iconRing
        walletRing: root.iconCase === 8
        percentText: root.iconPercent
      }
    }
    onPressed: function(buttonCode) {
      // v5.6: **right refreshes** and middle opens the TUI. v5.5 gave right-click the node
      // toggle, which is inert with a public node and had to be special-cased into an error
      // message; a toggle that does nothing on the owner's machine is not worth a button, and
      // refresh is what a person clicking a stale-looking bar icon means (§6.4).
      if (buttonCode === Qt.RightButton) svc.refresh()
      else if (buttonCode === Qt.MiddleButton) svc.openTui()
      else root.toggle()
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(380))
    contentHeight: panel.fittedContentHeight(column.implicitHeight, Style.space(520))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onMoveRequested: function(dx, dy) {
        if (!root.cursorActive) { root.cursorActive = true; return }
        root.moveCursor(dx, dy)
      }
      onActivateRequested: if (root.cursorActive) root.activateCursor()
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      // h j k l are consumed by the catcher as movement and never reach here (DESIGN.md §6.5).
      onTextKey: function(t) {
        var k = String(t).toLowerCase()
        // v5.6: `n` opens `Choose node…` in **both** modes. v5.5 made it a node toggle with your
        // own node and a picker with a public one — one key, two acts — and the panel now has
        // explicit Start/Stop buttons for the toggle.
        if (k === "n") { svc.openNodeScreen(); root.close() }
        else if (k === "r") svc.refresh()
        else if (k === "t") { svc.openTui(); root.close() }
        else if (k === "c") svc.copyAddress()
        else if (k === "d") { svc.openDoctor(); root.close() }
        // v5.5: one key whose meaning follows the mode exactly as the rows above it do. In remote
        // mode there is no node of ours to toggle, and "which node" is the only node question
        // left; `w` is the panel's own route to the thing the owner could not find.
        else if (k === "w") { svc.openWalletScreen(); root.close() }
      }

      Flickable {
        id: panelFlick
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        flickableDirection: Flickable.VerticalFlick
        interactive: contentHeight > height
        // AsNeeded is the QQC2 default: transparent until the user scrolls, which is no
        // affordance at all for content they do not know exists.
        ScrollBar.vertical: ScrollBar { policy: panelFlick.contentHeight > panelFlick.height
                                                ? ScrollBar.AlwaysOn : ScrollBar.AlwaysOff }

        Column {
          id: column
          width: panelFlick.width
          spacing: Style.space(10)

          // ---- header ------------------------------------------------------
          Item {
            id: heroBlock
            width: parent.width
            implicitHeight: hero.implicitHeight
            // The hero's trailingControl resolves `root` to PanelHero, so panel state is reached
            // through this wrapper instead.
            readonly property bool ringVisible: root.cursorActive && root.focusSection === "header"
            function focusHero() { root.setCursor("header", 0) }

            // **One header line and at most one supporting fact** (§6.4/§6.5). v5.5's lines were
            // `Node · 3 541 122` and `remote: 185.220.101.63:18081 · trusted`, and neither answers
            // what someone glances at a bar widget to answer — is my money there, is anything
            // wrong. The `detail` row (the nettype) was a third line of chrome on a two-line hero.
            PanelHero {
              id: hero
              width: parent.width
              title: root.headerLine
              meta: root.heroMeta
              detail: ""
              foreground: root.foreground
              fontFamily: root.fontFamily
              iconOpacity: svc.nodeUp ? 1.0 : 0.5
              iconComponent: Component {
                MoneroIcon {
                  iconSize: Style.font.display
                  color: svc.nodeUp ? root.foreground : root.dim
                  badgeColor: root.urgent
                  crossed: svc.alive && !svc.remote && !svc.nodeUp
                  warning: !svc.alive || root.anyError
                  filled: svc.walletOpen
                  remote: svc.remote
                }
              }
              // **Node up / Node down are rendered only in local mode** (v5.5, §6.5, tightening
              // §6.3's note from "disabled" to "absent"): in remote mode both commands are
              // specified to exit 4 `remote_node`, and a visible-but-disabled control that can
              // never be enabled from this panel is a control that teaches the user the app is
              // broken. `Choose node…` in the footer is what stands in its place, which is also
              // the honest mapping — in remote mode the node question is *which* node.
              trailingControl: Component {
                ToggleSwitch {
                  id: nodeSwitch
                  // Not drawn at all in remote mode — `visible: false`, not `enabled: false`.
                  visible: !svc.remote
                  checked: svc.desiredOn
                  busy: svc.busy
                  hasCursor: heroBlock.ringVisible
                  foreground: hero.foreground
                  onHovered: function(on) { if (on) heroBlock.focusHero() }
                  onToggled: svc.toggleNode()

                  PanelToolTip {
                    visible: nodeSwitch.containsMouse
                    text: svc.desiredOn ? "Stop your node" : "Start your node"
                    fontFamily: hero.fontFamily
                  }
                }
              }
            }
          }

          // ---- node: one row, plus the one or two that carry information (§6.5) ----
          //
          // **A row renders only when it carries information for the current state.** Not
          // "renders —", not "renders not available": absent. Five of v5.5's rows were `null`
          // with a public node and each one spent a line saying so, which is how a 520-pixel
          // panel came to be mostly disclaimers. The `Checked` row is read from
          // `status.node.remote_probe` and **the panel never checks anything itself**: it polls
          // `status`, and §6.3's `--check-node` variant is what keeps that cache fresh.
          Column {
            width: parent.width
            spacing: Style.space(3)
            visible: svc.alive && !svc.binaryMissing

            DetailRow { label: "node"; value: Model.nodeLabel(svc.node) }
            DetailRow {
              visible: svc.remote
              label: "Checked"; value: Model.checkedLabel(svc.node)
            }
          }

          // ---- your own node: two more rows, and no others (§6.5) ----------
          //
          // `Last block` and `Traffic` are gone: with your own node they are two rows nobody
          // glances at a bar for, and `Open TUI` is one key away and has all of them.
          Column {
            width: parent.width
            spacing: Style.space(3)
            visible: svc.alive && svc.nodeUp && !svc.remote && !svc.binaryMissing

            DetailRow { label: "Peers"; value: Model.peersLabel(svc.node) }
            DetailRow {
              label: "Waiting"
              // **`payments`, not `transactions`** (X8): §2o.2's vocabulary, and the word the
              // TUI's own waiting pane and `node info` both use. Rule 7's list never carried
              // `transaction`, which is why this surface, `output.rs` and the note modal all kept
              // it through three rounds. Pluralised, because `1 payments` is the same defect as
              // `Checked 1 nodes`.
              value: svc.node && svc.node.tx_pool_size !== null && svc.node.tx_pool_size !== undefined
                ? Model.int(svc.node.tx_pool_size, 0)
                  + (Model.int(svc.node.tx_pool_size, 0) === 1 ? " payment" : " payments")
                  + (svc.node.restricted === true ? " or more" : "")
                : "—"
            }
          }

          // Whatever the panel most recently learned: the action it is running, the error that
          // action returned, or the error Status still carries.
          Text {
            textFormat: Text.PlainText; visible: text !== ""; width: parent.width
            text: svc.actionStatus !== "" ? svc.actionStatus
              : (svc.localError !== "" ? svc.localError : svc.lastError)
            color: svc.actionStatus === "" && (svc.localError !== "" || svc.lastError !== "") ? root.urgent : root.dim
            font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
            wrapMode: Text.WordWrap; maximumLineCount: 3; elide: Text.ElideRight
          }

          Text {
            textFormat: Text.PlainText; visible: svc.binaryMissing; width: parent.width
            text: "fmonero is not installed."
            color: root.urgent; font.family: root.fontFamily
            font.pixelSize: Style.font.body; wrapMode: Text.WordWrap
          }

          // ---- problems (§6.5) ---------------------------------------------
          Column {
            width: parent.width
            spacing: Style.space(6)
            visible: root.problems.length > 0

            PanelSeparator { foreground: root.foreground }

            Column {
              id: warningColumn
              width: parent.width
              spacing: Style.space(4)

              Repeater {
                model: root.problems
                WarnRow {
                  required property var modelData
                  required property int index
                  section: "problems"
                  idx: index
                  width: warningColumn.width
                  message: String(modelData.text || "")
                  // §2o.1: the warning's **action**, not its `hint` — which is where
                  // `fmonero node local --yes` reached the bar.
                  hint: String(modelData.action || "")
                  severity: String(modelData.severity || "warn")
                }
              }
            }
          }

          // ---- wallet ------------------------------------------------------
          Column {
            width: parent.width
            spacing: Style.space(6)

            PanelSeparator { foreground: root.foreground }
            PanelSectionHeader { text: "WALLET"; foreground: root.foreground; fontFamily: root.fontFamily }

            Column {
              width: parent.width
              spacing: Style.space(3)
              visible: svc.alive && svc.walletOpen

              DetailRow {
                label: "Balance"
                value: root.walletHidden ? "•••••"
                  : (svc.walletOpen && svc.wallet.balance_xmr ? String(svc.wallet.balance_xmr) + " XMR" : "—")
              }
              DetailRow {
                label: "Unlocked"
                value: root.walletHidden ? "•••••"
                  : (svc.walletOpen && svc.wallet.unlocked_xmr ? String(svc.wallet.unlocked_xmr) + " XMR" : "—")
              }
              // Minutes, not blocks, and with the amount: blocks are the mechanism, minutes are
              // the answer, and the TUI's line reads word for word the same (§2o rule 9).
              Text {
                textFormat: Text.PlainText; visible: root.lockedFunds; width: parent.width
                text: "locked for about " + root.lockedMinutes + " min"
                color: root.dim; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
              }
            }

            Column {
              id: walletActionColumn
              width: parent.width
              spacing: Style.space(4)

              ListRow {
                visible: svc.alive && svc.walletOpen
                section: "wallet"; idx: 0; width: parent.width
                title: svc.walletOpen && svc.wallet.primary_address
                  ? Model.shortAddr(svc.wallet.primary_address) : "no address yet"
                meta: "copy (c)"
              }
              ListRow {
                visible: svc.alive && svc.walletOpen
                section: "wallet"; idx: 1; width: parent.width
                title: "Check for payments"
                meta: svc.walletOpen && svc.wallet.blocks_behind !== null
                        && svc.wallet.blocks_behind !== undefined
                  ? Model.int(svc.wallet.blocks_behind, 0) + " blocks to go" : ""
              }
            }

            // **Decision 7, said out loud.** The wallet keeps scanning inside the wallet service
            // whether or not the TUI is open, exactly as fbittorrent's transfers keep running when
            // its window closes, and until v5.6 nothing on either surface said so.
            //
            // **The sentence is `fmonero_core::SYNC_KEEPS_GOING`, byte for byte** (W10): this
            // widget, the TUI's two screens and core's own warning said one fact three ways.
            // `tests/plugin-smoke.sh` asserts the two literals are the same sentence.
            Text {
              textFormat: Text.PlainText; visible: svc.alive && svc.walletBehind
              width: parent.width
              text: "This keeps going in the background, even after you close this window."
              color: root.dim; font.family: root.fontFamily
              font.pixelSize: Style.font.bodySmall; wrapMode: Text.WordWrap
            }

            // **A closed wallet is a state, and never a warning** (§6.5, and §5 makes
            // `wallet_idle` an `info` for the same reason): one row, and the `Open wallet…` button
            // below it is the route. The service's own state follows in the quieter line, because
            // "not running" and "starts when you open a wallet" are the same fact in two
            // situations that are not the same.
            Column {
              width: parent.width
              spacing: Style.space(2)
              visible: !svc.walletOpen

              DetailRow { visible: svc.alive; label: "wallet"; value: "Wallet closed" }
              Text {
                textFormat: Text.PlainText; width: parent.width
                text: svc.alive
                  ? "wallet service: " + Model.walletStateLabel(svc.wallet, svc.warnings)
                  : "fmonero is not answering."
                color: root.dim; font.family: root.fontFamily
                font.pixelSize: Style.font.bodySmall; wrapMode: Text.WordWrap
              }
            }
          }

          // ---- pending (DESIGN.md §2a.4, §6.5) -----------------------------
          Column {
            width: parent.width
            spacing: Style.space(6)
            visible: !!svc.pending

            PanelSeparator { foreground: root.foreground }

            NavRow {
              id: pendingRow
              section: "pending"
              idx: 0
              width: parent.width
              implicitHeight: pendingText.implicitHeight + Style.spacing.rowPaddingX

              Column {
                id: pendingText
                anchors.left: parent.left; anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                anchors.leftMargin: Style.space(10); anchors.rightMargin: Style.space(10)
                spacing: Style.space(2)

                Text {
                  // `total_amount_xmr` is a pre-rendered string from Status JSON (§5b); a sweep's
                  // record holds several transactions and summing them here would be money
                  // arithmetic in QML (§2a.5, §6.5).
                  textFormat: Text.PlainText; width: parent.width
                  text: Model.pendingLabel(svc.pending, svc.showBalance)
                  color: root.urgent; font.family: root.fontFamily
                  font.pixelSize: Style.font.bodySmall; wrapMode: Text.WordWrap
                }
                Text {
                  textFormat: Text.PlainText; width: parent.width; text: "Enter opens the TUI"
                  color: root.dim; font.family: root.fontFamily; font.pixelSize: Style.font.caption
                }
              }
            }
          }

          // ---- footer ------------------------------------------------------
          Column {
            id: footerBlock
            width: parent.width
            spacing: Style.space(6)

            PanelSeparator { foreground: root.foreground }

            // **Two or three** (§6.5). `Open TUI` stays on `t` and on the hero, `Doctor` appears
            // as the action of the problem row that needs it — the only place a person wants it —
            // and `Send` was a button on a bar panel that opens a money screen, one key from a
            // route that is not.
            Row {
              width: parent.width
              spacing: Style.space(6)
              Chip {
                section: "footer"; idx: 0
                label: svc.walletOpen ? "Check for payments" : "Open wallet…"
              }
              Chip { section: "footer"; idx: 1; label: "Choose node…" }
              Chip {
                visible: root.footerCount > 2
                section: "footer"; idx: 2
                label: svc.desiredOn ? "Stop your node" : "Start your node"
              }
            }
          }
        }
      }
    }
  }

  // ---- reusable pieces -------------------------------------------------------
  component DetailRow: Row {
    property string label: ""
    property string value: ""
    width: parent ? parent.width : 0
    spacing: Style.space(8)

    Text {
      textFormat: Text.PlainText; width: Style.space(70); text: parent.label
      color: root.dim; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
    }
    Text {
      textFormat: Text.PlainText; width: parent.width - Style.space(78); text: parent.value
      color: root.foreground; font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall; elide: Text.ElideRight
    }
  }

  component NavRow: CursorSurface {
    id: navRow
    property string section: ""
    property int idx: 0

    hasCursor: root.cursorActive && root.focusSection === section && root.selectedIndex === idx
    foreground: root.foreground
    implicitHeight: Style.spacing.popupRowHeight

    MouseArea {
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onEntered: root.setCursor(navRow.section, navRow.idx)
      onClicked: { root.setCursor(navRow.section, navRow.idx); root.activateCursor() }
    }
  }

  component ListRow: NavRow {
    id: listRow
    property string title: ""
    property string meta: ""

    implicitHeight: visible
      ? Math.max(Style.spacing.popupRowHeight, rowText.implicitHeight + Style.spacing.rowPaddingX) : 0

    RowLayout {
      id: rowText
      anchors.left: parent.left; anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      anchors.leftMargin: Style.space(10); anchors.rightMargin: Style.space(10)
      spacing: Style.space(8)

      Text {
        textFormat: Text.PlainText; Layout.fillWidth: true; text: listRow.title
        color: root.foreground; font.family: root.fontFamily
        font.pixelSize: Style.font.body; elide: Text.ElideRight
      }
      Text {
        textFormat: Text.PlainText; text: listRow.meta; color: root.dim
        font.family: root.fontFamily; font.pixelSize: Style.font.caption; elide: Text.ElideRight
      }
    }
  }

  component WarnRow: NavRow {
    id: warnRow
    property string message: ""
    property string hint: ""
    property string severity: "warn"

    implicitHeight: warnText.implicitHeight + Style.spacing.rowPaddingX

    Column {
      id: warnText
      anchors.left: parent.left; anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      anchors.leftMargin: Style.space(10); anchors.rightMargin: Style.space(10)
      spacing: Style.space(2)

      Text {
        textFormat: Text.PlainText; width: parent.width; text: warnRow.message
        color: warnRow.severity === "info" ? root.foreground : root.urgent
        font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }
      Text {
        textFormat: Text.PlainText; visible: warnRow.hint !== ""
        width: parent.width; text: warnRow.hint; color: root.dim
        font.family: root.fontFamily; font.pixelSize: Style.font.caption; elide: Text.ElideRight
      }
    }
  }

  component Chip: NavRow {
    id: chip
    property string label: ""

    implicitWidth: chipText.implicitWidth + Style.space(20)
    implicitHeight: Style.spacing.controlHeight
    bordered: true

    Text {
      id: chipText
      anchors.centerIn: parent; textFormat: Text.PlainText; text: chip.label
      color: root.foreground; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
    }
  }
}
