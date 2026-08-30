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

/// Runs `update` on the interface thread.
///
/// Every model is fed from the engine's threads and may only touch itself from Qt's own, so
/// this is the one door between them. Queueing fails only once the window has been torn down,
/// and at that point there is nothing left to update and nobody left to tell — which is why the
/// answer is dropped here rather than at each of the places that ask.
pub fn queue<T>(
    thread: &cxx_qt::CxxQtThread<T>,
    update: impl FnOnce(std::pin::Pin<&mut T>) + Send + 'static,
) where
    T: cxx_qt::Threading,
{
    let _ = thread.queue(update);
}
