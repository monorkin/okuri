import QtQuick
import io.camion

/// A folder that opens if you hold something over it.
///
/// Dragging is a single gesture, so anywhere you might want to put a file has to be reachable
/// without letting go. Hovering opens the folder and the drag carries on inside it, which is
/// how something gets from one branch of a tree to another in one go.
DropArea {
    id: spring

    /// The folder to open, and to put things into.
    required property string folder

    /// Whether hovering here should open it. A folder you are already looking at has nothing
    /// to open.
    property bool opens: true

    // What the drag actually carries. A system drag is matched by its mime types, and the
    // drag that leaves this window is the same one that lands inside it.
    keys: ["application/x-camion-move"]

    /// Long enough not to fire while passing over on the way somewhere else, short enough that
    /// waiting on purpose does not feel like waiting.
    readonly property int delay: 1200

    onEntered: if (opens) waiting.restart()
    onExited: waiting.stop()

    onDropped: {
        waiting.stop()
        App.moveInto(spring.folder)
    }

    Timer {
        id: waiting
        interval: spring.delay
        onTriggered: {
            if (spring.containsDrag) {
                App.openPath(spring.folder)
            }
        }
    }
}
