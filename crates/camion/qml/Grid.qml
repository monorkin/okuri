import QtQuick
import QtQuick.Controls
import io.camion

/// The file list as a grid of icons.
///
/// The same rows, the same selection, and the same double-click as the list — only laid out for
/// looking rather than for reading. Everything it needs is handed to it, so the two views share
/// one selection and one model rather than each keeping their own.
GridView {
    id: grid

    required property var files
    required property var selection

    /// The full path of a cell, which is what a drop onto it means.
    function folderOf(row) {
        const name = files.nameAt(row)

        return App.path === "/" ? "/" + name : App.path + "/" + name
    }

    /// The cell the pointer is over, which the browser sets: the pointer is handled in one
    /// place so that a gesture survives the listing being replaced underneath it.
    property int hovered: -1

    clip: true
    focus: true
    currentIndex: -1
    boundsBehavior: Flickable.StopAtBounds

    cellWidth: Display.gridIcon + 44
    cellHeight: Display.gridIcon + 54
    model: files

    ScrollBar.vertical: ScrollBar {}

    delegate: Item {
        id: cell

        required property int index
        required property string name
        required property bool isFolder
        required property string icon
        required property bool uploading
        required property real fraction

        width: grid.cellWidth
        height: grid.cellHeight

        Rectangle {
            anchors.fill: parent
            anchors.margins: 4
            radius: 8
            color: {
                if (grid.selection.isSelected(cell.index)) {
                    return Theme.selection
                } else if (grid.hovered === cell.index) {
                    return Theme.surface
                } else {
                    return "transparent"
                }
            }

            /// A folder is somewhere to put things, so it accepts what is dragged onto it, and
            /// opens if you hold there.
            SpringLoaded {
                id: target
                anchors.fill: parent
                folder: grid.folderOf(cell.index)
                enabled: cell.isFolder && !grid.selection.isSelected(cell.index)
            }

            Rectangle {
                anchors.fill: parent
                visible: target.containsDrag
                color: "transparent"
                border.width: 2
                border.color: Theme.accent
                radius: 8
            }

            Column {
                anchors.centerIn: parent
                spacing: 6
                width: parent.width - 10

                Item {
                    width: Display.gridIcon
                    height: Display.gridIcon
                    anchors.horizontalCenter: parent.horizontalCenter

                    Image {
                        anchors.fill: parent
                        source: cell.icon
                        visible: cell.icon !== ""
                        sourceSize.width: Display.gridIcon
                        sourceSize.height: Display.gridIcon
                        fillMode: Image.PreserveAspectFit
                        opacity: cell.uploading ? 0.5 : 1
                    }

                    Text {
                        anchors.centerIn: parent
                        visible: cell.icon === ""
                        text: cell.isFolder ? "▸" : "·"
                        font.pixelSize: Display.gridIcon / 2
                        color: cell.isFolder ? Theme.accent : Theme.muted
                    }
                }

                Text {
                    width: parent.width
                    text: cell.name
                    color: grid.selection.isSelected(cell.index)
                        ? Theme.selectionText
                        : Theme.foreground
                    opacity: cell.uploading ? 0.6 : 1
                    horizontalAlignment: Text.AlignHCenter
                    elide: Text.ElideMiddle
                    maximumLineCount: 2
                    wrapMode: Text.Wrap
                }

                Rectangle {
                    width: parent.width - 16
                    height: 2
                    radius: 1
                    color: Theme.border
                    visible: cell.uploading
                    anchors.horizontalCenter: parent.horizontalCenter

                    Rectangle {
                        width: parent.width * Math.max(0, Math.min(1, cell.fraction))
                        height: parent.height
                        radius: 1
                        color: Theme.accent

                        Behavior on width {
                            NumberAnimation { duration: 120 }
                        }
                    }
                }
            }
        }
    }
}
