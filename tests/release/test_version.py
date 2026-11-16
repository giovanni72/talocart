import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/release'))
import version
import package
from operations import OperationError


class GitFixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.git('init', '-b', 'main')
        self.git('config', 'user.email', 'version@example.test')
        self.git('config', 'user.name', 'Version test')
        (self.root / 'Cargo.toml').write_text('[workspace]\nmembers=["app"]\nresolver="3"\n[workspace.package]\nversion="1.2.3"\n')
        (self.root / 'app/src').mkdir(parents=True)
        (self.root / 'app/Cargo.toml').write_text('[package]\nname="version-fixture"\nversion.workspace=true\nedition="2024"\n')
        (self.root / 'app/src/main.rs').write_text('fn main() {}\n')
        (self.root / '.gitignore').write_text('/target/\n/ignored\n')
        subprocess.run(['cargo', 'generate-lockfile', '--offline'], cwd=self.root, check=True, capture_output=True)
        self.commit()

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, text=True, stderr=subprocess.DEVNULL).strip()

    def commit(self):
        self.git('add', 'Cargo.toml', 'Cargo.lock', 'app', '.gitignore')
        self.git('commit', '-m', 'Synthetic version fixture')


class VersionContracts(GitFixture):
    def test_semver_accepts_candidates_and_rejects_ambiguous_numbers(self):
        for value in ['0.1.0', '5.4.0', '5.4.0-rc.1', '5.4.0-alpha-beta.0']:
            self.assertEqual(version.validate(value), value)
        for value in ['', 'v5.4.0', '05.4.0', '5.04.0', '5.4', '5.4.0-rc.01', '5.4.0-', '5.4.0+other', '../5.4.0']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                version.validate(value)

    def test_prepare_updates_cargo_and_lock_and_preserves_other_files(self):
        previous = (self.root / 'app/src/main.rs').read_bytes()
        version.prepare('1.3.0-rc.1', self.root)
        self.assertEqual(version.current(self.root), '1.3.0-rc.1')
        self.assertIn('version = "1.3.0-rc.1"', (self.root / 'Cargo.lock').read_text())
        self.assertEqual((self.root / 'app/src/main.rs').read_bytes(), previous)
        self.assertEqual(version.check(self.root), '1.3.0-rc.1')

    def test_failed_prepare_restores_manifest_and_lock(self):
        before = [(self.root / name).read_bytes() for name in ['Cargo.toml', 'Cargo.lock']]
        with patch.object(version.subprocess, 'run', side_effect=OSError('Synthetic failure')):
            with self.assertRaises(OSError):
                version.prepare('1.3.0', self.root)
        self.assertEqual(before, [(self.root / name).read_bytes() for name in ['Cargo.toml', 'Cargo.lock']])

    def test_release_tag_requires_main_clean_commit_matching_version_and_is_immutable(self):
        with self.assertRaises(ValueError):
            version.check(self.root, release=True)
        self.git('checkout', '-b', 'feature')
        with self.assertRaises(ValueError):
            version.tag(self.root)
        self.git('checkout', 'main')
        (self.root / 'uncommitted').write_text('fixture')
        with self.assertRaises(ValueError):
            version.tag(self.root)
        (self.root / 'uncommitted').unlink()
        self.assertEqual(version.tag(self.root), '1.2.3')
        self.assertEqual(version.check(self.root, release=True), '1.2.3')
        with self.assertRaises(ValueError):
            version.tag(self.root)
        version.prepare('1.3.0', self.root)
        self.commit()
        with self.assertRaises(ValueError):
            version.check(self.root, release=True)

    def test_stale_lock_is_rejected(self):
        manifest = self.root / 'Cargo.toml'
        manifest.write_text(manifest.read_text().replace('1.2.3', '1.3.0'))
        with self.assertRaises(ValueError):
            version.check(self.root)

    def test_package_rejects_wrong_binary_version_commit_dirty_and_untagged_release(self):
        info = {'version': '1.2.3', 'commit': 'a' * 40, 'source_dirty': False, 'tagged': False, 'source_fingerprint': 'f' * 40}
        for field, value in [('version', '0.1.0'), ('commit', 'b' * 40), ('source_dirty', True), ('source_fingerprint', 'old')]:
            with self.subTest(field=field), patch.object(package.subprocess, 'run', return_value=
                    subprocess.CompletedProcess([], 0, json.dumps({**info, field: value}))):
                with self.assertRaises(OperationError):
                    package.build_identity(self.root / 'binary', '1.2.3', 'a' * 40, False, 'f' * 40)
        with patch.object(package.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, json.dumps(info))):
            self.assertEqual(package.build_identity(self.root / 'binary', '1.2.3', 'a' * 40, False, 'f' * 40), info)
            with self.assertRaises(OperationError):
                package.build_identity(self.root / 'binary', '1.2.3', 'a' * 40, False, 'f' * 40, release=True)


