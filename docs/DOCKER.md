# Docker terminal lab

The lab signs in to iCloud Drive, lists files, and downloads a sample file from
Ubuntu in Docker. These steps only read from iCloud.

The environment uses Ubuntu 24.04 and rclone 1.75.2 with an unprivileged user.
It does not need Docker Compose. Configuration and downloads persist in the
`icloud-linux-state` Docker volume, outside the project directory.

Run host commands from the repository root. Steps that belong inside the
container are identified explicitly.

## Quick start

On macOS, double-click `start.command`. You can also run it from a terminal
on macOS or Linux:

```sh
./start.command
```

The launcher checks Docker, builds or updates the image, and opens Ubuntu. The
assistant asks for a local key to encrypt the saved session and, on first use,
the Apple account, password, and 2FA code. The connection name and backend are
already selected. Keep the `drive` service if prompted and accept the defaults
for advanced options.

It then lists the top level of iCloud Drive and leaves the Ubuntu terminal open.
Later runs reuse the configuration and ask for its local unlock key. The key
stays in the container session environment and is not saved in a separate file.

The configuration summary containing cookies and tokens is suppressed. If the
access check fails, the assistant shows the error and stops without reporting
success.

Type `exit` to close the lab. If Docker reports that `icloud-linux-lab` is already
in use, return to the terminal where the lab is open.

## Manual setup

These steps open a terminal without the assistant to diagnose a problem or
complete an interrupted configuration.

### 1. Build the environment

With Docker running, from the repository root:

```sh
docker build --tag icloud-linux-lab .
docker run --rm icloud-linux-lab rclone version
```

The output should identify rclone 1.75.2 and Linux, matching the tested
lab version. Check authentication before replacing it with a distribution package.

### 2. Open Ubuntu and sign in

In your host terminal:

```sh
docker run --rm -it \
  --name icloud-linux-lab \
  --mount type=volume,source=icloud-linux-state,target=/state \
  icloud-linux-lab bash
```

Inside the container:

```sh
umask 077
rclone config
```

In the configuration menu:

1. Select `s` to protect the configuration with a password. This local password
   encrypts the file and is independent of the Apple password.
2. Return to the main menu and select `n` to create a connection.
3. Name: `icloud`.
4. Storage type: `iclouddrive`; enter the name rather than a number.
5. Service: `drive`, if prompted.
6. Enter your Apple account and regular password. App-specific passwords do not
   work with this backend.
7. Complete 2FA from a trusted device or SMS and save the connection.

Enter passwords and codes only in your own terminal. The manual configuration
assistant may show cookies or tokens in its summary; do not share that summary
or `/state/rclone.conf`.

If access is denied, check whether web access to iCloud data is enabled in your
account settings. Use the specific error to guide the diagnosis.

## List and download a file

Still inside Ubuntu:

```sh
rclone lsd icloud:
rclone lsf icloud:
```

`lsd` and `lsf` list content without changing files. `--read-only` belongs to the
mount command and is not used with these commands.

To inspect a folder, replace `Documents` with its actual name:

```sh
rclone lsf 'icloud:Documents'
```

Choose a small existing file and adapt the example path:

```sh
rclone copy 'icloud:Documents' /state/downloads --include '/sample.pdf' --max-depth 1 --immutable --progress
sha256sum /state/downloads/sample.pdf
```

The source is the iCloud folder; the anchored filter selects only `sample.pdf`
at its top level. The destination is local to the container. This directory-copy
path applies `--immutable` to protect an existing download with different content.
Compare its hash with a reference copy of the same file to verify its contents.

Keep the source as a folder. In rclone 1.75.2, single-file `copy` and `copyto` use
a separate path that does not enforce the immutable overwrite check. See the
[command dispatch](https://github.com/rclone/rclone/blob/v1.75.2/cmd/copy/copy.go)
and [directory-copy guard](https://github.com/rclone/rclone/blob/v1.75.2/fs/sync/sync.go).

To copy it to the host, open another terminal at the repository root while the
container is still open:

```sh
mkdir -p downloads
docker cp icloud-linux-lab:/state/downloads/sample.pdf ./downloads/sample.pdf
```

Type `exit` to close Ubuntu. The volume keeps the encrypted session and downloads;
reopen it with the command from step 2. Do not remove the volume if you want to
keep those files.

## Acceptance checks

- Sign-in and 2FA work.
- Listings show the expected iCloud Drive contents.
- A downloaded file matches its reference content.
- Closing and reopening the container reuses the saved configuration.

Authentication may expire and require authorization again. The rclone iCloud
backend is experimental, so verify the result with the real account.

## File manager mount and development

Use the [Ubuntu Desktop guide](UBUNTU_DESKTOP.md) to open iCloud in Files. Docker
provides terminal authentication and downloads here; FUSE mounting is tested
directly on Linux with a desktop.

Code checks, including the real rclone test performed during the image build,
are in [CONTRIBUTING.md](../CONTRIBUTING.md).

## References

- [rclone iCloud backend](https://rclone.org/iclouddrive/)
- [Authentication changes and versions](https://rclone.org/changelog/)
- [Configuration encryption](https://rclone.org/docs/#configuration-encryption)
- [Backend stability](https://rclone.org/tiers/)
- [Persistent Docker volumes](https://docs.docker.com/engine/storage/volumes/)
