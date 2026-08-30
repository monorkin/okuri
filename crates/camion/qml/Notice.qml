import QtQuick
import io.camion

/// Something went wrong, said once along the bottom rather than in a dialog you have to dismiss
/// before you can carry on.
Rectangle {
    id: notice

    property string text: ""

    signal dismissed()

    implicitHeight: notice.text === "" ? 0 : 36
    visible: implicitHeight > 0
    color: Theme.surface
    clip: true

    Behavior on implicitHeight {
        NumberAnimation { duration: 120; easing.type: Easing.OutCubic }
    }

    Rectangle {
        width: parent.width
        height: 1
        color: Theme.border
    }

    Rectangle {
        width: 3
        height: parent.height
        color: Theme.error
    }

    Text {
        anchors.left: parent.left
        anchors.leftMargin: 16
        anchors.right: close.left
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        text: notice.text
        color: Theme.foreground
        elide: Text.ElideRight
    }

    FlatButton {
        id: close
        anchors.right: parent.right
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        text: "Dismiss"
        onClicked: notice.dismissed()
    }
}
