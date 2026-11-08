import contextlib
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


def require_linux():
    if sys.platform != 'linux' or os.getuid() == 0:
        raise RuntimeError('Run this check as a Linux desktop user')
    for command in ('rclone', 'fusermount3'):
        if not shutil.which(command):
            raise RuntimeError(f'Required command is missing: {command}')
    os.umask(0o077)


@contextlib.contextmanager
def private_workspace(prefix):
    workspace = Path(tempfile.mkdtemp(prefix=prefix))
    try:
        yield workspace
    finally:
        # A failed unmount must never turn cleanup into recursive remote deletion.
        if os.path.ismount(workspace / 'mount'):
            print(f'Private workspace retained because it is still mounted: {workspace}')
        else:
            shutil.rmtree(workspace)


def local_environment():
    environment = {
        key: value for key, value in os.environ.items()
        if not key.startswith(('RCLONE_', '_RCLONE_'))
    }
    environment['RCLONE_CONFIG'] = '/dev/null'
    return environment


def rclone(*arguments, environment, check=True, timeout=60):
    result = subprocess.run(
        ['rclone', '--ask-password=false', '--log-level', 'ERROR', *map(str, arguments)],
        env=environment, capture_output=True, timeout=timeout,
    )
    if check and result.returncode:
        raise RuntimeError(f'rclone {arguments[0]} failed with exit code {result.returncode}')
    return result


def wait_for(predicate, description, timeout=30):
    deadline = time.monotonic() + timeout
    while True:
        try:
            if predicate():
                return
        except BlockingIOError:
            pass
        if time.monotonic() >= deadline:
            raise TimeoutError(description)
        time.sleep(0.1)


@contextlib.contextmanager
def mounted(source, workspace, environment, flags=()):
    mountpoint = workspace / 'mount'
    mountpoint.mkdir(exist_ok=True)
    if os.path.ismount(mountpoint) or any(mountpoint.iterdir()):
        raise RuntimeError('The private mount point must be empty and unmounted')
    with (workspace / 'mount.log').open('ab') as log:
        process = subprocess.Popen([
            'rclone', '--ask-password=false', 'mount', str(source), str(mountpoint),
            '--vfs-cache-mode', 'writes', '--vfs-write-back', '0s',
            '--cache-dir', str(workspace / 'cache'), '--dir-cache-time', '1s',
            '--poll-interval', '0', '--attr-timeout', '0s',
            *flags,
        ], env=environment, stdout=log, stderr=log)
        try:
            def ready():
                if process.poll() is not None:
                    raise RuntimeError('The private test mount exited before becoming ready')
                return os.path.ismount(mountpoint)
            wait_for(ready, 'The private test mount did not become ready')
            yield mountpoint
        finally:
            try:
                if os.path.ismount(mountpoint):
                    subprocess.run(['fusermount3', '-u', str(mountpoint)], check=True, timeout=15)
            finally:
                if process.poll() is None:
                    process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


def same_bytes(actual, expected):
    return actual is not None and hashlib.sha256(actual).digest() == hashlib.sha256(expected).digest()


def exercise_writes(mountpoint, read_backend):
    original = b'Original fixture\n' + bytes(range(256)) * 4096
    edited = b'Edited fixture\n' + bytes(range(255, -1, -1)) * 257

    def verified(name, expected):
        wait_for(
            lambda: same_bytes(read_backend(name), expected),
            f'The backend did not retain the expected bytes for {name}', timeout=90,
        )
        assert same_bytes((mountpoint / name).read_bytes(), expected)

    name = 'draft space.bin'
    with (mountpoint / name).open('xb') as stream:
        stream.write(original)
    verified(name, original)
    print('PASS: create and independently verify binary content.')

    (mountpoint / name).write_bytes(edited)
    verified(name, edited)
    print('PASS: edit and independently verify the updated content.')

    renamed = 'renamed.bin'
    (mountpoint / name).rename(mountpoint / renamed)
    verified(renamed, edited)
    wait_for(lambda: read_backend(name) is None, 'The old name still exists')
    print('PASS: rename without changing file content.')

    folder = mountpoint / 'nested folder'
    folder.mkdir()
    nested = 'nested folder/unicode-\u03bb.txt'
    (mountpoint / nested).write_bytes(b'Nested fixture\n')
    verified(nested, b'Nested fixture\n')
    print('PASS: nested paths, spaces, and Unicode filenames.')

    for item in (nested, renamed):
        (mountpoint / item).unlink()
        wait_for(lambda: read_backend(item) is None, 'A deleted fixture still exists')
    folder.rmdir()
    assert not list(mountpoint.iterdir())
    print('PASS: delete only the files and directory created by this check.')
