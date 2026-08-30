import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import io.camion

ApplicationWindow {
    id: window

    width: 960
    height: 640
    minimumWidth: 520
    minimumHeight: 360
    visible: true
    color: Theme.background
    title: App.connected ? App.label + " — Camion" : "Camion"

    FileList { id: listing }
    Transfers { id: queue }

    header: Toolbar {
        files: listing
        transfers: queue
        onShowTransfers: transferQueue.open()
        onNewConnection: editor.compose()
        onEditColumns: columnsDialog.open()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Loader {
            Layout.fillWidth: true
            Layout.fillHeight: true
            sourceComponent: App.connected ? browser : start
        }

        Notice {
            Layout.fillWidth: true
            text: App.message
            onDismissed: App.dismissMessage()
        }
    }

    Component {
        id: start

        ConnectionPicker {
            onChosen: id => App.connectTo(id)
            onCompose: editor.compose()
            onEdit: id => editor.amend(id)
        }
    }

    Component {
        id: browser

        Browser {
            files: listing
            onRenameRequested: name => rename.begin(name)
            onNewFolderRequested: folderPrompt.open("")
        }
    }

    TransferQueue {
        id: transferQueue
        transfers: queue
    }

    ConnectionEditor { id: editor }

    ColumnsDialog { id: columnsDialog }

    NameDialog {
        id: folderPrompt
        heading: "New folder"
        placeholder: "Folder name"
        accept: "Create"
        onNamed: name => App.createFolder(name)
    }

    NameDialog {
        id: rename
        heading: "Rename"
        accept: "Rename"
        warning: App.renameIsACopy
            ? "On this kind of storage a rename is a copy and a delete, which takes as long as the file is big."
            : ""

        property string original: ""

        function begin(name) {
            original = name
            open(name)
        }

        onNamed: name => {
            if (name !== original) {
                App.rename(original, name)
            }
        }
    }

    Ask { id: ask }
}
