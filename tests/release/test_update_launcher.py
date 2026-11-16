"""Privileged-engine trust boundary: synthetic bundles/keys; no service mutations."""
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[2] / 'scripts/release'
sys.path.insert(0, str(SCRIPTS))
import update_launcher as launcher
import updater_bundle
import updates_feed as feed


class Launcher(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.keys = tempfile.TemporaryDirectory()
        cls.private = Path(cls.keys.name) / 'root-private.pem'
        cls.public = Path(cls.keys.name) / 'root-public.pem'
        feed.command(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', cls.private])
        cls.private.chmod(0o600)
        cls.public.write_bytes(feed.command(['openssl', 'pkey', '-in', cls.private, '-pubout']))

    @classmethod
    def tearDownClass(cls):
        cls.keys.cleanup()

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.bundles = self.root / 'engines'
        self.bundles.mkdir()
        self.current = self.root / 'engine-current'
        self.state = self.root / 'state'
        self.state.mkdir()
        self.runtime = self.root / 'run'
        self.runtime.mkdir()
        self.source = self.root / 'source'
        self.source.mkdir()
        for name in launcher.FILES:
            (self.source / name).write_text('# Synthetic trusted engine fixture\n')
        (self.source / 'update_agent.py').write_text(
            'import json\nprint(json.dumps(dict(format=1,state_format=1,engine_version="1.0.0")))\n')
        for name, value in [('BUNDLES', self.bundles), ('CURRENT', self.current),
                            ('STATE', self.state), ('RUNTIME', self.runtime)]:
            obj = patch.object(launcher, name, value)
            obj.start()
            self.addCleanup(obj.stop)
        # Unit tests run as the desktop user in an isolated temporary tree. Actual
        # ownership/ancestors are exercised separately in the disposable Linux container.
        obj = patch.object(launcher, 'trusted')
        obj.start()
        self.addCleanup(obj.stop)
        actual_stat = os.fstat
        def root_stat(fd):
            info = list(actual_stat(fd))
            info[4] = 0
            return os.stat_result(info)
        obj = patch.object(launcher.os, 'fstat', side_effect=root_stat)
        obj.start()
        self.addCleanup(obj.stop)
        def local_command(argv, timeout=120):
            # macOS ships LibreSSL; use the same OpenSSL 3 selected for release tests.
            result = subprocess.run(list(map(str, argv)), capture_output=True, timeout=timeout)
            launcher.require(result.returncode == 0, 'Synthetic command failure')
            return result.stdout
        obj = patch.object(launcher, 'command', side_effect=local_command)
        obj.start()
        self.addCleanup(obj.stop)
        self.record = updater_bundle.build(self.source, '1.0.0', self.private, self.root / 'dist')
        self.archive = self.root / 'dist' / self.record['asset']
        self.signature = self.root / 'dist' / self.record['signature_asset']

    def install(self, version='1.0.0'):
        return launcher.install_bundle(self.archive, self.signature, self.public, version)

    def test_deterministic_signed_bundle_is_activated_and_reverified_before_use(self):
        second = self.root / 'second'
        updater_bundle.build(self.source, '1.0.0', self.private, second)
        self.assertEqual(self.archive.read_bytes(), (second / self.archive.name).read_bytes())
        result = self.install()
        self.assertEqual(result, dict(version='1.0.0', previous=None))
        self.assertEqual(launcher.selected(), self.bundles / '1.0.0')
        self.assertEqual(self.current.resolve(), self.bundles / '1.0.0')
        with self.assertRaises(ValueError):
            self.install()  # no downgrade/equal reinstall
        (self.current / 'update_engine.py').write_text('# corrupted after installation')
        with self.assertRaises(ValueError):
            launcher.selected()

    def test_handoff_finishes_published_switch_and_acknowledges_selected_engine_after_crash(self):
        self.install()
        (self.source / 'update_agent.py').write_text(
            'import json\nprint(json.dumps(dict(format=1,state_format=1,engine_version="1.1.0")))\n')
        record = updater_bundle.build(self.source, '1.1.0', self.private, self.root / 'new')
        config = dict(incoming=str(self.root / 'new'), updater_public_key=str(self.public))
        handoff = dict(format=1, updater=record, plan_sha256='a'*64, minimum_launcher='1.0.0')
        path = self.state / 'engine-handoff.json'
        launcher.atomic_json(path, handoff)
        with patch.object(launcher, 'switch_engine', side_effect=OSError('Synthetic crash before switch')):
            with self.assertRaises(OSError):
                launcher.consume_handoff(config)
        self.assertEqual(self.current.resolve().name, '1.0.0')
        self.assertTrue((self.bundles / '1.1.0').is_dir())
        self.assertTrue(path.exists())
        self.assertEqual(launcher.consume_handoff(config), 'a'*64)
        self.assertEqual(self.current.resolve().name, '1.1.0')
        self.assertFalse(path.exists())
        launcher.atomic_json(path, handoff)  # crash after selection but before acknowledgement
        self.assertEqual(launcher.consume_handoff(config), 'a'*64)
        self.assertFalse(path.exists())
        for change in [dict(minimum_launcher='99.0.0'), dict(plan_sha256='../bad')]:
            launcher.atomic_json(path, handoff | change)
            with self.assertRaises(ValueError): launcher.consume_handoff(config)
        changed = dict(handoff, updater=record | {'sha256': 'b'*64})
        launcher.atomic_json(path, changed)
        with self.assertRaises(ValueError): launcher.consume_handoff(config)
        self.assertEqual(self.current.resolve().name, '1.1.0')
        launcher.atomic_json(path, handoff)
        (self.state / 'processing.json').write_text('{}')
        with self.assertRaises(ValueError): launcher.consume_handoff(config)
        self.assertTrue(path.exists())

    def test_wrong_root_key_tampering_and_applicable_recovery_state_prevent_switch(self):
        raw = self.archive.read_bytes()
        self.archive.write_bytes(raw[:-1] + bytes([raw[-1] ^ 1]))
        with self.assertRaises(ValueError):
            self.install()
        self.assertFalse(self.current.is_symlink())
        self.archive.write_bytes(raw)
        wrong = self.root / 'wrong.pem'
        feed.command(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', wrong])
        public = self.root / 'wrong-public.pem'
        public.write_bytes(feed.command(['openssl', 'pkey', '-in', wrong, '-pubout']))
        with self.assertRaises(ValueError):
            launcher.install_bundle(self.archive, self.signature, public, '1.0.0')
        (self.state / 'processing.json').write_text('{}')
        with self.assertRaises(ValueError):
            self.install()
        self.assertEqual(list(self.bundles.iterdir()), [])

    def rewrite_archive(self, transform):
        entries = []
        with tarfile.open(self.archive) as bundle:
            for member in bundle:
                entries.append((member, bundle.extractfile(member).read()))
        transform(entries)
        with tarfile.open(self.archive, 'w:gz', format=tarfile.USTAR_FORMAT) as bundle:
            for member, raw in entries:
                bundle.addfile(member, io.BytesIO(raw) if member.isfile() else None)
        self.signature.write_bytes(feed.sign(self.archive.read_bytes(), self.private))

    def test_even_correctly_signed_archive_cannot_supply_paths_links_hooks_or_duplicate_files(self):
        for name, kind in [('../escape', tarfile.REGTYPE), ('update_launcher.py', tarfile.REGTYPE),
                           ('update_agent.py', tarfile.SYMTYPE), ('bundle.json', tarfile.REGTYPE)]:
            original = self.archive.read_bytes()
            def add(entries):
                item = tarfile.TarInfo(name)
                item.type = kind
                item.size = 4 if kind == tarfile.REGTYPE else 0
                item.linkname = '/etc/passwd' if kind == tarfile.SYMTYPE else ''
                entries.append((item, b'bad!'))
            self.rewrite_archive(add)
            with self.subTest(name=name, kind=kind), self.assertRaises(ValueError):
                self.install()
            self.assertFalse(self.current.exists())
            self.assertFalse((self.root / 'escape').exists())
            self.archive.write_bytes(original)
            self.signature.write_bytes(feed.sign(original, self.private))

    def test_bad_self_check_leaves_previous_engine_and_no_published_partial_directory(self):
        self.install()
        (self.source / 'update_agent.py').write_text('raise SystemExit(1)\n')
        record = updater_bundle.build(self.source, '1.1.0', self.private, self.root / 'new')
        self.archive = self.root / 'new' / record['asset']
        self.signature = self.root / 'new' / record['signature_asset']
        with self.assertRaises(ValueError):
            self.install('1.1.0')
        self.assertEqual(self.current.resolve(), self.bundles / '1.0.0')
        self.assertFalse((self.bundles / '1.1.0').exists())
        self.assertEqual([p.name for p in self.bundles.iterdir()], ['1.0.0'])

    def test_operation_markers_only_name_bounded_units_and_uncertain_mutations_fence_start(self):
        path = self.state / 'operation.json'
        record = dict(format=1, unit='talocart-update-op-' + 'a'*32 + '.service', mutating=False)
        launcher.atomic_json(path, record)
        self.assertFalse(launcher.incomplete())
        with patch.object(launcher, 'command', return_value=b'not-found\n') as command:
            launcher.reap_operation()
        self.assertFalse(path.exists())
        self.assertEqual(command.call_args.args[0][-1], record['unit'])
        launcher.atomic_json(path, record | {'mutating': True})
        self.assertTrue(launcher.incomplete())
        self.assertFalse(launcher.application_start_allowed())
        launcher.atomic_json(path, record | {'unit': 'postgresql.service'})
        self.assertTrue(launcher.incomplete())
        with patch.object(launcher, 'command') as command:
            with self.assertRaises(ValueError): launcher.recovery(dict(service='app.service', writer_units=[]))
        command.assert_called_once_with(['systemctl', 'stop', 'app.service'])

    def test_operation_cleanup_rechecks_disappearance_and_preserves_uncertain_marker(self):
        path = self.state / 'operation.json'
        record = dict(format=1, unit='talocart-update-op-' + 'a'*32 + '.service', mutating=True)
        for after in ('not-found', 'loaded', 'unavailable'):
            with self.subTest(after=after):
                launcher.atomic_json(path, record)
                calls = []
                def command(argv, timeout=120):
                    calls.append(argv[1])
                    if argv[1] == 'stop':
                        raise ValueError('Unit disappeared or stop failed')
                    if len(calls) == 1:
                        return b'loaded\n'
                    if after == 'unavailable':
                        raise OSError('Status unavailable')
                    return after.encode() + b'\n'
                with patch.object(launcher, 'command', side_effect=command):
                    if after == 'not-found':
                        launcher.reap_operation()
                    else:
                        with self.assertRaises((ValueError, OSError)):
                            launcher.reap_operation()
                self.assertEqual(path.exists(), after != 'not-found')
                self.assertEqual(calls, ['show', 'stop', 'show'])

    def test_atomic_journal_sync_and_recovery_dont_depend_on_current_engine(self):
        launcher.atomic_json(self.state / 'journal.json', dict(phase='backup', attempt='synthetic'))
        self.assertEqual(json.loads((self.state / 'journal.json').read_text())['phase'], 'backup')
        self.assertEqual((self.state / 'journal.json').stat().st_mode & 0o777, 0o600)
        (self.runtime / 'application-start').write_text('')
        self.current.symlink_to(self.bundles / 'missing')
        config = dict(service='fixture.service', writer_units=['writer.service'])
        with patch.object(launcher, 'command') as command:
            launcher.recovery(config)
        command.assert_called_once_with(['systemctl', 'stop', 'fixture.service', 'writer.service'])
        self.assertTrue((self.state / 'maintenance').is_file())
        self.assertTrue((self.state / 'writer-fence').is_file())
        self.assertFalse((self.runtime / 'application-start').exists())
        self.assertFalse(launcher.application_start_allowed())

    def test_recovery_metadata_binds_verified_backup_to_attempt_without_importing_engine(self):
        attempt='a'*32
        attempts=self.state/'attempts';attempts.mkdir()
        launcher.atomic_json(self.state/'status.json',dict(format=2,notice='control_configuration',stage='applying',backup_id='b'*32))
        launcher.atomic_json(self.state/'processing.json',dict(format=1,attempt=attempt,plan_sha256='c'*64))
        path=attempts/(attempt+'.json')
        journal=dict(format=1,id=attempt,phase='backed_up',pending='applying',
            results=dict(backed_up=dict(id=attempt,manifest_sha256='d'*64)),snapshot=dict(private='/never/public'))
        launcher.atomic_json(path,journal)
        self.assertEqual(launcher.recovery_metadata(),dict(stage='applying',backup_id=attempt,notice='control_configuration'))
        for bad in [journal|dict(id='e'*32),journal|dict(results={}),journal|dict(results=dict(backed_up=dict(id=attempt,manifest_sha256='invalid')))]:
            launcher.atomic_json(path,bad)
            self.assertIsNone(launcher.recovery_metadata()['backup_id'])
        launcher.atomic_json(path,journal)
        original=attempts/'original';path.rename(original);path.symlink_to(original)
        self.assertIsNone(launcher.recovery_metadata()['backup_id'])
        launcher.atomic_json(self.state/'processing.json',dict(format=1,attempt='../private'))
        self.assertIsNone(launcher.recovery_metadata()['backup_id'])
        launcher.atomic_json(self.state/'status.json',dict(format=2,notice='/private/diagnostic',stage='raw-command',backup_id='a'*32))
        self.assertEqual(launcher.recovery_metadata(),dict(stage=None,backup_id=None,notice=None))

    def test_engine_switch_does_not_follow_input_symlinks_or_accept_unknown_files(self):
        actual = self.archive
        self.archive = self.root / 'linked.tar.gz'
        self.archive.symlink_to(actual)
        # The local read still opens with O_NOFOLLOW, even when ownership is patched.
        with self.assertRaises(OSError):
            self.install()
        self.assertFalse(self.current.is_symlink())

    def test_engine_identity_and_exact_file_digests_are_mandatory(self):
        value = dict(format=1, version='1.0.0', state_format=1,
                     files={name: 'a'*64 for name in launcher.FILES})
        launcher.manifest(json.dumps(value).encode(), '1.0.0')
        for change in [dict(state_format=2), dict(format=True), dict(version='2.0.0'), dict(extra=True)]:
            with self.assertRaises(ValueError):
                launcher.manifest(json.dumps(value | change).encode(), '1.0.0')
        value['files']['shell.sh'] = 'b'*64
        with self.assertRaises(ValueError):
            launcher.manifest(json.dumps(value).encode(), '1.0.0')


if __name__ == '__main__':
    unittest.main()
