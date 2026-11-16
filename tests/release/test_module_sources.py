"""Distribution contract: a customer can build the base without private packages."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('module_sources', ROOT / 'scripts/release/source.py')
source = importlib.util.module_from_spec(spec)
spec.loader.exec_module(source)

class SourceProfiles(unittest.TestCase):
    def public_fixture(self, root):
        files = {'Cargo.toml': '[workspace]\n', 'Cargo.lock': '', 'README.md': '# Base\n'}
        manifest = root / source.PUBLIC_FILES
        manifest.parent.mkdir(parents=True)
        for name, text in files.items():
            (root / name).write_text(text)
        names = [*files, source.PUBLIC_FILES.as_posix()]
        manifest.write_text(json.dumps({'format': 1, 'files': {n: n for n in names}}))
        return manifest, names

    def test_full_workspace_manifest_link_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            full = Path(temporary)
            root = full / 'base'
            (root / 'scripts/release').mkdir(parents=True)
            (full / 'integration').mkdir()
            (full / 'integration/Cargo.toml').write_text('[package]\n')
            (full / 'synthetic.txt').write_text('synthetic fixture')
            (full / 'Cargo.toml').symlink_to(full / 'synthetic.txt')
            (root / source.PUBLIC_FILES).write_text(json.dumps({'format': 1, 'files': {'Cargo.toml':'Cargo.toml'}}))
            with self.assertRaises(ValueError):
                source.public_files(root)

    def test_public_check_rejects_remapped_snapshot_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest, names = self.public_fixture(root)
            # A snapshot must validate the published path, not a different input.
            manifest.write_text(json.dumps({'format': 1, 'files': {
                n: 'Cargo.lock' if n == 'README.md' else n for n in names}}))
            (root / 'README.md').unlink()
            (root / 'README.md').symlink_to(root / 'Cargo.lock')
            with self.assertRaises(ValueError):
                source.check_public(root, no_history=True)

    def test_public_check_supports_git_worktrees_and_ignores_build_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / 'repo'
            root.mkdir()
            _, names = self.public_fixture(root)
            def git(*args):
                subprocess.run(['git', *args], cwd=root, check=True, capture_output=True)
            git('init', '-b', 'main')
            git('add', *names)
            git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-m', 'fixture')
            checkout = Path(temporary) / 'worktree'
            git('worktree', 'add', '--detach', str(checkout))
            (checkout / 'build-output.txt').write_text('generated')
            self.assertEqual(source.check_public(checkout), len(names))
            with self.assertRaisesRegex(ValueError, 'Git history'):
                source.check_public(checkout, no_history=True)

    def test_public_allowlist_rejects_module_paths_missing_files_and_symlinks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = root / source.PUBLIC_FILES
            manifest.parent.mkdir(parents=True)
            for name in ('modules/stripe/src/gateway.rs', '../private.txt', '/private.txt', 'missing.txt'):
                manifest.write_text(json.dumps({'format': 1, 'files': {'example.txt': name}}))
                with self.subTest(name=name), self.assertRaises(ValueError):
                    source.public_files(root)
            (root / 'private.txt').write_text('private synthetic fixture')
            (root / 'example.txt').symlink_to(root / 'private.txt')
            manifest.write_text(json.dumps({'format': 1, 'files': {'example.txt': 'example.txt'}}))
            with self.assertRaises(ValueError):
                source.public_files(root)

    def test_public_check_rejects_unreviewed_files_history_and_broken_documentation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            files = {'Cargo.toml': '[workspace]\n', 'Cargo.lock': '', 'README.md': '# Public base\n'}
            manifest = root / source.PUBLIC_FILES
            manifest.parent.mkdir(parents=True)
            for name, text in files.items():
                (root / name).write_text(text)
            names = [*files, source.PUBLIC_FILES.as_posix()]
            manifest.write_text(json.dumps({'format': 1, 'files': {n: n for n in names}}))
            self.assertEqual(source.check_public(root, no_history=True), len(names))
            (root / 'private-notes.txt').write_text('unreviewed synthetic content')
            with self.assertRaisesRegex(ValueError, 'file set differs'):
                source.check_public(root, no_history=True)
            (root / 'private-notes.txt').unlink()
            (root / '.git').mkdir()
            with self.assertRaisesRegex(ValueError, 'Git history'):
                source.check_public(root, no_history=True)
            (root / '.git').rmdir()
            (root / 'README.md').write_text('[private history](docs/private.md)\n')
            with self.assertRaisesRegex(ValueError, 'Broken public documentation'):
                source.check_public(root, no_history=True)

    def test_public_base_contains_only_reviewed_files_and_english_documentation(self):
        with tempfile.TemporaryDirectory(prefix='talocart-public-contract-') as temporary:
            destination = source.export(Path(temporary) / 'base')
            self.assertTrue((destination / 'migrations/0001_initial_schema.sql').is_file())
            self.assertTrue((destination / 'crates/web/tests/identity/orders/online_generic.rs').is_file())
            self.assertFalse((destination / 'audit-opus.md').exists())
            self.assertFalse((destination / 'note-interne.md').exists())
            self.assertFalse((destination / '.claude').exists())
            self.assertFalse((destination / 'docs/archive').exists())
            self.assertFalse((destination / 'scripts/test-native-payments.cjs').exists())
            self.assertFalse((destination / 'scripts/test-installable-packages.cjs').exists())
            self.assertIn('## Quick start', (destination / 'README.md').read_text())
            for path in (destination / 'apps/talocart').rglob('*.rs'):
                self.assertNotIn('talocart_koala_import', path.read_text())
                self.assertNotIn('feature = "koala-import"', path.read_text())
            workflow = (destination / '.github/workflows/ci.yml').read_text()
            self.assertNotIn('talocart-koala-import', workflow)
            self.assertNotIn('modules/stripe', workflow)

    def test_base_runtime_dependency_graph_excludes_koala_and_mysql(self):
        tree = subprocess.check_output(['cargo', 'tree', '--offline', '--locked', '-p', 'talocart', '-e', 'normal', '--prefix', 'none'], cwd=ROOT, text=True)
        self.assertNotIn('talocart-koala-import', tree)
        self.assertNotIn('talocart-stripe', tree)
        self.assertNotIn('sqlx-mysql', tree)

    def test_export_preserves_rust_sources_verbatim(self):
        with tempfile.TemporaryDirectory() as temporary:
            destination = source.export(Path(temporary) / 'base')
            for folder in ('apps', 'crates'):
                for path in (ROOT / folder).rglob('*.rs'):
                    self.assertEqual(path.read_bytes(), (destination / path.relative_to(ROOT)).read_bytes())

    def test_base_source_profile_physically_omits_module_and_resolves_its_own_lock(self):
        with tempfile.TemporaryDirectory(prefix='talocart-source-contract-') as temporary:
            destination = source.export(Path(temporary) / 'base')
            subprocess.run(['cargo', 'fmt', '--manifest-path', str(destination / 'Cargo.toml'), '--all', '--check'], check=True, capture_output=True)
            self.assertFalse((destination / 'modules').exists())
            for migration in (destination / 'migrations').glob('*.sql'):
                self.assertNotIn('catalog_import_', migration.read_text())
            self.assertFalse((destination / '.git').exists())
            self.assertFalse((destination / '.local').exists())
            self.assertTrue((destination / 'LICENSE').read_text().startswith('MIT License'))
            self.assertNotIn('talocart-koala-import', (destination / 'Cargo.lock').read_text())
            self.assertNotIn('talocart-stripe', (destination / 'Cargo.lock').read_text())
            self.assertFalse((destination/'assets/checkout-stripe.js').exists())
            self.assertFalse((destination/'crates/platform/src/stripe.rs').exists())
            for manifest in ('apps/talocart/Cargo.toml', 'crates/web/Cargo.toml'):
                content = tomllib.loads((destination / manifest).read_text())
                self.assertNotIn('talocart-koala-import', content.get('dependencies', {}))
                self.assertNotIn('talocart-koala-import', content.get('dev-dependencies', {}))
                self.assertNotIn('talocart-stripe', content.get('dependencies', {}))
                self.assertNotIn('talocart-stripe', content.get('dev-dependencies', {}))
            for path in (destination/'crates/web/tests').rglob('*.rs'):
                self.assertNotIn('modules/koala-import', path.read_text())
                self.assertNotIn('talocart_koala_import', path.read_text())
                self.assertNotIn('talocart_stripe', path.read_text())
            groups = json.loads((destination/'scripts/test-suite.json').read_text())['groups']
            self.assertFalse({'imports', 'native-package', 'unit-koala', 'clippy-koala', 'postgres-imports'} & {g['id'] for g in groups})
            with self.assertRaises(ValueError):
                source.export(destination)

    def test_optional_source_profiles_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            for profile in ('koala-import', 'stripe', 'all'):
                with self.subTest(profile=profile), self.assertRaises(ValueError):
                    source.export(Path(temporary) / profile, profile)

if __name__ == '__main__':
    unittest.main()
