import QtQuick
import io.camion

/// Something worth saying, said once along the bottom rather than in a dialog you have to
/// dismiss before you can carry on.
///
/// Coloured like what it is. A failure on a tinted strip the same shade as everything else is
/// something you scroll past; a failure has to be the loudest thing on screen for as long as it
/// is there.
Rectangle {
    id: notice

    property string text: ""

    /// Whether this is bad news. Confirmations use the same strip and must not look like one.
    property bool grave: true

    signal dismissed()

    readonly property color ink: notice.grave ? Theme.error : Theme.success

    implicitHeight: notice.text === "" ? 0 : Math.max(40, message.implicitHeight + 20)
    visible: implicitHeight > 0
    color: Qt.alpha(notice.ink, 0.22)
    clip: true

    Behavior on implicitHeight {
        NumberAnimation { duration: 120; easing.type: Easing.OutCubic }
    }

    Rectangle {
        width: parent.width
        height: 1
        color: notice.ink
    }

    Text {
        id: message

        anchors.left: parent.left
        anchors.leftMargin: 16
        anchors.right: close.left
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter

        text: notice.text
        color: Theme.bright
        wrapMode: Text.WordWrap

        // Two lines is enough for any sentence worth reading here, and stops one long answer
        // from a server pushing the file list off the window.
        maximumLineCount: 2
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
