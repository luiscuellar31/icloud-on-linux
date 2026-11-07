import contextlib
import io
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

from write_support import local_environment, mounted, private_workspace


class WriteSafetyTests(unittest.TestCase):
    def test_local_checks_cannot_inherit_account_credentials(self):
        with patch.dict(os.environ, {
            'RCLONE_CONFIG': '/private/account.conf',
            'RCLONE_CONFIG_PASS': 'fictional-key',
            '_RCLONE_CONFIG_KEY': 'fictional-runtime-key',
            'RCLONE_ICLOUD_PASSWORD': 'fictional-password',
        }):
            environment = local_environment()
        self.assertEqual(environment['RCLONE_CONFIG'], '/dev/null')
        self.assertNotIn('RCLONE_CONFIG_PASS', environment)
        self.assertNotIn('_RCLONE_CONFIG_KEY', environment)
        self.assertNotIn('RCLONE_ICLOUD_PASSWORD', environment)

    def test_workspace_cleanup_preserves_a_mount_after_unmount_failure(self):
        with contextlib.redirect_stdout(io.StringIO()):
            with patch('write_support.os.path.ismount', return_value=True):
                with private_workspace('icloud-safety-') as workspace:
                    sentinel = workspace / 'must-survive.txt'
                    sentinel.write_text('Keep mounted content')
        self.addCleanup(shutil.rmtree, workspace)
        self.assertEqual(sentinel.read_text(), 'Keep mounted content')

    def test_unmounted_workspace_is_removed(self):
        with private_workspace('icloud-safety-') as workspace:
            (workspace / 'fixture.txt').write_text('Disposable fixture')
        self.assertFalse(workspace.exists())

    def test_mount_refuses_an_occupied_directory_before_starting_rclone(self):
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            (workspace / 'mount').mkdir()
            (workspace / 'mount/existing.txt').write_text('Existing data')
            with patch('write_support.subprocess.Popen') as start:
                with self.assertRaisesRegex(RuntimeError, 'empty and unmounted'):
                    with mounted('/unused', workspace, local_environment()):
                        self.fail('An occupied mount point was accepted')
            start.assert_not_called()


if __name__ == '__main__':
    unittest.main()
