# Architecture

The project uses rclone 1.75.2 to access iCloud Drive from Linux. It provides a
Docker terminal lab, a read-only FUSE mount on Ubuntu 24.04 with GNOME, and a native
app written in Rust with GTK4 and libadwaita that controls that mount. KDE
integration is not implemented yet.

## Components

| File | Responsibility |
| --- | --- |
| [start.command](../start.command) | Check Docker, build the image, and open the container with its persistent volume. |
| [Dockerfile](../Dockerfile) | Define Ubuntu 24.04, rclone, the unprivileged user, and the session test with local files. |
| [scripts/session.sh](../scripts/session.sh) | Unlock or create the encrypted configuration, authorize the connection, check listings, and open the terminal or mount. |
| [scripts/linux-start.sh](../scripts/linux-start.sh) | Check the Linux environment, select the user configuration, and open or reuse `~/iCloud`. |
| [open-ubuntu.command](../open-ubuntu.command) | Open only the VirtualBox VM identified by `.vm/uuid`; do not create or provision machines. |
| [scripts/provision-desktop.sh](../scripts/provision-desktop.sh) | Prepare the dedicated VM desktop and shortcuts using resources from its `cidata` medium. |
| [crates/app/src/main.rs](../crates/app/src/main.rs) | Start the app for the desktop user; GApplication keeps one running copy and shows its window when opened again. |
| [crates/app/src/window.rs](../crates/app/src/window.rs) and [view.rs](../crates/app/src/view.rs) | Present the connection status, masked key entry, file manager opening, and safe close; `view.rs` decides what to show and is tested without a display. |
| [scripts/install_desktop.sh](../scripts/install_desktop.sh) | Build the app with Cargo and install it, its helper, and its desktop entry for the current user. |
| [crates/core/src/session.rs](../crates/core/src/session.rs) | Unlock the saved profile with the local key, mount iCloud Drive read-only through rclone, and disconnect only the app's own mount. |
| [crates/core](../crates/core) | Rust library for the native app without a user interface: paths, profile and mount checks, translated text, change signals, and child processes that die with the app. |

Provisioning assumes Ubuntu with GNOME, the `icloud` user, and a prepared medium
containing scripts, the rclone package, and its checksum. It verifies the package
and installs programs under `/usr/local/bin`, the application shortcut, and the
Files bookmark. It also configures lab sign-in and desktop services. This is not
a generic installer and cannot rebuild the VM on its own; the disk, keys, and
local installation medium stay outside the repository.

## Authentication and opening

`scripts/session.sh` owns the shared flow:

1. Receive the configuration path in `RCLONE_CONFIG` and the `shell` or `mount` mode.
2. Ask for the local key without echoing it. For a new configuration, ask for
   confirmation and enable encryption before saving account data.
3. Unlock and check the configuration. Create the connection only if exactly
   `icloud:` is absent. The user enters Apple credentials and 2FA in their
   terminal; the configuration summary containing tokens is suppressed.
4. List the root with `rclone lsf icloud:`. A failure stops the opening flow.
5. In `shell` mode, open Bash in the container. In `mount` mode, mount the folder
   and open it with `gio open`.

The local key reaches rclone through the environment and a command that reads
that variable, without placing its value in arguments. It is not saved in a
separate file. In terminal mode it remains in that session environment; in mount
mode the assistant unsets its variable after starting rclone.

## Native connection

The native session reuses the `check` mode of `scripts/session.sh`: it receives the
local key on standard input, checks the existing encrypted profile, and returns only
a status code. It prints no remote names and never creates or re-encrypts a profile.
The session then copies the profile into a private folder under the user runtime
directory, so rclone never rewrites the saved one, and runs `rclone mount` read-only
with a unique device name, a private VFS cache, and the key in its environment only.

The key stays in memory while connected and is overwritten when the session clears
it. Disconnect checks the mount identity before running `fusermount3 -u`; a busy or
replaced mount is preserved, and only the client the session started is ended. An
existing compatible mount from the terminal lab can be opened and disconnected.
Child processes report through threads and a channel; the window calls the
session's `tick` every 200 ms to collect results and compare the kernel's mounts.

## Mount and desktop integration

The user **iCloud on Linux** shortcut runs the native app. Its Connect action
uses the existing encrypted profile; Open uses the desktop file URL handler. The
installer hides the prepared lab shortcut for this user without changing the
system launcher or account configuration. The desktop file is named after the
application ID, `io.github.luiscuellar31.IcloudOnLinux`, so the desktop matches
the app's windows to it.

The legacy **iCloud Drive** shortcut runs `icloud-linux-start`, which uses the dedicated
user configuration. If `~/iCloud` is already mounted, it opens the folder without
starting another process or asking for the key again.

