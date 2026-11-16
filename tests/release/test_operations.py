import json
import os
from pathlib import Path, PurePosixPath
import struct
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import re
import stat
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[2] / 'scripts' / 'release'
sys.path.insert(0, str(SCRIPTS))
import operations as ops
import package as release


class RecoveryContracts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.addCleanup(self.temp.cleanup)

    def fixture(self):
        root = self.root / 'backup'
        root.mkdir()
        (root / 'database.dump').write_bytes(b'fixture')
        (root / 'backup.json').write_text(json.dumps({'format': 1, 'files': ops.files(root)}))
        return root

    def test_manifest_detects_corruption_missing_and_unexpected_files(self):
        root = self.fixture()
        self.assertEqual(ops.verify(root)['format'], 1)
        (root / 'database.dump').write_bytes(b'corrupt')
        with self.assertRaises(ops.OperationError):
            ops.verify(root)
        (root / 'database.dump').write_bytes(b'fixture')
        (root / 'unexpected').write_text('injected')
        with self.assertRaises(ops.OperationError):
            ops.verify(root)
        (root / 'unexpected').unlink()
        (root / 'database.dump').unlink()
        with self.assertRaises(ops.OperationError):
            ops.verify(root)

    def test_symlinks_are_rejected_even_when_their_bytes_match(self):
        root = self.fixture()
        outside = self.root / 'private'
        outside.write_bytes(b'fixture')
        (root / 'database.dump').unlink()
        (root / 'database.dump').symlink_to(outside)
        with self.assertRaises(ops.OperationError):
            ops.verify(root)

    def config(self, options=''):
        config = self.root / 'config.toml'
        config.write_text('[database]\nurl="postgres://user:synthetic%40secret@127.0.0.1/test' + options + '"\n[media]\nroot="media"\n')
        return config

    def test_online_preflight_estimates_without_backup_and_uses_readonly_connections(self):
        config = self.config()
        media = self.root / 'media'
        media.mkdir()
        (media / 'original').write_bytes(b'preserve')
        binary = self.root / 'talocart'
        binary.write_bytes(b'synthetic binary')
        def query(env, sql):
            self.assertEqual(env['PGOPTIONS'], '-c default_transaction_read_only=on')
            return '170006' if 'server_version_num' in sql else '1024'
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'query', side_effect=query), patch.object(ops, 'run', return_value=b'pg_dump (PostgreSQL) 17.6'):
            report = ops.preflight(config, binary)
        self.assertEqual(report['database_bytes'], 1024)
        self.assertEqual(report['storage_bytes'], 8)
        self.assertGreater(report['backup_bytes'], report['database_bytes'] + report['storage_bytes'])
        self.assertFalse((self.root / 'backup').exists())
        self.assertEqual((media / 'original').read_bytes(), b'preserve')
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'query', return_value='170006'), patch.object(ops, 'run', return_value=b'pg_dump (PostgreSQL) 16.1'):
            with self.assertRaises(ops.OperationError): ops.preflight(config, binary)

    def test_credentials_are_environment_only_and_paths_resolve_from_config(self):
        with patch.dict(os.environ, {}, clear=True):
            media, env = ops.configuration(self.config())
        self.assertEqual(media, self.root / 'media')
        self.assertEqual(env['PGPASSWORD'], 'synthetic@secret')
        self.assertEqual(env['PGDATABASE'], 'test')

    def test_runtime_overrides_and_partial_database_backups_are_rejected(self):
        config = self.config()
        with patch.dict(os.environ, {'TALOCART_MEDIA_ROOT': '/other'}, clear=True):
            with self.assertRaises(ops.OperationError):
                ops.configuration(config)
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(ops.OperationError):
                ops.configuration(self.config('?options=-csearch_path%3Dpartial'))

    def test_backup_destination_cannot_be_hidden_inside_media_by_dotdot(self):
        config = self.config()
        media = self.root / 'media'
        media.mkdir()
        (self.root / 'unused').mkdir()
        config.write_text(config.read_text().replace('root="media"', 'root="unused/../media"'))
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline') as check:
            with self.assertRaises(ops.OperationError):
                ops.backup(config, self.root / 'binary', media / 'backup')
            check.assert_not_called()

    def test_nonempty_database_refuses_restore_before_file_copy(self):
        root = self.fixture()
        for name in ['talocart', 'talocart.toml']:
            (root / name).write_text('fixture')
        (root / 'media').mkdir()
        (root / 'backup.json').unlink()
        (root / 'backup.json').write_text(json.dumps({'format': 1, 'files': ops.files(root)}))
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='1'):
            with self.assertRaises(ops.OperationError):
                ops.restore(root, self.config())
        self.assertFalse((self.root / 'media').exists())

    def test_failed_database_restore_preserves_media_for_inspection(self):
        root = self.fixture()
        for name in ['talocart', 'talocart.toml']:
            (root / name).write_text('fixture')
        (root / 'media').mkdir()
        (root / 'media' / 'photo').write_bytes(b'photo')
        (root / 'backup.json').unlink()
        (root / 'backup.json').write_text(json.dumps({'format': 1, 'files': ops.files(root)}))
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='0'), patch.object(ops, 'run', side_effect=ops.OperationError('fixture')):
            with self.assertRaises(ops.OperationError):
                ops.restore(root, self.config())
        self.assertEqual((self.root / 'media/photo').read_bytes(), b'photo')
        self.assertEqual((root / 'media/photo').read_bytes(), b'photo')


    def complete_backup(self):
        root = self.fixture()
        for name in ['talocart', 'talocart.toml']:
            (root / name).write_text('fixture')
        (root / 'media').mkdir()
        (root / 'media/photo').write_bytes(b'photo')
        (root / 'backup.json').unlink()
        (root / 'backup.json').write_text(json.dumps({'format': 1, 'files': ops.files(root), 'migration_count': 1}))
        return root

    def test_restore_preserves_private_smtp_key_and_rejects_existing_destination(self):
        root = self.complete_backup()
        secrets = root / 'smtp-secrets'
        secrets.mkdir(mode=0o700)
        (secrets / 'smtp.key').write_bytes(bytes(range(32)))
        (root / 'backup.json').unlink()
        (root / 'backup.json').write_text(json.dumps({'format': 1, 'files': ops.files(root), 'migration_count': 1, 'smtp_secrets_included': True}))
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', side_effect=['0', '1']), patch.object(ops, 'run'):
            ops.restore(root, self.config())
        key = self.root / 'media.secrets/smtp.key'
        self.assertEqual(key.read_bytes(), bytes(range(32)))
        self.assertEqual(key.stat().st_mode & 0o777, 0o600)
        self.assertEqual(key.parent.stat().st_mode & 0o777, 0o700)
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'run') as run:
            with self.assertRaisesRegex(ops.OperationError, 'new SMTP secrets path'):
                ops.restore(root, self.config())
            run.assert_not_called()

    def test_smtp_key_manifest_mismatch_aborts_before_restore(self):
        root = self.complete_backup()
        manifest = json.loads((root / 'backup.json').read_text())
        manifest['smtp_secrets_included'] = True
        (root / 'backup.json').write_text(json.dumps(manifest))
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'run') as run:
            with self.assertRaisesRegex(ops.OperationError, 'Incomplete SMTP secrets backup'):
                ops.restore(root, self.config())
            run.assert_not_called()

    def test_partial_media_copy_does_not_publish_or_start_sql(self):
        root = self.complete_backup()
        def fail_copy(source, target):
            target.mkdir()
            (target / 'partial').write_bytes(b'partial')
            raise OSError('synthetic disk full')
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='0'), patch.object(ops, 'copy_tree', side_effect=fail_copy), patch.object(ops, 'run') as run:
            with self.assertRaises(OSError):
                ops.restore(root, self.config())
            run.assert_not_called()
        self.assertFalse((self.root / 'media').exists())
        self.assertEqual(list(self.root.glob('.talocart-restore-*')), [])
        self.assertEqual((root / 'media/photo').read_bytes(), b'photo')

    def test_restore_timeout_preserves_media_and_uses_configured_limit(self):
        root = self.complete_backup()
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='0'), patch.object(ops.subprocess, 'run', side_effect=subprocess.TimeoutExpired('pg_restore', 7200)) as run:
            with self.assertRaisesRegex(ops.OperationError, 'Media preserved'):
                ops.restore(root, self.config(), database_timeout=7200)
            self.assertEqual(run.call_args.kwargs['timeout'], 7200)
        self.assertEqual((self.root / 'media/photo').read_bytes(), b'photo')

    def test_backup_timeout_is_separate_from_short_checks(self):
        config = self.config()
        (self.root / 'media').mkdir()
        def fail_dump(command, env=None, timeout=30):
            if command[0] == 'pg_dump':
                self.assertEqual(timeout, 7200)
                raise ops.OperationError('synthetic timeout')
            self.assertEqual(timeout, 30)
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'run', side_effect=fail_dump):
            with self.assertRaises(ops.OperationError):
                ops.backup(config, self.root / 'binary', self.root / 'output', database_timeout=7200)
        self.assertFalse((self.root / 'output').exists())
        self.assertEqual(list(self.root.glob('.talocart-backup-*')), [])

    def test_backup_restores_packages_and_executable_permissions(self):
        config = self.config()
        config.write_text(config.read_text() + '[packages]\nroot="packages"\n')
        media = self.root / 'media'
        media.mkdir()
        (media / 'photo').write_bytes(b'synthetic photo')
        packages = self.root / 'packages'
        module = packages / 'installed/fixture/1.0.0/module'
        module.parent.mkdir(parents=True)
        module.write_bytes(b'synthetic executable')
        module.chmod(0o700)
        (packages / 'registry.json').write_text('{"synthetic":true}')
        settings = packages / 'data/fixture/settings.toml'
        settings.parent.mkdir(parents=True)
        settings.write_text('password="synthetic"')
        binary = self.root / 'talocart'
        binary.write_bytes(b'synthetic base')
        def command(args, env=None, timeout=30):
            if args[0] == 'pg_dump':
                Path(args[args.index('--file') + 1]).write_bytes(b'synthetic dump')
            return b''
        output = self.root / 'saved'
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='70'), patch.object(ops, 'run', side_effect=command):
            ops.backup(config, binary, output)
        self.assertEqual(ops.files(packages), ops.files(output / 'packages'))
        self.assertEqual((output / 'packages/installed/fixture/1.0.0/module').stat().st_mode & 0o777, 0o700)
        target = self.root / 'restore.toml'
        target.write_text(config.read_text().replace('root="media"', 'root="restored-media"').replace('root="packages"', 'root="restored-packages"'))
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', side_effect=['0', '70']), patch.object(ops, 'run', return_value=b''):
            ops.restore(output, target)
        self.assertEqual(ops.files(packages), ops.files(self.root / 'restored-packages'))
        self.assertEqual((self.root / 'restored-packages/installed/fixture/1.0.0/module').stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.root / 'restored-packages/data/fixture/settings.toml').stat().st_mode & 0o777, 0o600)

    def package_backup_fixture(self):
        source = self.complete_backup()
        module = source / 'packages/installed/fixture/1.0.0/module'
        module.parent.mkdir(parents=True)
        module.write_bytes(b'synthetic executable')
        (source / 'packages/registry.json').write_text('{}')
        manifest = json.loads((source / 'backup.json').read_text())
        manifest['packages_included'] = True
        manifest['files'] = {k: v for k, v in ops.files(source).items() if k != 'backup.json'}
        (source / 'backup.json').write_text(json.dumps(manifest))
        return source

    def test_restore_refuses_existing_packages_before_sql_or_media_copy(self):
        source = self.package_backup_fixture()
        config = self.config()
        destination = self.root / 'storage/packages'
        destination.mkdir(parents=True)
        (destination / 'keep').write_text('existing')
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline') as offline, patch.object(ops, 'run') as run:
            with self.assertRaises(ops.OperationError):
                ops.restore(source, config)
            offline.assert_not_called()
            run.assert_not_called()
        self.assertEqual((destination / 'keep').read_text(), 'existing')
        self.assertFalse((self.root / 'media').exists())

    def test_failed_package_copy_or_publication_retracts_only_own_trees(self):
        source = self.package_backup_fixture()
        config = self.config()
        destination = self.root / 'storage/packages'
        def fail_copy(src, target):
            target.mkdir()
            (target / 'partial').write_text('partial')
            raise OSError('synthetic disk full')
        original_rename = Path.rename
        def fail_publish(path, target):
            if target == destination:
                raise OSError('synthetic publication failure')
            return original_rename(path, target)
        for fault in [patch.object(ops, 'copy_packages', side_effect=fail_copy), patch.object(Path, 'rename', fail_publish)]:
            with fault, patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='0'), patch.object(ops, 'run') as run:
                with self.assertRaises(OSError):
                    ops.restore(source, config)
                run.assert_not_called()
            self.assertFalse(destination.exists())
            self.assertFalse((self.root / 'media').exists())
            self.assertEqual(list(self.root.rglob('.talocart-restore-*')), [])
        self.assertEqual((source / 'packages/registry.json').read_text(), '{}')

    def test_sql_failure_preserves_complete_packages_for_inspection(self):
        source = self.package_backup_fixture()
        config = self.config()
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='0'), patch.object(ops, 'run', side_effect=ops.OperationError('synthetic SQL failure')):
            with self.assertRaisesRegex(ops.OperationError, 'packages preserved'):
                ops.restore(source, config)
        self.assertEqual(ops.files(source / 'packages'), ops.files(self.root / 'storage/packages'))
        self.assertEqual(ops.files(source / 'media'), ops.files(self.root / 'media'))

    def test_restore_syncs_all_files_and_published_parents_before_sql(self):
        source = self.package_backup_fixture()
        config = self.config()
        synced = []
        real_sync = os.fsync
        def sync(fd):
            info = os.fstat(fd)
            synced.append((info.st_dev, info.st_ino))
            real_sync(fd)
        def identity(path):
            info = path.stat()
            return info.st_dev, info.st_ino
        def restore(args, env=None, timeout=30):
            self.assertEqual(args[0], 'pg_restore')
            for tree in (self.root/'media', self.root/'storage/packages'):
                root_sync = synced.index(identity(tree))
                for child in tree.rglob('*'):
                    self.assertIn(identity(child), synced[:root_sync])
                self.assertIn(identity(tree.parent), synced[root_sync+1:])
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', side_effect=['0','1']), patch.object(ops, 'run', side_effect=restore), patch.object(ops.os, 'fsync', side_effect=sync):
            ops.restore(source, config)

    def test_restore_sync_failure_prevents_sql_and_retracts_published_trees(self):
        source = self.package_backup_fixture()
        config = self.config()
        real_sync = os.fsync
        for fail_parent in (False, True):
            with self.subTest(parent=fail_parent):
                failed = False
                def sync(fd):
                    nonlocal failed
                    is_parent = os.fstat(fd).st_ino == self.root.stat().st_ino and (self.root/'media').exists()
                    if not failed and (is_parent if fail_parent else stat.S_ISREG(os.fstat(fd).st_mode)):
                        failed = True
                        raise OSError('synthetic fsync failure')
                    real_sync(fd)
                with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='0'), patch.object(ops, 'run') as run, patch.object(ops.os, 'fsync', side_effect=sync):
                    with self.assertRaisesRegex(OSError, 'fsync failure'):
                        ops.restore(source, config)
                    run.assert_not_called()
                self.assertTrue(failed)
                self.assertFalse((self.root/'media').exists())
                self.assertFalse((self.root/'storage/packages').exists())
                self.assertEqual(list(self.root.rglob('.talocart-restore-*')), [])

    def test_persistent_publication_sync_failure_still_retracts_every_own_tree(self):
        source = self.package_backup_fixture()
        config = self.config()
        (self.root/'storage').mkdir()
        real_sync = os.fsync
        publication_failed = False
        def sync(fd):
            nonlocal publication_failed
            publication_failed |= (self.root/'storage/packages').exists()
            if publication_failed and stat.S_ISDIR(os.fstat(fd).st_mode):
                raise OSError('persistent directory fsync failure')
            real_sync(fd)
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='0'), patch.object(ops, 'run') as run, patch.object(ops.os, 'fsync', side_effect=sync):
            with self.assertRaises((OSError, ops.OperationError)):
                ops.restore(source, config)
            run.assert_not_called()
        self.assertTrue(publication_failed)
        self.assertFalse((self.root/'media').exists())
        self.assertFalse((self.root/'storage/packages').exists())

    def test_backup_syncs_contents_before_publication_and_published_parent_before_return(self):
        config = self.config()
        (self.root/'media').mkdir()
        (self.root/'media/photo').write_bytes(b'original')
        binary = self.root/'talocart'
        binary.write_bytes(b'synthetic base')
        output = self.root/'new-parent/saved'
        synced = []
        real_sync = os.fsync
        def identity(path):
            info = path.stat()
            return info.st_dev, info.st_ino
        def sync(fd):
            info = os.fstat(fd)
            synced.append(((info.st_dev, info.st_ino), output.exists()))
            real_sync(fd)
        def command(args, env=None, timeout=30):
            if args[0] == 'pg_dump':
                Path(args[args.index('--file') + 1]).write_bytes(b'synthetic dump')
            return b''
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='1'), patch.object(ops, 'run', side_effect=command), patch.object(ops.os, 'fsync', side_effect=sync):
            ops.backup(config, binary, output)
        self.assertTrue(synced)
        for member in [output, *output.rglob('*')]:
            self.assertIn((identity(member), False), synced)
        self.assertEqual(synced[-1], (identity(output.parent), True))
        self.assertIn((identity(self.root), False), synced)
        self.assertEqual(ops.verify(output)['migration_count'], 1)

    def test_backup_sync_failure_never_reports_an_incomplete_backup_as_complete(self):
        config = self.config()
        (self.root/'media').mkdir()
        (self.root/'media/photo').write_bytes(b'original')
        binary = self.root/'talocart'
        binary.write_bytes(b'synthetic base')
        real_sync = os.fsync
        def command(args, env=None, timeout=30):
            if args[0] == 'pg_dump':
                Path(args[args.index('--file') + 1]).write_bytes(b'synthetic dump')
            return b''
        for after_publish in (False, True):
            with self.subTest(after_publish=after_publish):
                output = self.root/('after' if after_publish else 'before')
                def sync(fd):
                    if output.exists() == after_publish:
                        raise OSError('synthetic backup fsync failure')
                    real_sync(fd)
                with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline'), patch.object(ops, 'query', return_value='1'), patch.object(ops, 'run', side_effect=command), patch.object(ops.os, 'fsync', side_effect=sync):
                    with self.assertRaisesRegex(OSError, 'backup fsync failure'):
                        ops.backup(config, binary, output)
                self.assertEqual(output.exists(), after_publish)
                if after_publish:
                    ops.verify(output)  # Published bytes remain for inspection after uncertain durability.
                self.assertEqual(list(self.root.glob('.talocart-backup-*')), [])
                self.assertEqual((self.root/'media/photo').read_bytes(), b'original')

    def test_sync_tree_rejects_links_and_special_files_without_syncing_their_targets(self):
        outside = self.root/'outside'
        outside.write_bytes(b'keep')
        for kind in ('root-link', 'symlink', 'fifo', 'hardlink'):
            with self.subTest(kind=kind):
                tree = self.root/kind
                if kind == 'root-link':
                    tree.symlink_to(self.root, target_is_directory=True)
                else:
                    tree.mkdir()
                    member = tree/'unsafe'
                    if kind == 'symlink':
                        member.symlink_to(outside)
                    elif kind == 'hardlink':
                        os.link(outside, member)
                    else:
                        os.mkfifo(member)
                with patch.object(ops.os, 'fsync') as sync:
                    with self.assertRaises((OSError, ops.OperationError)):
                        ops.sync_tree(tree)
                    sync.assert_not_called()
                self.assertEqual(outside.read_bytes(), b'keep')

    def test_backup_refuses_output_inside_packages_and_symlinked_roots(self):
        config = self.config()
        (self.root / 'media').mkdir()
        packages = self.root / 'storage/packages'
        packages.mkdir(parents=True)
        with patch.dict(os.environ, {}, clear=True), patch.object(ops, 'offline') as offline:
            with self.assertRaises(ops.OperationError):
                ops.backup(config, self.root / 'binary', packages / 'backup')
            packages.rmdir()
            packages.symlink_to(self.root / 'media', target_is_directory=True)
            with self.assertRaises(ops.OperationError):
                ops.backup(config, self.root / 'binary', self.root / 'saved')
            offline.assert_not_called()

    def test_short_checks_use_thirty_seconds(self):
        with patch.object(ops.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, b'OK')) as run:
            self.assertEqual(ops.run(['doctor']), b'OK')
            self.assertEqual(run.call_args.kwargs['timeout'], 30)

    def test_database_timeout_cli_rejects_nonpositive_values(self):
        for command in ('backup', 'restore'):
            for value in ('0', '-1', 'invalid'):
                args = ['operations.py', command, '--config', 'unused', '--database-timeout', value]
                args += ['--binary', 'unused', '--output', 'unused'] if command == 'backup' else ['--backup', 'unused']
                with patch.object(sys, 'argv', args), patch.object(ops, 'backup') as backup, patch.object(ops, 'restore') as restore:
                    with self.assertRaises(SystemExit) as error:
                        ops.main()
                    self.assertEqual(error.exception.code, 2)
                    backup.assert_not_called()
                    restore.assert_not_called()


