import QtQuick
import QtQuick.Controls
import sh.okuri

/// A menu entry that is on or off by itself, and the shortcut that also turns it.
MenuItem {
    id: item

    property string shortcut: ""

    checkable: true

    // The same left margin as every other entry, so the labels line up down the menu.
    leftPadding: 44
    rightPadding: 16
    implicitHeight: 34

    indicator: Rectangle {
        // An entry that only does something has no state to show, but still leaves the room
        // so its label lines up with the ones that do.
        visible: item.checkable
        x: 16
        y: item.topPadding + (item.availableHeight - height) / 2
        width: 15
        height: 15
        radius: 3
        color: item.checked ? Theme.accent : "transparent"
        border.width: item.checked ? 0 : 1
        border.color: Theme.muted

        Text {
            anchors.centerIn: parent
            visible: item.checked
            text: "✓"
            font.pixelSize: 11
            color: Theme.accentText
        }
    }

    contentItem: Row {
        spacing: 12

        Text {
            text: item.text
            color: Theme.foreground
            height: item.availableHeight
            verticalAlignment: Text.AlignVCenter
        }

        Text {
            text: item.shortcut
            visible: item.shortcut !== ""
            color: Theme.muted
            font.pixelSize: 12
            height: item.availableHeight
            verticalAlignment: Text.AlignVCenter
        }
    }

    background: Rectangle {
        color: item.highlighted ? Qt.alpha(Theme.foreground, 0.10) : "transparent"
    }
}
