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
| [crates/app/src/uploads.rs](../crates/app/src/uploads.rs) | Show the upload queue: the cloud folder for new files, the file dialog, each row's state and progress, and Resume, Pause, Skip, and Keep both. |
| [scripts/install_desktop.sh](../scripts/install_desktop.sh) | Build the app with Cargo and install it, its helper, and its desktop entry for the current user. |
| [crates/core/src/auth.rs](../crates/core/src/auth.rs) and [crates/app/src/signin.rs](../crates/app/src/signin.rs) | Run the sign-in steps on a private profile copy, answer setup questions, check access, and publish the profile; the dialog collects the key, Apple account, password, and verification answers. |
| [crates/core/src/session.rs](../crates/core/src/session.rs) | Unlock the saved profile with the local key, mount iCloud Drive read-only through rclone, and disconnect only the app's own mount. |
| [crates/core/src/uploads.rs](../crates/core/src/uploads.rs) | Own private upload snapshots, durable queue storage, path validation, and local cleanup. |
| [crates/core/src/transfers.rs](../crates/core/src/transfers.rs) | Prepare files in a worker thread, run one rclone command at a time, verify cloud bytes, and pause or resolve conflicts. |
| [crates/core](../crates/core) | Rust library for the native app without a user interface: paths, profile and mount checks, the upload queue, translated text, change signals, and child processes that die with the app. |

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

### Native sign-in

**Sign in to iCloud** starts on first use, and **Renew Apple session...** in the menu
renews later; renewing while connected disconnects first. Sign-in runs while the
app has no mount or client. It works on a private `.signin-*` copy next to the
profile and never edits the saved profile in place:

1. A new profile is encrypted with the local key before any answer is saved.
   For an existing profile, `auth-inspect` returns only the remote's type, service,
   and account; its password and tokens stay inside rclone. An existing `icloud`
   remote must be iCloud Drive for the same Apple account, compared without letter
   case. A profile without that remote can receive one.
2. `auth-create` recreates the remote in the copy. The app answers the service,
   account, password, and advanced-options questions; the user answers 2FA with a
   code, an SMS request, and a trusted phone choice when Apple offers several.
3. rclone saves continued answers verbatim, while the backend reads its password
   obscured. `auth-password` therefore pipes the password through `rclone obscure -`
   inside `scripts/session.sh` before saving it in the encrypted copy.
4. After the final answer, `auth-probe` checks read access with `lsjson --stat`.
   Only then does the app publish the copy: it checks that the saved profile is
   unchanged, links identities, and atomically replaces or creates the profile.

Each step is a separate process that Linux ends if the app dies. The local key,
rclone state, and answer travel only in its `RCLONE_CONFIG_PASS`, `RCLONE_STATE`,
and `RCLONE_RESULT` environment variables, never in arguments. The app overwrites
its own copies of the key, password, and step output when the attempt ends; copies
that GTK text fields and the standard library make for a child's environment cannot
be overwritten. Standard error is discarded. A failure, cancellation, or 120-second
step timeout keeps the saved profile and removes the copy, and the next sign-in
removes copies that a crash left behind for more than an hour. Quitting or ending
the desktop session cancels a running step first. A rejected code ends the attempt;
signing in again requests a new code.

Renewal changes the profile bytes. Before replacing them, the app writes
`profile-links.json`, which maps the new and old profile hashes to the identity of
the original profile. Either profile therefore keeps that identity after a crash,
so work bound to the account keeps its owner. A different account is refused.

## Managed uploads

The explorer mount stays read-only. Uploads run through the native app queue.
Each selected regular file is copied and hashed in a worker thread before any cloud
write. Its private source directory contains only that payload. The encrypted
profile and JSON journal stay outside every directory passed to rclone copy.

The queue is saved atomically with file and directory flushes. Interrupted active
rows reopen paused and require Resume. Each row is bound to the identity of the
encrypted profile used when it was added, its SHA-256 fingerprint; a different saved
profile cannot silently receive it. Before a transfer, the snapshot must still
match its SHA-256.

Every rclone command requires a successful durable queue checkpoint. A failed
save pauses the queue, stops the active command, and blocks new work until an
explicit Resume successfully saves the state. A completed or skipped snapshot
is retained if its terminal state cannot be persisted. Disconnect remains
available when storage is unavailable.
Commands start with the process guard: Linux
[`PR_SET_PDEATHSIG`](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)
kills an active transfer if the app dies, and the child checks that its parent is
still the app after arming it, which closes the startup race. A failed guard setup
starts no rclone command. Commands start from the main thread, to which the kernel
ties that signal; the preparation worker starts none. The window calls the queue's
`tick` every 100 ms, and active commands also trigger a checkpoint about once per
second. Connection ownership and profile identity are checked again after each
checkpoint, before the next command starts. A newly connected profile cannot claim
old pending rows or apply Keep both to another profile.

