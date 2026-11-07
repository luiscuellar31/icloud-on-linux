import contextlib
import getpass
import json
import os
from pathlib import Path
import re
import shutil
import stat
import uuid

from write_support import local_environment, rclone


FILES = {'draft space.bin', 'renamed.bin', 'nested folder/unicode-\u03bb.txt'}


def checked_root(root):
    if not re.fullmatch(r'icloud:icloud-on-linux-test-[0-9a-f]{32}', root):
        raise RuntimeError('Refusing a path outside a fresh iCloud test sandbox')
    return root


def cloud_environment(workspace):
    source = Path(os.environ.get(
        'RCLONE_CONFIG', str(Path.home() / '.config/icloud-linux/rclone.conf'),
    ))
    info = source.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError('Use a private, user-owned encrypted configuration')
    with source.open('rb') as stream:
        header = [stream.readline().strip() for _ in range(4)]
        if b'RCLONE_ENCRYPT_V0:' not in header:
            raise RuntimeError('The account configuration must be encrypted')
    environment = local_environment()
    for name in ('RCLONE_CONFIG_PASS', '_RCLONE_CONFIG_KEY'):
        if os.environ.get(name):
            environment[name] = os.environ[name]
    if not any(name in environment for name in ('RCLONE_CONFIG_PASS', '_RCLONE_CONFIG_KEY')):
        environment['RCLONE_CONFIG_PASS'] = getpass.getpass('Local configuration key: ')
        if not environment['RCLONE_CONFIG_PASS']:
            raise RuntimeError('The local configuration key cannot be empty')
    destination = workspace / 'rclone.conf'
    shutil.copyfile(source, destination)
    destination.chmod(0o600)
    environment['RCLONE_CONFIG'] = str(destination)
    return environment


class CloudSandbox:
    def __init__(self, workspace, environment):
        identifier = uuid.uuid4().hex
        self.root = f'icloud:icloud-on-linux-test-{identifier}'
        self.workspace = workspace
        self.environment = environment
        self.marker = json.dumps({'suite': 'icloud-on-linux', 'run': identifier}).encode()
        self.created = False

    def path(self, relative=''):
        root = checked_root(self.root)
        allowed = {'', 'owner.json', 'data', *('data/' + name for name in FILES)}
        if relative not in allowed:
            raise RuntimeError('Refusing an unrecognized sandbox path')
        return root + ('/' + relative if relative else '')

    def run(self, *arguments, **options):
        return rclone(*arguments, environment=self.environment, **options)

    def create(self):
        result = self.run('lsjson', '--stat', self.path(), check=False)
        if result.returncode not in (3, 4):
            raise RuntimeError('The test folder already exists or its absence cannot be verified')
        self.run('mkdir', self.path())
        self.created = True
        if self.run('lsf', self.path()).stdout.strip():
            raise RuntimeError('The new test folder is not empty')
        marker = self.workspace / 'owner.json'
        marker.write_bytes(self.marker)
        self.run('copyto', marker, self.path('owner.json'), '--immutable')
        if self.run('cat', self.path('owner.json')).stdout != self.marker:
            raise RuntimeError('The sandbox ownership marker could not be verified')
        self.run('mkdir', self.path('data'))

    def read(self, name):
        path = self.path('data/' + name)
        status = self.run('lsjson', '--stat', path, check=False)
        if status.returncode in (3, 4):
            return None
        if status.returncode:
            raise BlockingIOError('The independent file status is temporarily unavailable')
        info = json.loads(status.stdout)
        if info['IsDir'] or not 0 <= info['Size'] <= 2 * 1024 * 1024:
            raise RuntimeError('Refusing to download an unexpected directory or large file')
        result = self.run('cat', path, '--retries', '1', '--low-level-retries', '1', check=False)
        if result.returncode:
            raise BlockingIOError('The independent download is temporarily unavailable')
        return result.stdout

    def remove_empty(self):
        if self.run('cat', self.path('owner.json')).stdout != self.marker:
            raise RuntimeError('Ownership changed; the test folder will be preserved')
        entries = set(self.run('lsf', self.path()).stdout.splitlines())
        if entries != {b'data/', b'owner.json'}:
            raise RuntimeError('Unexpected contents; the test folder will be preserved')
        if self.run('lsf', self.path('data')).stdout.strip():
            raise RuntimeError('Fixture files remain; the test folder will be preserved')
        self.run('rmdir', self.path('data'))
        self.run('deletefile', self.path('owner.json'))
        self.run('rmdir', self.path())
        result = self.run('lsjson', '--stat', self.path(), check=False)
        if result.returncode not in (3, 4):
            raise RuntimeError('Removal of the empty test folder could not be verified')


@contextlib.contextmanager
def cloud_sandbox(workspace, environment):
    sandbox = CloudSandbox(workspace, environment)
    try:
        sandbox.create()
        print(f'Private test folder: {sandbox.root}')
        yield sandbox
        sandbox.remove_empty()
    except BaseException:
        if sandbox.created:
            print(f'Test folder preserved for review: {sandbox.root}')
        raise
    print('PASS: ownership verified; empty test folder removed without a recursive delete.')
