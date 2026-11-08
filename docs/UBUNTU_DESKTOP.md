# iCloud Drive in the Ubuntu file manager

Open iCloud Drive as `~/iCloud` in Files (Nautilus), in read-only mode. This guide
uses rclone installed directly on Ubuntu Desktop.

## Use the prepared VM

The local **Ubuntu-iCloud** machine runs Ubuntu 24.04 LTS ARM64 with 4 GB RAM,
2 CPUs, and a 40 GB dynamically allocated disk. It has rclone 1.75.2, FUSE 3,
and an application shortcut for iCloud.

1. On macOS, double-click [open-ubuntu.command](../open-ubuntu.command). If the
   VM is already running, select it in VirtualBox and choose **Show**.
2. In Ubuntu, click **iCloud Drive** in the dock or find it in the applications.
3. Enter the **same local key** created for the Docker lab. The configuration
   was transferred encrypted, preserving the Docker original.
4. The launcher checks the connection and opens **iCloud** in Files. It also
   appears as **iCloud Drive** in the sidebar.

The key is entered in the Ubuntu window without echoing it. If Apple requires
another authorization, complete it there. The mount allows reads and downloads;
writes are disabled.

The mount stays active after Files closes. To disconnect it, run this in the
Ubuntu terminal:

```sh
fusermount3 -u "$HOME/iCloud"
```

Shut down the VM through the Ubuntu power menu. This lab signs in automatically
as `icloud`. Its Ubuntu password, independent of the Apple password and local
configuration key, is stored in the private `.vm/ubuntu-password` file on the Mac.
The `.vm/` directory contains the disk and administration access for this VM
and is excluded from Git and the Docker build context.

Checks covered the real FUSE mount and write rejection with temporary local
files. Real account checks also covered access from Files, the read-only mount
flags, and a downloaded file whose SHA-256 matched the copy obtained in Docker.

The remaining steps prepare another Ubuntu system from scratch. After installing
rclone, install the native app and connect there, or continue with the manual
foreground mount.

## Install the native app

After steps 1 and 2 below, install the build tools, the GTK files, and a current
Rust toolchain, then build and install the app from a checkout of this repository:

```sh
sudo apt install --yes build-essential pkg-config libgtk-4-dev libadwaita-1-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
scripts/install_desktop.sh
```

Run the installer as your desktop user. It builds the app with Cargo and installs
it, its session helper, and the **iCloud on Linux** shortcut under `~/.local/`,
preserving your encrypted configuration. Rerun it after updating the checkout.

Open **iCloud on Linux**, enter your local key, and select **Connect**. When the
status shows **Connected**, **Open iCloud Drive** opens `~/iCloud` in Files.
**Cancel** stops a connection attempt, and **Disconnect** in the menu unmounts the
folder and ends its client. Close files first if disconnecting reports a busy
folder. Closing the connected app asks whether to disconnect and close it. An
existing read-only mount from the terminal lab can be opened or disconnected
without entering a key.

## Upload files

The file manager folder stays read-only; uploads go through the app queue.

1. Connect inside the native app with your local key. If the app adopted an older
   terminal mount, disconnect it first and reconnect here to unlock uploads.
2. Expand **Uploads** and enter a folder relative to iCloud Drive in **Upload to**,
   such as `Documents/Uploads`. The default is `iCloud on Linux uploads`; missing
   folders are created.
3. Select **Add files...**, or **Upload files...** in the menu, and choose regular
   files. The app prepares private local copies and shows each row as Checking,
   Uploading, Verifying, or Completed.
4. For **Conflict**, select the row and choose **Skip** or **Keep both**. Keep both
   uses a new filename; there is no replacement action. The status shows
   **Review needed** until you decide.

**Pause uploads** retains pending copies. **Resume uploads** rechecks the cloud
before retrying paused or failed rows. Restarted queues stay paused until you
connect and resume them. Disconnect and closing the app pause an active upload
before closing its connection. Files in another saved profile stay paused.

Completed means a separate cloud download matched the local snapshot SHA-256.
Verification uses download bandwidth. New uploads become visible in the explorer
after its directory cache refreshes, normally within ten seconds on the app mount.
The app does not yet synchronize edits made inside the explorer folder.

Pending payloads and opened files are stored unencrypted in your private user
cache. Keep enough free disk space and preserve that cache when recovering a
pending upload. Configuration encryption protects the session, not file contents.

