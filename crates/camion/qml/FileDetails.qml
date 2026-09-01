import QtQuick
import QtQuick.Controls
import io.camion

/// One file, on its own, with a way to get it.
///
/// What opening a file means on a remote server: there is nothing to open in place, so showing
/// what is known about it and offering to bring it down is the honest answer to a double click.
Dialog {
    id: details

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

    /// What was double-clicked, as the file list describes it.
    property var file: ({})

    signal downloadRequested()

    modal: true
    anchors.centerIn: Overlay.overlay
    width: Math.min(420, Overlay.overlay ? Overlay.overlay.width - 60 : 420)

    background: Rectangle {
        radius: 10
        color: Theme.elevated
        border.width: 1
        border.color: Theme.border
    }

    /// Which button last put something on the clipboard, so it can say so.
    property string copied: ""

    /// Whether a signature was asked for in order to copy it, rather than to look at it.
    property bool copyWhenSigned: false

    /// Puts `text` on the clipboard.
    ///
    /// Through a `TextEdit` because that is the only thing in QML that can reach the clipboard.
    /// It is never shown: what it holds is whichever address was last copied, which is not the
    /// same as what the panel is describing.
    function take(text, which) {
        clipboard.text = text
        clipboard.selectAll()
        clipboard.copy()
        clipboard.deselect()

        copied = which
        said.restart()
    }

    Timer {
        id: said
        interval: 1600
        onTriggered: details.copied = ""
    }

    TextEdit {
        id: clipboard
        visible: false
    }

    Connections {
        target: details.app

        function onSignedUrlChanged() {
            if (details.copyWhenSigned && app.signedUrl !== "") {
                details.copyWhenSigned = false
                details.take(app.signedUrl, "signed")
            }
        }
    }

    function allows(who, what) {
        return file[who + "-" + what] === true
    }

    /// Sends the whole mode, because that is what the protocols take: there is no way to change
    /// one bit on its own, so every other answer has to be sent back exactly as it was.
    ///
    /// What is shown is updated here rather than waiting for the server, since the listing that
    /// comes back afterwards does not know which row this panel is describing. A refusal shows
    /// along the bottom, and reopening the file shows the truth.
    function permit(who, what, allowed) {
        const bit = { "read": 4, "write": 2, "execute": 1 }
        const party = { "owner": 6, "group": 3, "everyone": 0 }

        let mode = 0
        let changed = file

        for (const whose in party) {
            for (const which in bit) {
                const on = whose === who && which === what
                    ? allowed
                    : details.allows(whose, which)

                if (on) {
                    mode |= bit[which] << party[whose]
                }
            }
        }

        changed[who + "-" + what] = allowed
        file = changed

        app.setPermissions(file.name, mode)
    }

    function show(shown) {
        file = shown
        visible = true
        copied = ""

        // Asked rather than assumed: whether a file is readable by anybody is a property of
        // the file on the server, not something the listing carries.
        if (app.canShare) {
            app.share(shown.name)
        }

        // What the listing carries is only what every destination has in common. The rest is
        // asked for now, one file at a time, because this is the moment anybody wants it.
        app.describe(shown.name)
    }

    contentItem: Column {
        spacing: 16
        padding: 20

        Row {
            spacing: 12
            width: parent.width - 40

            Image {
                source: details.file.icon || ""
                visible: source !== ""
                sourceSize.width: 48
                sourceSize.height: 48
                anchors.verticalCenter: parent.verticalCenter
            }

            Column {
                spacing: 2
                width: parent.width - 60
                anchors.verticalCenter: parent.verticalCenter

                Text {
                    width: parent.width
                    text: details.file.name || ""
                    font.pixelSize: 17
                    color: Theme.bright
                    elide: Text.ElideMiddle
                }

                Text {
                    text: details.file.kind || ""
                    font.pixelSize: 12
                    color: Theme.muted
                }
            }
        }

        Column {
            spacing: 8
            width: parent.width - 40

            /// Anything the server did not say is left out rather than shown empty — a blank
            /// Permissions line reads like the file has none.
            Repeater {
                model: [
                    { label: "Size", value: details.file.size || "", copies: false },
                    { label: "Modified", value: details.file.modified || "", copies: false },
                    // Where the file is on the server, not where the breadcrumb says: what the
                    // window shows is relative to wherever the connection starts. Worth
                    // copying, because a path is something you paste somewhere else.
                    { label: "Where", value: app.absolutePath, copies: true }
                ]
                    // Whatever else this destination knows. While the answer is still coming,
                    // the rows it will fill are already here with nothing in them — otherwise
                    // the panel opens short and grows under the pointer as each reply lands.
                    .concat(app.describing
                        ? app.expectedFacts.map(label =>
                            ({ label: label, value: " ", copies: false, waiting: true }))
                        : app.facts.reduce((facts, said, at) => {
                            // Label and value alternating, which is the one shape a list of
                            // pairs survives the trip in.
                            if (at % 2 === 0) {
                                facts.push({ label: said, value: "", copies: true })
                            } else {
                                facts[facts.length - 1].value = said
                            }

                            return facts
                        }, []))
                    .filter(fact => fact.value !== "")

                Row {
                    required property var modelData

                    width: parent.width

                    Text {
                        text: parent.modelData.label
                        width: 110
                        font.pixelSize: 13
                        color: Theme.muted
                    }

                    /// What is not there yet, drawn as the shape of what will be. A bar rather
                    /// than a spinner: three spinners in a column is a lot of movement for
                    /// something that is only slow the first time.
                    Rectangle {
                        visible: parent.modelData.waiting === true
                        width: 140
                        height: 10
                        radius: 3
                        color: Qt.alpha(Theme.foreground, 0.10)
                        anchors.verticalCenter: parent.verticalCenter

                        SequentialAnimation on opacity {
                            running: parent.visible
                            loops: Animation.Infinite

                            NumberAnimation { to: 0.4; duration: 600; easing.type: Easing.InOutQuad }
                            NumberAnimation { to: 1.0; duration: 600; easing.type: Easing.InOutQuad }
                        }
                    }

                    Text {
                        id: value

                        visible: parent.modelData.waiting !== true
                        text: details.copied === parent.modelData.label
                            ? "Copied"
                            : parent.modelData.value
                        width: parent.width - 110
                        font.pixelSize: 13
                        color: reach.containsMouse ? Theme.accent : Theme.foreground
                        elide: Text.ElideMiddle

                        MouseArea {
                            id: reach

                            anchors.fill: parent
                            enabled: value.parent.modelData.copies
                            hoverEnabled: enabled
                            cursorShape: Qt.PointingHandCursor
                            onClicked: details.take(
                                value.parent.modelData.value,
                                value.parent.modelData.label
                            )
                        }
                    }
                }
            }
        }

        /// The mode, as nine answers rather than nine characters of shorthand.
        ///
        /// Editable where the destination keeps modes at all, which means the file protocols —
        /// an object store has nothing to set. Ticking a box sends the whole mode, because that
        /// is what the protocol takes: there is no way to change one bit on its own.
        Column {
            spacing: 6
            width: parent.width - 40
            visible: details.file.permissions !== undefined && details.file.permissions !== ""

            Row {
                width: parent.width

                Text {
                    text: "Permissions"
                    width: 110
                    font.pixelSize: 13
                    color: Theme.muted
                }

                Repeater {
                    model: ["Read", "Write", "Execute"]

                    Text {
                        required property string modelData

                        text: modelData
                        width: 78
                        font.pixelSize: 12
                        color: Theme.muted
                    }
                }
            }

            Repeater {
                model: [
                    { label: "Owner", who: "owner" },
                    { label: "Group", who: "group" },
                    { label: "Everyone", who: "everyone" }
                ]

                Row {
                    required property var modelData

                    Text {
                        text: parent.modelData.label
                        width: 110
                        font.pixelSize: 12
                        color: Theme.foreground
                        anchors.verticalCenter: parent.verticalCenter
                    }

                    Repeater {
                        model: ["read", "write", "execute"]

                        Tick {
                            required property string modelData

                            width: 78
                            editable: details.app.canSetPermissions
                            allowed: details.allows(parent.modelData.who, modelData)
                            onToggled: details.permit(parent.modelData.who, modelData, checked)
                        }
                    }
                }
            }
        }

        /// Only for destinations that can hand a file to somebody with no account, which today
        /// means the S3-shaped ones. Everything else has no answer to give.
        ///
        /// The addresses are copied rather than displayed. Neither is worth reading: one is a
        /// path you already know, the other four hundred characters of signature and expiry.
        /// The button is what people came for.
        Column {
            spacing: 10
            width: parent.width - 40
            visible: app.canShare

            Rectangle {
                width: parent.width
                height: 1
                color: Theme.border
            }

            Row {
                spacing: 8
                width: parent.width

                Column {
                    spacing: 2
                    width: parent.width - toggle.width - 8
                    anchors.verticalCenter: parent.verticalCenter

                    Text {
                        text: "Public"
                        font.pixelSize: 13
                        color: Theme.foreground
                    }

                    Text {
                        width: parent.width
                        font.pixelSize: 11
                        color: Theme.muted
                        elide: Text.ElideRight
                        text: {
                            if (!app.sharedIsKnown) {
                                return "Camion cannot tell"
                            }

                            return app.sharedIsPublic
                                ? "Anyone with the address can read it"
                                : "Only this account's keys can read it"
                        }
                    }
                }

                Toggle {
                    id: toggle

                    checked: app.sharedIsPublic
                    enabled: app.sharedIsKnown
                    opacity: enabled ? 1 : 0.4
                    anchors.verticalCenter: parent.verticalCenter

                    // Why the switch will not move, kept out of the way until it is wanted:
                    // the reason is a sentence from the server and would otherwise be the
                    // largest thing in the panel.
                    ToolTip.visible: !enabled && hovered
                    ToolTip.text: app.sharedWhyNot

                    onToggled: {
                        app.reshare(details.file.name, checked)

                        // Clicking a switch replaces the binding with whatever was clicked.
                        // Putting it back means the server's answer is what shows — so a store
                        // that refuses to change this snaps back instead of lying about it.
                        checked = Qt.binding(() => app.sharedIsPublic)
                    }
                }
            }

            Row {
                spacing: 8
                width: parent.width

                FlatButton {
                    width: (parent.width - 8) / 2
                    text: details.copied === "plain" ? "Copied" : "Copy link"
                    enabled: app.sharedUrl !== ""
                    onClicked: details.take(app.sharedUrl, "plain")
                }

                /// Signs and copies in one go. A link nobody can see is no use as two steps.
                FlatButton {
                    width: (parent.width - 8) / 2
                    text: details.copied === "signed" ? "Copied" : "Copy signed link"
                    onClicked: {
                        details.copyWhenSigned = true
                        app.signLink(details.file.name)
                    }
                }
            }
        }

        FlatButton {
            width: parent.width - 40
            text: "Download"
            highlighted: true
            onClicked: {
                details.visible = false
                details.downloadRequested()
            }
        }
    }
}
