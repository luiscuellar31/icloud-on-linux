#!/usr/bin/env bash
# Build the native app and install it for the current desktop user under ~/.local.
set -euo pipefail

if [[ "$(uname -s)" != Linux || "$(id -u)" == 0 ]]; then
    printf 'Run this installer as your Linux desktop user.\n' >&2
    exit 1
fi
for command in cargo rclone fusermount3; do
    command -v "$command" >/dev/null || { printf 'Install %s before installing the desktop app.\n' "$command" >&2; exit 1; }
done
root="$(cd "$(dirname "$0")/.." && pwd)"
id=io.github.luiscuellar31.IcloudOnLinux
cargo build --release --locked --manifest-path "$root/Cargo.toml" --package icloud-on-linux

# The app finds its helper at ../share/icloud-on-linux beside its own folder.
install -D -m 0755 "$root/target/release/icloud-on-linux" "$HOME/.local/bin/icloud-on-linux"
install -D -m 0755 "$root/scripts/session.sh" "$HOME/.local/share/icloud-on-linux/scripts/session.sh"
entry="$HOME/.local/share/applications/$id.desktop"
if [[ -L "$entry" ]]; then
    printf 'Refusing to replace a linked desktop shortcut.\n' >&2
    exit 1
fi
# Desktop entries quote Exec arguments: inside the quotes a backslash, double quote, backtick, or dollar sign
# takes a backslash, every backslash is then escaped again for the file, and percent signs are doubled.
launcher="$HOME/.local/bin/icloud-on-linux"
launcher="${launcher//\\/\\\\}"
for character in '"' '`' '$'; do launcher="${launcher//"$character"/\\$character}"; done
launcher="${launcher//\\/\\\\}"
launcher="${launcher//%/%%}"
mkdir -p "$(dirname "$entry")"
while IFS= read -r line; do
    [[ "$line" == Exec=icloud-on-linux ]] && line="Exec=\"$launcher\""
    printf '%s\n' "$line"
done < "$root/packaging/$id.desktop" > "$entry"
# The prepared lab adds an iCloud Drive shortcut with the same purpose; hide it for this user only.
legacy="$HOME/.local/share/applications/icloud-drive.desktop"
if [[ -e /usr/share/applications/icloud-drive.desktop || -e "$legacy" ]] && [[ ! -L "$legacy" ]]; then
    printf '[Desktop Entry]\nType=Application\nName=iCloud Drive\nHidden=true\n' > "$legacy"
fi
if command -v update-desktop-database >/dev/null; then
    update-desktop-database "$(dirname "$entry")"
fi
printf 'Installed iCloud on Linux for the current desktop user.\n'
printf 'Launch it from the application menu or run %s.\n' "$HOME/.local/bin/icloud-on-linux"
