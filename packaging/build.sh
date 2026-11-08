#!/usr/bin/env bash
# Build the .deb and .rpm packages with native tools in disposable containers, for the host architecture.
# Debian 13 packages Rust 1.85, older than the GTK bindings need, so the .deb is built in the official
# Rust image based on Debian 13 with the oldest supported compiler; Fedora uses its own Rust.
set -euo pipefail

version=0.1.0
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$root/dist"

# Containers receive only the packaged sources on stdin, never the checkout with .vm/ or
# downloads/, and return the finished package as a tar on stdout. Logs go to stderr.
sources() {
    COPYFILE_DISABLE=1 tar -C "$root" -cf - Cargo.toml Cargo.lock rustfmt.toml crates scripts/session.sh packaging LICENSE
}

sources | docker run --interactive --rm --env version="$version" --env CARGO_BUILD_JOBS rust:1.92-slim-trixie bash -euo pipefail -c '
    mkdir /source && tar --warning=no-unknown-keyword -xf - -C /source
    export DEBIAN_FRONTEND=noninteractive
    apt-get update >/dev/null
    apt-get install --yes --no-install-recommends pkg-config dpkg-dev libgtk-4-dev libadwaita-1-dev >/dev/null
    cargo build --release --locked --manifest-path /source/Cargo.toml --package icloud-on-linux >&2
    # dpkg-shlibdeps reads the libraries the binary links and names the packages that provide them.
    mkdir -p /build/debian
    printf "Source: icloud-on-linux\n\nPackage: icloud-on-linux\nArchitecture: any\n" > /build/debian/control
    stage=/build/debian/icloud-on-linux
    install -d -m 0755 "$stage"
    bash /source/packaging/stage.sh "$stage" /source/target/release/icloud-on-linux
    install -D -m 0644 /source/LICENSE "$stage/usr/share/doc/icloud-on-linux/copyright"
    shlibs="$(cd /build && dpkg-shlibdeps -O "$stage/usr/bin/icloud-on-linux" | sed -n "s/^shlibs:Depends=//p")"
    arch="$(dpkg --print-architecture)"
    install -d "$stage/DEBIAN"
    sed -e "s/@VERSION@/$version/" -e "s/@ARCH@/$arch/" -e "s/@SHLIBS@/$shlibs/" /source/packaging/control \
        > "$stage/DEBIAN/control"
    echo "Installed-Size: $(du -sk --exclude=DEBIAN "$stage" | cut -f1)" >> "$stage/DEBIAN/control"
    mkdir /out
    dpkg-deb --root-owner-group --build "$stage" "/out/icloud-on-linux_${version}_${arch}.deb" >&2
    tar -C /out -cf - .' | tar -xf - -C "$root/dist"

sources | docker run --interactive --rm --env version="$version" --env CARGO_BUILD_JOBS fedora:45 bash -euo pipefail -c '
    mkdir /source && tar --warning=no-unknown-keyword -xf - -C /source
    dnf install --assumeyes --setopt=install_weak_deps=False rpm-build cargo rust gtk4-devel libadwaita-devel \
        < /dev/null >&2
    top="$(mktemp -d)"
    mkdir "$top/SOURCES"
    tar -C /source --transform "s,^,icloud-on-linux-$version/," \
        -czf "$top/SOURCES/icloud-on-linux-$version.tar.gz" Cargo.toml Cargo.lock rustfmt.toml crates scripts packaging LICENSE
    rpmbuild -bb --define "_topdir $top" --define "pkg_version $version" /source/packaging/icloud-on-linux.spec >&2
    mkdir /out && cp "$top"/RPMS/*/*.rpm /out
    tar -C /out -cf - .' | tar -xf - -C "$root/dist"

ls -1 "$root/dist"
