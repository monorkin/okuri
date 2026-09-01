import QtQuick
import QtQuick.Controls
import io.camion

Dialog {
    id: queue

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

    required property var transfers

    title: "Transfers"
    modal: true
    anchors.centerIn: Overlay.overlay
    width: Math.min(560, Overlay.overlay ? Overlay.overlay.width - 60 : 560)
    height: 420
    padding: 0

    background: Rectangle {
        radius: 10
        color: Theme.elevated
        border.width: 1
        border.color: Theme.border
    }

    header: Rectangle {
        implicitHeight: 46
        color: "transparent"

        Text {
            anchors.left: parent.left
            anchors.leftMargin: 18
            anchors.verticalCenter: parent.verticalCenter
            text: queue.transfers.active > 0
                ? queue.transfers.active + " in progress"
                : "Nothing in progress"
            color: Theme.foreground
        }

        FlatButton {
            anchors.right: parent.right
            anchors.rightMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            text: "Clear finished"
            enabled: queue.transfers.count > queue.transfers.active
            onClicked: queue.transfers.clearFinished()
        }

        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: Theme.border
        }
    }

    ListView {
        id: rows
        anchors.fill: parent
        clip: true
        model: queue.transfers
        boundsBehavior: Flickable.StopAtBounds

        ScrollBar.vertical: ScrollBar {}

        delegate: Item {
            id: item

            required property int index
            required property string name
            required property string status
            required property real fraction
            required property string direction
            required property bool running

            width: rows.width
            height: 56

            Column {
                anchors.left: parent.left
                anchors.right: cancel.left
                anchors.leftMargin: 18
                anchors.rightMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                spacing: 5

                Text {
                    width: parent.width
                    text: (item.direction === "download" ? "↓ " : "↑ ") + item.name
                    color: Theme.foreground
                    elide: Text.ElideMiddle
                }

                Rectangle {
                    width: parent.width
                    height: 3
                    radius: 2
                    color: Theme.border
                    visible: item.running

                    Rectangle {
                        width: parent.width * Math.max(0, Math.min(1, item.fraction))
                        height: parent.height
                        radius: 2
                        color: Theme.accent
                    }
                }

                Text {
                    text: item.status
                    color: Theme.muted
                    font.pixelSize: 12
                    elide: Text.ElideRight
                    width: parent.width
                }
            }

            FlatButton {
                id: cancel
                anchors.right: parent.right
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                text: "Cancel"
                visible: item.running
                onClicked: app.cancelTransfer(queue.transfers.idAt(item.index))
            }
        }

        Text {
            anchors.centerIn: parent
            visible: rows.count === 0
            text: "Drag files into the window to upload them."
            color: Theme.muted
        }
    }
}
