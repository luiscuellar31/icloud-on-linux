#!/usr/bin/env bash
# Run the visible window check under a headless GNOME or KDE Wayland compositor with fictional
# profiles. Each run uses a private D-Bus session and a temporary home; run it as a separate
# test user, not from a daily desktop account.
# Usage: tests/check_wayland.sh gnome|kde VISIBLE_CHECK
# Build VISIBLE_CHECK with: cargo test --package icloud-on-linux --test visible --no-run
set -euo pipefail

case "${1:-}" in
    gnome) desktop=GNOME manager=nautilus handler=org.gnome.Nautilus.desktop ;;
    kde) desktop=KDE manager=dolphin handler=org.kde.dolphin.desktop ;;
    *) printf 'Usage: %s gnome|kde VISIBLE_CHECK\n' "$0" >&2; exit 1 ;;
esac
program="${2:-}"
[[ -x "$program" ]] || { printf 'Pass the visible check program built by Cargo.\n' >&2; exit 1; }
program="$(cd "$(dirname "$program")" && pwd)/$(basename "$program")"
home="$(mktemp -d)"
cleanup() {
    # Stop everything this run started, including D-Bus activated services, which all carry its HOME.
    for environ in /proc/[0-9]*/environ; do
        if grep -qzxF "HOME=$home" "$environ" 2>/dev/null; then
            kill "$(basename "$(dirname "$environ")")" 2>/dev/null || true
        fi
    done
    sleep 1
    # A failed check can leave a fixture mount open in a file manager; never delete through a mount.
    findmnt -rn -o TARGET | grep -F "$home/" | while read -r target; do fusermount3 -uz "$target" || true; done || true
    if findmnt -rn -o TARGET | grep -qF "$home/"; then
        printf 'Kept %s because a fixture mount is still active.\n' "$home" >&2
    else
        rm -rf "$home" || printf 'Could not remove the temporary home %s.\n' "$home" >&2
    fi
}
trap cleanup EXIT
install -d -m 0700 "$home/runtime" "$home/tmp"

# No DISPLAY is passed, so the app and the file manager can only use the Wayland compositor.
env -i PATH=/usr/local/bin:/usr/bin:/bin HOME="$home" TMPDIR="$home/tmp" USER="$(id -un)" LANG=C.UTF-8 \
    XDG_RUNTIME_DIR="$home/runtime" XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP="$desktop" \
    KDE_SESSION_VERSION=5 WAYLAND_DISPLAY=wayland-check \
    dbus-run-session -- bash -euo pipefail -c '
        # The temporary home has no folder handler; pick the desktop file manager explicitly.
        xdg-mime default "$3" inode/directory
        if [[ "$XDG_CURRENT_DESKTOP" == GNOME ]]; then
            gnome-shell --headless --wayland --no-x11 --virtual-monitor 1280x800 \
                --wayland-display wayland-check > "$HOME/compositor.log" 2>&1 &
        else
            kwin_wayland --virtual --width 1280 --height 800 --socket wayland-check > "$HOME/compositor.log" 2>&1 &
        fi
        compositor=$! manager=$2
        # The file manager carries the HOME of this run, so the outer cleanup stops it with the other processes.
        stop() { kill "$compositor" 2>/dev/null || true; wait "$compositor" || true; }
        trap stop EXIT
        for _ in $(seq 300); do [[ -S "$XDG_RUNTIME_DIR/wayland-check" ]] && break; sleep 0.1; done
        [[ -S "$XDG_RUNTIME_DIR/wayland-check" ]] || { cat "$HOME/compositor.log"; exit 1; }
        "$1" --expect-file-manager "$manager"
        printf "PASS: visible checks ran on %s Wayland with %s.\n" "$XDG_CURRENT_DESKTOP" "$manager"' \
    check "$program" "$manager" "$handler"
