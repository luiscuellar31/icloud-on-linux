# Architecture

The project uses rclone 1.75.2 to access iCloud Drive from Linux. It provides a
Docker terminal lab, a read-only FUSE mount on Ubuntu 24.04 with GNOME, and a native
app written in Rust with GTK4 and libadwaita that controls that mount and keeps
two-way synced folders for editing. The visible
app is checked with GNOME 46 and KDE Plasma 5.27 on headless Wayland compositors.
Its `.deb` and `.rpm` packages are checked in Debian 13, Ubuntu 24.04, Ubuntu 26.04,
and Fedora 45 containers.

## Components

| File | Responsibility |
| --- | --- |
| [start.command](../start.command) | Check Docker, build the image, and open the container with its persistent volume. |
| [Dockerfile](../Dockerfile) | Define Ubuntu 24.04, rclone, the unprivileged user, and the session test with local files. |
| [scripts/session.sh](../scripts/session.sh) | Unlock or create the encrypted configuration, authorize the connection, check listings, and open the terminal or mount. |
| [scripts/linux-start.sh](../scripts/linux-start.sh) | Check the Linux environment, select the user configuration, and open or reuse `~/iCloud`. |
| [open-ubuntu.command](../open-ubuntu.command) | Open only the VirtualBox VM identified by `.vm/uuid`; do not create or provision machines. |
| [scripts/provision-desktop.sh](../scripts/provision-desktop.sh) | Prepare the dedicated VM desktop and shortcuts using resources from its `cidata` medium. |
| [crates/app/src/main.rs](../crates/app/src/main.rs) | Start the app for the desktop user in the saved language; GApplication keeps one running copy, shows its window when opened again, and accepts `--background` from autostart entries. |
| [crates/app/src/app.rs](../crates/app/src/app.rs) | Own the connection, the upload queue, and synced folders with their timers for as long as the app runs, together with the window that shows them, the tray icon, desktop notifications, and keyring calls. A language change builds a new window over the same parts. |
| [crates/app/src/window.rs](../crates/app/src/window.rs) and [view.rs](../crates/app/src/view.rs) | Present the status, first-use sign-in, masked key entry, the Apple session warning, file manager opening, the menu, closing to the tray, and quitting; `view.rs` decides what to show and is tested without a display. |
| [crates/app/src/tray.rs](../crates/app/src/tray.rs) | Export the tray icon as a StatusNotifierItem with a `com.canonical.dbusmenu` menu through GIO's D-Bus API, and register it again whenever a StatusNotifierWatcher appears. |
| [crates/app/src/folders.rs](../crates/app/src/folders.rs) | Show the synced folders: Add folder, each folder's state, a context menu with Open, Sync now, Backups, and Stop syncing, and Review for folders that stopped. |
| [crates/app/src/uploads.rs](../crates/app/src/uploads.rs) | Show the upload queue: the cloud folder for new files, the file dialog, each row's state and progress, and Resume, Pause, Skip, and Keep both. |
| [scripts/install_desktop.sh](../scripts/install_desktop.sh) | Build the app with Cargo and install it, its helper, and its desktop entry for the current user. |
| [packaging/build.sh](../packaging/build.sh) | Build the `.deb` and `.rpm` with Cargo, `dpkg-deb`, and `rpmbuild` in disposable Debian 13 and Fedora 45 containers. |
| [packaging/stage.sh](../packaging/stage.sh) | Install the app, session helper, and desktop entry into a package root shared by both formats. |
| [packaging/control](../packaging/control) and [packaging/icloud-on-linux.spec](../packaging/icloud-on-linux.spec) | Declare package metadata and dependencies. |
| [packaging/io.github.luiscuellar31.IcloudOnLinux.desktop](../packaging/io.github.luiscuellar31.IcloudOnLinux.desktop) | Desktop entry for the packaged and user installations. |
| [crates/core/src/auth.rs](../crates/core/src/auth.rs) and [crates/app/src/signin.rs](../crates/app/src/signin.rs) | Run the sign-in steps on a private profile copy, answer setup questions, check access, and publish the profile; the dialog collects the key, Apple account, password, and verification answers. |
| [crates/core/src/diagnostics.rs](../crates/core/src/diagnostics.rs) | Write the support report with versions, settings, and states, without keys, tokens, file contents, or file and folder names. |
| [crates/core/src/session.rs](../crates/core/src/session.rs) | Unlock the saved profile with the local key, mount iCloud Drive read-only through rclone, and disconnect only the app's own mount. |
| [crates/core/src/uploads.rs](../crates/core/src/uploads.rs) | Own private upload snapshots, durable queue storage, path validation, and local cleanup. |
| [crates/core/src/transfers.rs](../crates/core/src/transfers.rs) | Prepare files in a worker thread, run one rclone command at a time, verify cloud bytes, and pause or resolve conflicts. |
| [crates/core/src/sync.rs](../crates/core/src/sync.rs) | Keep chosen iCloud folders and local copies in two-way sync with rclone bisync, and surface pending changes, conflicts, reviews, and backups. |
| [crates/core/src/places.rs](../crates/core/src/places.rs) | Add an `iCloud Sync` place to the GTK bookmarks and existing Dolphin places, without duplicating or rewriting other entries. |
| [crates/core/src/settings.rs](../crates/core/src/settings.rs) | Read and write the language, the automatic unlock choice, and the last in-app sign-in time in `settings.ini`, keeping every other line as the Python app's `QSettings` wrote it. |
| [crates/core/src/autostart.rs](../crates/core/src/autostart.rs) | Create the XDG autostart entry from the installed desktop entry with `--background`, and remove it, together with the Python app's entry when that one runs this app. |
| [crates/core/src/keyring.rs](../crates/core/src/keyring.rs) | Store, find, and clear the local key in the Secret Service through `secret-tool`, without blocking the window, only while automatic unlock is on. |
| [crates/core/src/i18n.rs](../crates/core/src/i18n.rs) and [translation_es.rs](../crates/core/src/translation_es.rs) | Translate the app's own text: English is the source, and Spanish comes from a catalog. |
| [crates/core](../crates/core) | Rust library for the native app without a user interface: paths, profile and mount checks, the upload queue, synced folders, translated text, change signals, and child processes that die with the app. |

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
Child processes report through threads and a channel; the app calls the session's
`tick` every 200 ms to collect results and compare the kernel's mounts.

