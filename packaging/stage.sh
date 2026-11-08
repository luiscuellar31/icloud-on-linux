#!/usr/bin/env bash
# Install the app files into a package root shared by the .deb and .rpm builds.
set -euo pipefail

root="${1:?Usage: stage.sh PACKAGE_ROOT BINARY}"
binary="${2:?Usage: stage.sh PACKAGE_ROOT BINARY}"
source="$(cd "$(dirname "$0")/.." && pwd)"
id=io.github.luiscuellar31.IcloudOnLinux

# The app finds its helper at ../share/icloud-on-linux beside its own folder.
install -D -m 0755 "$binary" "$root/usr/bin/icloud-on-linux"
install -D -m 0755 "$source/scripts/session.sh" "$root/usr/share/icloud-on-linux/scripts/session.sh"
install -D -m 0644 "$source/packaging/$id.desktop" "$root/usr/share/applications/$id.desktop"
