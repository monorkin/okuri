import QtQuick
import QtQuick.Controls
import io.camion

/// What you see before you are connected: everything you have saved, and a way to add more.
Rectangle {
    id: picker

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

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
                currentIndex: -1
                boundsBehavior: Flickable.StopAtBounds
                focus: true

                // Enter opens whatever is picked, so the keyboard can do what the mouse does.
                Keys.onReturnPressed: picker.chosen(ConnectionList.idAt(saved.currentIndex))
                Keys.onEnterPressed: picker.chosen(ConnectionList.idAt(saved.currentIndex))

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
                    color: {
                        if (row.opening || saved.currentIndex === row.index) {
                            return Theme.elevated
                        }

                        return within.hovered ? Qt.alpha(Theme.foreground, 0.06) : "transparent"
                    }

                    readonly property bool opening: app.connectingTo === row.identifier

                    /// Whether the pointer is anywhere over this row, the Edit button included.
                    ///
                    /// A `HoverHandler` rather than the `MouseArea` below: a button on top of a
                    /// mouse area takes the hover away from it, so reaching for Edit made Edit
                    /// vanish and the click landed on the row instead.
                    HoverHandler {
                        id: within
                    }

                    /// Opened by double-clicking, the way a file manager opens anything. A
                    /// single click selects, and selecting must not be enough to start dialling
                    /// a server — reaching Edit meant connecting to whatever you passed over.
                    MouseArea {
                        anchors.fill: parent
                        onClicked: saved.currentIndex = row.index
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

                    /// Where the waiting is shown: in the row that was clicked, because that is
                    /// where whoever clicked it is looking.
                    Spinner {
                        anchors.right: parent.right
                        anchors.rightMargin: 16
                        anchors.verticalCenter: parent.verticalCenter
                        running: row.opening
                    }

                    FlatButton {
                        anchors.right: parent.right
                        anchors.rightMargin: 10
                        anchors.verticalCenter: parent.verticalCenter
                        text: "Edit"
                        visible: within.hovered && !row.opening
                        onClicked: picker.edit(row.identifier)
                    }
                }
            }

            Text {
                anchors.centerIn: parent
                visible: ConnectionList.count === 0
                text: "No connections yet"
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
