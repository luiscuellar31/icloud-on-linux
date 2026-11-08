//! Connection, upload, and sync logic of iCloud on Linux. The window drives it from the GLib main loop: components
//! start child processes, a timer calls their `tick` methods, and a `Signal` tells the window to render again.
#![deny(unsafe_code)]

pub mod auth;
pub mod autostart;
pub mod connection;
pub mod diagnostics;
pub mod i18n;
pub mod keyring;
pub mod paths;
pub mod places;
pub mod process;
pub mod profile;
pub mod session;
pub mod settings;
pub mod signal;
pub mod sync;
pub mod transfers;
pub mod translation_es;
pub mod uploads;

use std::fmt;
use std::io;

/// A failure with either a message for the user or an operating system error behind a generic message.
#[derive(Debug)]
pub enum Failure {
    Invalid(String),
    Io(io::Error),
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Invalid(message) => formatter.write_str(message),
            Failure::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for Failure {}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Failure::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, Failure>;
