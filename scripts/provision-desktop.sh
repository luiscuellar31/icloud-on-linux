#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Linux || "$EUID" != 0 ]]; then
    printf 'Run this setup as root inside the dedicated VM.\n' >&2
    exit 1
fi

seed_mount=/mnt/icloud-seed
mkdir -p "$seed_mount"
if ! mountpoint -q "$seed_mount"; then
    mount -o ro /dev/disk/by-label/cidata "$seed_mount"
fi
trap 'cd /; umount "$seed_mount"' EXIT
package_files=("$seed_mount"/rclone*.deb)
[[ "${#package_files[@]}" == 1 && -f "${package_files[0]}" ]]
read -r package_checksum _ < "$seed_mount/rclone.sha256"
printf '%s  %s\n' "$package_checksum" "${package_files[0]}" | sha256sum --check
DEBIAN_FRONTEND=noninteractive apt-get install --yes "${package_files[0]}"
cd "$seed_mount"
install -m 0755 scripts/session.sh /usr/local/bin/icloud-session
install -m 0755 scripts/linux-start.sh /usr/local/bin/icloud-linux-start
install -m 0755 tests/check-linux-mount.sh /usr/local/bin/icloud-check-mount

cat > /usr/share/applications/icloud-drive.desktop <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=iCloud Drive
Comment=Open your iCloud folder in read-only mode
Exec=/usr/local/bin/icloud-linux-start
Icon=folder-remote
Terminal=true
Categories=Network;FileTransfer;
DESKTOP

cat > /usr/share/glib-2.0/schemas/90-icloud-lab.gschema.override <<'SCHEMA'
[org.gnome.shell:ubuntu]
favorite-apps=['org.gnome.Nautilus.desktop', 'icloud-drive.desktop', 'org.gnome.Terminal.desktop']
SCHEMA
glib-compile-schemas /usr/share/glib-2.0/schemas

cat > /etc/gdm3/custom.conf <<'GDM'
[daemon]
WaylandEnable=false
AutomaticLoginEnable=true
AutomaticLogin=icloud
GDM

install -d -o icloud -g icloud -m 0700 \
    /home/icloud/.config /home/icloud/.config/icloud-linux /home/icloud/.config/gtk-3.0
install -o icloud -g icloud -m 0600 /dev/null /home/icloud/.config/gnome-initial-setup-done
install -d -o icloud -g icloud /home/icloud/iCloud
printf 'file:///home/icloud/iCloud iCloud Drive\n' > /home/icloud/.config/gtk-3.0/bookmarks
chown icloud:icloud /home/icloud/.config/gtk-3.0/bookmarks
chmod 0600 /home/icloud/.config/gtk-3.0/bookmarks
systemctl enable NetworkManager-wait-online.service
systemctl disable systemd-networkd-wait-online.service
systemctl set-default graphical.target
install -d /var/lib/icloud-lab
touch /var/lib/icloud-lab/desktop-ready
