import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import qs.Ui as Ui
import "Model.js" as Model

Panel {
  id: root
  moduleName: "oliver.monero"
  ipcTarget: "oliver.monero"
  manageIpc: false

  readonly property string panelVersion: "qml-63e18992"

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  readonly property bool anyError: Model.worstSeverity(svc.warnings) === "error"
  readonly property int iconCase: !svc.alive ? 0
    : anyError ? 1
    : svc.walletBehind ? 8
    : (svc.remote && svc.walletOpen) ? 6
    : svc.remote ? 7
    : !svc.nodeUp ? 2
    : !svc.synced ? 3
    : svc.walletOpen ? 4 : 5
  readonly property color barIconColor: iconCase === 1 ? urgent
    : (iconCase === 0 || iconCase === 2 || (!svc.walletUp && !svc.walletReachable)) ? Qt.darker(barForeground, 1.55) : barForeground
  readonly property bool iconCrossed: iconCase === 0 || iconCase === 2
  readonly property bool iconWarning: iconCase <= 1
  readonly property bool iconFilled: iconCase === 4 || iconCase === 6 || iconCase === 8
  readonly property bool iconRemote: iconCase === 6 || iconCase === 7 || (iconCase === 8 && svc.remote)
  readonly property real iconRing: iconCase === 8 ? svc.walletSyncPct / 100
    : iconCase === 3 ? svc.syncPct / 100 : -1
  readonly property string iconPercent: !(bar && bar.barSize >= 30) ? ""
    : iconCase === 8 ? String(svc.walletSyncPct)
    : iconCase === 3 ? String(svc.syncPct) : ""

  readonly property string headerLine:
    Model.headerLine(svc.status, svc.showBalance, svc.binaryMissing)
  readonly property string tooltipText: {
    var parts = [root.headerLine]
    if (svc.alive && svc.remote && svc.probe) parts.push(Model.checkedLabel(svc.node))
    if (svc.alive && svc.walletBehind) parts.push(Model.grouped(svc.blocksLeft) + " blocks left")
    return parts.join(" · ")
  }

  property bool cursorActive: false
  property string focusSection: "header"
  property int selectedIndex: 0
  property bool choosingWallet: false
  property string selectedWallet: ""
  function showWalletPicker() {
    if (svc.busy) return
    choosingWallet = true
    selectedWallet = ""
    unlockField.clear()
    svc.listWallets()
    setCursor("picker", 0)
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }
  function chooseWallet(index) {
    if (svc.busy || index < 0 || index >= svc.wallets.length) return
    if (svc.walletOpen && svc.wallet.name === svc.wallets[index]) { dismissPicker(); return }
    selectedWallet = String(svc.wallets[index])
    unlockField.clear()
    Qt.callLater(function() { unlockField.forceActiveFocus(); scrollItemIntoView(unlockForm) })
  }
  function dismissPicker() {
    unlockField.clear()
    selectedWallet = ""
    choosingWallet = false
    setCursor("header", 0)
    keyCatcher.forceActiveFocus()
  }
  function submitWallet() {
    if (svc.openWallet(selectedWallet, unlockField.text)) unlockField.clear()
  }
  function powerAction() {
    if (svc.busy) return
    if (svc.walletUp || svc.walletReachable) svc.stopWallet()
    else showWalletPicker()
  }
  function activateHeader(index) {
    if (index === 0) showWalletPicker()
    else if (index === 1) { svc.openTui(); root.close() }
    else if (index === 2) { svc.refresh(); if (choosingWallet) svc.listWallets() }
    else powerAction()
  }

  readonly property bool walletHidden: !svc.showBalance
  readonly property int walletTargets: svc.walletOpen && !choosingWallet ? 3 : 0
  readonly property bool lockedFunds:
    svc.walletOpen && Model.int(svc.wallet.blocks_to_unlock, 0) > 0
  readonly property string spendableLine: {
    if (root.walletHidden || !svc.wallet) return "some of it is locked"
    var short = svc.wallet.unlocked_xmr_short || svc.wallet.unlocked_xmr
    return short ? String(short) + " XMR to spend" : "some of it is locked"
  }
  readonly property int lockedMinutes: {
    if (!svc.walletOpen) return 0
    var sec = svc.wallet.time_to_unlock_sec
    if (sec !== null && sec !== undefined) return Math.ceil(Model.int(sec, 0) / 60)
    return Model.int(svc.wallet.blocks_to_unlock, 0) * 2
  }

  readonly property string heroMeta: {
    if (svc.binaryMissing || !svc.alive || svc.pending) return ""
    if (svc.walletBusy) return "A scan may be in progress. Checking again shortly."
    if (svc.walletBehind) return Model.grouped(svc.blocksLeft) + " blocks left"
    if (svc.walletOpen) return root.lockedFunds ? root.spendableLine : refreshedAgo()
    if (svc.remote || !svc.node) return ""
    if (svc.nodeUp && !svc.synced && svc.node.target_height)
      return Model.grouped(svc.node.height) + " / " + Model.grouped(svc.node.target_height)
    return ""
  }

  function refreshedAgo() {
    var raw = svc.walletOpen ? svc.wallet.last_refresh_at : null
    if (!raw) return ""
    var t = Date.parse(String(raw))
    if (!isFinite(t)) return ""
    return "refreshed " + Model.age(Math.floor((Date.now() - t) / 1000)) + " ago"
  }

  readonly property var problems: {
    var out = []
    for (var i = 0; i < svc.warnings.length; i++) {
      var w = svc.warnings[i]
      var sev = String(w.severity || "")
      var id = String(w.id || "")
      if (sev !== "warn" && sev !== "error") continue
      if (svc.pending && id === "send_may_be_in_flight") continue
      if (svc.walletBehind && id === "wallet_behind") continue
      out.push(w)
    }
    return out
  }

  readonly property int footerCount: svc.remote || !svc.alive ? 2 : 3

  readonly property var navSections: {
    var s = ["header"]
    if (choosingWallet) { s.push("picker"); return s }
    if (problems.length > 0) s.push("problems")
    if (walletTargets > 0) s.push("wallet")
    if (svc.pending) s.push("pending")
    s.push("footer")
    return s
  }

  function sectionCount(name) {
    if (name === "header") return 4
    if (name === "picker") return svc.wallets.length
    if (name === "problems") return problems.length
    if (name === "wallet") return walletTargets
    if (name === "footer") return footerCount
    return 1
  }
  function isHorizontal(name) { return name === "footer" || name === "header" }

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
    if (focusSection === "header") activateHeader(selectedIndex)
    else if (focusSection === "picker") chooseWallet(selectedIndex)
    else if (focusSection === "problems") activateProblem(selectedIndex)
    else if (focusSection === "wallet") activateWallet(selectedIndex)
    else if (focusSection === "pending") { svc.openTui(); root.close() }
    else if (focusSection === "footer") activateFooter(selectedIndex)
  }

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
    root.showWalletPicker()
  }

  function activateWallet(index) {
    if (index === 0) svc.copyAddress()
    else if (index === 1) svc.refreshWallet()
    else svc.closeWallet()
  }

  function activateFooter(index) {
    if (index === 0) showWalletPicker()
    else if (index === 1) { svc.openNodeScreen(); root.close() }
    else svc.toggleNode()
  }

  function sectionColumn(name) {
    if (name === "picker") return pickerRows
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
    if (!opened) { unlockField.clear(); selectedWallet = ""; choosingWallet = false; return }
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
    onWalletOpened: root.dismissPicker()
    onWalletOpenFailed: Qt.callLater(function() { if (root.opened && root.selectedWallet !== "") unlockField.forceActiveFocus() })
    onWalletStopped: root.close()
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
    contentWidth: panel.fittedContentWidth(Style.space(400))
    contentHeight: panel.fittedContentHeight(column.implicitHeight, Style.space(560))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      blocked: root.selectedWallet !== "" && !svc.busy
      onMoveRequested: function(dx, dy) {
        if (!root.cursorActive) { root.cursorActive = true; return }
        root.moveCursor(dx, dy)
      }
      onActivateRequested: if (root.cursorActive) root.activateCursor()
      onCloseRequested: { if (root.choosingWallet) root.dismissPicker(); else root.close() }
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onTextKey: function(t) {
        var k = String(t).toLowerCase()
        if (k === "n") { svc.openNodeScreen(); root.close() }
        else if (k === "r") svc.refresh()
        else if (k === "t") { svc.openTui(); root.close() }
        else if (k === "c") svc.copyAddress()
        else if (k === "d") { svc.openDoctor(); root.close() }
        else if (k === "w") root.showWalletPicker()
        else if (k === "q") root.powerAction()
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
        ScrollBar.vertical: ScrollBar { policy: panelFlick.contentHeight > panelFlick.height
                                                ? ScrollBar.AlwaysOn : ScrollBar.AlwaysOff }

        Column {
          id: column
          width: panelFlick.width
          spacing: Style.space(10)

          PanelHero {
            id: heroBlock
            width: parent.width
            title: "Monero"
            meta: svc.walletOpen && svc.wallet.name ? String(svc.wallet.name) : "Wallets"
            foreground: root.foreground
            fontFamily: root.fontFamily
            iconOpacity: svc.walletUp || svc.walletReachable ? 1 : 0.5
            iconComponent: Component {
              MoneroIcon { iconSize: Style.font.display; color: heroBlock.foreground; filled: svc.walletOpen; remote: svc.remote }
            }
            trailingControl: Component {
              Row {
                spacing: Style.space(4)
                HeaderAction { idx: 0; iconText: "󰖄"; tooltipText: "Open wallet (w)"; enabled: !svc.busy; onClicked: root.showWalletPicker() }
                HeaderAction { idx: 1; iconText: "󰆍"; tooltipText: "More in the TUI (t)"; onClicked: { svc.openTui(); root.close() } }
                HeaderAction { idx: 2; iconText: "󰑐"; tooltipText: "Refresh (r)"; enabled: !svc.busy; onClicked: root.activateHeader(2) }
                HeaderAction { idx: 3; iconText: "󰐥"; tooltipText: svc.walletUp || svc.walletReachable ? "Save and stop wallet service (q)" : "Open a wallet (q)"; enabled: !svc.busy; onClicked: root.powerAction() }
              }
            }
          }
          Column {
            width: parent.width
            visible: !root.choosingWallet
            spacing: Style.space(4)
            Text { textFormat: Text.PlainText; width: parent.width; text: root.headerLine; color: root.foreground; font.family: root.fontFamily; font.pixelSize: Style.font.body; wrapMode: Text.WordWrap }
            Text { textFormat: Text.PlainText; width: parent.width; visible: text !== ""; text: root.heroMeta; color: root.dim; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall; wrapMode: Text.WordWrap }
            Rectangle {
              visible: svc.walletBehind
              width: parent.width; height: Style.space(3); radius: height / 2
              color: Qt.darker(root.foreground, 3)
              Rectangle { width: parent.width * svc.walletSyncPct / 100; height: parent.height; radius: height / 2; color: root.foreground }
            }
          }
          Column {
            id: pickerBlock
            width: parent.width
            visible: root.choosingWallet
            spacing: Style.space(8)
            PanelSeparator { foreground: root.foreground }
            PanelSectionHeader { text: root.selectedWallet === "" ? "OPEN WALLET" : root.selectedWallet; foreground: root.foreground; fontFamily: root.fontFamily }
            Text {
              textFormat: Text.PlainText; width: parent.width
              visible: root.selectedWallet === "" && (svc.listingWallets || svc.walletListError !== "" || svc.wallets.length === 0)
              text: svc.listingWallets ? "Loading wallets…" : svc.walletListError !== "" ? svc.walletListError : "No wallets yet. Create or restore one in the TUI."
              color: svc.walletListError !== "" ? root.urgent : root.dim
              font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall; wrapMode: Text.WordWrap
            }
            Column {
              id: pickerRows
              width: parent.width; spacing: Style.space(4)
              visible: root.selectedWallet === ""
              Repeater {
                model: svc.wallets
                ListRow {
                  required property string modelData
                  required property int index
                  section: "picker"; idx: index; width: pickerRows.width
                  title: modelData
                  meta: svc.walletOpen && svc.wallet.name === modelData ? "Open" : "Open →"
                }
              }
            }
            Column {
              id: unlockForm
              width: parent.width; spacing: Style.space(8)
              visible: root.selectedWallet !== ""
              Keys.onEscapePressed: { if (!svc.busy) root.showWalletPicker() }
              Ui.TextField {
                id: unlockField
                objectName: "walletPassword"
                width: parent.width
                password: true
                maximumLength: 4096
                placeholderText: "Wallet password"
                enabled: !svc.busy
                inputMethodHints: Qt.ImhSensitiveData | Qt.ImhNoPredictiveText | Qt.ImhHiddenText
                onAccepted: root.submitWallet()
                Keys.onEscapePressed: root.showWalletPicker()
              }
              Row {
                spacing: Style.space(6)
                Ui.Button { text: svc.busy ? "Opening…" : "Open wallet"; bordered: true; focusable: true; enabled: !svc.busy; onClicked: root.submitWallet() }
                Ui.Button { text: "Back"; bordered: true; focusable: true; enabled: !svc.busy; onClicked: root.showWalletPicker() }
              }
            }
            Ui.Button { visible: root.selectedWallet === ""; text: "Back"; bordered: true; onClicked: root.dismissPicker() }
          }

          Column {
            width: parent.width
            spacing: Style.space(3)
            visible: !root.choosingWallet && svc.alive && !svc.binaryMissing

            DetailRow { label: "Node"; value: Model.nodeLabel(svc.node) }
            DetailRow {
              visible: svc.remote
              label: "Checked"; value: Model.checkedLabel(svc.node)
            }
          }

          Column {
            width: parent.width
            spacing: Style.space(3)
            visible: !root.choosingWallet && svc.alive && svc.nodeUp && !svc.remote && !svc.binaryMissing

            DetailRow { label: "Peers"; value: Model.peersLabel(svc.node) }
            DetailRow {
              label: "Waiting"
              value: svc.node && svc.node.tx_pool_size !== null && svc.node.tx_pool_size !== undefined
                ? Model.waitingLabel(svc.node.tx_pool_size, svc.node.restricted === true)
                : "—"
            }
          }

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

          Column {
            width: parent.width
            spacing: Style.space(6)
            visible: !root.choosingWallet && root.problems.length > 0

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
                  hint: String(modelData.panel_action || modelData.action || "")
                  severity: String(modelData.severity || "warn")
                }
              }
            }
          }

          Column {
            visible: !root.choosingWallet
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
              Text {
                textFormat: Text.PlainText; visible: root.lockedFunds; width: parent.width
                text: (root.walletHidden || !(svc.wallet && svc.wallet.locked_xmr)
                       ? "locked for about " : String(svc.wallet.locked_xmr) + " XMR locked for about ")
                      + root.lockedMinutes + " min"
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
              ListRow {
                visible: svc.walletOpen
                enabled: !svc.busy
                section: "wallet"; idx: 2; width: parent.width
                title: "Close wallet"; meta: "Keep service running"
              }
            }

            Text {
              textFormat: Text.PlainText; visible: svc.alive && svc.walletBehind
              width: parent.width
              text: "This keeps going in the background, even after you close this window."
              color: root.dim; font.family: root.fontFamily
              font.pixelSize: Style.font.bodySmall; wrapMode: Text.WordWrap
            }

            Column {
              width: parent.width
              spacing: Style.space(2)
              visible: !svc.walletOpen

              DetailRow { visible: svc.alive; label: "wallet"; value: svc.walletBusy ? "Waiting for the wallet" : "Wallet closed" }
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

          Column {
            id: footerBlock
            visible: !root.choosingWallet
            width: parent.width
            spacing: Style.space(6)

            PanelSeparator { foreground: root.foreground }

            Row {
              width: parent.width
              spacing: Style.space(6)
              Chip {
                section: "footer"; idx: 0
                label: svc.walletOpen ? "Switch wallet…" : "Open wallet…"
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

  component HeaderAction: PanelActionButton {
    property int idx: 0
    foreground: root.foreground
    fontFamily: root.fontFamily
    bordered: true
    hasCursor: root.cursorActive && root.focusSection === "header" && root.selectedIndex === idx
    onHovered: function(on) { if (on) root.setCursor("header", idx) }
  }

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
    enabled: !svc.busy

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