The assistant starts `rclone mount` in the background with `--read-only` and a VFS
cache target size of 1 GiB. GNOME/Nautilus shows the folder and its bookmark.
Closing Files does not unmount the filesystem. The
[Ubuntu Desktop guide](UBUNTU_DESKTOP.md) explains how to disconnect it.

The write restriction belongs to the FUSE mount. It does not change the account
or backend into a connection with read-only permissions. The Docker terminal
allows manual rclone commands; documented commands use listings or downloads
with iCloud as the source and a local destination.

## Persistence and private data

| Environment | Location | Contents and lifetime |
| --- | --- | --- |
| Docker | `icloud-linux-state` volume, mounted at `/state` | Encrypted `rclone.conf` and files under `downloads/`. They persist after the container closes. |
| Ubuntu | `~/.config/icloud-linux/rclone.conf` | Dedicated encrypted configuration with `0600` permissions, reused between sessions. |
| Ubuntu | `~/iCloud` | Mount point, not a complete offline copy. |
| Ubuntu | rclone VFS cache, under the user cache directory | Contents of opened files; configuration encryption does not encrypt these copies. |
| Linux desktop | `~/.cache/icloud-on-linux/vfs/`, or the equivalent under `XDG_CACHE_HOME` | The native app's VFS cache, with the same contents and limits. |
| Linux desktop | `$XDG_RUNTIME_DIR/icloud-on-linux/session-*/` | Private copy of the encrypted profile while connected, removed on disconnect. The runtime directory is in memory and is cleared when the user logs out. |
| Linux desktop | `~/.local/bin/icloud-on-linux` and `~/.local/share/icloud-on-linux/` | User installation of the app and its helper; the desktop entry is under `~/.local/share/applications/`. |
| Host | `.vm/` | Disk, identifier, installation media, keys, and administration data for the local VM. Excluded from Git and the Docker build context. |
| Host | `downloads/` | Personal copies extracted from the container. Excluded from Git and the Docker build context. |

Docker and Ubuntu configurations are independent. The prepared lab received an
encrypted copy; credentials do not synchronize automatically between environments.
No real configuration belongs in the source tree. `.dockerignore` sends only the
Dockerfile and the session script needed by the image to the builder.

## Where behavior is checked

- [crates/core/tests](../crates/core/tests): the native session with local stand-ins
  for rclone and FUSE, and on Linux a real read-only FUSE mount of a fictional
  encrypted profile. [crates/app/tests/visible.rs](../crates/app/tests/visible.rs)
  connects, opens the file manager, and disconnects in the visible window on a
  Wayland display.
- [tests/test_launchers.py](../tests/test_launchers.py): Docker launch, session
  creation and reuse, errors, and key handling through local stand-ins.
- [tests/test_vm_launcher.py](../tests/test_vm_launcher.py): VM identifier validation
  and opening the dedicated machine, without running VirtualBox itself.
- [Dockerfile](../Dockerfile): encrypted session and listings using the real rclone
  binary against local files during the build.
- [tests/check-linux-mount.sh](../tests/check-linux-mount.sh): real FUSE mount with
  local data, reads, write rejection, and unmounting on Linux.
- [tests/check_writes.py](../tests/check_writes.py): writable FUSE operations with
  disposable local files, or an explicit `--icloud` run in a new owned test folder.
  Reads verify the backing directory or an independent cloud download.
  [tests/write_support.py](../tests/write_support.py) owns the private mount
  lifecycle and shared byte checks; it does not change the desktop launcher.
- [tests/cloud_sandbox.py](../tests/cloud_sandbox.py): private encrypted configuration
  copies, strictly scoped cloud paths, ownership markers, and empty-only cleanup.
- [tests/test_write_safety.py](../tests/test_write_safety.py) and
  [tests/test_cloud_safety.py](../tests/test_cloud_safety.py): refusal of inherited
  credentials in local checks, unsafe mount points, existing cloud folders,
  paths outside the sandbox, and cleanup without verified ownership.
- [tests/check_recovery.py](../tests/check_recovery.py): a loopback-only WebDAV
  fixture for corruption detection, interrupted uploads, persistent write cache,
  and private mount restarts. An explicit `--icloud` run checks immutable conflict
  rejection in another owned cloud test folder. Production launchers and their
  read-only policy are unchanged.

Default tests do not access an Apple account. Optional cloud checks require an
explicit selection and use a newly owned test folder. Real account access,
navigation from Files, and download integrity have also been checked in the lab.
[CONTRIBUTING.md](../CONTRIBUTING.md) documents the commands and limits of each check.
The [write checks guide](WRITE_TESTS.md) covers isolated writable mounts.
