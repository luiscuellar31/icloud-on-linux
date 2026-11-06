#!/usr/bin/env bash
set -euo pipefail
umask 077

if [[ "$(uname -s)" != Linux || "$EUID" == 0 ]]; then
    printf 'This check requires a Linux desktop user.\n' >&2
    exit 1
fi
if mountpoint -q "$HOME/iCloud"; then
    printf 'The iCloud folder is already mounted and will be preserved.\n' >&2
    exit 1
fi

fixture_dir="$(mktemp -d)"
trap 'fusermount3 -u "$HOME/iCloud" 2>/dev/null || true; rm -rf "$fixture_dir"; unset RCLONE_CONFIG_PASS' EXIT
export RCLONE_CONFIG="$fixture_dir/rclone.conf" RCLONE_CONFIG_PASS="local-mount-fixture"
mkdir "$fixture_dir/source"
printf 'File manager read test\n' > "$fixture_dir/source/sample.txt"
rclone --ask-password=false config encryption set \
    --password-command 'printenv RCLONE_CONFIG_PASS' >/dev/null
rclone --ask-password=false config create icloud alias remote "$fixture_dir/source" --no-output
printf '%s\n' "$RCLONE_CONFIG_PASS" | /usr/local/bin/icloud-session mount
cmp "$fixture_dir/source/sample.txt" "$HOME/iCloud/sample.txt"

python3 - <<'PY'
import errno
from pathlib import Path

try:
    with (Path.home() / 'iCloud/forbidden-write.txt').open('xb'):
        pass
except OSError as error:
    assert error.errno == errno.EROFS, error
else:
    raise AssertionError('The mount allowed a write')
print('PASS: real FUSE mount, correct read, and rejected write (EROFS).')
PY

fusermount3 -u "$HOME/iCloud"
mountpoint -q "$HOME/iCloud" && exit 1
printf 'PASS: unmounted successfully.\n'
