import QtQuick

/// The application, which is not a window.
///
/// What Qt loads at startup, so that the thing outliving every window is not itself one of
/// them. A window made here is owned here and asks for the next one through its `another`
/// signal, rather than knowing how many windows there are or which of them is first.
QtObject {
    id: camion

    property Component blueprint: Component {
        Main {}
    }

    function open() {
        const window = camion.blueprint.createObject(camion)

        window.another.connect(camion.open)
    }

    Component.onCompleted: camion.open()
}
