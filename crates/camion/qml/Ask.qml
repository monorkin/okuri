import QtQuick
import QtQuick.Controls
import io.camion

/// The engine's questions, on screen.
///
/// One dialog for all of them: an unknown host key, a changed one, a password, a passphrase.
/// They differ only in wording and in whether they want typing, so they share a shape.
Dialog {
    id: ask

    modal: true
    closePolicy: Popup.NoAutoClose
    anchors.centerIn: Overlay.overlay
    width: Math.min(480, Overlay.overlay ? Overlay.overlay.width - 60 : 480)
    visible: App.asking

    background: Rectangle {
        radius: 10
        color: Theme.elevated
        border.width: 1
        border.color: App.questionIsGrave ? Theme.error : Theme.border
    }

    onVisibleChanged: {
        if (visible) {
            first.text = ""
            second.text = ""
            first.forceActiveFocus()
        }
    }

    contentItem: Column {
        spacing: 14
        padding: 20

        Text {
            width: parent.width - 40
            text: App.questionTitle
            font.pixelSize: 17
            color: App.questionIsGrave ? Theme.error : Theme.bright
            wrapMode: Text.WordWrap
        }

        Text {
            width: parent.width - 40
            text: App.questionBody
            visible: text !== ""
            color: Theme.foreground
            wrapMode: Text.WordWrap
            lineHeight: 1.3
        }

        Rectangle {
            width: parent.width - 40
            height: fingerprint.implicitHeight + 20
            visible: App.questionDetail !== ""
            radius: 6
            color: Theme.surface

            Text {
                id: fingerprint
                anchors.centerIn: parent
                width: parent.width - 20
                text: App.questionDetail
                font.family: "monospace"
                color: Theme.foreground
                wrapMode: Text.WrapAnywhere
                horizontalAlignment: Text.AlignHCenter
            }
        }

        Field {
            id: first
            width: parent.width - 40
            visible: App.questionWantsText || App.questionWantsPair
            label: App.questionFirstLabel
            // An access key is not a secret and is easier to check when you can read it.
            secret: App.questionIsSecret && !App.questionWantsPair
            onAccepted: ask.confirm()
        }

        Field {
            id: second
            width: parent.width - 40
            visible: App.questionWantsPair
            label: App.questionSecondLabel
            secret: App.questionIsSecret
            onAccepted: ask.confirm()
        }

        Row {
            spacing: 8
            anchors.right: parent.right
            anchors.rightMargin: 20

            FlatButton {
                text: "Cancel"
                onClicked: App.answer(false, "", "")
            }

            /// Only some questions have a third answer — replacing a file can also mean
            /// keeping both — so this is here when there is one and gone when there is not.
            FlatButton {
                text: App.questionAlternative
                visible: App.questionAlternative !== ""
                onClicked: App.answerAlternative()
            }

            FlatButton {
                text: App.questionAccept
                highlighted: true
                onClicked: ask.confirm()
            }
        }
    }

    function confirm() {
        App.answer(true, first.text, second.text)
    }
}
