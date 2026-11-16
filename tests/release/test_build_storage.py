import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    'build_storage', Path(__file__).resolve().parents[2] / 'scripts/build-storage.py')
STORAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STORAGE)


class BuildStorageTests(unittest.TestCase):
    def test_hardlinks_count_once_and_symlinks_are_not_followed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data = root / 'artifact'
            data.write_bytes(b'x' * 8192)
            os.link(data, root / 'second-name')
            (root / 'cycle').symlink_to(root, target_is_directory=True)
            self.assertEqual(STORAGE.allocated_bytes(root), data.stat().st_blocks * 512)
            with self.assertRaises(ValueError):
                STORAGE.allocated_bytes(root / 'cycle')

    def test_budget_exit_codes_and_missing_profile(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            debug = root / 'target/debug'
            debug.mkdir(parents=True)
            (debug / 'artifact').write_bytes(b'x' * 8192)
            for limit, expected in [('40', 0), ('0.000001', 1)]:
                output = io.StringIO()
                with patch.object(STORAGE, 'ROOT', root), contextlib.redirect_stdout(output):
                    self.assertEqual(STORAGE.main(['--json', '--max-gib', limit]), expected)
                report = json.loads(output.getvalue())
                self.assertEqual(report['over_budget'], bool(expected))
                self.assertEqual(report['allocated_bytes']['debug-full'], 0)
            self.assertTrue((debug / 'artifact').exists())

    def test_invalid_limit_and_scan_error_are_not_success(self):
        with contextlib.redirect_stderr(io.StringIO()):
            for value in ['0', '-1', 'nan', 'inf']:
                with self.assertRaises(SystemExit):
                    STORAGE.main(['--max-gib', value])
            with patch.object(STORAGE, 'allocated_bytes', side_effect=PermissionError):
                self.assertEqual(STORAGE.main([]), 2)
