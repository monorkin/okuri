import QtQuick
import QtQuick.Controls
import io.camion

/// The path, one clickable folder at a time.
Flickable {
    id: trail

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

    property var crumbs: []

    implicitHeight: 28
    contentWidth: row.width
    flickableDirection: Flickable.HorizontalFlick
    clip: true

    function reload() {
        crumbs = app.breadcrumb()
        contentX = Math.max(0, row.width - width)
    }

    Component.onCompleted: reload()

    Connections {
        target: trail.app
        function onPathChanged() { trail.reload() }
    }

    Row {
        id: row
        height: trail.height
        spacing: 2

        Repeater {
            model: trail.crumbs

            Row {
                spacing: 2
                anchors.verticalCenter: parent ? parent.verticalCenter : undefined

                Text {
                    text: "›"
                    color: Theme.muted
                    visible: index > 0
                    anchors.verticalCenter: parent.verticalCenter
                }

                Item {
                    width: crumb.width
                    height: crumb.height

                    FlatButton {
                        id: crumb
                        // The root has no name of its own, so it is shown as the connection.
                        text: index === 0 ? trail.app.label : modelData.split("/").pop()
                        enabled: index < trail.crumbs.length - 1
                        highlighted: here.containsDrag
                        onClicked: trail.app.openPath(modelData)
                    }

                    /// Dropping onto a crumb is how something goes back up, and holding over one
                    /// opens it — so a file can go from one branch of the tree to another
                    /// without ever being let go of.
                    SpringLoaded {
                        id: here
                        app: trail.app
                        anchors.fill: parent
                        folder: modelData
                        enabled: modelData !== trail.app.path
                    }
                }
            }
        }
    }
}
