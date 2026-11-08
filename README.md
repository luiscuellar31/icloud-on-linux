<div align='center'>
  <h1>iCloud on Linux</h1>

  <p><strong>A native iCloud Drive client for the Linux desktop.</strong></p>
</div>

iCloud on Linux aims to make your iCloud Drive files part of your everyday Linux
workflow. The goal is to browse, manage, and sync them from GNOME Files and KDE
Dolphin, so you can use the iCloud storage you already have without leaving Linux.

The project is in early development. A native app, written in Rust with GTK4,
connects your saved session and opens iCloud Drive in Files on Ubuntu with GNOME,
using rclone to reach your account. It speaks English and Spanish and keeps
syncing from the tray after you close its window.

## Features

- Sign in with your Apple account and two-factor authentication.
- Browse iCloud Drive in GNOME Files or KDE Dolphin and open files with your usual apps.
- Download files when you open them and keep a local cache.
- Reuse your saved session, encrypted with a separate local key.
- Connect and disconnect from a native desktop window in English or Spanish, and
  sign in or renew the session there without a terminal.
- Keep syncing from a tray icon after you close the window, and get notified when
  something needs you.
- Optionally start at login and unlock through your desktop keyring, reconnect
  after a dropped connection, and get a reminder before the Apple session expires.
- Upload files with a visible queue, pause and resume, and conflict decisions.
- Create, edit, move, and delete files from Files or Dolphin in synced folders,
  with both versions kept on conflicts and mass deletions held for review.
- List and download files from a terminal through the Docker environment.

The `~/iCloud` mount is read-only. To edit, sync a folder to `~/iCloud Sync`, or
upload through the app queue and select Skip or Keep both when a different cloud
file already exists.

## Try it

### Ubuntu Desktop

The [Ubuntu Desktop guide](docs/UBUNTU_DESKTOP.md) walks through installing rclone,
signing in, and opening iCloud Drive as the folder `~/iCloud` in Files. The tested
desktop stacks are GNOME 46 and KDE Plasma 5.27 on Ubuntu 24.04, on Wayland; the
window is also checked on X11.

### Fedora, Debian, and Ubuntu packages

With Docker running, build the packages for your computer's architecture from a
checkout:

```sh
packaging/build.sh
```

On Fedora 45, install the `.rpm`; its dependencies, including rclone, come from the
Fedora repositories:

```sh
sudo dnf install ./dist/icloud-on-linux-*.rpm
```

On Debian 13, Ubuntu 24.04, or Ubuntu 26.04, first install the official rclone
1.75.2 package with its checksum, as in step 2 of the
[Ubuntu Desktop guide](docs/UBUNTU_DESKTOP.md); the distribution rclone lacks the
iCloud backend. Then install the `.deb`:

```sh
sudo apt install ./dist/icloud-on-linux_*.deb
```

Open **iCloud on Linux** from the applications menu and select **Sign in to iCloud**.
Packages are checked in containers without a full desktop session.

### Docker

With Docker running on macOS or Linux:

```sh
git clone https://github.com/luiscuellar31/icloud-on-linux.git
cd icloud-on-linux
./start.command
```

The launcher builds the environment and asks for a local key to protect your saved
session. On first use, it also asks for your Apple account, password, and 2FA code
in your terminal. The local key is separate from your Apple password.

Once connected, you can list and download files. Type `exit` to close the session;
your encrypted configuration stays in the Docker volume for the next run. See
the [Docker guide](docs/DOCKER.md) for the commands and download steps.

## Roadmap

- Edit files on demand in the mounted drive, without a full local copy.
- Publish release packages and test full Fedora and Debian desktop sessions.

## Documentation and contributions

- [Using iCloud Drive on Ubuntu](docs/UBUNTU_DESKTOP.md)
- [Using the Docker environment](docs/DOCKER.md)
- [Architecture and local data](docs/ARCHITECTURE.md)

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening
a pull request. Automated tests use local files and do not need an Apple account.

## License

iCloud on Linux is available under the [MIT License](LICENSE).
