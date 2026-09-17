import QtQuick
import QtQuick.Shapes
import qs.Commons
import qs.Ui

// The Monero mark, drawn natively so it takes the theme foreground instead of the brand orange —
// the same reason Omarchy's Tailscale widget redraws its logo rather than scaling a tiny SVG
// (DESIGN.md §6.1: Shapes + PathSvg, no image asset). The geometry is constructed here rather than
// traced from the brand artwork: a ring, and the M as one stroked polyline. `crossed`/`warning`
// follow Omarchy's own Ui/../tailscale/TailscaleIcon.qml.
Item {
  id: root

  property real iconSize: Style.font.icon
  property color color: Color.foreground
  property color badgeColor: Color.urgent
  property bool crossed: false
  property bool warning: false
  property bool filled: false        // a wallet is open (DESIGN.md §6.4)
  property real ringProgress: -1     // 0..1 sync ring around the glyph; < 0 draws none
  property string percentText: ""    // drawn instead of the M when the bar is tall enough
  readonly property real viewBox: 24

  width: iconSize; height: iconSize; implicitWidth: iconSize; implicitHeight: iconSize

  Item {
    anchors.centerIn: parent
    width: root.viewBox; height: root.viewBox
    // Scaled from the 24px viewBox to the bar's icon size, so the geometry stays crisp.
    scale: Math.min(root.width, root.height) / root.viewBox

    Shape {
      anchors.fill: parent; antialiasing: true; preferredRendererType: Shape.CurveRenderer

      ShapePath {   // the ring, filled when a wallet is open
        fillColor: root.filled ? Util.alpha(root.color, 0.3) : "transparent"
        strokeColor: root.color; strokeWidth: 1.6
        PathAngleArc { centerX: 12; centerY: 12; radiusX: 9.4; radiusY: 9.4; startAngle: 0; sweepAngle: 360 }
      }
      ShapePath {   // the M
        fillColor: "transparent"; strokeColor: root.color; strokeWidth: 1.8
        capStyle: ShapePath.RoundCap; joinStyle: ShapePath.RoundJoin
        PathSvg { path: root.percentText === "" ? "M 5.4 16.2 L 5.4 8.4 L 12 16.2 L 18.6 8.4 L 18.6 16.2" : "" }
      }
      ShapePath {   // the sync ring, outside the glyph
        fillColor: "transparent"; strokeColor: root.color; strokeWidth: 1.8; capStyle: ShapePath.RoundCap
        PathAngleArc { centerX: 12; centerY: 12; radiusX: 11.1; radiusY: 11.1; startAngle: -90
                       sweepAngle: root.ringProgress < 0 ? 0 : 360 * Math.min(1, root.ringProgress) }
      }
    }

    Text {
      anchors.centerIn: parent; textFormat: Text.PlainText
      visible: root.percentText !== ""; text: root.percentText; color: root.color
      font.family: Style.font.family; font.pixelSize: 11; font.bold: true
    }
  }

  // Node down / no answer: one stroke through the mark.
  Rectangle {
    visible: root.crossed; anchors.centerIn: parent
    width: parent.width * 1.22; height: Math.max(2, parent.height * 0.14)
    radius: height / 2; color: root.color; rotation: -45
  }

  // Something needs a human: an error-severity warning, or no answer at all.
  BorderSurface {
    visible: root.warning
    width: Math.max(7, parent.width * 0.42); height: width; radius: width / 2
    color: root.badgeColor
    anchors.right: parent.right; anchors.bottom: parent.bottom
    borderSpec: Border.flat(Color.popups.background, 1)

    Text {
      anchors.centerIn: parent; textFormat: Text.PlainText
      text: "!"; color: Color.background; font.family: Style.font.family
      font.pixelSize: Math.max(6, parent.height * 0.72); font.bold: true
    }
  }
}
