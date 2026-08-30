import QtQuick
import QtQuick.Controls
import io.camion

/// A labelled text field, so the editor reads as a form rather than as a stack of boxes.
Row {
    id: field

    property string label: ""
    property string placeholder: ""
    property bool secret: false
    property alias text: input.text

    signal accepted()

    spacing: 8

    function forceActiveFocus() {
        input.forceActiveFocus()
    }

    Text {
        text: field.label
        width: field.label === "" ? 0 : 80
        visible: field.label !== ""
        color: Theme.muted
        anchors.verticalCenter: parent.verticalCenter
    }

    TextField {
        id: input
        width: field.width - (field.label === "" ? 0 : 88)
        placeholderText: field.placeholder
        echoMode: field.secret ? TextInput.Password : TextInput.Normal
        color: Theme.foreground
        onAccepted: field.accepted()

        background: Rectangle {
            radius: 6
            color: Theme.surface
            border.width: 1
            border.color: input.activeFocus ? Theme.accent : Theme.border
        }
    }
}
