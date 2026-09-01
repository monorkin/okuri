import QtQuick
import QtQuick.Controls
import sh.okuri

/// A menu entry that is one of several, only one of which can be true.
///
/// A tick box says "any number of these"; a dot says "one of these". The sort order is the
/// second kind, so it is drawn as the second kind.
MenuItem {
    id: item

    checkable: true

    // Every entry in the menu leaves the same room for a mark, so the labels line up whether or
    // not the entry has one.
    leftPadding: 44
    rightPadding: 16
    implicitHeight: 34

    indicator: Rectangle {
        x: 16
        y: item.topPadding + (item.availableHeight - height) / 2
        width: 15
        height: 15
        radius: 8
        color: "transparent"
        border.width: item.checked ? 5 : 1
        border.color: item.checked ? Theme.accent : Theme.muted

        Behavior on border.width {
            NumberAnimation { duration: 90 }
        }
    }

    contentItem: Text {
        text: item.text
        color: Theme.foreground
        verticalAlignment: Text.AlignVCenter
    }

    background: Rectangle {
        color: item.highlighted ? Qt.alpha(Theme.foreground, 0.10) : "transparent"
    }
}
