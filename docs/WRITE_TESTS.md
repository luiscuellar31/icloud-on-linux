# Write checks

These checks exercise writable FUSE mounts using disposable files. The normal
`~/iCloud` mount stays read-only. Each check creates its own private mount point
and cache; it never reuses the desktop mount or its cache.

## Local files

Run as a non-root Linux user with Python 3, rclone 1.75.2, and FUSE 3 installed:

```sh
python3 tests/check_writes.py
```

The check creates a temporary source directory and mounts it through rclone.
It creates, edits, renames, and deletes fictional files, including binary
content, nested folders, spaces, and a Unicode filename. SHA-256 comparisons
read the source independently of the mount cache before advancing to each step.

The rclone environment is cleared and the configuration path is `/dev/null`.
This check cannot use the saved Apple session. It unmounts its private mount
before removing the temporary files. The existing iCloud mount is unaffected.
If unmounting fails, the workspace is retained instead of recursively removing
files through a mount that might still be active.

## iCloud test folder

After the local check passes, opt in to the same operations against iCloud:

```sh
python3 tests/check_writes.py --icloud
```

This uses the dedicated encrypted configuration at
`~/.config/icloud-linux/rclone.conf`, or the file selected by `RCLONE_CONFIG`.
Enter its local key in your terminal when prompted. An already supplied runtime
key can also be used through the environment; never put a key in arguments or
send it in a message.

The check copies the encrypted configuration into its private temporary workspace
and leaves the original file unchanged. The temporary copy has `0600` permissions
and is removed with the workspace after unmounting.

Every run chooses a new `icloud-on-linux-test-<random identifier>` folder. It
refuses an existing folder, writes and verifies an ownership marker, and mounts
only its `data` subfolder. No root account listing is printed. Files are downloaded
independently through rclone to verify that writes reached iCloud rather than
only the local cache.

Metadata must confirm an absent file before a deletion check can pass. A failed
download is retried within the check deadline and never treated as absence.
Directories and unexpected files larger than 2 MiB are refused before download.

Successful runs remove their own fixture files, verify the ownership marker and
empty data folder, then remove the marker and empty directories. There is no
recursive remote delete or purge. Failed runs retain the test folder for review;
unexpected files or a changed ownership marker prevent cleanup. Review any
retained folder in iCloud before removing it manually.

## Integrity and reconnection

Run the local fault check as a non-root Linux user:

```sh
python3 tests/check_recovery.py
```

It starts a WebDAV server bound only to `127.0.0.1`, serving a private folder
of fictional files. It verifies an online upload and detects an intentional
same-size corruption using SHA-256. It then stops only that server, observes
the failed connection, and checks that a closed file remains in the write cache.

The private mount is stopped while offline and restarted with the same cache
after the server returns. The check requires correct bytes in both the backing
directory and the mounted file, and verifies that the earlier file is unchanged.
The normal iCloud connection and VM network services are not interrupted.

This check uses a one-second write-back delay to exercise queued uploads. A zero
delay uses close-time uploads instead. Directory metadata uses a short cache
lifetime so entries discovered during an upload can be refreshed before the
recovery is declared complete. Backend visibility alone is insufficient evidence
that a mounted file is ready to read.

Fault injection targets local WebDAV. Apple session expiry and recovery during
an actual iCloud outage still require separate validation.

## Conflicting iCloud edits

Opt in to a new, exclusive iCloud folder:

```sh
python3 tests/check_recovery.py --icloud
```

The check uploads an original fixture and replaces it with a different version
to represent another editor. It then attempts to upload the stale original with
directory `copy --immutable --ignore-times`. A private upload directory contains
only the fixture; configuration files are kept outside it. The expected conflict
error must occur, and an independent
download must still match the competing version. Only that fixture and its
verified empty test folder are removed afterwards.

The check first proves the directory-copy guard locally, including its anchored
file filter. Rclone 1.75.2 routes single-file `copy` and `copyto` through a different
path that does not apply the immutable overwrite check. The test deliberately
uses directory copy and forces the stale transfer comparison rather than trusting
size and modification time to indicate an attempted write.

This verifies the explicit immutable transfer guard. A sync client still needs
its own conflict detection and resolution before general writes are enabled.
The normal desktop mount stays read-only.
