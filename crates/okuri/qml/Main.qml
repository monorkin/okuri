import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import sh.okuri

/// One window onto one server.
///
/// Everything a window has is its own: the connection, the folder, the selection, the questions
/// being asked about it. What it shares with the others is the engine underneath, the transfer
/// queue, and the list of saved connections — the things that would be wrong to have two of.
ApplicationWindow {
    id: window

    /// Somebody asked for another window. Answered by whatever made this one, because a window
    /// is not the right thing to own the list of windows.
    signal another()

    width: 960
    height: 640
    minimumWidth: 520
    minimumHeight: 360
    visible: true
    color: Theme.background
    title: app.connected ? app.label + " — Okuri" : "Okuri"

    /// This window's half of the application. Not a singleton: two windows are two connections,
    /// and everything below here is handed this one rather than looking one up.
    ///
    /// Named through the window rather than by its own id, so that `app: window.app` on a child
    /// reads the window's and not the child's own property of the same name.
    readonly property App app: theApp

    App { id: theApp }

    FileList {
        id: listing

        /// Bound to the window's own connection rather than to whichever opened last, so a
        /// listing arriving for the window next door is not drawn over this one.
        property var connection: app.session
        onConnectionChanged: listing.follow(connection)
    }

    Transfers { id: queue }

    // Closing a window closes what it had open. Leaving the session behind would leave a live
    // SSH connection belonging to a window nobody can see any more.
    onClosing: {
        app.disconnect()
        window.destroy()
    }

    header: Toolbar {
        app: window.app
        files: listing
        transfers: queue
        onShowTransfers: transferQueue.open()
        onNewConnection: editor.compose()
        onNewWindow: window.another()
        onEditColumns: columnsDialog.open()
    }

    Shortcut {
        sequence: "Ctrl+Shift+N"
        onActivated: window.another()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Loader {
            Layout.fillWidth: true
            Layout.fillHeight: true
            sourceComponent: app.connected ? browser : start
        }

        Notice {
            Layout.fillWidth: true
            text: app.message
            grave: app.messageIsGrave
            onDismissed: app.dismissMessage()
        }
    }

    Component {
        id: start

        ConnectionPicker {
            app: window.app
            onChosen: id => app.connectTo(id)
            onCompose: editor.compose()
            onEdit: id => editor.amend(id)
        }
    }

    Component {
        id: browser

        Browser {
            app: window.app
            files: listing
            onRenameRequested: name => rename.begin(name)
            onNewFolderRequested: folderPrompt.open("")
        }
    }

    TransferQueue {
        id: transferQueue
        app: window.app
        transfers: queue
    }

    ConnectionEditor {
        id: editor
        app: window.app
    }

    ColumnsDialog { id: columnsDialog }

    NameDialog {
        id: folderPrompt
        heading: "New folder"
        placeholder: "Folder name"
        accept: "Create"
        onNamed: name => app.createFolder(name)
    }

    NameDialog {
        id: rename
        heading: "Rename"
        accept: "Rename"
        warning: app.renameIsACopy
            ? "On this kind of storage a rename is a copy and a delete, which takes as long as the file is big."
            : ""

        property string original: ""

        function begin(name) {
            original = name
            open(name)
        }

        onNamed: name => {
            if (name !== original) {
                app.rename(original, name)
            }
        }
    }

    Ask {
        id: ask
        app: window.app
    }
}