class CompiledIdentityContracts(GitFixture):
    @classmethod
    def setUpClass(cls):
        cls.compiler_temp = tempfile.TemporaryDirectory()
        cls.executable = Path(cls.compiler_temp.name) / 'identity'
        source = Path(cls.compiler_temp.name) / 'main.rs'
        source.write_text('#[path=' + json.dumps(str(ROOT / 'scripts/build-version.rs')) + ']\nmod identity;\n'
                          'fn main() { let args: Vec<_> = std::env::args().collect(); '
                          'identity::emit(std::path::Path::new(&args[1]), &args[2]); }\n')
        subprocess.run(['rustc', '--edition=2024', str(source), '-o', str(cls.executable)], check=True, capture_output=True)

    @classmethod
    def tearDownClass(cls):
        cls.compiler_temp.cleanup()

    def identity(self, root=None):
        text = subprocess.check_output([str(self.executable), str(root or self.root), '1.2.3'], text=True)
        return dict(line.removeprefix('cargo:rustc-env=TALOCART_BUILD_').split('=', 1)
                    for line in text.splitlines() if line.startswith('cargo:rustc-env='))

    def test_debug_identity_keeps_staged_source_deletions(self):
        source = self.root/'apps/fixture/src/removed.rs'
        source.parent.mkdir(parents=True)
        source.write_text('pub fn removed() {}')
        self.git('add', 'apps'); self.git('commit', '-m', 'Tracked source')
        self.git('tag', '-a', 'v1.2.3', '-m', 'Tagged source')
        with patch.dict(os.environ, {'PROFILE':'debug', 'TALOCART_BUILD_IDENTITY_EXACT':'0'}):
            before = self.identity()
            source.unlink()
            unstaged = self.identity()
            self.git('add', '-u')
            staged = self.identity()
        self.assertEqual(staged, unstaged, 'Staging a deletion must not change its source identity')
        self.assertEqual(staged['DIRTY'], 'true')
        self.assertEqual(staged['TAGGED'], 'false')
        self.assertNotEqual(staged['FINGERPRINT'], before['FINGERPRINT'])

    def test_debug_identity_ignores_docs_tests_and_index_but_tracks_sources(self):
        source = self.root/'apps/fixture/src/main.rs'
        source.parent.mkdir(parents=True)
        source.write_text('fn main() {}')
        self.git('add', 'apps'); self.git('commit', '-m', 'Source fixture')
        def emit(exact=False):
            env = {**os.environ, 'PROFILE':'debug'}
            env.pop('TALOCART_BUILD_IDENTITY_EXACT', None)
            if exact:
                env['TALOCART_BUILD_IDENTITY_EXACT'] = '1'
            output = subprocess.check_output([str(self.executable), str(self.root), '1.2.3'], env=env, text=True)
            identity = [line for line in output.splitlines() if line.startswith('cargo:rustc-env=')]
            return output, identity
        output, before = emit()
        self.assertNotIn(str(self.root/'.git/index'), output)
        (self.root/'README.md').write_text('Only documentation')
        (self.root/'tests').mkdir(); (self.root/'tests/regression.rs').write_text('// test')
        (self.root/'apps/fixture/tests').mkdir(); (self.root/'apps/fixture/tests/ui.rs').write_text('// test')
        self.git('add', 'README.md', 'tests', 'apps/fixture/tests')
        self.assertEqual(emit()[1], before)
        self.assertNotEqual(emit(exact=True)[1], before)
        source.write_text('fn main() { println!("updated"); }')
        self.assertNotEqual(emit()[1], before)
        license_file = self.root/'LICENSE'
        license_file.write_text('Embedded license')
        licensed = emit()[1]
        license_file.write_text('Changed embedded license')
        self.assertNotEqual(emit()[1], licensed)
        source.unlink()
        output, _ = emit()
        watched = [Path(line.split('=', 1)[1]) for line in output.splitlines() if line.startswith('cargo:rerun-if-changed=')]
        self.assertTrue(all(path.exists() for path in watched))
        self.assertIn(source.parent, watched)
        release = subprocess.check_output([str(self.executable), str(self.root), '1.2.3'], text=True,
            env={**os.environ, 'PROFILE':'release', 'TALOCART_BUILD_IDENTITY_EXACT':'0'})
        self.assertIn('TALOCART_BUILD_FINGERPRINT=' + version.fingerprint(self.root), release)

    def test_real_cargo_debug_rebuilds_only_for_sources_refs_or_exact_identity(self):
        app = self.root/'apps/fixture'
        (app/'src').mkdir(parents=True)
        (app/'Cargo.toml').write_text('[package]\nname="incremental-fixture"\nversion="1.2.3"\nedition="2024"\n')
        (self.root/'Cargo.toml').write_text('[workspace]\nmembers=["apps/fixture"]\nresolver="3"\n')
        (app/'build.rs').write_text('#[path=' + json.dumps(str(ROOT/'scripts/build-version.rs')) + ']\nmod identity;\n'
            'fn main() { identity::emit(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").as_path(), "1.2.3"); }\n')
        removable = app/'src/removed.rs'
        removable.write_text('// tracked source removed by the regression\n')
        source = app/'src/main.rs'
        source.write_text('fn main() { println!("{}:{}:{}", env!("TALOCART_BUILD_COMMIT"), '
            'env!("TALOCART_BUILD_FINGERPRINT"), env!("TALOCART_BUILD_TAGGED")); }\n')
        subprocess.run(['cargo', 'generate-lockfile', '--offline'], cwd=self.root, check=True, capture_output=True)
        self.git('add', '.'); self.git('commit', '-m', 'Incremental fixture')
        env = os.environ.copy(); env.pop('TALOCART_BUILD_IDENTITY_EXACT', None)
        env.pop('CARGO_TARGET_DIR', None)
        def build(exact=False):
            result = subprocess.run(['cargo','build','--offline','--message-format=json'], cwd=self.root,
                env={**env, **({'TALOCART_BUILD_IDENTITY_EXACT':'1'} if exact else {})},
                capture_output=True, text=True, check=True)
            artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
            binary = next(a for a in artifacts if a.get('reason') == 'compiler-artifact' and a['target']['kind'] == ['bin'])
            return binary['fresh'], subprocess.check_output([binary['executable']], text=True).strip()
        self.assertFalse(build()[0])
        self.assertTrue(build()[0])
        (self.root/'docs').mkdir(); (self.root/'docs/note.md').write_text('Documentation')
        (app/'tests').mkdir(); (app/'tests/case.rs').write_text('#[test] fn example() {}')
        self.git('add', 'docs', 'apps/fixture/tests')
        self.assertTrue(build()[0], 'Prose, test fixtures and index updates must leave the app fresh')
        source.write_text(source.read_text() + '// source edit\n')
        self.assertFalse(build()[0]); self.assertTrue(build()[0])
        extra = app/'src/new.rs'; extra.write_text('// newly discovered source\n')
        self.assertFalse(build()[0]); self.assertTrue(build()[0])
        extra.unlink()
        self.assertFalse(build()[0]); self.assertTrue(build()[0], 'Deletion must not create a rebuild loop')
        before = build()[1]
        self.git('add', '.'); self.git('commit', '-m', 'New source commit')
        fresh, after = build()
        self.assertFalse(fresh); self.assertNotEqual(before, after)
        self.git('tag', '-a', 'v1.2.3', '-m', 'Release identity fixture')
        fresh, tagged = build()
        self.assertFalse(fresh); self.assertTrue(tagged.endswith(':true'))
        self.git('rm', 'apps/fixture/src/removed.rs')
        fresh, deleted = build()
        self.assertFalse(fresh)
        self.assertNotEqual(deleted, tagged)
        self.assertTrue(deleted.endswith(':false'))
        self.assertTrue(build()[0], 'Staged deletion must not create a rebuild loop')
        self.git('tag', '-d', 'v1.2.3')
        fresh, untagged = build()
        self.assertFalse(fresh); self.assertTrue(untagged.endswith(':false'))
        self.assertFalse(build(exact=True)[0])
        (self.root/'docs/note.md').write_text('Exact packaging identity must see documentation edits')
        self.assertFalse(build(exact=True)[0])
        self.assertTrue(build(exact=True)[0])

    def test_clean_tag_development_dirty_ignored_and_commit_changes(self):
        before = self.identity()
        self.assertTrue(before['DISPLAY'].startswith('1.2.3-dev+'))
        self.git('tag', 'v1.2.3')
        self.assertEqual(self.identity()['TAGGED'], 'false')
        self.git('tag', '-d', 'v1.2.3')
        self.git('tag', '-a', 'v1.2.3', '-m', 'Fixture')
        tagged = self.identity()
        self.assertEqual(tagged['TAGGED'], 'true')
        self.assertNotIn('-dev', tagged['DISPLAY'])
        (self.root / 'ignored').write_text('not source')
        self.assertEqual(self.identity(), tagged)
        source = self.root / 'app/src/main.rs'
        source.write_text('fn main() { println!("new code"); }\n')
        dirty = self.identity()
        self.assertEqual(dirty['FINGERPRINT'], version.fingerprint(self.root))
        self.assertNotEqual(dirty['FINGERPRINT'], tagged['FINGERPRINT'])
        self.assertEqual(dirty['DIRTY'], 'true')
        self.assertTrue(dirty['DISPLAY'].endswith('.dirty'))
        self.assertEqual(dirty['TAGGED'], 'false')
        untracked = self.root / 'new-file'
        untracked.write_text('fixture')
        self.assertEqual(self.identity()['FINGERPRINT'], version.fingerprint(self.root))
        self.assertNotEqual(self.identity()['FINGERPRINT'], dirty['FINGERPRINT'])
        untracked.unlink()
        self.commit()
        self.assertNotEqual(self.identity()['COMMIT'], before['COMMIT'])
        self.assertEqual(self.identity()['DIRTY'], 'false')

    def test_removed_sources_do_not_register_missing_watch_paths_and_new_modules_are_watched(self):
        before = self.identity()
        tracked = self.root / 'app/src/main.rs'
        tracked.unlink()
        module = self.root / 'modules/koala-import/src'
        module.mkdir(parents=True)
        (module / 'lib.rs').write_text('// synthetic extension\n')
        output = subprocess.check_output([str(self.executable), str(self.root), '1.2.3'], text=True)
        watched = [Path(line.split('=', 1)[1]) for line in output.splitlines() if line.startswith('cargo:rerun-if-changed=')]
        self.assertTrue(watched)
        self.assertTrue(all(path.exists() for path in watched), 'Missing watched paths make every Cargo command rebuild')
        self.assertIn(tracked.parent, watched, 'Watch an existing parent to detect source recreation')
        self.assertIn(self.root / 'modules', watched, 'Detect new files in separately owned module packages')
        self.assertNotEqual(before['FINGERPRINT'], self.identity()['FINGERPRINT'])

    def test_full_base_identity_normalizes_crate_relative_root(self):
        crate = self.root / 'base/crates/platform'
        crate.mkdir(parents=True)
        (self.root / 'integration').mkdir()
        (self.root / 'integration/Cargo.toml').write_text('[package]\nname="integration"\n')
        identity = self.identity(crate / '../..')
        self.assertEqual(identity['COMMIT'], self.identity()['COMMIT'])
        self.assertNotEqual(identity['COMMIT'], 'unknown')
        archive = self.root / 'base/archive'
        archive.mkdir()
        self.assertEqual(self.identity(archive)['COMMIT'], 'unknown')

    def test_full_moved_sources_do_not_watch_runtime_root(self):
        source = self.root / 'apps/fixture/src/lib.rs'
        source.parent.mkdir(parents=True)
        source.write_text('pub fn fixture() {}')
        self.git('add', 'apps')
        self.git('commit', '-m', 'Source before relocation')
        (self.root / 'base').mkdir()
        (self.root / 'apps').rename(self.root / 'base/apps')
        (self.root / 'integration').mkdir()
        (self.root / 'integration/Cargo.toml').write_text('[package]\n')
        for profile in ('debug', 'release'):
            with patch.dict(os.environ, {'PROFILE':profile, 'TALOCART_BUILD_IDENTITY_EXACT':'0'}):
                output = subprocess.check_output([str(self.executable), str(self.root / 'base'), '1.2.3'], text=True)
            watched = [Path(line.split('=',1)[1]) for line in output.splitlines() if line.startswith('cargo:rerun-if-changed=')]
            self.assertNotIn(self.root, watched)
            self.assertNotIn(self.root / 'base', watched)
            self.assertIn(self.root / 'base/apps/fixture/src/lib.rs', watched)

    def test_missing_git_metadata_and_linked_worktree(self):
        archive = self.root / 'archive'
        archive.mkdir()
        self.assertEqual(self.identity(archive)['COMMIT'], 'unknown')
        with tempfile.TemporaryDirectory() as other:
            linked = Path(other) / 'linked'
            self.git('worktree', 'add', '--detach', str(linked), 'HEAD')
            self.assertEqual(self.identity(linked)['COMMIT'], self.identity()['COMMIT'])
            self.git('tag', '-a', 'v1.2.3', '-m', 'Fixture')
            self.assertEqual(self.identity(linked)['TAGGED'], 'true')
            self.git('worktree', 'remove', str(linked))


if __name__ == '__main__':
    unittest.main()
