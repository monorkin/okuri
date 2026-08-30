mod app;
mod bus;
mod color;
mod connections;
mod desktop;
mod display;
mod file_list;
mod format;
mod icons;
mod palette;
mod qt;
mod store;
mod theme;
mod transfers;
mod view;
mod watcher;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

fn main() {
    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/io/camion/qml/Main.qml"));
    }

    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
