//! iCloud on Linux: the native app that connects iCloud Drive to the desktop.
#![deny(unsafe_code)]
use std::cell::RefCell;
use std::env;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use icloud_core::paths::Paths;
use icloud_core::session::Session;
use icloud_on_linux::window;

const APP_ID: &str = "io.github.luiscuellar31.IcloudOnLinux";

/// `scripts/session.sh` beside an installed app (`<prefix>/bin` and `<prefix>/share`). A debug build run from this
/// checkout uses the checkout's copy; a release build never falls back to a path from the machine that built it.
fn helper() -> PathBuf {
    let installed = env::current_exe()
        .ok()
        .and_then(|program| Some(program.parent()?.join("../share/icloud-on-linux/scripts/session.sh")))
        .unwrap_or_default();
    #[cfg(debug_assertions)]
    if !installed.is_file() {
        return PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/session.sh"));
    }
    installed
}

/// Draw this small window in software unless the user chose a renderer. Skipping OpenGL avoids loading the
/// graphics driver, which costs 60 MB or more without GPU acceleration, as in virtual machines.
#[allow(unsafe_code)]
fn prefer_software_rendering() {
    if env::var_os("GSK_RENDERER").is_some() {
        return;
    }
    // GTK 4.16 renamed the switch that keeps GDK from opening OpenGL and Vulkan at startup.
    let (name, value) = if gtk::minor_version() >= 16 {
        ("GDK_DISABLE", "gl,vulkan")
    } else {
        ("GDK_DEBUG", "gl-disable")
    };
    // SAFETY: this runs first in main, before any thread exists, so nothing reads the environment meanwhile.
    unsafe {
        env::set_var("GSK_RENDERER", "cairo");
        if env::var_os(name).is_none() {
            env::set_var(name, value);
        }
    }
}

fn main() -> glib::ExitCode {
    prefer_software_rendering();
    if !cfg!(target_os = "linux") || nix::unistd::getuid().is_root() {
        eprintln!("Open this app as your Linux desktop user.");
        return glib::ExitCode::FAILURE;
    }
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));
    icloud_core::i18n::apply("auto");
    let application = adw::Application::builder().application_id(APP_ID).build();
    // GApplication keeps one copy running; opening the app again shows its window.
    let shown: Rc<RefCell<Option<Rc<window::Window>>>> = Rc::default();
    application.connect_activate(move |application| {
        if let Some(window) = shown.borrow().as_ref() {
            window.window.present();
            return;
        }
        let window = window::Window::new(application, Session::new(Paths::for_user(helper())));
        // Quitting with the window keeps a late activation from showing a window that was already destroyed.
        let running = application.clone();
        window.window.connect_destroy(move |_| running.quit());
        window.window.present();
        *shown.borrow_mut() = Some(window);
    });
    application.run()
}
