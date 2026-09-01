import QtQuick

/// Which rows are picked, and the rules for picking them.
///
/// Held apart from either view because the list and the grid are two ways of showing the same
/// thing: switching between them should not lose what you had selected, and neither should have
/// its own idea of the rules.
QtObject {
    id: selection

    property int count: 0

    /// Rows are remembered rather than a single current index, so Shift and Ctrl can build up
    /// a selection the way they do everywhere else.
    property var rows: []
    property int anchorRow: -1
    property int current: -1

    function isSelected(row) {
        return rows.indexOf(row) !== -1
    }

    function selectOnly(row) {
        rows = [row]
        anchorRow = row
        current = row
    }

    function toggle(row) {
        let next = rows.slice()
        const at = next.indexOf(row)

        if (at === -1) {
            next.push(row)
        } else {
            next.splice(at, 1)
        }

        rows = next
        anchorRow = row
        current = row
    }

    function extendTo(row) {
        if (anchorRow === -1) {
            selectOnly(row)
            return
        }

        let next = []
        const first = Math.min(anchorRow, row)
        const last = Math.max(anchorRow, row)

        for (let each = first; each <= last; each++) {
            next.push(each)
        }

        rows = next
        current = row
    }

    function selectAll() {
        let next = []
        for (let row = 0; row < count; row++) {
            next.push(row)
        }
        rows = next
    }

    function clear() {
        rows = []
        anchorRow = -1
        current = -1
    }

    /// One click, with whatever modifiers were held.
    function click(row, modifiers) {
        if (modifiers & Qt.ShiftModifier) {
            extendTo(row)
        } else if (modifiers & Qt.ControlModifier) {
            toggle(row)
        } else {
            selectOnly(row)
        }
    }

    function step(by) {
        if (count === 0) {
            return -1
        }

        const row = Math.max(0, Math.min(count - 1, current + by))
        selectOnly(row)

        return row
    }
}
