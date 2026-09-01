import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import sh.okuri

/// The file list: the whole point of the application.
///
/// A plain list — or a grid — that you can drop files onto and drive from the keyboard. The
/// shortcuts are the ones every file manager already uses, because the promise is that you do
/// not have to learn this.
FocusScope {
    id: browser

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

    required property var files

    signal renameRequested(string name)
    signal newFolderRequested()
    signal editColumnsRequested()

    readonly property var view: Display.isGrid ? gridView : listView

    Selection {
        id: selection
        count: browser.files.count
    }

    /// A drag is a system drag from the moment it begins.
    ///
    /// It cannot be anything else. The instant the pointer leaves the window the compositor
    /// owns it and no further mouse events arrive here — so there is no later moment at which
    /// to hand the files over, and a drag that might leave has to be able to from the outset.
    ///
    /// The same drag is what the folders and breadcrumbs in this window accept, by the marker
    /// it carries. One gesture, one drag, whichever side of the edge it ends on.
    function beginMove() {
        if (selection.rows.length === 0) {
            return
        }

        app.beginMove(selectedNames())

        // Setting this is what starts the drag. `startDrag()` is for a drag that is already
        // going, which this is not.
        //
        // It does not block: the compositor takes the gesture and this returns straight away,
        // long before anything has been dropped. What is being carried is therefore cleared
        // when the drag ends rather than here — clearing it here throws it away while the drag
        // is still in flight, and every drop then lands carrying nothing.
        dragSource.Drag.active = true
    }

    /// What carries the drag, and what it offers.
    ///
    /// The marker is what this window's own folders look for. The addresses are for everything
    /// else — a destination the desktop cannot open contributes none, so dragging out of one
    /// lands nowhere while moving inside it still works.
    Item {
        id: dragSource

        width: 1
        height: 1

        Drag.dragType: Drag.Automatic
        // Copy, and only copy. A file manager offered a move will take the original away from
        // the server, which is not what dragging a file into a folder is asking for.
        Drag.supportedActions: Qt.CopyAction
        Drag.proposedAction: Qt.CopyAction
        // What the pointer carries while dragging. Without one the desktop is given this
        // item, which is a single pixel and looks like nothing at all.
        Drag.imageSource: browser.files.iconAt(selection.current)
        // Qt clears this when the drag finishes, which is after any drop has been delivered.
        Drag.onActiveChanged: if (!dragSource.Drag.active) app.endMove()

        Drag.mimeData: app.dragUrls.length > 0
            ? ({
                "application/x-okuri-move": app.dragPayload,
                "text/uri-list": app.dragUrls.join("\r\n")
            })
            : ({ "application/x-okuri-move": app.dragPayload })
    }


    function selectedNames() {
        let names = []
        for (const row of selection.rows) {
            names.push(browser.files.nameAt(row))
        }
        return names
    }

    /// What double-clicking, or pressing Enter, means.
    ///
    /// A folder opens. A file cannot — there is nothing on this machine to open, and fetching
    /// it silently would be a download nobody asked for — so it shows what is known about it
    /// and offers to bring it down.
    function openRow(row) {
        if (row < 0) {
            return
        }

        if (browser.files.isFolderAt(row)) {
            app.open(browser.files.nameAt(row))
        } else {
            selection.selectOnly(row)
            fileDetails.show(browser.files.details(row))
        }
    }

    function confirmDelete() {
        const names = selectedNames()

        if (names.length === 0) {
            return
        }

        const what = names.length === 1
            ? "Delete " + names[0] + "?"
            : "Delete " + names.length + " items?"

        confirmation.ask(what, "There is no trash on a remote server — this cannot be undone.")
    }

    /// Right-clicking a row that is not part of the current selection selects it first, the way
    /// every file manager does — otherwise the menu would act on something you cannot see.
    function openMenuAt(row) {
        if (row >= 0 && !selection.isSelected(row)) {
            selection.selectOnly(row)
        }

        menu.rows = selection.rows.length
        menu.onFolder = row >= 0 && browser.files.isFolderAt(row)
        menu.popup()
    }

    function moveBy(step) {
        const row = selection.step(step)

        if (row >= 0) {
            browser.view.positionViewAtIndex(row, ListView.Contain)
        }
    }

    /// How many cells fit across the grid, so Up and Down move a line rather than an item.
    function perLine() {
        if (!Display.isGrid) {
            return 1
        }

        return Math.max(1, Math.floor(gridView.width / gridView.cellWidth))
    }

    Connections {
        target: browser.files
        function onPathChanged() { selection.clear() }
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.background

        Header {
            id: header
            width: parent.width
            visible: !Display.isGrid
            columns: listView.columns
            nameWidth: parent.width - Display.listIcon - listView.columnsWidth - 48
            onSort: column => Display.sortBy(column)
        }

        Item {
            id: content

            anchors.top: header.visible ? header.bottom : parent.top
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom

            /// The folder you are looking at is somewhere to put things too.
            ///
            /// Sits beneath the rows, so a folder under the pointer still wins — this is what
            /// catches a drop onto the empty space below them, which otherwise landed nowhere
            /// after opening a folder to put something in.
            SpringLoaded {
                id: hereTarget
                app: browser.app
                anchors.fill: parent
                folder: app.path
                opens: false
            }

            Rectangle {
                anchors.fill: parent
                anchors.margins: 6
                radius: 8
                visible: hereTarget.containsDrag
                color: "transparent"
                border.width: 2
                border.color: Theme.accent
            }

            Rows {
                id: listView
                app: browser.app
                anchors.fill: parent
                visible: !Display.isGrid
                enabled: visible
                files: browser.files
                selection: selection
            }

            Grid {
                id: gridView
                app: browser.app
                anchors.fill: parent
                visible: Display.isGrid
                enabled: visible
                files: browser.files
                selection: selection
            }

            /// Everything the pointer does, in one place above both views.
            ///
            /// Not in the rows themselves: opening a folder replaces every row, and a gesture
            /// owned by one of them would be destroyed halfway through — the drag would freeze
            /// and letting go would land nowhere. This outlives the listing, so holding over a
            /// breadcrumb can open a folder and the same drag carries on inside it.
            MouseArea {
                id: pointer

                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.LeftButton | Qt.RightButton

                property point from: Qt.point(0, 0)
                property int pressedRow: -1
                property bool collapse: false
                property bool dragged: false

                /// Which row is under a point, in whichever view is showing.
                function rowAt(x, y) {
                    return Display.isGrid
                        ? gridView.indexAt(x + gridView.contentX, y + gridView.contentY)
                        : listView.indexAt(x + listView.contentX, y + listView.contentY)
                }

                function hover(row) {
                    listView.hovered = row
                    gridView.hovered = row
                }

                onExited: hover(-1)

                onPressed: press => {
                    browser.forceActiveFocus()

                    from = Qt.point(press.x, press.y)
                    pressedRow = rowAt(press.x, press.y)
                    dragged = false
                    collapse = false

                    if (press.button === Qt.RightButton) {
                        if (pressedRow < 0) {
                            selection.clear()
                        }

                        browser.openMenuAt(pressedRow)
                        return
                    }

                    if (pressedRow < 0) {
                        selection.clear()
                        return
                    }

                    // Picking happens on the way down, not on the way up: a drag begins before
                    // the button is released, and it has to know what it is dragging.
                    //
                    // Pressing something already picked is the exception. Collapsing a
                    // selection of ten files to one the moment you touch it would make them
                    // impossible to drag together, so that waits until the button comes back
                    // up having gone nowhere.
                    const picked = selection.isSelected(pressedRow)
                    const plain = !(press.modifiers & (Qt.ShiftModifier | Qt.ControlModifier))

                    if (picked && plain) {
                        collapse = true
                    } else {
                        selection.click(pressedRow, press.modifiers)
                    }
                }

                onPositionChanged: move => {
                    if (!pressed) {
                        hover(rowAt(move.x, move.y))
                        return
                    }

                    if (!dragged && pressedRow >= 0
                        && Math.hypot(move.x - from.x, move.y - from.y) > 12) {
                        dragged = true
                        browser.beginMove()
                    }

                    if (!dragged) {
                        return
                    }

                }

                onReleased: {
                    // A drag that happened has already finished: starting it blocked here
                    // until it was dropped or abandoned.
                    if (!dragged && collapse && pressedRow >= 0) {
                        selection.selectOnly(pressedRow)
                    }

                    collapse = false
                    dragged = false
                }

                onDoubleClicked: click => browser.openRow(rowAt(click.x, click.y))
            }

            /// Waiting, where whoever is waiting is looking.
            ///
            /// A listing can take seconds on a slow server, and an empty window with a small
            /// mark turning in the corner reads as a folder with nothing in it.
            Column {
                anchors.centerIn: parent
                spacing: 12
                visible: browser.files.working && browser.files.count === 0

                Spinner {
                    running: parent.visible
                    implicitWidth: 28
                    implicitHeight: 28
                    anchors.horizontalCenter: parent.horizontalCenter
                }

                Text {
                    text: "Loading…"
                    color: Theme.muted
                    anchors.horizontalCenter: parent.horizontalCenter
                }
            }

            Text {
                anchors.centerIn: parent
                visible: browser.files.count === 0 && !browser.files.working
                text: "This folder is empty.\nDrag files here to upload them."
                horizontalAlignment: Text.AlignHCenter
                color: Theme.muted
            }

        }

        Keys.onPressed: key => {
            switch (key.key) {
            case Qt.Key_Return:
            case Qt.Key_Enter:
                browser.openRow(selection.current)
                break
            case Qt.Key_Backspace:
                app.up()
                break
            case Qt.Key_Up:
                browser.moveBy(-browser.perLine())
                break
            case Qt.Key_Down:
                browser.moveBy(browser.perLine())
                break
            case Qt.Key_Left:
                if (Display.isGrid) browser.moveBy(-1)
                break
            case Qt.Key_Right:
                if (Display.isGrid) browser.moveBy(1)
                break
            case Qt.Key_Delete:
                browser.confirmDelete()
                break
            case Qt.Key_F2:
                if (selection.current >= 0 && app.canRename) {
                    browser.renameRequested(browser.files.nameAt(selection.current))
                }
                break
            default:
                // Anything else is treated as type-ahead, which is how you find a file in a
                // long listing without reaching for the mouse.
                if (key.text.length === 1 && key.text >= " ") {
                    typed.text += key.text
                    typed.restart()

                    const found = browser.files.find(typed.text, -1)
                    if (found >= 0) {
                        selection.selectOnly(found)
                        browser.view.positionViewAtIndex(found, ListView.Contain)
                    }
                }
            }
        }

        Timer {
            id: typed
            property string text: ""
            interval: 800
            onTriggered: typed.text = ""
        }

        Shortcut {
            sequences: [StandardKey.SelectAll]
            onActivated: selection.selectAll()
        }

        Shortcut {
            sequences: [StandardKey.Refresh]
            onActivated: app.refresh()
        }

        Shortcut {
            sequence: "Alt+Up"
            onActivated: app.up()
        }

        Shortcut {
            sequence: "Ctrl+N"
            onActivated: {
                if (app.canCreateFolder) {
                    browser.newFolderRequested()
                }
            }
        }

        Shortcut {
            sequence: "Ctrl+X"
            onActivated: app.beginMove(browser.selectedNames())
        }

        Shortcut {
            sequence: "Ctrl+V"
            onActivated: app.moveInto(app.dragPayload, app.path)
        }

        Shortcut {
            sequence: "Ctrl+H"
            onActivated: Display.toggleHidden()
        }

        Shortcut {
            sequence: "Ctrl+="
            onActivated: Display.resize(1)
        }

        Shortcut {
            sequence: "Ctrl+-"
            onActivated: Display.resize(-1)
        }

        FileMenu {
            id: menu
            app: browser.app

            onOpenRequested: browser.openRow(selection.current)
            onDownloadRequested: destination.open()
            onRenameRequested: browser.renameRequested(browser.files.nameAt(selection.current))
            onDeleteRequested: browser.confirmDelete()
            onNewFolderRequested: browser.newFolderRequested()
        }

        FileDetails {
            id: fileDetails
            app: browser.app
            onDownloadRequested: destination.open()
        }

        FolderDialog {
            id: destination
            title: "Download to"
            onAccepted: app.download(browser.selectedNames(), selectedFolder)
        }

        Confirm {
            id: confirmation
            onConfirmed: app.remove(browser.selectedNames())
        }

        DropArea {
            anchors.fill: parent
            keys: ["text/uri-list"]

            /// Okuri's own drags carry addresses as well, so that a file can be dropped onto
            /// the desktop. Those addresses are not for us — a drag that started in Okuri is a
            /// move, wherever it started — so this declines them and the folder underneath
            /// catches the drop instead of racing this for it.
            onEntered: drag => {
                if (drag.formats.indexOf("application/x-okuri-move") !== -1) {
                    drag.accepted = false
                }
            }

            onDropped: drop => app.dropUrls(drop.urls)

            Rectangle {
                anchors.fill: parent
                anchors.margins: 6
                radius: 8
                visible: parent.containsDrag
                color: "transparent"
                border.width: 2
                border.color: Theme.accent

                Rectangle {
                    anchors.fill: parent
                    radius: 8
                    color: Theme.accent
                    opacity: 0.08
                }
            }
        }
    }
}