## 1. Open Ubuntu Desktop

If you already have an Ubuntu Desktop VM, continue with step 2. The iCloud
configuration will use a dedicated directory separate from other rclone connections.

To create a dedicated VM on an ARM64 Mac:

1. Get the **Ubuntu Desktop LTS, ARM 64-bit** ISO from the
   [official download page](https://ubuntu.com/download/desktop).
2. In VirtualBox, create a machine, for example **Ubuntu-iCloud**.
3. Select the ISO and Ubuntu ARM64. Use manual installation; VirtualBox does not
   provide unattended installation for ARM guests.
4. The tested VM uses Ubuntu 24.04 LTS with 4 GB RAM and 2 CPUs. Leave enough
   resources for macOS and keep VirtualBox resource indicators in the green range.
   Check the requirements before choosing a different Ubuntu version.
5. Create a new 40 GB dynamically allocated disk. Preserve other VM disks.
6. Start that machine, install Ubuntu on its new disk, and enter the desktop.

## 2. Install FUSE and the tested rclone version

Run these commands in the **Ubuntu desktop terminal**. The packages below are
for Ubuntu ARM64 on the ARM64 Mac.

```sh
sudo apt update
sudo apt install --yes fuse3 curl ca-certificates

icloud_package_dir="$(mktemp -d)"
cd "$icloud_package_dir"
curl --fail --location --remote-name https://downloads.rclone.org/v1.75.2/rclone-v1.75.2-linux-arm64.deb
curl --fail --location --remote-name https://downloads.rclone.org/v1.75.2/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS && sudo apt install --yes ./rclone-v1.75.2-linux-arm64.deb
rclone version
```

Check that the package passes checksum verification and `rclone version` shows
`v1.75.2` and `arm64`. Choose the package for the target architecture when using
another Linux machine.

## 3. Authorize iCloud in Ubuntu

The Docker volume stays in Docker. Create a separate encrypted configuration
inside Ubuntu for this manual setup:

```sh
umask 077
mkdir -p "$HOME/.config/icloud-linux"
rclone --config "$HOME/.config/icloud-linux/rclone.conf" config
```

In the assistant:

1. Select `s` and protect the configuration with a local key.
2. Return to the menu and select `n`.
3. Connection name: `icloud`.
4. Storage type: `iclouddrive`.
5. Service: `drive`.
6. Enter your Apple account and password, then complete 2FA.
7. Keep advanced defaults, save, and quit with `q`.

Enter credentials in your terminal. The assistant summary may contain cookies
and tokens; do not share it. Then check access:

```sh
rclone --config "$HOME/.config/icloud-linux/rclone.conf" lsf icloud:
```

## 4. Mount the folder

In the same Ubuntu terminal:

```sh
mkdir -p "$HOME/iCloud"
rclone --config "$HOME/.config/icloud-linux/rclone.conf" mount \
  icloud: "$HOME/iCloud" --read-only --vfs-cache-mode full
```

Enter the local configuration key when prompted. The terminal stays occupied
maintaining the mount; leave it open during the test.

## 5. Open it in Files

Open **Files**, go to your home folder, and open **iCloud**. Its files should
match the Docker listings. Open a sample image to check that it downloads when
opened from the file manager.

You can bookmark the folder in the Files sidebar. Browsing and reading are
available; `--read-only` disables writes.

To finish, return to the mount terminal and press **Ctrl+C**. The mount stops;
the saved configuration lets you repeat step 4. This manual setup does not
configure automatic mounting or a complete offline copy.

## References

- [Lab architecture](ARCHITECTURE.md)
- [Checks and development rules](../CONTRIBUTING.md)
- [Ubuntu Desktop and requirements](https://ubuntu.com/download/desktop)
- [Ubuntu in VirtualBox](https://ubuntu.com/tutorials/how-to-run-ubuntu-desktop-on-a-virtual-machine-using-virtualbox)
- [VirtualBox ARM limitations](https://docs.oracle.com/en/virtualization/virtualbox/7.2/user/installation.html)
- [Official rclone 1.75.2 packages](https://downloads.rclone.org/v1.75.2/)
- [rclone mounting](https://rclone.org/commands/rclone_mount/)
- [iCloud authentication](https://rclone.org/iclouddrive/)
