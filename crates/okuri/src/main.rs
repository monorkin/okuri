mod browser;
mod bus;
mod color;
mod connections;
mod desktop;
mod details;
mod dialogs;
mod editor;
mod file_list;
mod format;
mod icons;
mod kinds;
mod palette;
mod picker;
mod relay;
mod running;
mod screen;
mod store;
mod theme;
mod transfers;
mod view;
mod watcher;
mod window;

use adw::prelude::*;
use gtk::gio;

fn main() {
    // Before GTK starts a thread of its own: reading the local clock is only allowed while this
    // process is the only one running in it.
    format::remember_local_clock();

    let app = adw::Application::builder()
        .application_id("sh.okuri.Okuri")
        // Every launch is its own process. `okuri production-web` from a terminal opens that
        // connection in the window it makes, rather than nudging one already open elsewhere.
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_startup(|app| {
        relay::start();
        theme::install();
        window::install_shortcuts(app);
    });

    app.connect_activate(|app| {
        window::open(app);
    });

    // The command line is read by the first window rather than parsed here: the only thing
    // that can be on it is the name of a connection to open.
    app.run_with_args::<&str>(&[]);
}
