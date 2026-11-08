//! iCloud on Linux: the native app that connects iCloud Drive to the desktop.
#![deny(unsafe_code)]
use std::cell::RefCell;
use std::env;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use icloud_core::i18n;
use icloud_core::paths::Paths;
use icloud_core::session::Session;
use icloud_core::settings::Settings;
use icloud_on_linux::app::App;

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

/// Set what GTK reads from the environment as it starts: drawing in software unless the user chose a renderer, and
/// `language` for GTK's own text when the user chose one in the app.
#[allow(unsafe_code)]
fn prepare_gtk(language: Option<&str>) {
    // Skipping OpenGL avoids loading the graphics driver, which costs 60 MB or more without GPU acceleration, as in
    // virtual machines.
    let software = env::var_os("GSK_RENDERER").is_none();
    // GTK 4.16 renamed the switch that keeps GDK from opening OpenGL and Vulkan at startup.
    let (name, value) = if gtk::minor_version() >= 16 {
        ("GDK_DISABLE", "gl,vulkan")
    } else {
        ("GDK_DEBUG", "gl-disable")
    };
    // SAFETY: main calls this before GTK starts and before any thread exists, so nothing reads the environment
    // meanwhile.
    unsafe {
        if software {
            env::set_var("GSK_RENDERER", "cairo");
            if env::var_os(name).is_none() {
                env::set_var(name, value);
            }
        }
        if let Some(language) = language {
            env::set_var("LANGUAGE", language);
        }
    }
}

fn main() -> glib::ExitCode {
    if !cfg!(target_os = "linux") || nix::unistd::getuid().is_root() {
        eprintln!("Open this app as your Linux desktop user.");
        return glib::ExitCode::FAILURE;
    }
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));
    let paths = Paths::for_user(helper());
    let choice = Settings::new(paths.settings()).language();
    // Automatic follows the desktop's languages, which are read here, before LANGUAGE changes for GTK.
    let language = i18n::apply(choice);
    prepare_gtk((choice != "auto").then_some(language));
    let application = adw::Application::builder().application_id(APP_ID).build();
    // GApplication keeps one copy running; opening the app again shows its window.
    let shown: Rc<RefCell<Option<Rc<App>>>> = Rc::default();
    application.connect_activate(move |application| {
        if let Some(app) = shown.borrow().as_ref() {
            app.present();
            return;
        }
        *shown.borrow_mut() = Some(App::new(application, Session::new(paths.clone())));
    });
    application.run()
}
