import QtQuick
import QtQuick.Controls
import sh.okuri

/// How the list is shown: how big, in what order, and with which columns.
Menu {
    id: menu

    signal editColumns()

    background: Rectangle {
        implicitWidth: 260
        radius: 8
        color: Theme.elevated
        border.width: 1
        border.color: Theme.border
    }

    /// Icon size, as two steps rather than a slider — and it changes both views, so it sits
    /// above the rest rather than inside either one.
    Item {
        implicitWidth: parent.width
        implicitHeight: 38

        Label {
            anchors.left: parent.left
            anchors.leftMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            text: "Icon size"
            color: Theme.foreground
        }

        Row {
            anchors.right: parent.right
            anchors.rightMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2

            FlatButton {
                text: "−"
                hint: "Smaller"
                enabled: Display.canShrink
                onClicked: Display.resize(-1)
            }

            FlatButton {
                text: "+"
                hint: "Larger"
                enabled: Display.canGrow
                onClicked: Display.resize(1)
            }
        }
    }

    MenuSeparator {}

    Label {
        text: "Sort"
        color: Theme.muted
        font.pixelSize: 12
        leftPadding: 16
        topPadding: 6
        bottomPadding: 2
    }

    Repeater {
        model: [
            { label: "A–Z", column: "name", descending: false },
            { label: "Z–A", column: "name", descending: true },
            { label: "Last modified", column: "modified", descending: true },
            { label: "First modified", column: "modified", descending: false },
            { label: "Size", column: "size", descending: true },
            { label: "Type", column: "kind", descending: false }
        ]

        RadioItem {
            required property var modelData

            text: modelData.label
            checked: Display.sortColumn === modelData.column
                && Display.sortDescending === modelData.descending
            onTriggered: Display.sortAs(modelData.column, modelData.descending)
        }
    }

    MenuSeparator {}

    CheckItem {
        text: "Show hidden files"
        // The shortcut is shown rather than only bound, so it can be learned from here.
        shortcut: "Ctrl+H"
        checked: Display.showHidden
        onTriggered: Display.toggleHidden()
    }

    CheckItem {
        text: "Visible columns…"
        checkable: false
        enabled: !Display.isGrid
        onTriggered: menu.editColumns()
    }
}
