# iCloud on Linux

A prototype for accessing iCloud Drive from Linux and opening its files in the
file manager. It uses rclone 1.75.2 and provides a Docker terminal lab and a
**read-only** FUSE mount on Ubuntu 24.04 with GNOME/Nautilus.

The lab has verified sign-in with 2FA, reuse of the encrypted session, listings,
and downloads. Ubuntu checks also covered access from Files, the read-only mount,
and the integrity of a downloaded file.

## Open the lab

For the local **Ubuntu-iCloud** VM already prepared on this machine, open
[open-ubuntu.command](open-ubuntu.command) from macOS. In Ubuntu, launch
**iCloud Drive** and enter your local key to open `~/iCloud` in Files.
The launcher needs the private configuration in `.vm/`; cloning this repository
does not create the machine. The [Ubuntu Desktop guide](docs/UBUNTU_DESKTOP.md)
covers its use and manual setup on another Ubuntu system.

To explore and download files from a terminal, start Docker and open
[start.command](start.command), or run this from the repository root:

```sh
./start.command
```

The assistant asks for a local key to protect the saved session. On first use,
it also requests the Apple account and 2FA in your terminal. Listing, download,
and file extraction commands are in the [Docker guide](docs/DOCKER.md).

## Current scope

The integration supports browsing and opening files; uploads, edits, and deletions
are disabled on the mount. Caching opened files does not keep all of iCloud
available offline. The mount stays active when Files closes; the Ubuntu guide
explains how to unmount it.

The next functional work is to test writes with disposable data and an agreed
scope, including reconnection and conflicts. A dedicated native interface,
installation packages, and Fedora or Debian tests with KDE/Dolphin remain pending.

## Documentation and development

| Document | Contents |
| --- | --- |
| [AGENTS.md](AGENTS.md) | Brief rules for working in the repository. |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Change workflow, checks, data protection, and commit conventions. |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Responsibilities, authentication, mounting, and persistence. |
| [docs/DOCKER.md](docs/DOCKER.md) | Sign-in, listings, and downloads from a terminal. |
| [docs/UBUNTU_DESKTOP.md](docs/UBUNTU_DESKTOP.md) | VM usage and manual setup on Ubuntu Desktop. |

Automated checks use local data and do not need an Apple account. Credentials,
the VM, and personal downloads stay outside Git.

## License

[MIT](LICENSE).
