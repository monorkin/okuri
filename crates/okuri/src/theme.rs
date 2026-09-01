use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::palette::Palette;

/// The palette, as a QML singleton.
///
/// Every colour in the interface comes from here, so a theme change is one object emitting its
/// property signals and the whole window repainting itself.
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
        #[qproperty(bool, dark)]
        #[qproperty(QString, background)]
        #[qproperty(QString, surface)]
        #[qproperty(QString, elevated)]
        #[qproperty(QString, foreground)]
        #[qproperty(QString, bright)]
        #[qproperty(QString, muted)]
        #[qproperty(QString, accent)]
        #[qproperty(QString, accent_text)]
        #[qproperty(QString, selection)]
        #[qproperty(QString, selection_text)]
        #[qproperty(QString, border)]
        #[qproperty(QString, error)]
        #[qproperty(QString, warning)]
        #[qproperty(QString, success)]
        type Theme = super::ThemeRust;

        /// Re-reads the desktop's palette. Called when the theme on disk changes, and cheap
        /// enough to call at any other time.
        #[qinvokable]
        fn reload(self: Pin<&mut Theme>);
    }

    impl cxx_qt::Threading for Theme {}

    impl cxx_qt::Initialize for Theme {}
}

/// Declares the palette's colours once, and writes the three lists that have to agree about
/// them: the fields, how a [`Palette`] becomes them, and how each one is handed to Qt.
///
/// The bridge above names them a fourth time, because `#[qproperty]` has to be written out for
/// CXX-Qt to see it. Everything below is generated, so adding a colour is one line there and
/// one line here rather than four places to remember.
macro_rules! colours {
    ($($name:ident => $setter:ident),* $(,)?) => {
        pub struct ThemeRust {
            dark: bool,
            $($name: QString,)*
        }

        impl From<Palette> for ThemeRust {
            fn from(palette: Palette) -> Self {
                let color = |color: crate::color::Color| QString::from(&color.to_string());

                Self {
                    dark: palette.dark,
                    $($name: color(palette.$name),)*
                }
            }
        }

        impl qobject::Theme {
            /// Assigns through the generated setters rather than replacing the struct, so Qt
            /// emits a change signal for every colour that actually moved and for none that
            /// did not.
            ///
            /// A palette that cannot be read right now leaves the current one alone. During a
            /// theme switch there is a moment with no palette on disk at all, and flashing the
            /// built-in colours through that gap would be worse than waiting for the new theme
            /// to land.
            pub fn reload(mut self: Pin<&mut Self>) {
                let Some(palette) = Palette::current() else {
                    return;
                };

                let refreshed = ThemeRust::from(palette);

                self.as_mut().set_dark(refreshed.dark);
                $(self.as_mut().$setter(refreshed.$name);)*
            }
        }
    };
}

colours! {
    background => set_background,
    surface => set_surface,
    elevated => set_elevated,
    foreground => set_foreground,
    bright => set_bright,
    muted => set_muted,
    accent => set_accent,
    accent_text => set_accent_text,
    selection => set_selection,
    selection_text => set_selection_text,
    border => set_border,
    error => set_error,
    warning => set_warning,
    success => set_success,
}

impl Default for ThemeRust {
    fn default() -> Self {
        Self::from(Palette::load())
    }
}

impl cxx_qt::Initialize for qobject::Theme {
    /// Follows the desktop's theme for as long as the window is open.
    ///
    /// `omarchy-theme-set` deletes the theme directory and moves the new one into its place, so
    /// the directory above it is what gets watched — the one whose name does not change.
    fn initialize(self: Pin<&mut Self>) {
        let thread = self.qt_thread();

        crate::desktop::on_theme_change(move || {
            crate::qt::queue(&thread, |theme| theme.reload());
        });
    }
}


