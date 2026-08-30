import QtQuick
import QtQuick.Controls
import io.camion

/// Asks for one name. Used for new folders and for renaming.
Dialog {
    id: prompt

    /// Drawn inside the dialog rather than set as its title, which would put a second copy of
    /// the same words in a header above it.
    property string heading: ""
    property string placeholder: "Name"
    property string accept: "Save"
    property string warning: ""

    /// Deliberately not `accepted`: a Dialog already has a signal by that name, and declaring
    /// a second one is refused — leaving the handler bound to the built-in one, which carries
    /// no name with it.
    signal named(string name)

    modal: true
    anchors.centerIn: Overlay.overlay
    width: Math.min(400, Overlay.overlay ? Overlay.overlay.width - 60 : 400)

    background: Rectangle {
        radius: 10
        color: Theme.elevated
        border.width: 1
        border.color: Theme.border
    }

    function open(initial) {
        field.text = initial
        visible = true
        field.forceActiveFocus()
        field.selectAll()
    }

    contentItem: Column {
        spacing: 14
        padding: 20

        Text {
            text: prompt.heading
            font.pixelSize: 17
            color: Theme.bright
        }

        TextField {
            id: field
            width: parent.width - 40
            placeholderText: prompt.placeholder
            color: Theme.foreground
            onAccepted: prompt.confirm()

            background: Rectangle {
                radius: 6
                color: Theme.surface
                border.width: 1
                border.color: field.activeFocus ? Theme.accent : Theme.border
            }
        }

        Text {
            width: parent.width - 40
            text: prompt.warning
            visible: text !== ""
            color: Theme.warning
            font.pixelSize: 12
            wrapMode: Text.WordWrap
        }

        Row {
            spacing: 8
            anchors.right: parent.right
            anchors.rightMargin: 20

            FlatButton {
                text: "Cancel"
                onClicked: prompt.visible = false
            }

            FlatButton {
                text: prompt.accept
                highlighted: true
                enabled: field.text.trim() !== ""
                onClicked: prompt.confirm()
            }
        }
    }

    function confirm() {
        const name = field.text.trim()

        if (name !== "") {
            prompt.named(name)
            prompt.visible = false
        }
    }
}
