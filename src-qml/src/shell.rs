// SPDX-License-Identifier: MIT

//! The Qt application object, made in C++ because cxx-qt-lib only offers
//! QGuiApplication and the tray needs QApplication.

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!(<QtWidgets/QApplication>);
        type QApplication;

        include!("daevalog-qml/cpp/shell.h");

        #[cxx_name = "newApplication"]
        fn new_application(args: &[String]) -> UniquePtr<QApplication>;

        #[cxx_name = "execApplication"]
        fn exec_application(app: Pin<&mut QApplication>) -> i32;

        #[cxx_name = "exitApplication"]
        fn exit_application(code: i32);
    }
}
