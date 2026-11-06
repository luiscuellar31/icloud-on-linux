#!/usr/bin/env bash
set +x
set -euo pipefail
umask 077

if [[ "$(uname -s)" != Linux || "$EUID" == 0 ]]; then
    printf 'Open this shortcut as your Linux desktop user.\n' >&2
    exit 1
fi

if ! command -v fusermount3 >/dev/null || ! command -v rclone >/dev/null; then
    printf 'rclone or FUSE 3 is missing. See the lab guide.\n' >&2
    exit 1
fi

if mountpoint -q "$HOME/iCloud"; then
    exec gio open "$HOME/iCloud"
fi

export RCLONE_CONFIG="$HOME/.config/icloud-linux/rclone.conf"
mkdir -p "$(dirname "$RCLONE_CONFIG")"
if /usr/local/bin/icloud-session mount; then
    exit 0
else
    session_status=$?
    printf '\nCould not open iCloud. The error is shown above.\n'
    IFS= read -r -p 'Press Enter to close this window. ' || true
    exit "$session_status"
fi