When a connection that reached the connected state stops on its own, the session
keeps the key in memory and reconnects after 30 seconds, 2 minutes, and 5 minutes,
while the window shows **Reconnecting...** and **Cancel** stops waiting. Each attempt
can sign in to Apple again, which may notify trusted devices when the session has
expired, so there are only three attempts; a connection that then stays up for ten
minutes earns them again. A first connection that fails is not retried. Connecting,
disconnecting, or quitting cancels a pending attempt, and giving up forgets the key.
Suspend does not stop the rclone client, so resuming needs no reconnection.

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
be overwritten. Of standard error, only a 16 KiB tail stays in memory, to recognize
an Advanced Data Protection refusal; it is never shown or saved. A failure,
cancellation, or 120-second step timeout keeps the saved profile and removes the
copy, and the next sign-in removes copies that a crash left behind for more than an
hour. Quitting or ending the desktop session cancels a running step first. A
rejected code ends the attempt; signing in again requests a new code.

The app records in `settings.ini` when it last published a profile. Apple trusts a
signed-in rclone session for 30 days, so five days before that the window offers
**Renew** in a banner, and a hidden window sends one notification. Profiles signed
in from the terminal have no recorded time and get no estimate.

Renewal changes the profile bytes. Before replacing them, the app writes
`profile-links.json`, which maps the new and old profile hashes to the identity of
the original profile. Either profile therefore keeps that identity after a crash,
so work bound to the account keeps its owner. A different account is refused.

### Account management and diagnostics

Accounts with Advanced Data Protection need extra cookies that Apple issues only
when 'Access iCloud Data on the Web' is on and a trusted device approves. rclone
then reports `requestPCS` or Apple's `Missing PCS cookies`. The sign-in steps and
the connection keep only the tail of rclone's error output, in memory, to recognize
those words and explain the setting; that refusal is never retried automatically.

**Sign out of this computer...** removes the saved session, its identity links,
interrupted sign-in copies, the cached file contents, the key in the keyring, the
sign-in time, pending uploads, and the synced folder list with its state and
Backups. The local copies stay unless the user asks to move them to the Trash,
where unsynced edits remain recoverable; the iCloud Sync place leaves the sidebar
only when that empties the folder. iCloud is never changed. **Change Apple
account...** signs out the same way and then opens sign-in, because pending uploads
and synced folders belong to the account that created them. A connected or
reconnecting app disconnects first; if that fails, nothing is removed.

