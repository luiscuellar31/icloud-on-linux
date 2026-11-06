import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


PROJECT = Path(__file__).resolve().parents[1]
KEY = "fixture-local-key"
ENCRYPTED_FIXTURE = "RCLONE_ENCRYPT_V0:\nconfiguration-fixture\n"

STUB = """#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

tool = Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['CALL_LOG'], 'a') as log:
    log.write(json.dumps([tool, args]) + '\\n')

if tool == 'docker':
    if args[0] == os.environ.get('DOCKER_FAIL'):
        sys.exit(9)
    sys.exit(0)

assert args.pop(0) == '--ask-password=false'
assert os.environ.get('RCLONE_CONFIG_PASS')
operation = ' '.join(args[:3]) if args[0] == 'config' else args[0]
if operation == os.environ.get('RCLONE_FAIL'):
    print('simulated failure', file=sys.stderr)
    sys.exit(9)
if args[:3] == ['config', 'encryption', 'set']:
    assert args[3:] == ['--password-command', 'printenv RCLONE_CONFIG_PASS']
    Path(os.environ['RCLONE_CONFIG']).write_text('RCLONE_ENCRYPT_V0:\\nconfiguration-fixture\\n')
elif args[:3] == ['config', 'encryption', 'check']:
    assert Path(os.environ['RCLONE_CONFIG']).read_text().startswith('RCLONE_ENCRYPT_V0:')
elif args[0] == 'listremotes':
    print(os.environ.get('REMOTE_NAMES', ''))
elif args[:3] == ['config', 'create', 'icloud']:
    assert args[3:] == ['iclouddrive', '--all', '--no-output']
    assert Path(os.environ['RCLONE_CONFIG']).read_text().startswith('RCLONE_ENCRYPT_V0:')
elif args[0] == 'lsf':
    assert args[1:] == ['icloud:']
    print('Documents/')
else:
    raise AssertionError('Unexpected operation')
"""


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="icloud-launcher-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.bin = self.directory / "bin"
        self.bin.mkdir()
        for tool in ("docker", "rclone"):
            executable = self.bin / tool
            executable.write_text(STUB)
            executable.chmod(0o755)
        self.config = self.directory / "rclone.conf"
        self.log = self.directory / "calls.jsonl"
        self.env = {
            **os.environ,
            "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            "CALL_LOG": str(self.log),
            "RCLONE_CONFIG": str(self.config),
        }

    def calls(self):
        if not self.log.exists():
            return []
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def run_script(self, script, inputs="", arguments=(), **environment):
        result = subprocess.run(
            ["bash", str(PROJECT / script), *arguments],
            input=inputs,
            text=True,
            capture_output=True,
            env={**self.env, **environment},
            cwd=self.directory,
            timeout=10,
        )
        self.assertNotIn(KEY, result.stdout + result.stderr)
        self.assertNotIn(KEY, json.dumps(self.calls()))
        return result

    def test_invalid_session_mode_stops_before_reading_credentials(self):
        result = self.run_script('scripts/session.sh', arguments=('invalid',))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.calls(), [])
        self.assertNotIn('Local key', result.stdout + result.stderr)

    def test_new_session_encrypts_before_creating_remote_and_lists_read_only(self):
        result = self.run_script("scripts/session.sh", f"{KEY}\n{KEY}\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Access verified', result.stdout)
        operations = [args[1:4] for _, args in self.calls()]
        self.assertEqual(operations[:2], [
            ["config", "encryption", "set"],
            ["config", "encryption", "check"],
        ])
        self.assertIn(["config", "create", "icloud"], operations)
        self.assertEqual(self.calls()[-1][1], ["--ask-password=false", "lsf", "icloud:"])

    def test_existing_session_reuses_remote_without_asking_for_new_credentials(self):
        self.config.write_text(ENCRYPTED_FIXTURE)
        result = self.run_script("scripts/session.sh", f"{KEY}\n", REMOTE_NAMES="other:\nicloud:")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any("create" in args for _, args in self.calls()))
        self.assertNotIn('Confirm', result.stdout + result.stderr)

    def test_similarly_named_remote_does_not_hide_missing_icloud_connection(self):
        result = self.run_script("scripts/session.sh", f"{KEY}\n{KEY}\n", REMOTE_NAMES="othericloud:")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any("create" in args for _, args in self.calls()))

    def test_invalid_confirmation_does_not_write_configuration(self):
        for inputs in ("\n", f"{KEY}\ndifferent\n"):
            with self.subTest(inputs_valid=False):
                result = self.run_script("scripts/session.sh", inputs)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.config.exists())
                self.assertEqual(self.calls(), [])

    def test_wrong_unlock_key_stops_without_replacing_existing_configuration(self):
        self.config.write_text(ENCRYPTED_FIXTURE)
        result = self.run_script(
            "scripts/session.sh", f"{KEY}\n", RCLONE_FAIL="config encryption set"
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.config.read_text(), ENCRYPTED_FIXTURE)
        self.assertEqual(len(self.calls()), 1)

    def test_setup_failures_do_not_continue_to_listing_or_report_success(self):
        for operation in ("config encryption check", "listremotes", "config create icloud"):
            with self.subTest(operation=operation):
                self.config.write_text(ENCRYPTED_FIXTURE)
                self.log.write_text("")
                result = self.run_script("scripts/session.sh", f"{KEY}\n", RCLONE_FAIL=operation)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn('Access verified', result.stdout)
                self.assertFalse(any("lsf" in args for _, args in self.calls()))

    def test_listing_failure_is_reported_without_claiming_access(self):
        self.config.write_text(ENCRYPTED_FIXTURE)
        result = self.run_script(
            "scripts/session.sh", f"{KEY}\n", REMOTE_NAMES="icloud:", RCLONE_FAIL="lsf"
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Could not read', result.stderr)
        self.assertNotIn('Access verified', result.stdout)

    def test_host_launcher_works_from_another_directory_and_reuses_state_volume(self):
        result = self.run_script("start.command")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([args[0] for _, args in self.calls()], ["ps", "build", "run"])
        self.assertEqual(self.calls()[1][1][-1], str(PROJECT))
        self.assertIn(
            "type=volume,source=icloud-linux-state,target=/state", self.calls()[-1][1]
        )

    def test_host_launcher_stops_when_docker_or_build_fails(self):
        for failure in ("ps", "build"):
            with self.subTest(failure=failure):
                self.log.write_text("")
                result = self.run_script("start.command", DOCKER_FAIL=failure)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(args[0] == "run" for _, args in self.calls()))


if __name__ == "__main__":
    unittest.main()
