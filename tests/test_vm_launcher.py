import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


PROJECT = Path(__file__).resolve().parents[1]
VM_UUID = '11111111-2222-3333-4444-555555555555'


class VmLauncherTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='icloud-vm-launcher-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        shutil.copy(PROJECT / 'open-ubuntu.command', self.root)
        (self.root / '.vm').mkdir()
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.log = self.root / 'calls.jsonl'
        for name in ('VBoxManage', 'open'):
            command = self.bin / name
            command.write_text(
                '#!/usr/bin/env python3\n'
                'import json, os, sys\n'
                'from pathlib import Path\n'
                'with open(os.environ["VM_TEST_LOG"], "a") as log:\n'
                '    log.write(json.dumps([Path(sys.argv[0]).name, sys.argv[1:]]) + "\\n")\n'
                'if sys.argv[1] == "showvminfo":\n'
                '    print("VMState=\\\"" + os.environ["VM_TEST_STATE"] + "\\\"")\n'
            )
            command.chmod(0o755)

    def run_launcher(self, state='poweroff'):
        return subprocess.run(
            ['bash', str(self.root / 'open-ubuntu.command')],
            env={
                **os.environ,
                'PATH': f'{self.bin}{os.pathsep}{os.environ["PATH"]}',
                'VM_TEST_LOG': str(self.log),
                'VM_TEST_STATE': state,
            },
            capture_output=True,
            text=True,
            timeout=10,
        )

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_missing_or_invalid_identifier_does_not_contact_virtualbox(self):
        for identifier in (None, '--name another-machine', ''):
            with self.subTest(identifier=identifier):
                if identifier is not None:
                    (self.root / '.vm/uuid').write_text(identifier + '\n')
                result = self.run_launcher()
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.log.exists())

    def test_only_the_dedicated_identifier_is_started(self):
        (self.root / '.vm/uuid').write_text(VM_UUID + '\n')
        result = self.run_launcher()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls(), [
            ['VBoxManage', ['showvminfo', VM_UUID, '--machinereadable']],
            ['VBoxManage', ['startvm', VM_UUID, '--type', 'gui']],
        ])

    def test_running_vm_is_preserved_and_manager_is_opened(self):
        (self.root / '.vm/uuid').write_text(VM_UUID + '\n')
        result = self.run_launcher(state='running')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls()[-1], ['open', ['-a', 'VirtualBox']])
        self.assertNotIn('startvm', json.dumps(self.calls()))


if __name__ == '__main__':
    unittest.main()
