import QtQuick
import QtQuick.Controls
import sh.okuri

/// The engine's questions, on screen.
///
/// One dialog for all of them: an unknown host key, a changed one, a password, a passphrase.
/// They differ only in wording and in whether they want typing, so they share a shape.
Dialog {
    id: ask

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

    modal: true
    closePolicy: Popup.NoAutoClose
    anchors.centerIn: Overlay.overlay
    width: Math.min(480, Overlay.overlay ? Overlay.overlay.width - 60 : 480)
    visible: app.asking

    background: Rectangle {
        radius: 10
        color: Theme.elevated
        border.width: 1
        border.color: app.questionIsGrave ? Theme.error : Theme.border
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
            text: app.questionTitle
            font.pixelSize: 17
            color: app.questionIsGrave ? Theme.error : Theme.bright
            wrapMode: Text.WordWrap
        }

        Text {
            width: parent.width - 40
            text: app.questionBody
            visible: text !== ""
            color: Theme.foreground
            wrapMode: Text.WordWrap
            lineHeight: 1.3
        }

        Rectangle {
            width: parent.width - 40
            height: fingerprint.implicitHeight + 20
            visible: app.questionDetail !== ""
            radius: 6
            color: Theme.surface

            Text {
                id: fingerprint
                anchors.centerIn: parent
                width: parent.width - 20
                text: app.questionDetail
                font.family: "monospace"
                color: Theme.foreground
                wrapMode: Text.WrapAnywhere
                horizontalAlignment: Text.AlignHCenter
            }
        }

        Field {
            id: first
            width: parent.width - 40
            visible: app.questionWantsText || app.questionWantsPair
            label: app.questionFirstLabel
            // An access key is not a secret and is easier to check when you can read it.
            secret: app.questionIsSecret && !app.questionWantsPair
            onAccepted: ask.confirm()
        }

        Field {
            id: second
            width: parent.width - 40
            visible: app.questionWantsPair
            label: app.questionSecondLabel
            secret: app.questionIsSecret
            onAccepted: ask.confirm()
        }

        Row {
            spacing: 8
            anchors.right: parent.right
            anchors.rightMargin: 20

            FlatButton {
                text: "Cancel"
                onClicked: app.answer(false, "", "")
            }

            /// Only some questions have a third answer — replacing a file can also mean
            /// keeping both — so this is here when there is one and gone when there is not.
            FlatButton {
                text: app.questionAlternative
                visible: app.questionAlternative !== ""
                onClicked: app.answerAlternative()
            }

            FlatButton {
                text: app.questionAccept
                highlighted: true
                onClicked: ask.confirm()
            }
        }
    }

    function confirm() {
        app.answer(true, first.text, second.text)
    }
}