class DistributionContracts(unittest.TestCase):
    def test_service_example_places_packages_in_writable_state(self):
        config = tomllib.loads((release.ROOT / 'distribution/talocart.https.toml').read_text())
        folder = Path(config.get('packages', {}).get('root', 'storage/packages'))
        if not folder.is_absolute():
            folder = Path('/etc/talocart') / folder
        service = (release.ROOT / 'distribution/talocart.service').read_text()
        writable = [Path(v) for v in re.search(r'^ReadWritePaths=(.+)$', service, re.M)[1].split()]
        self.assertTrue(any(folder.is_relative_to(p) for p in writable), str(folder))
        self.assertFalse(folder.is_relative_to(config['media']['root']))

    def test_package_upload_proxy_accepts_maximum_envelope(self):
        nginx = (release.ROOT / 'distribution/nginx.conf').read_text()
        location = re.search(r'location = /admin/modules/preview \{(.*?)\}', nginx, re.S)
        self.assertIsNotNone(location, 'Package upload needs a dedicated location')
        block = location[1]
        limit = int(re.search(r'client_max_body_size (\d+)m;', block)[1]) * 1024 * 1024
        self.assertGreaterEqual(limit, 128 * 1024 * 1024 + 40000)
        self.assertIn('proxy_pass http://127.0.0.1:3000;', block)
        self.assertIn('client_max_body_size 12m;', nginx)

    def test_package_versions_follow_metadata_and_reject_wrong_compiler(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'rust-toolchain.toml').write_text('[toolchain]\nchannel="1.95.0"\n')
            metadata = {'workspace_members': ['app'], 'packages': [{'id': 'app', 'name': 'talocart', 'version': '2.3.4', 'source': None}]}
            with patch.object(release, 'ROOT', root), patch.object(release.subprocess, 'check_output', return_value='rustc 1.95.0 (fixture)'):
                self.assertEqual(release.package_versions(metadata), ('1.95.0', '2.3.4'))
            with patch.object(release, 'ROOT', root), patch.object(release.subprocess, 'check_output', return_value='rustc 1.94.0 (fixture)'):
                with self.assertRaises(ops.OperationError):
                    release.package_versions(metadata)

    def test_selected_local_module_notices_are_preserved_and_base_omits_them(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            module = root / 'koala'
            module.mkdir()
            (module / 'LICENSE').write_text('Reserved additions')
            (module / 'LICENSE-MIT').write_text('Previous MIT grant')
            metadata = {'packages': [{'name': 'talocart-koala-import', 'version': '6.3.0',
                'source': None, 'manifest_path': str(module / 'Cargo.toml'),
                'license_file': 'LICENSE', 'license': None, 'repository': None}]}
            with self.assertRaises(ops.OperationError):
                release.licenses({'packages': []}, root / 'absent', local_packages={'talocart-koala-import'})
            release.licenses(metadata, root / 'base')
            self.assertFalse((root / 'base/talocart-koala-import-6.3.0').exists())
            release.licenses(metadata, root / 'composed', local_packages={'talocart-koala-import'})
            for name in ('LICENSE', 'LICENSE-MIT'):
                self.assertEqual((root / 'composed/talocart-koala-import-6.3.0' / name).read_bytes(), (module / name).read_bytes())
            records = json.loads((root / 'composed/dependencies.json').read_text())
            self.assertEqual(records[0]['name'], 'talocart-koala-import')
            self.assertNotIn('source_archive', records[0])
            (module / 'LICENSE').unlink()
            with self.assertRaises(ops.OperationError):
                release.licenses(metadata, root / 'missing', local_packages={'talocart-koala-import'})

    def test_vendored_dependency_keeps_notices_without_claiming_registry_source(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            vendor = root / 'vendor' / 'askama_derive'
            vendor.mkdir(parents=True)
            (vendor / 'LICENSE-MIT').write_text('Upstream license')
            metadata = {'packages': [{'name': 'askama_derive', 'version': '0.16.1',
                'source': None, 'manifest_path': str(vendor / 'Cargo.toml'),
                'license': 'MIT OR Apache-2.0', 'repository': 'https://github.com/askama-rs/askama'}]}
            with patch.object(release, 'ROOT', root):
                release.licenses(metadata, root / 'notices')
            self.assertEqual((root / 'notices/askama_derive-0.16.1/LICENSE-MIT').read_text(), 'Upstream license')
            records = json.loads((root / 'notices/dependencies.json').read_text())
            self.assertNotIn('source_archive', records[0])

    def test_target_mismatch_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / 'binary'
            binary.write_bytes(b'not executable')
            with self.assertRaises(ops.OperationError):
                release.check_binary(binary, 'x86_64-unknown-linux-gnu')

    def test_archive_reproducibility_allowlist_checksums_and_modes(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            paths = ['LICENSE', 'CONTRIBUTING.md', 'SECURITY.md', 'talocart.example.toml',
                     'distribution/talocart.service', 'distribution/nginx.conf', 'distribution/talocart.https.toml',
                     'docs/release-operations.md', 'docs/operational-readiness-checklist.md', 'docs/guides/versioning.md', 'scripts/release/operations.py', 'docs/guides/application-updates.md', 'assets/vendor/LICENSE', 'runtime/share/doc/rust/COPYRIGHT-library.html', 'runtime/share/doc/rust/licenses/MIT.txt']
            for name in paths:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('fixture')
            (root / 'rust-toolchain.toml').write_text('[toolchain]\nchannel="1.95.0"\n')
            metadata = {'workspace_members': ['app'], 'packages': [{'id': 'app', 'name': 'talocart', 'version': '2.3.4', 'source': None}]}
            module = root / 'modules/koala-import'
            module.mkdir(parents=True)
            (module / 'LICENSE').write_text('Reserved additions')
            (module / 'LICENSE-MIT').write_text('Previous MIT grant')
            metadata['packages'].append({'id': 'koala', 'name': 'talocart-koala-import',
                'version': '2.3.4', 'source': None, 'manifest_path': str(module / 'Cargo.toml'),
                'license_file': 'LICENSE', 'license': None, 'repository': None})
            (root / 'talocart.toml').write_text('PRIVATE SECRET')
            (root / 'Cargo.toml').write_text('[workspace.package]\nversion="2.3.4"\n')
            (root / '.env').write_text('PRIVATE SECRET')
            binary = root / 'fixture-binary'
            header = bytearray(64)
            header[:6] = b'\x7fELF\x02\x01'
            header[18:20] = struct.pack('<H', 62)
            binary.write_bytes(header)
            def git(args, **kwargs):
                if args[0] == 'rustc': return 'rustc 1.95.0 (fixture)' if '--version' in args else str(root / 'runtime')
                return '1700000000' if 'show' in args else ('0' * 40 if 'rev-parse' in args else '')
            info = {'version': '2.3.4', 'commit': '0' * 40, 'source_dirty': False,
                    'tagged': False, 'source_fingerprint': 'f' * 40, 'compiled_modules': []}
            result = subprocess.CompletedProcess([], 0, json.dumps(info))
            with patch.object(release, 'ROOT', root), patch.object(release.subprocess, 'check_output', side_effect=git), patch.object(release.subprocess, 'run', return_value=result), patch.object(release.versions, 'fingerprint', return_value='f' * 40), patch.dict(os.environ, {}, clear=True):
                a = release.package(binary, 'x86_64-unknown-linux-gnu', None, metadata, root / 'a')
                b = release.package(binary, 'x86_64-unknown-linux-gnu', '2.3.4', metadata, root / 'b')
                with self.assertRaises(ops.OperationError):
                    release.package(binary, 'x86_64-unknown-linux-gnu', None, metadata, root / 'base-as-koala', profile='koala-import')
            self.assertEqual(a.read_bytes(), b.read_bytes())
            with tarfile.open(a) as tar:
                names = tar.getnames()
                self.assertTrue(any(n.endswith('/third-party/rust-runtime/COPYRIGHT-library.html') for n in names))
                self.assertFalse(any(n.endswith('/.env') or n.endswith('/talocart.toml') for n in names))
                executable = next(m for m in tar.getmembers() if m.name.endswith('/bin/talocart'))
                self.assertEqual(executable.mode, 0o755)
                self.assertEqual(executable.uid, 0)
                extracted = root / 'extract'
                for member in tar.getmembers():
                    self.assertTrue(member.isfile() or member.isdir())
                    self.assertFalse(PurePosixPath(member.name).is_absolute())
                    self.assertNotIn('..', PurePosixPath(member.name).parts)
                kwargs = {'filter': 'data'} if hasattr(tarfile, 'data_filter') else {}
                tar.extractall(extracted, **kwargs)
                folder = next(extracted.iterdir())
                manifest = ops.verify(folder, 'release.json')
                self.assertEqual(manifest['version'], '2.3.4')
                self.assertEqual(manifest['cargo_package_version'], '2.3.4')
                self.assertEqual(manifest['build'], info)
                self.assertEqual(manifest['profile'], 'base')
                self.assertEqual(manifest['modules'], [])
                self.assertFalse(any('/talocart-koala-import-' in n for n in names))
            def composed_run(args, **kwargs):
                return subprocess.CompletedProcess([], 0, 'import-doctor' if '--help' in args else json.dumps(dict(info,compiled_modules=['koala-import'])))
            with patch.object(release, 'ROOT', root), patch.object(release.subprocess, 'check_output', side_effect=git), patch.object(release.subprocess, 'run', side_effect=composed_run), patch.object(release.versions, 'fingerprint', return_value='f' * 40), patch.dict(os.environ, {}, clear=True):
                with self.assertRaises(ops.OperationError):
                    release.package(binary, 'x86_64-unknown-linux-gnu', None, metadata, root / 'composed', profile='koala-import')
                with self.assertRaises(ops.OperationError):
                    release.package(binary, 'x86_64-unknown-linux-gnu', None, metadata, root / 'wrong-profile')
            notices = root / 'module-notices'
            release.licenses(metadata, notices, local_packages={'talocart-koala-import'})
            for name, content in [('LICENSE', b'Reserved additions'), ('LICENSE-MIT', b'Previous MIT grant')]:
                self.assertEqual((notices / 'talocart-koala-import-2.3.4' / name).read_bytes(), content)

            stripe_module=root/'modules/stripe'
            stripe_module.mkdir(parents=True)
            (stripe_module/'LICENSE').write_text('MIT provider fixture')
            (stripe_module/'stripe.example.toml').write_text('[modules]\nstripe=true\n[extensions.stripe]\nenabled=false\n')
            metadata['packages'].append({'id':'stripe','name':'talocart-stripe','version':'2.3.4','source':None,
                'manifest_path':str(stripe_module/'Cargo.toml'),'license':'MIT','repository':None})
            def stripe_run(args,**kwargs):
                return subprocess.CompletedProcess([],0,'ordinary help' if '--help' in args else json.dumps(dict(info,compiled_modules=['stripe'])))
            with patch.object(release,'ROOT',root),patch.object(release.subprocess,'check_output',side_effect=git),patch.object(release.subprocess,'run',side_effect=stripe_run),patch.object(release.versions,'fingerprint',return_value='f'*40),patch.dict(os.environ,{},clear=True):
                with self.assertRaises(ops.OperationError):
                    release.package(binary,'x86_64-unknown-linux-gnu',None,metadata,root/'stripe-as-base')
                with self.assertRaises(ops.OperationError):
                    release.package(binary,'x86_64-unknown-linux-gnu',None,metadata,root/'stripe',profile='stripe')
                with self.assertRaises(ops.OperationError):
                    release.package(binary,'x86_64-unknown-linux-gnu',None,metadata,root/'all',profile='all')

            (root / 'assets/vendor/leak').symlink_to(root / '.env')
            with patch.object(release, 'ROOT', root), patch.object(release.subprocess, 'check_output', side_effect=git), patch.object(release.subprocess, 'run', return_value=result), patch.object(release.versions, 'fingerprint', return_value='f' * 40), patch.dict(os.environ, {}, clear=True):
                with self.assertRaises(ops.OperationError):
                    release.package(binary, 'x86_64-unknown-linux-gnu', '2.3.4', metadata, root / 'c')


if __name__ == '__main__':
    unittest.main()
