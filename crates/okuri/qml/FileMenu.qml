import QtQuick
import QtQuick.Controls
import sh.okuri

/// The right-click menu.
///
/// What it offers follows the connection's capabilities and the selection, so an object store
/// that cannot rename shows the item greyed rather than failing once you click it.
Menu {
    id: menu

    /// The window this belongs to. Passed in rather than reached for: there is one of these per
    /// window now, and a component that went looking for "the" application would find whichever
    /// window happened to be first.
    required property App app

    property int rows: 0
    property bool onFolder: false

    signal openRequested()
    signal downloadRequested()
    signal renameRequested()
    signal deleteRequested()
    signal newFolderRequested()

    readonly property bool one: rows === 1
    readonly property bool any: rows > 0

    background: Rectangle {
        implicitWidth: 200
        radius: 8
        color: Theme.elevated
        border.width: 1
        border.color: Theme.border
    }

    MenuItem {
        text: "Open"
        enabled: menu.one && menu.onFolder
        onTriggered: menu.openRequested()
    }

    MenuItem {
        text: menu.one ? "Download…" : "Download " + menu.rows + " items…"
        enabled: menu.any
        onTriggered: menu.downloadRequested()
    }

    MenuSeparator {}

    MenuItem {
        text: "Rename…"
        enabled: menu.one && app.canRename
        onTriggered: menu.renameRequested()
    }

    MenuItem {
        text: menu.one ? "Delete" : "Delete " + menu.rows + " items"
        enabled: menu.any
        onTriggered: menu.deleteRequested()
    }

    MenuSeparator {}

    MenuItem {
        text: "New folder…"
        enabled: app.canCreateFolder
        onTriggered: menu.newFolderRequested()
    }

    MenuItem {
        text: "Refresh"
        onTriggered: app.refresh()
    }
}
