import QtQuick
import QtQuick.Controls
import io.camion

Button {
    id: control

    property string hint: ""

    /// Which corners are rounded. Buttons that sit together in a group round only their outer
    /// edges, so a pair reads as one control rather than two that happen to be adjacent.
    property string side: "all"

    readonly property real corner: 6

    hoverEnabled: true
    ToolTip.visible: hint !== "" && hovered
    ToolTip.text: hint
    ToolTip.delay: 600

    contentItem: Text {
        text: control.text
        font: control.font
        color: control.highlighted ? Theme.accentText : Theme.foreground
        opacity: control.enabled ? 1 : 0.4
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }

    background: Rectangle {
        implicitHeight: 30

        // Tinted with the foreground rather than set to a fixed colour, so the hover shows up
        // whatever the button is sitting on — a toolbar, a menu, or a dialog.
        color: {
            if (control.highlighted) {
                return Theme.accent
            } else if (control.down) {
                return Qt.alpha(Theme.foreground, 0.20)
            } else if (control.hovered && control.enabled) {
                return Qt.alpha(Theme.foreground, 0.10)
            } else {
                return "transparent"
            }
        }

        topLeftRadius: control.side === "right" ? 0 : control.corner
        bottomLeftRadius: control.side === "right" ? 0 : control.corner
        topRightRadius: control.side === "left" ? 0 : control.corner
        bottomRightRadius: control.side === "left" ? 0 : control.corner

        Behavior on color {
            ColorAnimation { duration: 90 }
        }
    }

    leftPadding: 12
    rightPadding: 12
}
