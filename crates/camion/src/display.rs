use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::view::{self, Mode};

/// How the file list is displayed, as QML sees it.
///
/// A window onto [`crate::view`], which is where the settings actually live and are written
/// down. Everything that draws reads from the same place, so the menu, the list, and the
/// headers cannot disagree about what is on screen.
#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, mode)]
        #[qproperty(bool, is_grid)]
        #[qproperty(i32, grid_icon)]
        #[qproperty(i32, list_icon)]
        #[qproperty(i32, row_height)]
        #[qproperty(bool, can_grow)]
        #[qproperty(bool, can_shrink)]
        #[qproperty(bool, show_size)]
        #[qproperty(bool, show_kind)]
        #[qproperty(bool, show_modified)]
        #[qproperty(bool, show_permissions)]
        #[qproperty(QString, sort_column)]
        #[qproperty(bool, sort_descending)]
        #[qproperty(bool, show_hidden)]
        type Display = super::DisplayRust;

        /// Switches between the list and the grid.
        #[qinvokable]
        fn show_as(self: Pin<&mut Display>, mode: QString);

        #[qinvokable]
        fn toggle_mode(self: Pin<&mut Display>);

        /// Steps the icon size, which changes both views at once.
        #[qinvokable]
        fn resize(self: Pin<&mut Display>, by: i32);

        #[qinvokable]
        fn show_column(self: Pin<&mut Display>, column: QString, visible: bool);


        #[qinvokable]
        fn sort_as(self: Pin<&mut Display>, column: QString, descending: bool);

        /// Re-sorts by a column, flipping the direction when it is already the sorted one —
        /// which is what clicking a column header is expected to do.
        #[qinvokable]
        fn sort_by(self: Pin<&mut Display>, column: QString);

        #[qinvokable]
        fn toggle_hidden(self: Pin<&mut Display>);
    }

    impl cxx_qt::Threading for Display {}

    impl cxx_qt::Initialize for Display {}
}

#[derive(Default)]
pub struct DisplayRust {
    mode: QString,
    is_grid: bool,
    grid_icon: i32,
    list_icon: i32,
    row_height: i32,
    can_grow: bool,
    can_shrink: bool,
    show_size: bool,
    show_kind: bool,
    show_modified: bool,
    show_permissions: bool,
    sort_column: QString,
    sort_descending: bool,
    show_hidden: bool,
}

impl cxx_qt::Initialize for qobject::Display {
    fn initialize(mut self: Pin<&mut Self>) {
        let thread = self.as_mut().qt_thread();

        crate::view::on_change(move || {
            crate::qt::queue(&thread, |display| display.publish());
        });

        self.publish();
    }
}

impl qobject::Display {
    pub fn show_as(self: Pin<&mut Self>, mode: QString) {
        let mode = Mode::parse(&mode.to_string());

        view::update(|settings| settings.mode = mode);
    }

    pub fn toggle_mode(self: Pin<&mut Self>) {
        view::update(|settings| settings.mode = settings.mode.other());
    }

    pub fn resize(self: Pin<&mut Self>, by: i32) {
        view::update(|settings| settings.size_step = settings.resized(by));
    }

    pub fn show_column(self: Pin<&mut Self>, column: QString, visible: bool) {
        let column = column.to_string();

        view::update(|settings| settings.columns.set(&column, visible));
    }

    pub fn sort_as(self: Pin<&mut Self>, column: QString, descending: bool) {
        let column = column.to_string();

        view::update(|settings| {
            settings.sort_column = column;
            settings.sort_descending = descending;
        });
    }

    pub fn sort_by(self: Pin<&mut Self>, column: QString) {
        let column = column.to_string();

        view::update(|settings| {
            settings.sort_descending = settings.sort_column == column && !settings.sort_descending;
            settings.sort_column = column;
        });
    }

    pub fn toggle_hidden(self: Pin<&mut Self>) {
        view::update(|settings| settings.show_hidden = !settings.show_hidden);
    }

    /// Copies the settings onto the properties QML is bound to.
    fn publish(mut self: Pin<&mut Self>) {
        let settings = view::current();

        self.as_mut().set_mode(QString::from(settings.mode.name()));
        self.as_mut().set_is_grid(settings.mode == Mode::Grid);
        self.as_mut().set_grid_icon(settings.grid_icon());
        self.as_mut().set_list_icon(settings.list_icon());
        self.as_mut().set_row_height(settings.row_height());
        self.as_mut().set_can_grow(settings.can_grow());
        self.as_mut().set_can_shrink(settings.can_shrink());
        self.as_mut().set_show_size(settings.columns.size);
        self.as_mut().set_show_kind(settings.columns.kind);
        self.as_mut().set_show_modified(settings.columns.modified);
        self.as_mut().set_show_permissions(settings.columns.permissions);
        self.as_mut()
            .set_sort_column(QString::from(&settings.sort_column));
        self.as_mut().set_sort_descending(settings.sort_descending);
        self.as_mut().set_show_hidden(settings.show_hidden);
    }
}
