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
