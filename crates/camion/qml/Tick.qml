import QtQuick
import QtQuick.Controls
import io.camion

/// One permission bit, on or off.
///
/// Disabled where the destination keeps no modes — an object store has nothing to set — and it
/// still shows what the file says, because reading the mode and changing it are different
/// things and only one of them needs the protocol's help.
CheckBox {
    id: tick

    /// Whether the file allows this. Kept as its own property so the box can be told what the
    /// file says without having to go looking for the file.
    property bool allowed: false

    /// Whether this destination lets a mode be changed, which only the panel above knows.
    property bool editable: false

    padding: 0
    checked: allowed
    enabled: editable
    opacity: enabled ? 1 : 0.45

    // Stated, because a control sizes itself from what its indicator asks for and a plain
    // rectangle asks for nothing — leaving the box no height at all, and so nothing on screen.
    implicitHeight: 22

    indicator: Rectangle {
        implicitWidth: 16
        implicitHeight: 16
        y: (tick.height - height) / 2
        radius: 4

        color: tick.checked ? Theme.accent : "transparent"
        border.width: 1
        border.color: tick.checked ? Theme.accent : Qt.alpha(Theme.foreground, 0.35)

        Behavior on color {
            ColorAnimation { duration: 90 }
        }

        Text {
            anchors.centerIn: parent
            visible: tick.checked
            text: "✓"
            font.pixelSize: 11
            color: Theme.accentText
        }
    }

    contentItem: Item {}
}
