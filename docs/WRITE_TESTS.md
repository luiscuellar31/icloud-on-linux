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
