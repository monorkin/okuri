import QtQuick
import io.camion

/// One column heading. Shows which way the list is sorted when it is the one sorting it.
Item {
    id: heading

    property string text: ""
    property string column: ""
    property int alignment: Text.AlignLeft

    signal clicked()

    height: parent ? parent.height : 28

    readonly property bool sorting: Display.sortColumn === column

    Row {
        anchors.fill: parent
        layoutDirection: heading.alignment === Text.AlignRight
            ? Qt.RightToLeft
            : Qt.LeftToRight
        spacing: 4

        Text {
            text: heading.text
            height: parent.height
            verticalAlignment: Text.AlignVCenter
            color: heading.sorting || area.containsMouse ? Theme.foreground : Theme.muted
            font.pixelSize: 12
        }

        /// Bigger than the heading it sits beside, not smaller: these are the only mark saying
        /// which column the list is ordered by, and at the size of a full stop they read as one.
        Text {
            text: Display.sortDescending ? "▾" : "▴"
            height: parent.height
            verticalAlignment: Text.AlignVCenter
            visible: heading.sorting
            color: Theme.foreground
            font.pixelSize: 16
        }
    }

    MouseArea {
        id: area
        anchors.fill: parent
        hoverEnabled: true
        onClicked: heading.clicked()
    }
}
