import QtQuick
import QtQuick.Controls
import io.camion

/// Asks before something that cannot be undone.
///
/// There is no trash on the other end of a connection: a deleted file is gone, and the only
/// chance to say otherwise is here.
Dialog {
    id: confirm

    property string question: ""
    property string detail: ""
    property string accept: "Delete"

    signal confirmed()

    modal: true
    anchors.centerIn: Overlay.overlay
    width: Math.min(400, Overlay.overlay ? Overlay.overlay.width - 60 : 400)

    background: Rectangle {
        radius: 10
        color: Theme.elevated
        border.width: 1
        border.color: Theme.border
    }

    function ask(question, detail) {
        confirm.question = question
        confirm.detail = detail
        visible = true
    }

    contentItem: Column {
        spacing: 14
        padding: 20

        Text {
            width: parent.width - 40
            text: confirm.question
            font.pixelSize: 17
            color: Theme.bright
            wrapMode: Text.WordWrap
        }

        Text {
            width: parent.width - 40
            text: confirm.detail
            visible: text !== ""
            color: Theme.muted
            wrapMode: Text.WordWrap
        }

        Row {
            spacing: 8
            anchors.right: parent.right
            anchors.rightMargin: 20

            FlatButton {
                text: "Cancel"
                onClicked: confirm.visible = false
            }

            FlatButton {
                text: confirm.accept
                highlighted: true
                onClicked: {
                    confirm.visible = false
                    confirm.confirmed()
                }
            }
        }
    }
}
