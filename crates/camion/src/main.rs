mod app;
mod bus;
mod color;
mod connections;
mod desktop;
mod display;
mod file_list;
mod format;
mod icons;
mod kinds;
mod palette;
mod qt;
mod screen;
mod store;
mod theme;
mod transfers;
mod view;
mod watcher;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

fn main() {
    // Before Qt starts a thread of its own: reading the local clock is only allowed while this
    // process is the only one running in it.
    format::remember_local_clock();

    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    // Neither of these can fail in practice, and if one ever did, running an event loop with no
    // window would leave a process with nothing on screen and no way to say why.
    engine
        .as_mut()
        .expect("a QML engine")
        .load(&QUrl::from("qrc:/qt/qml/io/camion/qml/Main.qml"));

    app.as_mut().expect("a Qt application").exec();
}
