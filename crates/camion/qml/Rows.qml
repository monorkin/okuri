import QtQuick
import QtQuick.Controls
import io.camion

/// The file list as rows, with the columns you asked for.
ListView {
    id: rows

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

    required property var files
    required property var selection

    /// The full path of a row, which is what a drop onto it means.
    function folderOf(row) {
        const name = files.nameAt(row)

        return files.path === "/" ? "/" + name : files.path + "/" + name
    }

    /// The row the pointer is over, which the browser sets: the pointer is handled in one
    /// place so that a gesture survives the listing being replaced underneath it.
    property int hovered: -1

    clip: true
    focus: true
    currentIndex: -1
    boundsBehavior: Flickable.StopAtBounds
    model: files

    /// The columns on the right, in order. The name takes whatever is left, so turning a
    /// column off widens the names rather than leaving a gap.
    readonly property var columns: {
        let showing = []

        if (Display.showSize) showing.push({ role: "size", width: 100, align: Text.AlignRight })
        if (Display.showKind) showing.push({ role: "kind", width: 110, align: Text.AlignLeft })
        if (Display.showModified) showing.push({ role: "modified", width: 120, align: Text.AlignRight })
        if (Display.showPermissions) showing.push({ role: "permissions", width: 100, align: Text.AlignRight })

        return showing
    }

    readonly property int columnsWidth: {
        let total = 0
        for (const column of columns) {
            total += column.width + 10
        }
        return total
    }

    ScrollBar.vertical: ScrollBar {}

    delegate: Rectangle {
        id: entry

        required property int index
        required property string name
        required property string size
        required property string kind
        required property string modified
        required property string permissions
        required property bool isFolder
        required property string icon
        required property bool uploading
        required property real fraction

        width: rows.width
        height: Display.rowHeight
        color: {
            if (rows.selection.isSelected(entry.index)) {
                return Theme.selection
            } else if (rows.hovered === entry.index) {
                return Theme.surface
            } else {
                return "transparent"
            }
        }

        readonly property color ink: rows.selection.isSelected(entry.index)
            ? Theme.selectionText
            : Theme.foreground

        readonly property color quiet: rows.selection.isSelected(entry.index)
            ? Theme.selectionText
            : Theme.muted

        /// A folder is somewhere to put things, so it accepts what is dragged onto it, lights
        /// up while it is under the pointer, and opens if you hold there.
        SpringLoaded {
            id: target
            app: rows.app
            anchors.fill: parent
            folder: rows.folderOf(entry.index)
            enabled: entry.isFolder && !rows.selection.isSelected(entry.index)
        }

        Rectangle {
            anchors.fill: parent
            visible: target.containsDrag
            color: "transparent"
            border.width: 2
            border.color: Theme.accent
            radius: 4
        }

        Row {
            anchors.fill: parent
            anchors.leftMargin: 14
            anchors.rightMargin: 14
            spacing: 10

            Item {
                width: Display.listIcon
                height: parent.height

                Image {
                    anchors.centerIn: parent
                    source: entry.icon
                    visible: entry.icon !== ""
                    sourceSize.width: Display.listIcon
                    sourceSize.height: Display.listIcon
                    width: Display.listIcon
                    height: Display.listIcon
                    fillMode: Image.PreserveAspectFit
                    // Something still on its way is not there yet, and looks it.
                    opacity: entry.uploading ? 0.5 : 1
                }

                // A desktop with no icon theme still gets a legible list.
                Text {
                    anchors.centerIn: parent
                    visible: entry.icon === ""
                    text: entry.isFolder ? "▸" : "·"
                    color: entry.isFolder ? Theme.accent : Theme.muted
                }
            }

            Column {
                width: parent.width - Display.listIcon - rows.columnsWidth - 20
                anchors.verticalCenter: parent.verticalCenter
                spacing: 3

                Text {
                    width: parent.width
                    text: entry.name
                    color: entry.ink
                    opacity: entry.uploading ? 0.6 : 1
                    elide: Text.ElideMiddle
                }

                Rectangle {
                    width: parent.width
                    height: 2
                    radius: 1
                    color: Theme.border
                    visible: entry.uploading

                    Rectangle {
                        width: parent.width * Math.max(0, Math.min(1, entry.fraction))
                        height: parent.height
                        radius: 1
                        color: Theme.accent

                        Behavior on width {
                            NumberAnimation { duration: 120 }
                        }
                    }
                }
            }

            Repeater {
                model: rows.columns

                Text {
                    required property var modelData

                    width: modelData.width
                    height: parent.height
                    horizontalAlignment: modelData.align
                    verticalAlignment: Text.AlignVCenter
                    color: entry.quiet
                    elide: Text.ElideRight
                    text: {
                        switch (modelData.role) {
                        case "size": return entry.uploading ? "Uploading" : entry.size
                        case "kind": return entry.kind
                        case "modified": return entry.modified
                        default: return entry.permissions
                        }
                    }
                }
            }
        }
    }
}
