import argparse
import socket
import subprocess
import urllib.error
import urllib.request
import uuid

from write_support import (
    local_environment, mounted, private_workspace, rclone, require_linux, same_bytes, wait_for,
)


def check_copy_guard(workspace):
    source = workspace / 'guard-source'
    destination = workspace / 'guard-destination'
    source.mkdir()
    destination.mkdir()
    original = b'Stale fictional content'
    competing = b'Competing fictional content with another size'
    (source / 'conflict.bin').write_bytes(original)
    (source / 'unselected.txt').write_bytes(b'This file must stay local')
    target = destination / 'conflict.bin'
    target.write_bytes(competing)
    arguments = ('copy', source, destination, '--include', '/conflict.bin', '--max-depth', '1',
                 '--immutable', '--ignore-times', '--retries', '1')
    result = rclone(*arguments, environment=local_environment(), check=False)
    assert result.returncode != 0 and b'immutable file modified' in result.stderr.lower()
    assert target.read_bytes() == competing
    assert not (destination / 'unselected.txt').exists()
    target.unlink()
    rclone(*arguments, environment=local_environment())
    assert target.read_bytes() == original
    assert {path.name for path in destination.iterdir()} == {'conflict.bin'}
    print('PASS: directory copy rejects an overwrite and selects only the intended file.', flush=True)


class WebDAVFixture:
    def __init__(self, workspace):
        self.workspace = workspace
        self.root = workspace / 'server'
        self.data = self.root / 'data'
        self.data.mkdir(parents=True)
        self.identity = uuid.uuid4().hex.encode()
        (self.root / 'ready.txt').write_bytes(self.identity)
        with socket.socket() as connection:
            connection.bind(('127.0.0.1', 0))
            self.address = f'127.0.0.1:{connection.getsockname()[1]}'
        self.url = f'http://{self.address}/'
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        self.process = None

    def ready(self):
        if self.process.poll() is not None:
            raise RuntimeError('The local WebDAV fixture exited')
        try:
            with self.opener.open(self.url + 'ready.txt', timeout=0.5) as response:
                return response.read() == self.identity
        except (OSError, urllib.error.URLError):
            return False

    def start(self):
        with (self.workspace / 'server.log').open('ab') as log:
            self.process = subprocess.Popen([
                'rclone', '--ask-password=false', 'serve', 'webdav', str(self.root),
                '--addr', self.address,
            ], env=local_environment(), stdout=log, stderr=log)
        try:
            wait_for(self.ready, 'The local WebDAV fixture did not become ready')
        except BaseException:
            self.stop()
            raise

    def stop(self):
        if self.process is not None:
            if self.process.poll() is None:
                self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
            self.process = None

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *error):
        self.stop()


