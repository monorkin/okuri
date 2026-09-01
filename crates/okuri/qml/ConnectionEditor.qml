import QtQuick
import QtQuick.Controls
import sh.okuri

/// Adds or changes a saved connection.
///
/// The fields shown follow the kind: an object store has no port and a bucket, SFTP has a port
/// and no bucket. Everything is handed to the model as one object, so adding a destination
/// later means adding fields here and nothing else.
Dialog {
    id: editor

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

    property string editingId: ""

    modal: true
    anchors.centerIn: Overlay.overlay
    width: Math.min(460, Overlay.overlay ? Overlay.overlay.width - 60 : 460)

    readonly property var kindLabels: ({
        "sftp": "SFTP",
        "ftp": "FTP",
        "s3": "Amazon S3",
        "r2": "Cloudflare R2",
        "b2": "Backblaze B2",
        "webdav": "WebDAV",
        "azure": "Azure Blob Storage"
    })

    readonly property var kindKeys: ["sftp", "ftp", "s3", "r2", "b2", "webdav", "azure"]
    readonly property string kind: kindKeys[kinds.currentIndex]
    readonly property bool isStorage: ["s3", "r2", "b2"].indexOf(kind) !== -1
    readonly property bool isAzure: kind === "azure"
    readonly property bool isWebDav: kind === "webdav"
    readonly property bool isHostBased: kind === "sftp" || kind === "ftp"

    background: Rectangle {
        radius: 10
        color: Theme.elevated
        border.width: 1
        border.color: Theme.border
    }

    function compose() {
        editingId = ""
        credentialsCanChange = false
        name.text = ""
        host.text = ""
        port.text = ""
        username.text = ""
        bucket.text = ""
        region.text = ""
        endpoint.text = ""
        url.text = ""
        key.text = ""
        kinds.currentIndex = 0
        credential.currentIndex = 0
        visible = true
        name.forceActiveFocus()
    }

    /// Whether this connection signs in with anything worth replacing. Read when the editor
    /// opens rather than bound, because it is about what is saved, not what is being typed.
    property bool credentialsCanChange: false

    /// Opens the editor on a connection that already exists, showing what it holds. Opening
    /// blank and saving would quietly replace the whole thing with an empty one.
    function amend(id) {
        compose()

        const saved = ConnectionList.details(id)

        if (!saved.id) {
            return
        }

        editingId = saved.id
        credentialsCanChange = ConnectionList.needsCredentials(saved.id)
        name.text = saved.name || ""
        host.text = saved.host || ""
        port.text = saved.port || ""
        username.text = saved.username || ""
        bucket.text = saved.bucket || ""
        region.text = saved.region || ""
        endpoint.text = saved.endpoint || ""
        url.text = saved.url || ""
        key.text = saved.key || ""

        kinds.currentIndex = Math.max(0, kindKeys.indexOf(saved.kind))
        credential.currentIndex = Math.max(0, ["agent", "password", "key"].indexOf(saved.credential))

        visible = true
    }

    contentItem: Column {
        spacing: 12
        padding: 20

        Text {
            text: editor.editingId === "" ? "New connection" : "Edit connection"
            font.pixelSize: 17
            color: Theme.bright
        }

        Field {
            id: name
            width: parent.width - 40
            label: "Name"
            placeholder: "Production web"
        }

        Row {
            spacing: 8
            width: parent.width - 40

            Text {
                text: "Kind"
                width: 80
                color: Theme.muted
                anchors.verticalCenter: parent.verticalCenter
            }

            ComboBox {
                id: kinds
                width: parent.width - 88
                model: editor.kindKeys.map(key => editor.kindLabels[key])
            }
        }

        Field {
            id: host
            width: parent.width - 40
            label: "Host"
            placeholder: "example.com"
            visible: editor.isHostBased
        }

        Field {
            id: port
            width: parent.width - 40
            label: "Port"
            placeholder: editor.kind === "ftp" ? "21" : "22"
            visible: editor.isHostBased
        }

        Row {
            spacing: 8
            width: parent.width - 40
            visible: editor.kind === "sftp"

            Text {
                text: "Sign in with"
                width: 80
                color: Theme.muted
                anchors.verticalCenter: parent.verticalCenter
            }

            ComboBox {
                id: credential
                width: parent.width - 88
                model: ["SSH agent", "Password", "Key file"]
            }
        }

        Field {
            id: key
            width: parent.width - 40
            label: "Key file"
            placeholder: "~/.ssh/id_ed25519"
            visible: editor.kind === "sftp" && credential.currentIndex === 2
        }

        Field {
            id: url
            width: parent.width - 40
            label: "URL"
            placeholder: "https://dav.example.com/remote.php/dav"
            visible: editor.isWebDav
        }

        Field {
            id: username
            width: parent.width - 40
            label: editor.isAzure ? "Account" : "Username"
            visible: !editor.isStorage
        }

        Field {
            id: bucket
            width: parent.width - 40
            label: editor.isAzure ? "Container" : "Bucket"
            visible: editor.isStorage || editor.isAzure
        }

        Field {
            id: region
            width: parent.width - 40
            label: editor.kind === "r2" ? "Account id" : "Region"
            placeholder: {
                switch (editor.kind) {
                case "r2": return "your account id"
                case "b2": return "eu-central-003"
                default: return "eu-central-1"
                }
            }
            visible: editor.isStorage
        }

        Field {
            id: endpoint
            width: parent.width - 40
            label: "Endpoint"
            placeholder: "only if it is not the usual one"
            visible: editor.isStorage || editor.isAzure
        }

        Text {
            width: parent.width - 40
            // Says where the secret goes, rather than referring to a file the reader cannot
            // see and has not been told about.
            text: {
                switch (editor.kind) {
                case "azure": return "Your account key is asked for when you connect, and stored in your keyring."
                case "s3": case "r2": case "b2": return "Your access key and secret are asked for when you connect, and stored in your keyring."
                case "sftp": return credential.currentIndex === 0
                    ? "Okuri signs in the way ssh does, using your agent and ~/.ssh/config."
                    : "Your password is asked for when you connect, and stored in your keyring."
                default: return "Your password is asked for when you connect, and stored in your keyring."
                }
            }
            color: Theme.muted
            font.pixelSize: 12
            wrapMode: Text.WordWrap
        }

        Row {
            spacing: 8
            anchors.right: parent.right
            anchors.rightMargin: 20

            /// Connecting only asks for a credential when none is saved, so without this a
            /// mistyped access key could only be corrected from the desktop's keyring.
            FlatButton {
                text: "Change credentials"
                visible: editor.editingId !== "" && editor.credentialsCanChange
                onClicked: {
                    app.changeCredentials(editor.editingId)
                    editor.visible = false
                }
            }

            FlatButton {
                text: "Delete"
                visible: editor.editingId !== ""
                onClicked: {
                    ConnectionList.forget(editor.editingId)
                    editor.visible = false
                }
            }

            FlatButton {
                text: "Cancel"
                onClicked: editor.visible = false
            }

            FlatButton {
                text: "Save"
                highlighted: true
                enabled: name.text.trim() !== ""
                onClicked: {
                    ConnectionList.save({
                        "id": editor.editingId,
                        "name": name.text.trim(),
                        "kind": editor.kind,
                        "host": host.text.trim(),
                        "port": port.text.trim(),
                        "username": username.text.trim(),
                        "bucket": bucket.text.trim(),
                        "region": region.text.trim(),
                        "endpoint": endpoint.text.trim(),
                        "url": url.text.trim(),
                        "credential": ["agent", "password", "key"][credential.currentIndex],
                        "key": key.text.trim()
                    })

                    editor.visible = false
                }
            }
        }
    }
}