**Save diagnostic report...** writes a text file the user chooses. It lists the
distribution, desktop, app, GTK, libadwaita, and rclone versions, how the app was
installed, the app settings, the connection state, each synced folder as `Folder N`
with its state and counts, and upload counts by state. Quoted names in messages and
the home path are hidden.

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
ties that signal; the preparation worker starts none. The app calls the queue's
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
and releasing the unlocked session; quitting the app and the end of the desktop
session do the same before the app exits. Cloud mutations are never used for
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

## Synced folders

Files and Dolphin edit synced folders; the explorer mount stays read-only. Each
synced folder pairs an iCloud Drive folder, chosen inside the mount, with a new
private local folder under `iCloud Sync/` next to the mount point. The whole drive
and folders that overlap an existing pair, compared without letter case, are refused.
Pairs are bound to the profile identity that added them, like pending uploads.

The app runs one `rclone bisync` process at a time with the process guard, only
while it owns an unlocked connection; the app calls the folders' `tick` every
second for as long as it runs. A first run copies iCloud into the empty local
folder with `--resync-mode path2`, which copies but never deletes. Folders with no
files keep using such runs, because bisync refuses a normal run with an empty side.
Later runs compare size and modification time. A periodic local scan, every five
seconds or less often for large folders, starts a run after local changes; iCloud
is polled every two minutes and once after each reconnect.

Every run uses `--resilient --recover --max-lock 2m` and excludes editor lock and
temporary files. Conflicts use `--conflict-resolve newer`: the newer version keeps
the name, and the other is kept on both sides as `name.conflictN.ext`. bisync records
the winner only on its next run, so the app starts one more run whenever conflict
copies appear; deleting either version afterwards then propagates normally.

A local deletion moves the iCloud item to Recently Deleted, as rclone removes
iCloud items to the trash. A deletion or replacement arriving from iCloud moves the
local file into that run's private Backups folder through `--backup-dir1`; each
run's folder is named by its time in UTC.

Results map to states the window shows:

- bisync aborts without changes when more than half the files would be deleted or
  every file changed. Renames count as deletions, so renaming or editing a few
  files in a small folder also trips these ratios. When at most five items are
  deleted, or every item changed in a folder of at most five, the app continues
  once with `--force`: such changes stay recoverable from Recently Deleted and
  Backups. Larger changes show the count and side, the status shows
  **Review needed**, and **Review...** asks before **Continue** allows one run with
  `--force`.
- An empty or missing side, or a missing or untrusted sync history, also stops
  without changes. **Continue** recreates the iCloud folder if needed and runs a
  copy-only restore: where both sides differ, iCloud wins and Backups keep the
  local bytes.
- Other failures retry after a minute; local changes do not restart a failed run
  sooner.
- Names that differ only in letter case pause the folder before bisync runs,
  because iCloud Drive treats them as one name and one could replace the other.

Before each run, the app removes lock files from its private work folder: this
single-instance app runs bisync there sequentially, so any lock is left by an
interrupted run. Disconnect sends SIGINT so bisync can finish started transfers
and save its state, and kills it after 30 seconds. Disconnect, quitting the app, and
the end of the desktop session close the connection only once uploads and sync
have both stopped.

Like uploads, sync has no server transaction. iCloud writes cannot be made
conditional, so a change on another device that races a transfer of the same file
can still be replaced; ordinary edits elsewhere are detected before each run. Moves
and renames transfer files again: local ones upload under the new name and send the
old iCloud copies to Recently Deleted, while moves made elsewhere download again and
put the old local copies in Backups. Moved files count toward the deletion review.

## Packages

The `.deb` and `.rpm` are built for one architecture and contain the app binary,
`session.sh` under `/usr/share/icloud-on-linux/`, and the desktop entry. They bundle
no GTK, libadwaita, or rclone. The app finds its helper relative to its own
binary, so the same build runs from a checkout, the user installation, or a
package.

`dpkg-shlibdeps` and RPM's automatic requirements name the GTK 4, libadwaita, GLib,
and C library versions the binary links. Both formats also require FUSE 3 and
rclone 1.75.2 or later, and recommend the package that provides `secret-tool` for
automatic unlock: `libsecret-tools` on Debian and Ubuntu, `libsecret` on Fedora.
Fedora 45 provides every dependency. Debian 13 and Ubuntu 24.04 and 26.04 provide
GTK and libadwaita, but their rclone predates the iCloud backend, so the official
rclone package must be installed first. Debian 12 and Ubuntu 22.04 lack GTK 4.14
and libadwaita 1.5.

