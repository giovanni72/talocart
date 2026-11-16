"""Contracts for repeatable test services without application configuration."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('test_env', Path(__file__).resolve().parents[2] / 'scripts/test-env.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class TestEnvironment(unittest.TestCase):
    def test_test_services_override_foreign_database_and_strip_application_overrides(self):
        env = module.environment('synthetic', {
            'PATH': '/bin', 'TALOCART_DATABASE_URL': 'application',
            'TALOCART_MEDIA_ROOT': '/application/media', 'TALOCART_PACKAGES_ROOT': '/application/packages',
            'TALOCART_TEST_DATABASE_URL': 'postgres://remote/production',
            'TALOCART_TEST_SMTP_PORT': '25', 'TALOCART_TEST_MAILPIT_URL': 'https://remote',
            'TALOCART_TEST_BINARY': '/test/binary', 'TALOCART_BROWSER_CHANNEL': 'chrome',
            'TALOCART_BASE_TEST_BINARY': '/test/base', 'TALOCART_MODULE_TEST_BINARY': '/test/module',
        })
        self.assertEqual(env['TALOCART_TEST_DATABASE_URL'], 'postgres://talocart_test:synthetic@127.0.0.1:55432/talocart_test')
        self.assertEqual(env['TALOCART_TEST_SMTP_PORT'], '11025')
        self.assertEqual(env['TALOCART_TEST_MAILPIT_URL'], 'http://127.0.0.1:18025')
        self.assertEqual(env['TALOCART_TEST_BINARY'], '/test/binary')
        self.assertEqual(env['TALOCART_BASE_TEST_BINARY'], '/test/base')
        self.assertEqual(env['TALOCART_MODULE_TEST_BINARY'], '/test/module')
        self.assertEqual(env['PATH'], '/bin')
        for name in ['TALOCART_DATABASE_URL', 'TALOCART_MEDIA_ROOT', 'TALOCART_PACKAGES_ROOT']:
            self.assertNotIn(name, env)

    def test_private_password_is_persisted_and_never_rotated_on_reuse(self):
        with tempfile.TemporaryDirectory() as folder:
            config = Path(folder) / 'private' / 'test-services.json'
            with patch.object(module, 'CONFIG', config):
                first = module.password()
                self.assertEqual(first, module.password())
                self.assertEqual(len(first), 64)
                self.assertEqual(os.stat(config).st_mode & 0o777, 0o600)
                config.write_text(json.dumps({'password': 'invalid'}))
                with self.assertRaises(ValueError):
                    module.password()
                self.assertEqual(json.loads(config.read_text())['password'], 'invalid')
