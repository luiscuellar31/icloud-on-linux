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
  across Docker, Ubuntu, and the guides.
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
current stable toolchain from rustup; the workspace declares the oldest supported
version. For changes there, run from the repository root:

```sh
cargo fmt --check
cargo clippy --all-targets
cargo test
```

Run the tests on Linux as well: process protection and mount checks depend on it.
Keep `unsafe` code out of the crates except where a system call requires it, and
explain each such block in a `SAFETY` comment.

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
