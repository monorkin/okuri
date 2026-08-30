import QtQuick
import QtQuick.Controls
import io.camion

/// An on/off switch in the window's own colours.
Switch {
    id: control

    implicitWidth: 42
    implicitHeight: 24
    padding: 0

    indicator: Rectangle {
        width: control.implicitWidth
        height: control.implicitHeight
        radius: height / 2
        color: control.checked ? Theme.accent : Qt.alpha(Theme.foreground, 0.18)
        border.width: control.visualFocus ? 2 : 0
        border.color: Theme.accent

        Behavior on color {
            ColorAnimation { duration: 120 }
        }

        Rectangle {
            x: control.checked ? parent.width - width - 3 : 3
            y: 3
            width: parent.height - 6
            height: width
            radius: width / 2
            color: control.checked ? Theme.accentText : Theme.bright

            Behavior on x {
                NumberAnimation { duration: 120; easing.type: Easing.OutCubic }
            }
        }
    }

    // The label lives in the row beside it, so the control itself is only the switch.
    contentItem: Item {}
}
