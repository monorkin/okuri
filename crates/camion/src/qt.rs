/// Qt base classes the models inherit from.
///
/// Declared once and aliased everywhere else. Declaring `QAbstractListModel` in more than one
/// bridge generates the same cast helpers twice and the link fails, so there is exactly one
/// place that introduces it and every model points back here.
#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++Qt" {
        include!(<QtCore/QAbstractListModel>);

        #[qobject]
        type QAbstractListModel;
    }
}
