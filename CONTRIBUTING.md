# Contributing

This project maintains an iCloud Drive access lab built around rclone. The
[architecture map](docs/ARCHITECTURE.md) describes its components; the
[Docker](docs/DOCKER.md) and [Ubuntu Desktop](docs/UBUNTU_DESKTOP.md) guides
explain how to use them.

## Make a change

1. Find the component that owns the behavior in the architecture map. Read its
   code and tests before editing.
2. Address one need with a focused change. Follow existing patterns and avoid
   cleanup, dependencies, or layers the task does not need.
3. Update tests where the behavior is owned. Cover errors, existing
   configurations, and credential handling when relevant.
4. Review the architecture for every change. Update it alongside changes to
   ownership, flows, persistence, or documented paths; do not edit it artificially
   when it remains accurate. Update guides whose behavior or commands change.
5. Run the relevant checks and review the full diff. Describe the resulting
   behavior, the checks performed, and their limits when handing over the change.

The initial branch is `main`. There is no release workflow or additional
integration branch yet. Creating commits or publishing changes requires an
explicit request from the maintainer; preparing a change does not authorize either.

## Engineering rules

- Prefer clear names and focused responsibilities over fewer lines. Reuse the
  existing scripts and tools; do not add a dependency or test framework for one use.
- Keep shared authentication in `scripts/session.sh`. Launchers prepare their
  environment and delegate to it rather than maintaining separate copies.
- Comments briefly explain a reason or constraint and describe the current code.
  A regression test may describe the failure it guards against.
- Write project content in English, including identifiers, comments, documentation,
  messages, and commits. Use uppercase Markdown filenames. Use single quotation
  marks in prose when needed and backticks for code references. Avoid double and
  typographic quotation marks in prose; preserve quoting required by code syntax.
- Verify external tool options against the code, installed version, or official
  documentation before changing them. Keep the tested rclone version consistent
  across Docker, Ubuntu, the packages, and the guides.
- Documentation must have lasting value. Extend an existing document before
  adding another; keep session logs and temporary reports outside the repository.

## Test data and environment

Automated tests use local files and fictional credentials. Checks against the
real account run in the user terminal without copying credentials into arguments,
logs, or documentation. Do not print rclone summaries that might contain cookies
or tokens, or dump process environments.

`.gitignore` excludes `.vm/`, `downloads/`, `rclone.conf` and `.env` files, and local
caches. Do not force-add them, even when a configuration is encrypted. Before
creating a requested commit, review the staged files and their contents to ensure
they contain no account data or personal files.

The mount stays read-only. Write tests require an explicit scope and disposable
data. Provisioning changes the dedicated VM desktop and services; it is not an
installer for a daily-use machine. Preserve other VMs, disks, and Docker volumes.

## Checks

From the repository root, for launcher and session changes:

```sh
python3 -m unittest discover -s tests -v
bash -n start.command open-ubuntu.command scripts/*.sh tests/check-linux-mount.sh
```

The Python tests replace Docker, rclone, and VirtualBox with local stand-ins. They
need no Apple account and do not start a VM. No linter is configured for them.

The native app is written in Rust, in the Cargo workspace under `crates/`. Use a
current stable toolchain from rustup; each crate declares the oldest Rust version
it supports. For changes there, run from the repository root:

```sh
cargo fmt --check
cargo clippy --all-targets
cargo test
```

Run the tests on Linux as well: process protection, mounts, uploads, and synced
folders depend on it. There, the upload queue runs the real rclone against local
fictional remotes: verification by download, Keep both and Skip, interrupted and
recovered uploads, a backend outage, a changed profile, cloud edits during an
upload, and crashes. Synced folders run real bisync between a private local folder
and a fictional `icloud:` folder: the first copy, automatic local changes, remote
changes with Backups, a conflict, held mass deletions, a killed run, a deleted
iCloud folder, and lost sync history.

Keep `unsafe` code out of the crates except where a system call requires it, and
explain each such block in a `SAFETY` comment.

When `WAYLAND_DISPLAY` or `DISPLAY` is set, as in a desktop session, `cargo test`
also runs the visible window check with a fictional encrypted profile. It syncs a
folder, continues its review of mass deletions, and stops syncing it; uploads files
and decides a conflict; disconnects during an upload and resumes it after
reconnecting; and ends the app during an upload. Then, in a new window, it connects
and disconnects. To also click Open and wait for the file manager, run:

