import QtQuick
import sh.okuri

/// The column headings, which double as the sort control.
Rectangle {
    id: header

    required property var columns
    required property real nameWidth

    signal sort(string column)

    implicitHeight: 28
    color: Theme.background

    Row {
        anchors.fill: parent
        anchors.leftMargin: 14
        anchors.rightMargin: 14
        spacing: 10

        Item { width: Display.listIcon; height: parent.height }

        Heading {
            width: header.nameWidth
            text: "Name"
            column: "name"
            onClicked: header.sort("name")
        }

        Repeater {
            model: header.columns

            Heading {
                required property var modelData

                width: modelData.width
                alignment: modelData.align
                column: modelData.role
                text: {
                    switch (modelData.role) {
                    case "size": return "Size"
                    case "kind": return "Type"
                    case "modified": return "Modified"
                    default: return "Permissions"
                    }
                }
                onClicked: header.sort(modelData.role)
            }
        }
    }

    Rectangle {
        anchors.bottom: parent.bottom
        width: parent.width
        height: 1
        color: Theme.border
    }
}
