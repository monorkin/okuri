import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import sh.okuri

Rectangle {
    id: toolbar

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

    required property var files
    required property var transfers

    signal showTransfers()
    signal newConnection()
    signal newWindow()
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
            enabled: app.connected && !app.atRoot
            onClicked: app.up()
        }

        FlatButton {
            text: "↻"
            hint: "Refresh"
            enabled: app.connected
            onClicked: app.refresh()
        }

        Breadcrumb {
            app: toolbar.app
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignVCenter
            visible: app.connected
        }

        Item {
            Layout.fillWidth: true
            visible: !app.connected
        }

        // Only for work that has somewhere else to be shown: connecting says so in the row
        // that was clicked, and a first listing says so in the middle of the window.
        Spinner {
            running: toolbar.files.working && toolbar.files.count > 0
        }

        FlatButton {
            text: "New connection"
            visible: !app.connected
            onClicked: toolbar.newConnection()
        }

        /// The view mode and the options that go with it belong together, so they are drawn as
        /// one control with a seam rather than as two buttons that happen to be adjacent.
        Rectangle {
            id: display

            Layout.alignment: Qt.AlignVCenter
            visible: app.connected
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
            // The queue is one queue for the whole application, so a window with nothing open
            // can still be the one you happen to be looking at while a transfer started
            // somewhere else is running.
            enabled: app.connected || toolbar.transfers.count > 0
            highlighted: toolbar.transfers.active > 0
            onClicked: toolbar.showTransfers()
        }

        // Always here, connected or not. A second window is how you reach a second server, so
        // hiding it until you have reached the first one has it missing exactly when somebody
        // wants two things open side by side.
        FlatButton {
            hint: "New window"
            text: "❐"
            onClicked: toolbar.newWindow()
        }

        FlatButton {
            hint: "Disconnect"
            text: "✕"
            visible: app.connected
            onClicked: app.disconnect()
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
