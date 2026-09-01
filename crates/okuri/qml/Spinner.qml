import QtQuick
import sh.okuri

/// Something is happening and there is no way to say how far along it is.
///
/// A ring with a gap in it, turning. Deliberately not a progress bar: connecting to a server
/// has no percentage, and inventing one is a promise about how long it will take.
Item {
    id: spinner

    property bool running: false

    /// How much of the ring is drawn, as a fraction. Small enough to read as motion.
    property real arc: 0.28

    implicitWidth: 16
    implicitHeight: 16

    visible: running
    opacity: running ? 1 : 0

    Behavior on opacity {
        NumberAnimation { duration: 120 }
    }

    Canvas {
        id: ring

        anchors.fill: parent
        antialiasing: true

        onPaint: {
            const context = getContext("2d")
            const middle = width / 2
            const radius = middle - 2

            context.reset()
            context.lineWidth = 2
            context.lineCap = "round"

            context.beginPath()
            context.arc(middle, middle, radius, 0, 2 * Math.PI)
            context.strokeStyle = Qt.alpha(Theme.foreground, 0.18)
            context.stroke()

            context.beginPath()
            context.arc(middle, middle, radius, 0, 2 * Math.PI * spinner.arc)
            context.strokeStyle = Theme.accent
            context.stroke()
        }

        // Repainted when the theme changes, since the colours are read at paint time.
        Connections {
            target: Theme
            function onAccentChanged() { ring.requestPaint() }
        }

        RotationAnimator {
            target: ring
            running: spinner.running
            from: 0
            to: 360
            duration: 900
            loops: Animation.Infinite
        }
    }
}
