import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import io.camion

Rectangle {
    id: toolbar

    required property var files
    required property var transfers

    signal showTransfers()
    signal newConnection()
    signal editColumns()

    implicitHeight: 52
    color: Theme.surface

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 12
        anchors.rightMargin: 12
        spacing: 8

        FlatButton {
            text: "←"
            hint: "Parent folder"
            enabled: App.connected && !App.atRoot
            onClicked: App.up()
        }

        FlatButton {
            text: "↻"
            hint: "Refresh"
            enabled: App.connected
            onClicked: App.refresh()
        }

        Breadcrumb {
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignVCenter
            visible: App.connected
        }

        Item {
            Layout.fillWidth: true
            visible: !App.connected
        }

        BusyIndicator {
            running: toolbar.files.working || App.connecting
            visible: running
            implicitWidth: 20
            implicitHeight: 20
        }

        FlatButton {
            text: "New connection"
            visible: !App.connected
            onClicked: toolbar.newConnection()
        }

        /// The view mode and the options that go with it belong together, so they are drawn as
        /// one control with a seam rather than as two buttons that happen to be adjacent.
        Rectangle {
            id: display

            Layout.alignment: Qt.AlignVCenter
            visible: App.connected
            implicitWidth: mode.implicitWidth + options.implicitWidth + 1
            implicitHeight: 30
            radius: 6
            color: Qt.alpha(Theme.foreground, 0.06)

            Row {
                anchors.fill: parent
                spacing: 0

                FlatButton {
                    id: mode
                    height: parent.height
                    side: "left"
                    hint: Display.isGrid ? "List view" : "Grid view"
                    text: Display.isGrid ? "☰" : "▦"
                    onClicked: Display.toggleMode()
                }

                Rectangle {
                    width: 1
                    height: parent.height - 10
                    anchors.verticalCenter: parent.verticalCenter
                    color: Qt.alpha(Theme.foreground, 0.15)
                }

                FlatButton {
                    id: options
                    height: parent.height
                    side: "right"
                    // Narrower than the mode button beside it: switching how the list is drawn
                    // is the common thing to want, and the menu behind this is the rarer one.
                    leftPadding: 6
                    rightPadding: 6
                    hint: "Display options"
                    text: "⌄"
                    onClicked: view.popup(display.x, toolbar.height)
                }
            }
        }

        FlatButton {
            hint: "Transfers"
            text: toolbar.transfers.active > 0
                ? "↑ " + toolbar.transfers.active
                : "↑"
            // Nothing can be transferring with nothing open, so there is nothing to look at.
            enabled: App.connected
            highlighted: toolbar.transfers.active > 0
            onClicked: toolbar.showTransfers()
        }

        FlatButton {
            hint: "Disconnect"
            text: "✕"
            visible: App.connected
            onClicked: App.disconnect()
        }
    }

    ViewMenu {
        id: view
        onEditColumns: toolbar.editColumns()
    }

    Rectangle {
        anchors.bottom: parent.bottom
        width: parent.width
        height: 1
        color: Theme.border
    }
}
