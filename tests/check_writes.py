import argparse

from write_support import (
    exercise_writes, local_environment, mounted, private_workspace, require_linux,
)


def check_local():
    require_linux()
    with private_workspace('icloud-local-writes-') as workspace:
        source = workspace / 'source'
        source.mkdir()

        def read_backend(name):
            try:
                return (source / name).read_bytes()
            except FileNotFoundError:
                return None

        with mounted(source, workspace, local_environment()) as mountpoint:
            exercise_writes(mountpoint, read_backend)
        assert not list(source.iterdir())
    print('PASS: isolated local writes; no Apple configuration or account was used.')


def check_icloud():
    from cloud_sandbox import cloud_environment, cloud_sandbox

    require_linux()
    with private_workspace('icloud-cloud-writes-') as workspace:
        environment = cloud_environment(workspace)
        with cloud_sandbox(workspace, environment) as sandbox:
            with mounted(sandbox.path('data'), workspace, environment) as mountpoint:
                exercise_writes(mountpoint, sandbox.read)
    print('PASS: real iCloud writes and independent downloads in an exclusive test folder.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description='Check isolated writable FUSE operations.')
    parser.add_argument('--icloud', action='store_true', help='Use a fresh, exclusive iCloud test folder.')
    arguments = parser.parse_args()
    if arguments.icloud:
        check_icloud()
    else:
        check_local()
