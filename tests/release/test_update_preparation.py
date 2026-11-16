"""Complete preparation binding; OS/transport/Rust are covered by their real fixtures."""
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'scripts/release'))
import distribution
import update_coordinator as c
import update_engine as engine
import updates_feed as feed
import test_distribution

capacity_check = c.require_capacity


class Preparation(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        fixture = test_distribution.Distribution('test_sequence_rejects_replay_and_equivocation_but_allows_exact_retry')
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.assets, self.manifest = fixture.root, fixture.value
        self.platform = self.manifest['platforms'][0]
        self.target = self.platform['target']
        self.state, releases, incoming, backups = [self.root / name for name in ('state', 'releases', 'incoming', 'backups')]
        for directory in (self.state, releases, incoming, backups): directory.mkdir()
        (self.root / 'storage/packages').mkdir(parents=True)
        app = self.root / 'talocart.toml'
        app.write_text('[updates]\nroot=' + json.dumps(str(self.state)) + '\n')
        self.config = dict(feed_format=2, service_user='fixture', config=str(app), repository='owner/repo',
                           current=str(self.root / 'current'), release_root=str(releases), incoming=str(incoming),
                           backup_root=str(backups), service='fixture.service', writer_units=[], public_key=str(self.root / 'app.pem'), updater_public_key=str(self.root / 'root.pem'))
        self.build = dict(version='11.11.0', commit='a'*40, source_dirty=False, tagged=True,
                          sdk_version='2.0.0', protocol=2, database=self.platform['base']['contracts']['database'])
        self.old = releases / '11.11.0'
        (self.old / 'bin').mkdir(parents=True)
        (self.old / 'scripts/release').mkdir(parents=True)
        (self.old / 'bin/talocart').write_text('synthetic executable')
        (self.old / 'scripts/release/operations.py').write_text('# synthetic operations')
        files = {p.relative_to(self.old).as_posix(): engine.sha256(p) for p in self.old.rglob('*') if p.is_file()}
        metadata = dict(format=1, profile='base', version='11.11.0', source_commit='a'*40,
                        source_dirty=False, target=self.target, build=self.build, files=files)
        (self.old / 'release.json').write_text(json.dumps(metadata))
        (self.root / 'current').symlink_to(self.old)
        base = self.platform['base']
        archive = self.assets / base['asset']
        with tarfile.open(archive, 'w:gz') as stream:
            stream.add(self.old, arcname='talocart-11.11.0-' + self.target)
        base.update(size=archive.stat().st_size, sha256=engine.sha256(archive))
        c.atomic(self.state / 'bootstrap.json', dict(format=1, launcher_version='1.0.0'), lambda _: None)
        receipts = self.state / 'engine-receipts'
        receipts.mkdir()
        c.atomic(receipts / '1.0.0.json', dict(sha256=self.platform['updater']['sha256']), lambda _: None)
        self.snapshot = dict(inventory_sha256='a'*64, modules=[dict(id='koala-import',version='11.11.0',enabled=False,sha256='e'*64)], suspended=False)
        self.native = dict(format=1, inventory_sha256='a'*64, host_version='11.11.0', host_commit='a'*40,
                           sdk_version='2.0.0', protocol=2, database=self.platform['base']['contracts']['database'],
                           database_checked=True, database_sha256='b'*64, ignored=[],
                           modules=[dict(id='koala-import', **{'from':'11.11.0'},to='11.12.0',enabled=False,
                                         publisher='c'*64,sha256='d'*64,action='update',blocker=None,
                                         package_sha256=self.platform['modules'][0]['sha256'])])
        self.downloads, self.calls = [], []
        class Ops:
            uid, gid = 501, os.getgid()
            def application(inner, binary, command, **kwargs):
                self.calls.append(command)
                return json.dumps(self.native if command == 'package-update-plan' else self.build)
            def backup_preflight(inner, *args): return json.dumps(dict(format=1, backup_bytes=1024))
        class Probe:
            def refresh_gate(self, engine): pass
            token = 'f'*64
            def check(inner, _): self.calls.append('https')
        def download(_, __, artifact, destination, ___):
            self.downloads.append(artifact['asset'])
            shutil.copyfile(self.assets / artifact['asset'], destination)
        github = SimpleNamespace(asset=lambda _, name, limit, dest: shutil.copyfile(self.assets / name, dest))
        self.agent = SimpleNamespace(connection=lambda *_: github, download=download)
        self.dist = SimpleNamespace(approved=lambda *_: ({}, self.manifest, 'd'*64), check_sequence=distribution.check_sequence)
        def run(argv):
            if '-pubout' in argv: return str(argv[argv.index('-in')+1])
            if 'LoadState' in argv: return 'loaded'
            if 'User' in argv: return 'fixture'
            if 'Type' in argv: return 'simple'
            if 'RemainAfterExit' in argv: return 'no'
            if 'ExecCondition' in argv: return '{ path=/usr/bin/python3 ; argv[]=/usr/bin/python3 -I /usr/local/libexec/talocart/update_launcher.py --application-start-allowed ; }'
            return ''
        self.engine = SimpleNamespace(STATE=self.state, trusted=lambda _: None, sha256=engine.sha256, extract=engine.extract,
                    check_config=lambda _:None, runtime_configuration=lambda _:None, running_binary=lambda *_:None,
                    run=run)
        real_root_bytes = c.root_bytes
        def root_bytes(path, engine, *args, **kwargs):
            if str(path).startswith('/etc/systemd/'):
                return b'[Service]\nExecCondition=+/usr/bin/python3 -I /usr/local/libexec/talocart/update_launcher.py --application-start-allowed\n'
            return real_root_bytes(path, engine, *args, **kwargs)
        context = patch.object(c, 'root_bytes', side_effect=root_bytes)
        context.start(); self.addCleanup(context.stop)
        for name, value in [('Operations', lambda *_: Ops()), ('Probe', lambda *_: Probe()),
                            ('control', lambda *_args, **_kwargs: copy.deepcopy(self.snapshot)),
                            ('require_capacity', lambda *_: None)]:
            context = patch.object(c, name, value)
            context.start(); self.addCleanup(context.stop)
        context = patch.object(c.os, 'chown', lambda *_: None)
        context.start(); self.addCleanup(context.stop)
        # Architecture is selected by the real platform name; align the fixture metadata.
        import platform
        context = patch.object(platform, 'machine', return_value='x86_64')
        context.start(); self.addCleanup(context.stop)

    def prepare(self):
        return c.Preparation(self.config, feed, self.dist, self.agent, self.engine).run()

    def test_preparation_binds_original_state_and_reuses_cached_verified_artifacts(self):
        first = self.prepare()
        self.assertFalse(first.plan['blocked'])
        self.assertFalse(first.plan['modules'][0]['enabled'])
        self.assertEqual(first.plan, first.store.load(first.identity))
        self.assertIn('doctor', self.calls)
        self.assertIn('https', self.calls)
        self.assertNotIn('package-update-suspend', self.calls)
        self.assertEqual(self.prepare().identity, first.identity)
        self.assertFalse((self.state / 'maintenance').exists())
        self.assertFalse((self.state / 'processing.json').exists())

    def test_catalogue_does_not_download_or_install_absent_modules(self):
        self.platform['modules'].append(dict(id='other',version='1.0.0',publisher='f'*64,sha256='1'*64,size=7,asset='unused.talopkg'))
        self.prepare()
        self.assertNotIn('unused.talopkg', self.downloads)

    def test_changed_live_inventory_or_current_binary_blocks_before_mutation(self):
        self.native['inventory_sha256'] = 'b'*64
        with self.assertRaises(ValueError): self.prepare()
        self.native['inventory_sha256'] = 'a'*64
        (self.old / 'bin/talocart').write_text('changed')
        self.downloads.clear()
        with self.assertRaises(ValueError): self.prepare()
        self.assertEqual(self.downloads, [])

    def test_private_and_application_signing_keys_cannot_be_the_same(self):
        self.config['updater_public_key'] = self.config['public_key']
        with self.assertRaises(ValueError): self.prepare()

    def test_blocked_module_produces_a_blocked_proposal_not_an_installation(self):
        self.native.update(database_checked=False, database_sha256=None)
        self.native['modules'][0].update(action='blocked', blocker='incompatible')
        result = self.prepare()
        self.assertTrue(result.plan['blocked'])
        self.assertFalse((self.state / 'processing.json').exists())

    def test_unsupported_unit_lifecycles_and_start_conditions_block_before_downloads(self):
        original = self.engine.run
        for field, value in [('Type','oneshot'),('RemainAfterExit','yes'),('ExecCondition','{ argv[]=/bin/unsafe-hook ; }')]:
            self.downloads.clear()
            self.engine.run = lambda argv: value if field in argv else original(argv)
            with self.subTest(field=field), self.assertRaises(ValueError): self.prepare()
            self.assertEqual(self.downloads, [])
        self.engine.run = original

    def test_actual_retained_engine_is_recorded_instead_of_older_catalogue_version(self):
        self.platform['updater']['version'] = '0.9.0'
        prepared = self.prepare()
        self.assertFalse(prepared.needs_engine)
        self.assertEqual(prepared.active_updater['version'], c.ENGINE_VERSION)
        self.assertNotEqual(prepared.active_updater['version'], prepared.plan['updater']['version'])

    def test_capacity_budgets_on_one_filesystem_are_added(self):
        with patch.object(shutil, 'disk_usage', return_value=SimpleNamespace(free=150)):
            with self.assertRaises(ValueError): capacity_check([(self.root, 100), (self.root / 'incoming', 100)])
            capacity_check([(self.root, 50), (self.root / 'incoming', 100)])


if __name__ == '__main__': unittest.main()
