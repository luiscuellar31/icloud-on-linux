Name:           icloud-on-linux
Version:        %{pkg_version}
Release:        1%{?dist}
Summary:        Native iCloud Drive client for the Linux desktop
License:        MIT
URL:            https://github.com/luiscuellar31/icloud-on-linux
Source0:        %{name}-%{version}.tar.gz
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gtk4-devel
BuildRequires:  libadwaita-devel
Requires:       rclone >= 1.75.2
Requires:       fuse3
# secret-tool, for Unlock automatically on this computer.
Recommends:     libsecret
# The stripped release binary carries no debug information to extract.
%global debug_package %{nil}

%description
Signs in to iCloud Drive through rclone and opens it read-only in the file
manager. The app is written in Rust with GTK 4 and libadwaita.

%prep
%autosetup

%build
# Build with the project's release profile. Fedora's Rust flags add debug information for a debuginfo
# package that this package does not ship, and need more memory than small build machines have.
env -u RUSTFLAGS cargo build --release --locked --package icloud-on-linux

%install
bash packaging/stage.sh %{buildroot} target/release/icloud-on-linux

%files
%license LICENSE
%{_bindir}/icloud-on-linux
%{_datadir}/icloud-on-linux/
%{_datadir}/applications/io.github.luiscuellar31.IcloudOnLinux.desktop
