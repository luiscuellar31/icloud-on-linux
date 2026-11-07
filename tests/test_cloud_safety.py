from pathlib import Path
import json
import os
import stat
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from cloud_sandbox import CloudSandbox, checked_root, cloud_environment


class CloudSafetyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.sandbox = CloudSandbox(Path(self.temporary.name), {})

    def test_only_a_dedicated_random_sandbox_is_accepted(self):
        for root in ('icloud:', 'icloud:Documents', 'other:icloud-on-linux-test-' + 'a' * 32,
                     self.sandbox.root + '/..', 'icloud:icloud-on-linux-test-short'):
            with self.subTest(root=root), self.assertRaises(RuntimeError):
                checked_root(root)
        self.assertEqual(checked_root(self.sandbox.root), self.sandbox.root)

    def test_commented_encrypted_config_is_copied_without_changing_the_original(self):
        source = Path(self.temporary.name) / 'account.conf'
        original = b'# Encrypted rclone configuration File\n\nRCLONE_ENCRYPT_V0:\nfixture-only\n'
        source.write_bytes(original)
        source.chmod(0o600)
        workspace = Path(self.temporary.name) / 'workspace'
        workspace.mkdir()
        with patch.dict(os.environ, {'RCLONE_CONFIG': str(source), 'RCLONE_CONFIG_PASS': 'fictional-key'}):
            environment = cloud_environment(workspace)
        copy = Path(environment['RCLONE_CONFIG'])
        self.assertNotEqual(copy, source)
        self.assertEqual(copy.read_bytes(), original)
        self.assertEqual(stat.S_IMODE(copy.stat().st_mode), 0o600)
        self.assertEqual(source.read_bytes(), original)

    def test_unrecognized_and_traversing_paths_are_refused(self):
        for path in ('../Documents', '/data', 'data/../owner.json', 'data/personal.txt'):
            with self.subTest(path=path), self.assertRaises(RuntimeError):
                self.sandbox.path(path)

    def test_an_existing_folder_is_never_reused(self):
        with patch('cloud_sandbox.rclone', return_value=subprocess.CompletedProcess([], 0, b'{}')) as run:
            with self.assertRaisesRegex(RuntimeError, 'already exists'):
                self.sandbox.create()
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[0], 'lsjson')
        self.assertFalse(self.sandbox.created)

    def test_unverified_ownership_prevents_all_cleanup_mutations(self):
        with patch('cloud_sandbox.rclone', return_value=subprocess.CompletedProcess([], 0, b'foreign marker')) as run:
            with self.assertRaisesRegex(RuntimeError, 'Ownership changed'):
                self.sandbox.remove_empty()
        self.assertEqual([call.args[0] for call in run.call_args_list], ['cat'])

    def test_unexpected_files_are_preserved(self):
        replies = [
            subprocess.CompletedProcess([], 0, self.sandbox.marker),
            subprocess.CompletedProcess([], 0, b'data/\nowner.json\npersonal.txt\n'),
        ]
        with patch('cloud_sandbox.rclone', side_effect=replies) as run:
            with self.assertRaisesRegex(RuntimeError, 'Unexpected contents'):
                self.sandbox.remove_empty()
        self.assertEqual([call.args[0] for call in run.call_args_list], ['cat', 'lsf'])

    def test_a_failed_download_is_never_reported_as_an_absent_file(self):
        replies = [
            subprocess.CompletedProcess([], 0, json.dumps({'IsDir': False, 'Size': 12}).encode()),
            subprocess.CompletedProcess([], 1, b'', b'temporary fixture error'),
        ]
        with patch('cloud_sandbox.rclone', side_effect=replies):
            with self.assertRaises(BlockingIOError):
                self.sandbox.read('draft space.bin')

    def test_known_missing_files_need_no_download(self):
        with patch('cloud_sandbox.rclone', return_value=subprocess.CompletedProcess([], 3, b'')) as run:
            self.assertIsNone(self.sandbox.read('draft space.bin'))
        self.assertEqual([call.args[0] for call in run.call_args_list], ['lsjson'])

    def test_a_directory_or_large_file_is_never_downloaded(self):
        for info in ({'IsDir': True, 'Size': 0}, {'IsDir': False, 'Size': 3 * 1024 * 1024}):
            reply = subprocess.CompletedProcess([], 0, json.dumps(info).encode())
            with self.subTest(info=info), patch('cloud_sandbox.rclone', return_value=reply) as run:
                with self.assertRaisesRegex(RuntimeError, 'Refusing to download'):
                    self.sandbox.read('draft space.bin')
            self.assertEqual(run.call_count, 1)


if __name__ == '__main__':
    unittest.main()