Packages are built and checked in disposable containers. Builds receive only the
packaged sources; checks receive tracked files, packages, and test programs.
Neither receives `.vm/` or `downloads/`. The build images are pinned by digest, so a
replaced tag cannot change a package, and the checks install rclone only after it
matches a checksum taken from the signed release list. There is no release or
signed repository yet.

## Mount and desktop integration

The user **iCloud on Linux** shortcut runs the native app. Its Connect action
uses the existing encrypted profile; Open uses the desktop file URL handler. The
installer hides the prepared lab shortcut for this user without changing the
system launcher or account configuration. The desktop file is named after the
application ID, `io.github.luiscuellar31.IcloudOnLinux`, so the desktop matches
the app's windows to it.

The app keeps running after its window closes when the desktop shows tray icons
(StatusNotifierItem, as on KDE and on Ubuntu's GNOME with its AppIndicator
extension). The icon's tooltip shows the status, and its menu offers **Show
window**, **Open iCloud Drive**, **Sync now**, and **Quit**. While the window is
hidden, GApplication notifications report a connection problem, a folder waiting for
a review, uploads waiting for a decision, and an expiring Apple session; clicking
one shows the window. Without tray icons, closing a working window asks whether to
keep syncing; opening the app again shows the hidden window. **Quit** asks first
while uploads, sync, or the connection are active. Quit, `SIGTERM`, and the end of
the session pause uploads and sync, unmount, and exit.

**Start at login** copies the installed desktop entry to the user's XDG autostart
folder with `--background`. The Python app's `icloud-drive.desktop` entry also
counts when it runs `~/.local/bin/icloud-on-linux` or `icloud-on-linux`, which this
app replaced, and turning the option off removes it. Started with `--background`,
the app stays in the tray only when **Unlock automatically on this computer** can
connect with the key saved in the Secret Service (GNOME Keyring or KWallet). It
shows the window when the keyring has no key or no tray icon appears within ten
seconds. That option is off by default. Turning it on asks for confirmation, and
the key is saved only after it unlocks the session; turning it off deletes it.
`secret-tool` receives the key through standard input, never as an argument.

The app's text is in English and Spanish. **Language** in the menu chooses
Automatic, which follows the desktop languages, English, or Español, and applies at
once: the window is built again in that language while the connection, uploads, and
sync keep running, and their messages switch when they next change. GTK's own text,
such as in its menus and tooltips, follows an explicit choice from the next start,
when the app sets `LANGUAGE` before GTK starts.

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
| Linux desktop | `~/.config/icloud-linux/settings.ini` | `0600` language, automatic unlock choice, and the time of the last in-app sign-in, in the format the Python app also reads. It holds no keys or tokens. |
| Linux desktop | Desktop keyring item `application=icloud-on-linux secret=local-key` | The local key, only while **Unlock automatically on this computer** is on; deleted when it is turned off. |
| Linux desktop | `~/.config/autostart/io.github.luiscuellar31.IcloudOnLinux.desktop`, or the equivalent under `XDG_CONFIG_HOME` | Present only while **Start at login** is on. |
| Linux desktop | `~/.config/icloud-linux/.signin-*/` | Private encrypted profile copy during sign-in, removed when the attempt ends. An abrupt exit can leave it behind; it is never used again. |
| Ubuntu | `~/iCloud` | Mount point, not a complete offline copy. |
| Ubuntu | rclone VFS cache, under the user cache directory | Contents of opened files; configuration encryption does not encrypt these copies. |
| Linux desktop | `~/.cache/icloud-on-linux/vfs/`, or the equivalent under `XDG_CACHE_HOME` | The native app's VFS cache, with the same contents and limits. |
| Linux desktop | `$XDG_RUNTIME_DIR/icloud-on-linux/session-*/` | Private copy of the encrypted profile while connected, removed on disconnect. The runtime directory is in memory and is cleared when the user logs out. |
| Linux desktop | `~/.cache/icloud-on-linux/uploads/`, or the equivalent under `XDG_CACHE_HOME` | Private unencrypted payload snapshots and a `0600` journal containing filenames, destinations, hashes, profile fingerprints, and state. Pending copies survive disconnect and restart. |
| Linux desktop | `~/iCloud Sync/` | Full local copies of synced folders, in `0700` folders. They are not encrypted and stay when syncing stops. |
| Linux desktop | `~/.cache/icloud-on-linux/sync/`, or the equivalent under `XDG_CACHE_HOME` | `0600` list of synced folders with their profile identities, bisync state per folder, and per-run Backups of local files replaced or deleted from iCloud. Losing the state stops sync until **Continue** rebuilds it by copying. |
| Linux desktop | `~/.local/bin/icloud-on-linux` and `~/.local/share/icloud-on-linux/` | User installation of the app and its helper; the desktop entry is under `~/.local/share/applications/`. |
| Host | `.vm/` | Disk, identifier, installation media, keys, and administration data for the local VM. Excluded from Git and the Docker build context. |
| Host | `downloads/` | Personal copies extracted from the container. Excluded from Git and the Docker build context. |
| Linux system | `/usr/bin/icloud-on-linux`, `/usr/share/icloud-on-linux/`, and `/usr/share/applications/io.github.luiscuellar31.IcloudOnLinux.desktop` | Files owned by the installed package. User data stays in the per-user locations above. |
| Host | `dist/` and `target/` | Built packages, binaries, and the package check programs. Excluded from Git. |

