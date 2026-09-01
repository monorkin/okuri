import QtQuick
import QtQuick.Controls
import sh.okuri

/// Which columns the list shows.
///
/// The heading is drawn here rather than set as the dialog's title, which would draw a second
/// one above it saying the same thing.
Dialog {
    id: columns

    modal: true
    anchors.centerIn: Overlay.overlay
    width: Math.min(360, Overlay.overlay ? Overlay.overlay.width - 60 : 360)
    padding: 20

    background: Rectangle {
        radius: 10
        color: Theme.elevated
        border.width: 1
        border.color: Theme.border
    }

    contentItem: Column {
        spacing: 2

        Text {
            text: "Visible columns"
            font.pixelSize: 17
            color: Theme.bright
            bottomPadding: 10
        }

        /// Name is not a switch. A file list without names is not a file list, so it is shown
        /// as fixed rather than as something you can turn off and regret.
        Item {
            width: parent.width
            height: 40

            Text {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                text: "Name"
                color: Theme.muted
            }

            Text {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: "always"
                color: Theme.muted
                font.pixelSize: 12
            }
        }

        Repeater {
            model: [
                { label: "Size", column: "size" },
                { label: "Type", column: "kind" },
                { label: "Modified", column: "modified" },
                { label: "Permissions", column: "permissions" }
            ]

            // Anchored rather than laid out in a row, so every switch lines up down the right
            // edge however long its label is.
            Item {
                id: row

                required property var modelData

                width: parent.width
                height: 40

                Text {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    text: row.modelData.label
                    color: Theme.foreground
                }

                Toggle {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    checked: {
                        switch (row.modelData.column) {
                        case "size": return Display.showSize
                        case "kind": return Display.showKind
                        case "modified": return Display.showModified
                        default: return Display.showPermissions
                        }
                    }
                    onToggled: Display.showColumn(row.modelData.column, checked)
                }
            }
        }

        Item { width: 1; height: 14 }

        FlatButton {
            width: parent.width
            text: "Done"
            highlighted: true
            onClicked: columns.visible = false
        }
    }
}
