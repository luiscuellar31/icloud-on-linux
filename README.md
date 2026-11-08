<div align='center'>
  <h1>iCloud on Linux</h1>

  <p><strong>A native iCloud Drive client for the Linux desktop.</strong></p>
</div>

iCloud on Linux aims to make your iCloud Drive files part of your everyday Linux
workflow. The goal is to browse, manage, and sync them from GNOME Files and KDE
Dolphin, so you can use the iCloud storage you already have without leaving Linux.

The project is in early development. A native app, written in Rust with GTK4,
connects your saved session and opens iCloud Drive in Files on Ubuntu with GNOME,
using rclone to reach your account.

## Features

- Sign in with your Apple account and two-factor authentication.
- Browse iCloud Drive in GNOME Files and open files with your usual apps.
- Download files when you open them and keep a local cache.
- Reuse your saved session, encrypted with a separate local key.
- Connect and disconnect from a native desktop window.
- List and download files from a terminal through the Docker environment.

The current file manager mount is read-only. Uploads, editing, synchronization,
and KDE support are planned.

## Try it

### Ubuntu Desktop

The [Ubuntu Desktop guide](docs/UBUNTU_DESKTOP.md) walks through installing rclone,
signing in, and opening iCloud Drive as the folder `~/iCloud` in Files. The tested
setup is Ubuntu 24.04 with GNOME.

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

- Upload, edit, move, and delete files from the file manager.
- Keep folders in sync and handle conflicts and interrupted connections.
- Bring account setup into the native app.
- Package and test the app for Ubuntu, Fedora, and Debian with GNOME and KDE.

## Documentation and contributions

- [Using iCloud Drive on Ubuntu](docs/UBUNTU_DESKTOP.md)
- [Using the Docker environment](docs/DOCKER.md)
- [Architecture and local data](docs/ARCHITECTURE.md)

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening
a pull request. Automated tests use local files and do not need an Apple account.

## License

iCloud on Linux is available under the [MIT License](LICENSE).
