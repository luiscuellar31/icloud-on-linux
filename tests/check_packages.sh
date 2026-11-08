#!/usr/bin/env bash
# Install the built packages in disposable containers and check them with fictional profiles: the Rust tests
# with the installed helper, including a real FUSE mount and the sign-in steps, the visible window under
# headless Wayland, and the installed app.
# Usage: tests/check_packages.sh [IMAGE...]   (run packaging/build.sh first)
set -euo pipefail

if [[ "${1:-}" != --inside ]]; then
    root="$(cd "$(dirname "$0")/.." && pwd)"
    (( $# )) || set -- debian:13 ubuntu:24.04 ubuntu:26.04 fedora:45
    cd "$root"
    compgen -G 'dist/*.deb' >/dev/null && compgen -G 'dist/*.rpm' >/dev/null ||
        { printf 'Build the packages first with packaging/build.sh.\n' >&2; exit 1; }
    # Only tracked sources enter the containers; .vm/ and downloads/ stay here.
    sources() { git ls-files -z --cached --others --exclude-standard; }
    # The test programs are built once, in the image that builds the .deb, and the same programs then run
    # on every distribution. Their paths to the repository are fixed at /source.
    # The test programs are built in the same image as the .deb.
    rust_image="$(sed -n 's/^rust_image=//p' packaging/build.sh)"
    rm -rf target/package-checks
    mkdir -p target/package-checks
    sources | COPYFILE_DISABLE=1 tar --null -T - -cf - |
        docker run --interactive --rm --env CARGO_BUILD_JOBS --env CARGO_PROFILE_DEV_DEBUG=false \
            --env CARGO_PROFILE_DEV_STRIP=debuginfo "$rust_image" bash -euo pipefail -c '
            mkdir /source && tar --warning=no-unknown-keyword -xf - -C /source
            export DEBIAN_FRONTEND=noninteractive
            apt-get update >/dev/null
            apt-get install --yes --no-install-recommends pkg-config libgtk-4-dev libadwaita-1-dev >/dev/null
            # Keep the test programs: the artifacts whose build profile, not only their target, is a test.
            cargo test --locked --workspace --no-run --message-format=json --manifest-path /source/Cargo.toml |
                grep -E "\"profile\":\{[^}]*\"test\":true" | grep -o "\"executable\":\"[^\"]*\"" |
                cut -d "\"" -f 4 > /tests
            install -d /checks
            xargs -a /tests install -m 0755 -t /checks
            tar -C /checks -cf - .' | tar -xf - -C target/package-checks
    for image in "$@"; do
        printf '\n== %s\n' "$image"
        { sources; printf '%s\0' dist/*.deb dist/*.rpm target/package-checks/*; } |
            COPYFILE_DISABLE=1 tar --null -T - -cf - |
            docker run --interactive --rm --device /dev/fuse --cap-add SYS_ADMIN \
                --security-opt apparmor=unconfined "$image" bash -euo pipefail -c '
                    mkdir /source && tar --warning=no-unknown-keyword -xf - -C /source
                    exec bash /source/tests/check_packages.sh --inside'
    done
    exit 0
fi

# Inside a disposable container, as root.
cd /source
if command -v apt-get >/dev/null; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update >/dev/null
    apt-get install --yes --no-install-recommends curl ca-certificates >/dev/null
    arch="$(dpkg --print-architecture)"
    # Checksums from the signed SHA256SUMS of rclone 1.75.2, verified with rclone's release key
    # FBF737ECE9F8AB18604BD2AC93935E02FF3B54FA; a file from the download server alone could be replaced with it.
    case "$arch" in
        arm64) checksum=b8ba161d5c837206fad79ff1aabf0a81aa02a5fb164cdbc0ff5db4160aecd278 ;;
        amd64) checksum=efbfe852181f7191eb3c9043ed1ab49c9b2d0ba045c61c926a5da111c939ec5c ;;
        *) printf 'No verified rclone package for %s.\n' "$arch" >&2; exit 1 ;;
    esac
    download="$(mktemp -d)"
    (cd "$download" && curl --fail --silent --show-error --location \
        --remote-name "https://downloads.rclone.org/v1.75.2/rclone-v1.75.2-linux-$arch.deb" &&
        printf '%s  %s\n' "$checksum" "rclone-v1.75.2-linux-$arch.deb" | sha256sum --check --quiet)
    # Recommends stay enabled, as on a desktop.
    apt-get install --yes "$download/rclone-v1.75.2-linux-$arch.deb" ./dist/icloud-on-linux_*_"$arch".deb \
        weston desktop-file-utils dbus >/dev/null
    libraries="$(dpkg-query --show --showformat '${Package} ${Version}, ' libgtk-4-1 libadwaita-1-0)"
else
    dnf install --assumeyes ./dist/icloud-on-linux-*."$(uname -m)".rpm weston desktop-file-utils shadow-utils \
        util-linux dbus-daemon < /dev/null >/dev/null
    libraries="$(rpm --query --queryformat '%{NAME} %{VERSION}, ' gtk4 libadwaita)"
fi
desktop-file-validate /usr/share/applications/io.github.luiscuellar31.IcloudOnLinux.desktop
. /etc/os-release
printf 'PASS: package installed on %s with %s, %s.\n' "$PRETTY_NAME" "$(rclone version | head -1)" "${libraries%, }"

# Run the checks as an unprivileged user, with the installed helper in place of the repository copy.
useradd --create-home checker
uid="$(id -u checker)"
install -d -o checker -g checker -m 0700 "/run/user/$uid"
ln -sf /usr/share/icloud-on-linux/scripts/session.sh scripts/session.sh
check() { runuser -u checker -- env -C /home/checker HOME=/home/checker XDG_RUNTIME_DIR="/run/user/$uid" "$@"; }
for program in target/package-checks/*; do
    [[ "$(basename "$program")" == visible-* ]] && continue
    check "/source/$program" > /tmp/tests.log 2>&1 || { cat /tmp/tests.log; exit 1; }
    cat /tmp/tests.log >> /tmp/all-tests.log
done
printf 'PASS: Rust tests with the installed helper: %s passed.\n' \
    "$(awk '/^test result: ok/ { passed += $4 } END { print passed }' /tmp/all-tests.log)"

check weston --backend=headless --socket=wayland-check --idle-time=0 > /tmp/weston.log 2>&1 &
for _ in $(seq 100); do [[ -S "/run/user/$uid/wayland-check" ]] && break; sleep 0.1; done
wayland=(env WAYLAND_DISPLAY=wayland-check)

# The containers have no file manager. A stand-in handles folders, so Open must start it.
printf '#!/bin/sh\nsleep 60\n' > /usr/local/bin/check-files
chmod 0755 /usr/local/bin/check-files
check mkdir -p /home/checker/.local/share/applications /home/checker/.config
printf '[Desktop Entry]\nType=Application\nName=Check files\nExec=check-files %%u\nMimeType=inode/directory;\nNoDisplay=true\n' |
    check tee /home/checker/.local/share/applications/check-files.desktop >/dev/null
printf '[Default Applications]\ninode/directory=check-files.desktop\n' | check tee /home/checker/.config/mimeapps.list >/dev/null
# The tray stand-in needs a session bus, as on a desktop.
check "${wayland[@]}" dbus-run-session -- /source/target/package-checks/visible-* --expect-file-manager check-files

# The installed app must keep running on Wayland and close on SIGTERM. The exec keeps the
# recorded PID, so the signal reaches the app rather than a wrapper.
check "${wayland[@]}" sh -c 'echo $$ > /tmp/app.pid; exec icloud-on-linux' > /tmp/app.log 2>&1 &
launcher=$!
sleep 5
kill -0 "$(cat /tmp/app.pid)" || { cat /tmp/app.log; exit 1; }
kill -TERM "$(cat /tmp/app.pid)"
wait "$launcher" || { cat /tmp/app.log; exit 1; }
printf 'PASS: the installed app ran on Wayland and closed cleanly.\n'