Each ready snapshot also has a private `upload.json` outside its data directory.
Startup recovers ready snapshots missing from the queue as paused rows. Incomplete
preparation directories without that record are preserved and reported; they
cannot upload automatically. A partial batch retains its ready snapshots.
If preparing another batch fails, an already active upload is stopped as well;
the queue never reports a paused transfer while allowing its child to continue.

Keep both changes the journal destination while keeping the snapshot filename
stable. When needed, a private `send/` directory contains a hard link under the
new destination name. Only that one-file directory is copied. Failed checkpoints
and crashes cannot detach the queue from its payload by renaming it first.

The reader accepts journal versions 1 and 2 without moving payloads or rewriting
the queue at startup. A pending version 1 row can recover its sole renamed file;
the normal background hash check still applies before any cloud command. Saves
use version 2, which records the stable source filename.

An independent remote stat and download detect existing equal or conflicting
content. Matching bytes finish without another upload. Different bytes require
Skip or Keep both, which chooses a new random filename. Missing files use
directory copy with `--immutable --ignore-times`; the single-file copy path in
rclone 1.75.2 bypasses that protection. Success requires an independent SHA-256
download check before releasing the snapshot. Network errors retain local data.
Disconnect pauses the queue and waits for its command to stop before unmounting
and releasing the unlocked session; closing the app and the end of the desktop
session do the same before the window closes. Cloud mutations are never used for
cleanup of ordinary queue items.

These checks do not provide a server transaction against another writer creating
or changing the same name simultaneously. The app serializes its own uploads;
shared editing and automatic replacement need a stronger backend conflict model.
The tested [rclone iCloud backend](https://github.com/rclone/rclone/blob/v1.75.2/backend/iclouddrive/iclouddrive.go)
can resolve an existing object again in Put and replace it in Update, leaving a
gap after the client check. Local tests cover a competing file before copy and
same-size, shorter, and longer edits before verification; they do not establish
an atomic create-if-absent guarantee. A mismatched download becomes Conflict,
including a stream that grows beyond the expected size, and retains the snapshot.

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
| Linux desktop | `~/.config/icloud-linux/profile-links.json` | `0600` map from renewed profile hashes to the identity of the original profile. It contains hashes only. |
| Linux desktop | `~/.config/icloud-linux/.signin-*/` | Private encrypted profile copy during sign-in, removed when the attempt ends. An abrupt exit can leave it behind; it is never used again. |
| Ubuntu | `~/iCloud` | Mount point, not a complete offline copy. |
| Ubuntu | rclone VFS cache, under the user cache directory | Contents of opened files; configuration encryption does not encrypt these copies. |
| Linux desktop | `~/.cache/icloud-on-linux/vfs/`, or the equivalent under `XDG_CACHE_HOME` | The native app's VFS cache, with the same contents and limits. |
| Linux desktop | `$XDG_RUNTIME_DIR/icloud-on-linux/session-*/` | Private copy of the encrypted profile while connected, removed on disconnect. The runtime directory is in memory and is cleared when the user logs out. |
| Linux desktop | `~/.cache/icloud-on-linux/uploads/`, or the equivalent under `XDG_CACHE_HOME` | Private unencrypted payload snapshots and a `0600` journal containing filenames, destinations, hashes, profile fingerprints, and state. Pending copies survive disconnect and restart. |
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
  encrypted profile. [crates/core/tests/uploads.rs](../crates/core/tests/uploads.rs)
  runs the upload queue with the real rclone against local fictional remotes:
  verification, Keep both and Skip, interrupted uploads and crashes, a WebDAV
  outage, a changed profile, and cloud edits before the copy or during verification.
- [crates/app/tests/visible](../crates/app/tests/visible): the visible window on a
  Wayland or X11 display. It uploads files and decides a conflict with Keep both,
  disconnects during an upload and resumes it after reconnecting, and ends the app
  during an upload. Then, in a new window, it connects, opens the file manager,
  and disconnects.
- [crates/core/tests/auth.rs](../crates/core/tests/auth.rs): native sign-in, 2FA,
  SMS, renewal identity, cancellation, and save failures through a local rclone
  stand-in. [crates/core/tests/sign_in.rs](../crates/core/tests/sign_in.rs) runs
  the real rclone setup questions with fictional answers up to the step that would
  contact Apple.
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
