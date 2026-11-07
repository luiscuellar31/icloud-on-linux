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


if __name__ == '__main__':
    check_local()