Docker and Ubuntu configurations are independent. The prepared lab received an
encrypted copy; credentials do not synchronize automatically between environments.
No real configuration belongs in the source tree. `.dockerignore` sends only the
Dockerfile and the session script needed by the image to the builder.

## Where behavior is checked

- [crates/core/tests](../crates/core/tests): the native session, including automatic
  reconnection, with local stand-ins for rclone and FUSE, and on Linux a real
  read-only FUSE mount of a fictional encrypted profile. [crates/core/tests/uploads.rs](../crates/core/tests/uploads.rs)
  runs the upload queue with the real rclone against local fictional remotes:
  verification, Keep both and Skip, interrupted uploads and crashes, a WebDAV
  outage, a changed profile, and cloud edits before the copy or during verification.
  [crates/core/tests/sync.rs](../crates/core/tests/sync.rs) runs real bisync between
  a private local folder and a fictional `icloud:` folder: the first copy, local and
  remote changes with Backups, a conflict, held mass deletions, renames in a small
  folder that continue without review, a killed run, a deleted iCloud folder, and
  lost sync history. [crates/core/src/places.rs](../crates/core/src/places.rs)
  checks the sidebar places in temporary GTK and Dolphin settings.
  [crates/core/src/settings.rs](../crates/core/src/settings.rs) and
  [crates/core/src/autostart.rs](../crates/core/src/autostart.rs) check
  `settings.ini` and the autostart entries, including the Python app's, in temporary
  folders. [crates/core/tests/keyring.rs](../crates/core/tests/keyring.rs) stores,
  finds, and clears a fictional key in gnome-keyring inside a private D-Bus session
  with a temporary home, and [crates/core/tests/translations.rs](../crates/core/tests/translations.rs)
  requires a Spanish entry with the same placeholders for every translated text.
- [crates/app/src/tray.rs](../crates/app/src/tray.rs): the tray icon and its menu
  against a stand-in StatusNotifierWatcher on a private `dbus-daemon`.
- [crates/app/tests/visible](../crates/app/tests/visible): the visible window on a
  Wayland or X11 display. While connected, it switches to Spanish and back; turns
  Start at login on and off in temporary folders; and closes the window to a
  stand-in tray, which keeps the app connected. It adds a synced folder with its
  sidebar place in temporary settings, continues its review of mass deletions, and
  stops syncing it; uploads files and decides a conflict with Keep both; disconnects
  during an upload and resumes it after reconnecting; and ends the app during an
  upload. Then, in a new window, it connects, reconnects on its own after the mount
  drops and stops waiting with Cancel after a second drop, opens the file manager,
  and disconnects.
- The unit tests of [crates/core/src/diagnostics.rs](../crates/core/src/diagnostics.rs)
  check that the report omits names, keys, and profile contents, and the visible
  check signs out, changes the account, and saves a report with fictional data and
  stand-ins for the keyring, the sidebar places, and the Trash.
  [crates/core/tests/icloud_sync.rs](../crates/core/tests/icloud_sync.rs) exercises
  synced folders against the real account in a new owned test folder, only when run
  with `--account`, or against a local stand-in with `--fictional`.
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
- [tests/check_packages.sh](../tests/check_packages.sh): package installation and
  the Rust checks against the installed helper in Debian, Ubuntu, and Fedora
  containers, including headless Weston and the installed app.
- [tests/check_wayland.sh](../tests/check_wayland.sh): the visible check under
  headless GNOME Shell and KWin Wayland compositors, with Nautilus or Dolphin.
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
