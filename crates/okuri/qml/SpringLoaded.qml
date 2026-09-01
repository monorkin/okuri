import QtQuick
import sh.okuri

/// A folder that opens if you hold something over it.
///
/// Dragging is a single gesture, so anywhere you might want to put a file has to be reachable
/// without letting go. Hovering opens the folder and the drag carries on inside it, which is
/// how something gets from one branch of a tree to another in one go.
DropArea {
    id: spring

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

    /// The folder to open, and to put things into.
    required property string folder

    /// Whether hovering here should open it. A folder you are already looking at has nothing
    /// to open.
    property bool opens: true

    // What the drag actually carries. A system drag is matched by its mime types, and the
    // drag that leaves this window is the same one that lands inside it.
    keys: ["application/x-okuri-move"]

    /// Long enough not to fire while passing over on the way somewhere else, short enough that
    /// waiting on purpose does not feel like waiting.
    readonly property int delay: 1200

    onEntered: if (opens) waiting.restart()
    onExited: waiting.stop()

    // What the drop is carrying comes from the drop, not from this window: it may have been
    // picked up in another one, whose `App` this is not.
    onDropped: drop => {
        waiting.stop()
        app.moveInto(drop.getDataAsString("application/x-okuri-move"), spring.folder)
    }

    Timer {
        id: waiting
        interval: spring.delay
        onTriggered: {
            if (spring.containsDrag) {
                app.openPath(spring.folder)
            }
        }
    }
}
