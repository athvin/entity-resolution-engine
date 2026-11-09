"""Exercise cache corruption, downloads, and disk guards without installing tools."""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('installer', HERE / 'install-tools.py')
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class Tools(unittest.TestCase):
    def test_verified_hit_needs_no_network_and_corruption_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = root / 'tool.tar.gz'
            with tarfile.open(archive, 'w:gz') as bundle:
                member = tarfile.TarInfo('bin/tool')
                member.size = 4
                bundle.addfile(member, io.BytesIO(b'tool'))
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            archive.rename(root / (digest + '.tar.gz'))
            pin = dict(version='1', sha256=digest, member='bin/tool', url='https://unused.invalid')
            with patch.object(installer.subprocess, 'run', side_effect=AssertionError('network on hit')):
                installer.install('tool', pin, root, root / 'bin')
                self.assertEqual((root / 'bin/tool').read_bytes(), b'tool')
                (root / (digest + '.tar.gz')).write_bytes(b'corrupt')
                with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                    installer.install('tool', pin, root, root / 'bin')

    def test_download_failure_is_bounded_and_not_cached(self):
        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(installer.subprocess, 'run', side_effect=subprocess.CalledProcessError(22, 'curl')) as curl:
                with self.assertRaises(subprocess.CalledProcessError):
                    installer.install('tool', dict(version='1', sha256='0'*64, url='https://unused.invalid'), Path(tmp), Path(tmp)/'bin')
                self.assertIn('--max-time', curl.call_args.args[0])
                self.assertFalse(list(Path(tmp).glob('*.tar.gz')))

    def test_pins_have_checksums(self):
        pins = json.loads((HERE / 'tools.json').read_text())
        for pin in pins.values():
            self.assertRegex(pin['sha256'], '^[a-f0-9]{64}$')
            self.assertTrue(pin['url'].startswith('https://github.com/'))


class Disk(unittest.TestCase):
    def check_disk(self, gib, minimum='40'):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root/'df').write_text('#!/bin/sh\nprintf "Filesystem blocks used available capacity mount\\n/dev/test 999999999 0 '+str(gib*1024*1024)+' 0 /\\n"\n')
            (root/'sudo').write_text('#!/bin/sh\necho cleanup >> "$DISK_LOG"\n')
            for f in root.iterdir():
                f.chmod(0o755)
            result = subprocess.run(['bash', str(HERE/'reclaim-disk.sh')], capture_output=True,
                                    env={**os.environ, 'PATH': str(root)+':'+os.environ['PATH'],
                                         'DISK_LOG': str(root/'log'), 'CI_MIN_FREE_GIB': minimum})
            return result.returncode, (root/'log').exists()

    def test_high_disk_skips_deletion(self):
        self.assertEqual(self.check_disk(88), (0, False))

    def test_low_disk_cleans_and_refuses_if_still_low(self):
        self.assertEqual(self.check_disk(10), (1, True))

    def test_bad_threshold_refused_without_deletion(self):
        self.assertEqual(self.check_disk(88, 'garbage'), (2, False))


class GeneratedReferences(unittest.TestCase):
    def test_python_bytecode_does_not_enter_infrastructure_references(self):
        import re
        source = (HERE.parent/'skills-check.sh').read_text()
        function = re.search(r'^infra_references\(\) \{.*?^\}', source, re.M | re.S).group()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root/'scripts/__pycache__').mkdir(parents=True)
            (root/'scripts/direct.py').write_text('infra/k8s/probe')
            (root/'scripts/__pycache__/direct.pyc').write_bytes(b'\0infra/k8s/probe\0')
            output = subprocess.check_output(['bash', '-c', function+'\ninfra_references infra/k8s/probe'], cwd=root, text=True)
            self.assertEqual(output.strip(), 'scripts/direct.py')