```sh
cargo test --package icloud-on-linux --test visible -- --expect-file-manager nautilus
```

Use `dolphin` on KDE.

For native sign-in changes, run the tests on Linux. `crates/core/tests/sign_in.rs`
answers the real rclone iCloud setup questions with fictional values in private
profiles. It checks that the password is saved obscured inside the encrypted
copy, that renewal reads the saved account and refuses another one, and that the
copy is removed. It stops before the answer that would contact Apple; a real
sign-in requires the maintainer's account.

For packaging or distribution changes, build the `.deb` and `.rpm` with native
tools in disposable containers, then install and check them:

```sh
packaging/build.sh
tests/check_packages.sh
```

Packages land in the ignored `dist/` directory, built for the host architecture.
Debian 13 packages an older Rust than the GTK bindings need, so the `.deb` is built
in the official Rust 1.92 image based on Debian 13; the `.rpm` is built in Fedora
45 with its own Rust. The check builds the test programs once in that Debian image,
then defaults to `debian:13 ubuntu:24.04 ubuntu:26.04 fedora:45`; pass other images
as arguments. Containers receive only tracked sources, the packages, and those
programs, never `.vm/` or `downloads/`. Each installs the package with rclone
1.75.2 (the verified official package on Debian and Ubuntu) and runs as an
unprivileged user with the installed helper: the Rust tests, including a real FUSE
mount, real bisync for synced folders, and the real rclone sign-in steps; the
visible window under headless Weston, where Open must start a stand-in file
manager; and the installed app, which must close on SIGTERM. Containers get
`/dev/fuse` and `SYS_ADMIN` for the private mounts. Compiling the GTK bindings
takes up to 2 GB of memory per job; with less, set `CARGO_BUILD_JOBS=1` before
running both scripts.

To check the real GNOME and KDE Wayland compositors, run this in the dedicated VM
as a separate test user that can read the checkout, with `gnome-shell`,
`kwin-wayland`, Nautilus, and Dolphin installed:

```sh
cargo test --package icloud-on-linux --test visible --no-run
tests/check_wayland.sh gnome target/debug/deps/visible-HASH
tests/check_wayland.sh kde target/debug/deps/visible-HASH
```

The first command prints the program path after `Executable tests/visible/main.rs`;
build it as your own user if the test user has no Rust toolchain. Each run starts
a headless compositor without X11 in a private D-Bus session and a temporary home,
selects that desktop's file manager, and requires its process to appear after
Open. It stops the processes it started, unmounts any fixture mount left by a
failed check, and removes that home.

For container or shared session changes, build the image:

```sh
docker build --tag icloud-linux-lab .
```

The build tests the real rclone binary with an encrypted configuration and local
files, without mounting the account volume or contacting iCloud.

For mount changes, run this in a desktop session of the prepared VM, with
`~/iCloud` unmounted:

```sh
/usr/local/bin/icloud-check-mount
```

This test uses a temporary configuration and local directory to check reads,
write rejection with `EROFS`, and unmounting. It stops if a mount already exists
to preserve it. If you change this test or the session script, update their
installed copies in that dedicated VM first.

For documentation-only changes, check affected links, paths, and commands, and
run `git diff --check` for whitespace. There is no need to rebuild the container
or use the real account. Report relevant checks you could not run without
presenting them as passed.

For writable filesystem experiments, follow [the write checks guide](docs/WRITE_TESTS.md).
Those checks use private mount points and preserve the normal read-only mount.

## Commits

Use Conventional Commits in English with an imperative summary. The scope is
optional when it adds no clarity:

```text
type(scope): imperative summary
type: imperative summary
```

Each commit contains one logical change. The subject describes its effect; the
body can explain the reason, implementation, and checks.

- `feat`, `fix`, and `perf`: capabilities, corrections, and improvements users notice.
- `docs`, `test`, `refactor`, `build`, `ci`, and `chore`: documentation, tests,
  internal structure, and maintenance of the corresponding tools.

Choose a scope for the area that changes, such as `auth`, `mount`, `docker`, or
`vm`. There is no required scope catalog or release note generator.

```text
fix(auth): stop when the local key cannot unlock the session
feat(mount): open iCloud Drive from the file manager
test(vm): cover an invalid virtual machine identifier
docs: explain the read-only mount
```

For a breaking change, append `!` to the type or scope and explain the impact
and migration in a `BREAKING CHANGE:` footer. Do not include automatic attribution
or credentials in the message.
