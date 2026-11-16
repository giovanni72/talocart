import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('audit_dependencies', Path(__file__).resolve().parents[2] / 'scripts/audit-dependencies.py')
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class DependencyAudit(unittest.TestCase):
    def test_inactive_sqlite_exempts_only_known_advisory(self):
        self.assertEqual(audit.audit_arguments('sqlx v0.9.0\nsqlx-postgres v0.9.0\n', 'Cargo.lock'),
                         ['cargo', 'audit', '--file', 'Cargo.lock', '--ignore', 'RUSTSEC-2026-0459'])

    def test_active_sqlite_keeps_full_audit(self):
        for line in ('libsqlite3-sys v0.37.0', 'libsqlite3-sys v0.37.0 (*)'):
            self.assertEqual(audit.audit_arguments('sqlx v0.9.0\n' + line, 'Cargo.lock'),
                             ['cargo', 'audit', '--file', 'Cargo.lock'])
