//! What uploads and sync need from an open connection.
use std::process::Command;

use crate::paths::Paths;
use crate::signal::Signal;

pub trait Connection {
    fn paths(&self) -> &Paths;

    /// Whether this app owns an unlocked, mounted connection that may run rclone.
    fn unlocked(&self) -> bool;

    /// The identity of the profile in use; work that belongs to another profile stays paused.
    fn profile_id(&self) -> String;

    /// An `rclone` command whose environment holds the private profile copy and the key, without inherited rclone
    /// variables. Call it only while `unlocked()` is true.
    fn rclone(&self) -> Command;

    /// Emitted whenever the connection changes.
    fn changed(&self) -> &Signal;
}
