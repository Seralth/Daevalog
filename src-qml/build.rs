// SPDX-License-Identifier: MIT

use cxx_qt_build::{CxxQtBuilder, QmlFile, QmlModule};

fn main() {
    // The QML files sit next to the module's qmldir. In a subfolder, Qt 6.4
    // takes Theme from the folder's implicit import and loses the singleton.
    let module = QmlModule::new("Daevalog").qml_files([
        QmlFile::from("Main.qml"),
        QmlFile::from("MeterWindow.qml"),
        QmlFile::from("LayerMeterWindow.qml"),
        QmlFile::from("MeterRow.qml"),
        QmlFile::from("HudText.qml"),
        QmlFile::from("Theme.qml").singleton(true),
    ]);

    CxxQtBuilder::new_qml_module(module)
        .files(["src/overlay.rs", "src/shell.rs"])
        .cpp_file("cpp/shell.cpp")
        // Class icons, the app icon and Pretendard live elsewhere in the repo;
        // the .qrc gives them short ASCII names.
        .qrc("resources.qrc")
        .qt_module("Gui")
        .qt_module("Quick")
        .qt_module("Widgets")
        .build();
}