def check_recovery():
    require_linux()
    with private_workspace('icloud-recovery-') as workspace:
        check_copy_guard(workspace)
        with WebDAVFixture(workspace) as server:
            config = workspace / 'fixture.conf'
            config.write_text(f'[fixture]\ntype = webdav\nurl = {server.url}\nvendor = other\n')
            environment = {**local_environment(), 'RCLONE_CONFIG': str(config)}
            # Delayed write-back exercises the retry queue rather than close-time uploads.
            flags = ('--contimeout', '1s', '--timeout', '2s', '--low-level-retries', '1',
                     '--retries', '1', '--vfs-write-back', '1s')
            original = b'Online fixture\n' + bytes(range(256)) * 4096
            pending = b'Offline fixture\n' + bytes(range(255, -1, -1)) * 4096
            with mounted('fixture:data', workspace, environment, flags) as mountpoint:
                (mountpoint / 'online.bin').write_bytes(original)
                wait_for(lambda: (server.data / 'online.bin').exists(), 'Initial upload did not reach the server')
                wait_for(lambda: same_bytes((server.data / 'online.bin').read_bytes(), original), 'Initial bytes differ')
                print('PASS: online upload matches its independent backend bytes.', flush=True)

                corrupted = bytes([original[0] ^ 1]) + original[1:]
                (server.data / 'online.bin').write_bytes(corrupted)
                assert len(corrupted) == len(original)
                assert not same_bytes((server.data / 'online.bin').read_bytes(), original)
                (server.data / 'online.bin').write_bytes(original)
                print('PASS: same-size corruption is detected by the byte check.', flush=True)

                list(mountpoint.iterdir())
                server.stop()
                (mountpoint / 'pending.bin').write_bytes(pending)
                assert not (server.data / 'pending.bin').exists()
                log = workspace / 'mount.log'
                try:
                    wait_for(
                        lambda: b'pending.bin' in log.read_bytes() and b'connection refused' in log.read_bytes().lower(),
                        'The disconnected upload was not observed',
                    )
                except TimeoutError:
                    print('Local fixture diagnostics:\n' + log.read_text(errors='replace')[-3000:])
                    raise
                print('PASS: a connection failure leaves the new write queued locally.', flush=True)

            cached = list((workspace / 'cache').rglob('pending.bin'))
            assert any(path.is_file() and same_bytes(path.read_bytes(), pending) for path in cached)
            assert not (server.data / 'pending.bin').exists()
            print('PASS: pending bytes survive stopping the private mount.', flush=True)

            server.start()
            with mounted('fixture:data', workspace, environment, flags) as mountpoint:
                wait_for(
                    lambda: (server.data / 'pending.bin').exists() and same_bytes((server.data / 'pending.bin').read_bytes(), pending),
                    'The queued write did not recover after reconnection', timeout=90,
                )
                print('Recovered file sizes (backend, initial mount metadata):',
                      (server.data / 'pending.bin').stat().st_size,
                      (mountpoint / 'pending.bin').stat().st_size, flush=True)
                wait_for(
                    lambda: same_bytes((mountpoint / 'pending.bin').read_bytes(), pending),
                    'The recovered bytes are not yet visible through the mount',
                )
                assert same_bytes((server.data / 'online.bin').read_bytes(), original)
                print('PASS: restart with the same cache uploads the pending bytes without changing the earlier file.', flush=True)
    print('PASS: isolated disconnection and recovery; the real iCloud connection was not interrupted.')


def check_cloud_conflict():
    from cloud_sandbox import cloud_environment, cloud_sandbox

    require_linux()
    with private_workspace('icloud-conflict-') as workspace:
        check_copy_guard(workspace)
        environment = cloud_environment(workspace)
        with cloud_sandbox(workspace, environment) as sandbox:
            uploads = workspace / 'uploads'
            uploads.mkdir()
            source = uploads / 'conflict.bin'
            target = sandbox.path('data/conflict.bin')
            original = b'Original editor fixture\n' * 17
            competing = b'Competing editor fixture\n' * 31
            source.write_bytes(original)
            sandbox.run('copy', uploads, sandbox.path('data'), '--immutable')
            wait_for(lambda: same_bytes(sandbox.read('conflict.bin'), original), 'Original cloud bytes differ', timeout=90)
            source.write_bytes(competing)
            sandbox.run('copyto', source, target)
            wait_for(lambda: same_bytes(sandbox.read('conflict.bin'), competing), 'Competing cloud bytes differ', timeout=90)

            source.write_bytes(original)
            rejected = sandbox.run('copy', uploads, sandbox.path('data'), '--immutable',
                                   '--ignore-times', '--retries', '1', check=False)
            assert rejected.returncode != 0 and b'immutable file modified' in rejected.stderr.lower()
            wait_for(lambda: same_bytes(sandbox.read('conflict.bin'), competing), 'A stale write changed the remote bytes', timeout=90)
            print('PASS: an immutable transfer rejects the stale editor copy and preserves the competing bytes.', flush=True)
            sandbox.run('deletefile', target)
            wait_for(lambda: sandbox.read('conflict.bin') is None, 'The conflict fixture was not removed')
    print('PASS: real iCloud conflict guard and independent integrity checks in an exclusive test folder.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description='Check isolated recovery or an opt-in iCloud conflict.')
    parser.add_argument('--icloud', action='store_true', help='Check immutable conflict rejection in a new iCloud folder.')
    if parser.parse_args().icloud:
        check_cloud_conflict()
    else:
        check_recovery()
