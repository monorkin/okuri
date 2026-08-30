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

pub struct ThemeRust {
    dark: bool,
    background: QString,
    surface: QString,
    elevated: QString,
    foreground: QString,
    bright: QString,
    muted: QString,
    accent: QString,
    accent_text: QString,
    selection: QString,
    selection_text: QString,
    border: QString,
    error: QString,
    warning: QString,
    success: QString,
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
            let _ = thread.queue(|theme| theme.reload());
        });
    }
}

impl From<Palette> for ThemeRust {
    fn from(palette: Palette) -> Self {
        let color = |color: crate::color::Color| QString::from(&color.to_string());

        Self {
            dark: palette.dark,
            background: color(palette.background),
            surface: color(palette.surface),
            elevated: color(palette.elevated),
            foreground: color(palette.foreground),
            bright: color(palette.bright),
            muted: color(palette.muted),
            accent: color(palette.accent),
            accent_text: color(palette.accent_text),
            selection: color(palette.selection),
            selection_text: color(palette.selection_text),
            border: color(palette.border),
            error: color(palette.error),
            warning: color(palette.warning),
            success: color(palette.success),
        }
    }
}

impl qobject::Theme {
    /// Assigns through the generated setters rather than replacing the struct, so Qt emits a
    /// change signal for every colour that actually moved and for none that did not.
    ///
    /// A palette that cannot be read right now leaves the current one alone. During a theme
    /// switch there is a moment with no palette on disk at all, and flashing the built-in
    /// colours through that gap would be worse than waiting for the new theme to land.
    pub fn reload(mut self: Pin<&mut Self>) {
        let Some(palette) = Palette::current() else {
            return;
        };

        let refreshed = ThemeRust::from(palette);

        self.as_mut().set_dark(refreshed.dark);
        self.as_mut().set_background(refreshed.background);
        self.as_mut().set_surface(refreshed.surface);
        self.as_mut().set_elevated(refreshed.elevated);
        self.as_mut().set_foreground(refreshed.foreground);
        self.as_mut().set_bright(refreshed.bright);
        self.as_mut().set_muted(refreshed.muted);
        self.as_mut().set_accent(refreshed.accent);
        self.as_mut().set_accent_text(refreshed.accent_text);
        self.as_mut().set_selection(refreshed.selection);
        self.as_mut().set_selection_text(refreshed.selection_text);
        self.as_mut().set_border(refreshed.border);
        self.as_mut().set_error(refreshed.error);
        self.as_mut().set_warning(refreshed.warning);
        self.as_mut().set_success(refreshed.success);
    }
}
