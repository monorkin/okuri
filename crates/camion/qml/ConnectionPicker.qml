import QtQuick
import QtQuick.Controls
import io.camion

/// What you see before you are connected: everything you have saved, and a way to add more.
Rectangle {
    id: picker

    signal chosen(string id)
    signal compose()
    signal edit(string id)

    color: Theme.background

    Column {
        anchors.centerIn: parent
        width: Math.min(parent.width - 80, 460)
        spacing: 18

        Text {
            text: "Camion"
            font.pixelSize: 28
            color: Theme.bright
            anchors.horizontalCenter: parent.horizontalCenter
        }

        Text {
            text: "Open a connection, or add one."
            color: Theme.muted
            anchors.horizontalCenter: parent.horizontalCenter
        }

        Rectangle {
            width: parent.width
            height: Math.min(300, Math.max(60, ConnectionList.count * 58))
            radius: 8
            color: Theme.surface
            border.width: 1
            border.color: Theme.border
            clip: true

            ListView {
                id: saved
                anchors.fill: parent
                anchors.margins: 1
                model: ConnectionList
                boundsBehavior: Flickable.StopAtBounds

                ScrollBar.vertical: ScrollBar {}

                delegate: Rectangle {
                    id: row

                    required property int index
                    required property string identifier
                    required property string name
                    required property string summary
                    required property string kind

                    width: saved.width
                    height: 58
                    color: hover.containsMouse ? Theme.elevated : "transparent"

                    MouseArea {
                        id: hover
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: picker.chosen(row.identifier)
                        onDoubleClicked: picker.chosen(row.identifier)
                    }

                    Column {
                        anchors.left: parent.left
                        anchors.leftMargin: 16
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2

                        Text {
                            text: row.name
                            color: Theme.foreground
                        }

                        Text {
                            text: row.kind + " · " + row.summary
                            color: Theme.muted
                            font.pixelSize: 12
                        }
                    }

                    FlatButton {
                        anchors.right: parent.right
                        anchors.rightMargin: 10
                        anchors.verticalCenter: parent.verticalCenter
                        text: "Edit"
                        visible: hover.containsMouse
                        onClicked: picker.edit(row.identifier)
                    }
                }
            }

            Text {
                anchors.centerIn: parent
                visible: ConnectionList.count === 0
                text: "Nothing saved yet."
                color: Theme.muted
            }
        }

        FlatButton {
            text: "New connection"
            highlighted: true
            anchors.horizontalCenter: parent.horizontalCenter
            onClicked: picker.compose()
        }
    }
}
