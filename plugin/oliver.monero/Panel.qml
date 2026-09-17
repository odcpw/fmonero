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

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  // ---- bar icon state (DESIGN.md §6.4, first match wins) -------------------
  readonly property bool anyError: Model.worstSeverity(svc.warnings) === "error"
  readonly property int iconCase: !svc.alive ? 0
    : anyError ? 1
    : !svc.nodeUp ? 2
    : !svc.synced ? 3
    : svc.walletOpen ? 4 : 5
  readonly property color barIconColor: iconCase === 1 ? urgent
    : (iconCase === 0 || iconCase === 2) ? Qt.darker(barForeground, 1.55) : barForeground
  readonly property bool iconCrossed: iconCase === 0 || iconCase === 2
  readonly property bool iconWarning: iconCase <= 1
  readonly property bool iconFilled: iconCase === 4
  readonly property real iconRing: iconCase === 3 ? svc.syncPct / 100 : -1
  // The number only when the bar is tall enough to draw two legible digits inside the ring.
  readonly property string iconPercent:
    iconCase === 3 && bar && bar.barSize >= 30 ? String(svc.syncPct) : ""

  readonly property string tooltipText: {
    if (!svc.alive) return "Monero · fmonero is not answering"
    var parts = ["Monero"]
    if (svc.node && svc.node.height !== null && svc.node.height !== undefined)
      parts.push(Model.grouped(svc.node.height))
    if (svc.node && svc.node.outgoing_connections_count !== null
        && svc.node.outgoing_connections_count !== undefined)
      parts.push(Model.int(svc.node.outgoing_connections_count, 0) + " peers")
    parts.push(Model.syncLabel(svc.status))
    if (svc.walletOpen && svc.showBalance && svc.wallet.balance_xmr)
      parts.push(String(svc.wallet.balance_xmr) + " XMR")
    return parts.join(" · ")
  }

  // ---- panel state ---------------------------------------------------------
  property bool cursorActive: false
  property string focusSection: "header"
  property int selectedIndex: 0

  readonly property bool walletHidden: !svc.showBalance
  // 0 = nothing to act on (the daemon is down), 1 = "open one in the TUI", 2 = copy + refresh.
  readonly property int walletTargets: svc.walletOpen ? 2 : (svc.walletReachable ? 1 : 0)
  readonly property bool lockedFunds:
    svc.walletOpen && Model.int(svc.wallet.blocks_to_unlock, 0) > 0

  readonly property var navSections: {
    var s = ["header"]
    if (svc.warnings.length > 0) s.push("warnings")
    if (walletTargets > 0) s.push("wallet")
    if (svc.pending) s.push("pending")
    s.push("footer")
    return s
  }

  function sectionCount(name) {
    if (name === "warnings") return svc.warnings.length
    if (name === "wallet") return walletTargets
    if (name === "footer") return 3
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
    if (focusSection === "header") svc.toggleNode()
    else if (focusSection === "warnings") activateWarning(selectedIndex)
    else if (focusSection === "wallet") activateWallet(selectedIndex)
    else if (focusSection === "pending") { svc.openTui(); root.close() }
    else if (focusSection === "footer") activateFooter(selectedIndex)
  }

  // A hint is a runnable command (DESIGN.md §4). The doctor is the one we can run ourselves;
  // everything else goes to the clipboard so the user can paste it into a terminal.
  function activateWarning(index) {
    var w = svc.warnings[index]
    if (!w) return
    var hint = String(w.hint || "")
    if (hint === "") return
    if (hint.indexOf("fmonero doctor") === 0) { svc.openDoctor(); root.close(); return }
    svc.copyText(hint)
  }

  function activateWallet(index) {
    if (walletTargets === 1) { svc.openTui(); root.close(); return }
    if (index === 0) svc.copyAddress()
    else svc.refreshWallet()
  }

  function activateFooter(index) {
    if (index === 0) { svc.openTui(); root.close() }
    else if (index === 1) { svc.openSendScreen(); root.close() }
    else { svc.openDoctor(); root.close() }
  }

  // ---- scrolling -----------------------------------------------------------
  function sectionColumn(name) {
    if (name === "warnings") return warningColumn
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
      return JSON.stringify({ section: root.focusSection, index: root.selectedIndex,
                              active: root.cursorActive, opened: root.opened,
                              sections: root.navSections, alive: svc.alive,
                              binaryMissing: svc.binaryMissing, nodeUp: svc.nodeUp,
                              synced: svc.synced, walletOpen: svc.walletOpen,
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
        ringProgress: root.iconRing
        percentText: root.iconPercent
      }
    }
    onPressed: function(buttonCode) {
      if (buttonCode === Qt.RightButton) svc.toggleNode()
      else if (buttonCode === Qt.MiddleButton) svc.refresh()
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
        if (k === "n") svc.toggleNode()
        else if (k === "r") svc.refresh()
        else if (k === "t") { svc.openTui(); root.close() }
        else if (k === "c") svc.copyAddress()
        else if (k === "d") { svc.openDoctor(); root.close() }
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

            PanelHero {
              id: hero
              width: parent.width
              title: svc.alive ? Model.heightLabel(svc.node) : "Node"
              meta: Model.syncLabel(svc.status)
              detail: svc.node && svc.node.nettype ? String(svc.node.nettype) : ""
              foreground: root.foreground
              fontFamily: root.fontFamily
              iconOpacity: svc.nodeUp ? 1.0 : 0.5
              iconComponent: Component {
                MoneroIcon {
                  iconSize: Style.font.display
                  color: svc.nodeUp ? root.foreground : root.dim
                  badgeColor: root.urgent
                  crossed: svc.alive && !svc.nodeUp
                  warning: !svc.alive || root.anyError
                  filled: svc.walletOpen
                }
              }
              trailingControl: Component {
                ToggleSwitch {
                  id: nodeSwitch
                  checked: svc.desiredOn
                  busy: svc.busy
                  hasCursor: heroBlock.ringVisible
                  foreground: hero.foreground
                  onHovered: function(on) { if (on) heroBlock.focusHero() }
                  onToggled: svc.toggleNode()

                  PanelToolTip {
                    visible: nodeSwitch.containsMouse
                    text: svc.desiredOn ? "Stop the node (n)" : "Start the node (n)"
                    fontFamily: hero.fontFamily
                  }
                }
              }
            }
          }

          // ---- node detail (not cursor targets) ----------------------------
          Column {
            width: parent.width
            spacing: Style.space(3)
            visible: svc.alive && svc.nodeUp && !svc.binaryMissing

            MiniBar {
              width: parent.width
              visible: !svc.synced
              progress: svc.syncPct / 100
              label: svc.node && svc.node.target_height
                ? Model.grouped(svc.node.height) + " / " + Model.grouped(svc.node.target_height)
                : Model.grouped(svc.node ? svc.node.height : 0)
              percent: svc.syncPct
            }

            DetailRow { label: "Peers"; value: Model.peersLabel(svc.node) }
            DetailRow {
              label: "Mempool"
              value: svc.node && svc.node.tx_pool_size !== null && svc.node.tx_pool_size !== undefined
                ? Model.int(svc.node.tx_pool_size, 0) + " tx" + (svc.node.restricted === true ? " (lower bound)" : "")
                : "—"
            }
            DetailRow { label: "Last block"; value: svc.node ? Model.age(svc.node.top_block_age_sec) + " ago" : "—" }
            DetailRow {
              label: "Net"
              value: svc.node && svc.node.net
                ? "↓ " + Model.rate(svc.node.net.rx_bps) + "   ↑ " + Model.rate(svc.node.net.tx_bps)
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
            text: "fmonero is not installed or not on PATH (~/.local/bin)."
            color: root.urgent; font.family: root.fontFamily
            font.pixelSize: Style.font.body; wrapMode: Text.WordWrap
          }

          // ---- warnings ----------------------------------------------------
          Column {
            width: parent.width
            spacing: Style.space(6)
            visible: svc.warnings.length > 0

            PanelSeparator { foreground: root.foreground }

            Column {
              id: warningColumn
              width: parent.width
              spacing: Style.space(4)

              Repeater {
                model: svc.warnings
                WarnRow {
                  required property var modelData
                  required property int index
                  section: "warnings"
                  idx: index
                  width: warningColumn.width
                  message: String(modelData.text || "")
                  hint: String(modelData.hint || "")
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
              visible: svc.walletOpen

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
                text: "↑ locked for " + (svc.walletOpen ? Model.int(svc.wallet.blocks_to_unlock, 0) : 0) + " blocks"
                color: root.dim; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
              }
            }

            Column {
              id: walletActionColumn
              width: parent.width
              spacing: Style.space(4)

              ListRow {
                visible: svc.walletOpen
                section: "wallet"; idx: 0; width: parent.width
                title: svc.walletOpen && svc.wallet.primary_address
                  ? Model.shortAddr(svc.wallet.primary_address) : "no address yet"
                meta: "copy (c)"
              }
              ListRow {
                visible: svc.walletOpen
                section: "wallet"; idx: 1; width: parent.width
                title: "Refresh the wallet"
                meta: svc.walletOpen && svc.wallet.blocks_behind !== null
                        && svc.wallet.blocks_behind !== undefined
                  ? Model.int(svc.wallet.blocks_behind, 0) + " blocks behind" : ""
              }
              ListRow {
                visible: !svc.walletOpen && svc.walletReachable
                section: "wallet"; idx: 0; width: parent.width
                title: "No wallet is loaded — load one in the TUI"
                meta: "open TUI (t)"
              }
            }

            Text {
              textFormat: Text.PlainText; visible: !svc.walletReachable; width: parent.width
              text: svc.alive ? "Wallet daemon is not running." : "No wallet state — fmonero is not answering."
              color: root.dim; font.family: root.fontFamily
              font.pixelSize: Style.font.bodySmall; wrapMode: Text.WordWrap
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

            Row {
              width: parent.width
              spacing: Style.space(6)
              Chip { section: "footer"; idx: 0; label: "Open TUI" }
              Chip { section: "footer"; idx: 1; label: "Send" }
              Chip { section: "footer"; idx: 2; label: "Doctor" }
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

  // The sync gauge: a track, a fill and the percentage. Not a cursor target.
  component MiniBar: Item {
    id: gauge
    property real progress: 0
    property string label: ""
    property int percent: 0

    implicitHeight: gaugeLabel.implicitHeight + Style.space(9)

    Rectangle {
      id: track
      anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
      height: Math.max(3, Style.space(4)); radius: height / 2
      color: Util.alpha(root.foreground, 0.18)

      Rectangle {
        height: parent.height; radius: parent.radius; color: root.foreground
        width: Math.max(0, Math.min(1, gauge.progress)) * parent.width
        Behavior on width { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
      }
    }

    Text {
      id: gaugeLabel
      textFormat: Text.PlainText
      anchors.left: parent.left; anchors.top: track.bottom; anchors.topMargin: Style.space(3)
      text: gauge.percent + "%  " + gauge.label
      color: root.dim; font.family: root.fontFamily; font.pixelSize: Style.font.caption
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
