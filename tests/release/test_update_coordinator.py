"""Root-owned immutable plans and write-ahead phases; synthetic filesystem only."""
import copy
import json
from pathlib import Path
import sys
import socket
import struct
from unittest.mock import patch
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'scripts/release'))
import update_coordinator as coordinator
import test_distribution


class Coordinator(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        # Same format-2 fixture shape; this test does not sign or mutate services.
        fixture = test_distribution.Distribution('test_sequence_rejects_replay_and_equivocation_but_allows_exact_retry')
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.manifest = fixture.value
        self.platform = self.manifest['platforms'][0]
        self.native = dict(format=1, inventory_sha256='a'*64, host_version='11.11.0', host_commit='a'*40,
                           database=self.platform['base']['contracts']['database'], database_checked=True, database_sha256='e'*64,
                           sdk_version=self.platform['base']['contracts']['sdk_version'], protocol=self.platform['base']['contracts']['protocol'],
                           modules=[dict(id='koala-import', **{'from': '11.11.0'}, to='11.12.0', enabled=False,
                                         action='update', blocker=None, sha256='d'*64, publisher=self.platform['modules'][0]['publisher'],
                                         package_sha256=self.platform['modules'][0]['sha256'])], ignored=[])
        self.current = dict(version='11.11.0', commit='a'*40)

    def plan(self):
        return coordinator.create_plan(self.manifest, 'b'*64, self.platform, self.native, 'c'*64, self.current)

    def test_packaged_planning_is_independent_of_source_imports(self):
        import builtins
        import updater_bundle
        source = Path(__file__).resolve().parents[2] / 'scripts/release'
        raw = updater_bundle.coordinator_source(source)
        namespace = {'__file__': str(self.root / 'update_coordinator.py')}
        original_import = builtins.__import__
        def isolated_import(name, *args, **kwargs):
            if name in ('update_plan', 'update_state', 'update_host', 'update_probe'):
                raise ImportError('Planning source is absent from the runtime bundle')
            return original_import(name, *args, **kwargs)
        with patch('builtins.__import__', side_effect=isolated_import):
            exec(compile(raw, namespace['__file__'], 'exec'), namespace)
            packaged = namespace['create_plan']
            args = (self.manifest, 'b'*64, self.platform, self.native, 'c'*64, self.current)
            self.assertEqual(packaged(*args), self.plan())
            store = namespace['PlanStore'](self.root, lambda _: None)
            plan, identity = packaged(*args)
            self.assertEqual(store.load(store.save(plan)), plan)
            journal_path = self.root / 'embedded-attempt.json'
            journal = namespace['Journal'].begin(journal_path, lambda _: None, identity, 'a'*32, {})
            journal.intent('updater_ready')
            self.assertEqual(json.loads(journal_path.read_text())['pending'], 'updater_ready')
            journal.complete('updater_ready')
            self.assertEqual(json.loads(journal_path.read_text())['phase'], 'updater_ready')
            self.native['host_commit'] = 'f'*40
            with self.assertRaises(ValueError):
                packaged(*args)

    def test_operation_cleanup_rechecks_unit_disappearance_after_failed_stop(self):
        from types import SimpleNamespace
        for after in ('not-found', 'loaded', 'unavailable'):
            with self.subTest(after=after):
                calls = []
                def run(argv, *args):
                    calls.append(argv)
                    if argv[1] == 'stop':
                        raise ValueError('Unit disappeared or stop failed')
                    if len(calls) == 1:
                        return 'loaded'
                    if after == 'unavailable':
                        raise ValueError('Status unavailable')
                    return after
                ops = object.__new__(coordinator.Operations)
                ops.state, ops.uid, ops.gid = self.root, 501, 501
                ops.engine = SimpleNamespace(trusted=lambda _: None, diagnostics=lambda *_: None, run=run)
                marker = self.root / 'operation.json'
                marker.unlink(missing_ok=True)
                with patch('subprocess.Popen', side_effect=OSError('Synthetic launch failure')):
                    with self.assertRaises(OSError):
                        ops._run(['/synthetic'], 1, False)
                self.assertEqual(marker.exists(), after != 'not-found')
                self.assertEqual([call[1] for call in calls], ['show', 'stop', 'show'])

    def test_plan_binds_exact_release_inventory_config_and_original_activation(self):
        plan, digest = self.plan()
        self.assertFalse(plan['blocked'])
        self.assertFalse(plan['modules'][0]['enabled'])
        self.assertEqual(digest, coordinator.digest(plan))
        self.native['inventory_sha256'] = 'd'*64
        self.assertNotEqual(self.plan()[1], digest)
        self.native['modules'][0].update(action='blocked', blocker='incompatible')
        self.assertTrue(self.plan()[0]['blocked'])

    def test_manifest_mismatch_wrong_module_or_downgrade_never_form_a_plan(self):
        native = copy.deepcopy(self.native)
        for field, value in [('host_commit', 'b'*40), ('host_version', '12.0.0'),
                             ('format', True), ('database_checked', 'yes'), ('database_sha256', '../bad'), ('sdk_version', '1.0.0'), ('protocol', 1), ('inventory_sha256', '../bad')]:
            self.native = native | {field: value}
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.plan()
        self.native = copy.deepcopy(native)
        self.native['modules'][0]['package_sha256'] = 'd'*64
        with self.assertRaises(ValueError):
            self.plan()
        self.native = native
        self.current = dict(version='12.0.0', commit='f'*40)
        with self.assertRaises(ValueError):
            self.plan()
        self.current = dict(version='11.11.0', commit='f'*40)
        with self.assertRaises(ValueError):
            self.plan()

    def test_unknown_fields_and_ambiguous_contract_types_are_rejected(self):
        original = copy.deepcopy(self.native)
        for change in [dict(extra='ignored'), dict(format=True), dict(modules='bad'),
                       dict(database=original['database'] | {'version': float(original['database']['version'])})]:
            self.native = original | change
            with self.assertRaises(ValueError): self.plan()
        for change in [dict(path='/untrusted'), dict(publisher='bad'), dict(enabled=1), dict(id='../bad')]:
            self.native = copy.deepcopy(original)
            self.native['modules'][0].update(change)
            with self.assertRaises(ValueError): self.plan()

    def test_unchecked_database_is_blocked_and_changed_ledgers_invalidate_confirmation(self):
        original = self.plan()[1]
        self.native['database_sha256'] = 'f'*64
        self.assertNotEqual(self.plan()[1], original)
        self.native.update(database_checked=False, database_sha256=None)
        self.assertTrue(self.plan()[0]['blocked'])
        self.native['modules'][0]['publisher'] = 'f'*64
        self.assertTrue(self.plan()[0]['blocked'], 'Legacy publisher metadata is not an installation gate')
        self.native['modules'][0]['package_sha256'] = '0'*64
        with self.assertRaises(ValueError):
            self.plan()

    def test_keyless_module_plan_still_binds_exact_archive_to_signed_distribution(self):
        self.native['modules'][0]['publisher'] = ''
        self.platform['modules'][0].pop('publisher')
        self.assertFalse(self.plan()[0]['blocked'])
        self.native['modules'][0]['package_sha256'] = 'f' * 64
        with self.assertRaises(ValueError):
            self.plan()

    def test_configuration_fingerprint_is_private_stable_and_host_specific(self):
        store = coordinator.PlanStore(self.root, lambda _: None)
        original = store.configuration_fingerprint(dict(app='synthetic password'))
        self.assertEqual(store.configuration_fingerprint(dict(app='synthetic password')), original)
        self.assertNotEqual(store.configuration_fingerprint(dict(app='changed')), original)
        self.assertNotEqual(original, coordinator.digest(dict(app='synthetic password')))
        path = self.root / 'plan-secret'
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        path.unlink()
        self.assertNotEqual(store.configuration_fingerprint(dict(app='synthetic password')), original)
        path.chmod(0o644)
        with self.assertRaises(ValueError): store.configuration_fingerprint({})

    def test_plan_store_rejects_replay_equivocation_tampering_links_and_public_files(self):
        store = coordinator.PlanStore(self.root, lambda _: None)
        import distribution
        store.accept_manifest(self.manifest, 'b'*64, distribution)
        store.accept_manifest(self.manifest, 'b'*64, distribution)
        for value, identity in [(self.manifest, 'c'*64), (self.manifest | {'release': self.manifest['release'] | {'sequence': 6}}, 'b'*64)]:
            with self.assertRaises(ValueError):
                store.accept_manifest(value, identity, distribution)
        plan, identity = self.plan()
        self.assertEqual(store.save(plan), identity)
        self.assertEqual(store.save(plan), identity)
        self.assertEqual(store.load(identity), plan)
        path = self.root / 'plans' / (identity + '.json')
        changed = plan | {'configuration_sha256': 'd'*64}
        path.write_text(json.dumps(changed))
        with self.assertRaises(ValueError):
            store.load(identity)
        with self.assertRaises(ValueError):
            store.save(plan)
        path.write_text(json.dumps(plan))
        path.chmod(0o644)
        with self.assertRaises(ValueError):
            store.load(identity)
        path.chmod(0o600)
        original = path.with_suffix('.original')
        path.rename(original)
        path.symlink_to(original)
        with self.assertRaises(OSError):
            store.load(identity)
        with self.assertRaises(ValueError):
            store.load('../not-a-plan')

    def test_control_checks_kernel_peer_before_sending_and_binds_quiesce_inventory(self):
        snapshot = dict(inventory_sha256='a'*64, modules=[], suspended=True)
        class Client:
            def __init__(inner, uid=501, reply=None):
                inner.uid, inner.sent = uid, b''
                inner.raw = json.dumps(reply if reply is not None else dict(ok=True, snapshot=snapshot, error=None, configuration_sha256='c'*64)).encode()+b'\n'
            def __enter__(inner): return inner
            def __exit__(inner, *_): pass
            def settimeout(inner, _): pass
            def connect(inner, _): pass
            def getsockopt(inner, *_): return struct.pack('3i', 123, inner.uid, 20)
            def sendall(inner, raw): inner.sent += raw
            def recv(inner, size):
                part, inner.raw = inner.raw[:size], inner.raw[size:]
                return part
        client = Client()
        with patch.object(socket, 'socket', return_value=client), patch.object(socket, 'SO_PEERCRED', 17, create=True):
            actual = coordinator.control(self.root, 501, 'quiesce', 'a'*64, configuration_sha256='c'*64)
        self.assertEqual(actual, snapshot)
        self.assertEqual(json.loads(client.sent), dict(operation='quiesce', inventory_sha256='a'*64, configuration_sha256='c'*64))
        wrong = Client(uid=502)
        with patch.object(socket, 'socket', return_value=wrong), patch.object(socket, 'SO_PEERCRED', 17, create=True):
            with self.assertRaises(ValueError): coordinator.control(self.root, 501, 'inventory', configuration_sha256='c'*64)
        self.assertEqual(wrong.sent, b'')
        for code in ('busy', 'conflict', 'configuration', 'unavailable'):
            source = 'd'*64 if code == 'configuration' else 'c'*64
            reply = dict(ok=False, snapshot=None, error=code, configuration_sha256=source)
            with self.subTest(code=code), patch.object(socket, 'socket', return_value=Client(reply=reply)), patch.object(socket, 'SO_PEERCRED', 17, create=True):
                with self.assertRaises(coordinator.ControlRefusal) as caught:
                    coordinator.control(self.root, 501, 'quiesce', 'a'*64, configuration_sha256='c'*64)
                self.assertEqual(caught.exception.code, code)
                self.assertNotIn(source, str(caught.exception))
        for error in ('private/path/diagnostic', None, {'busy': True}):
            reply = dict(ok=False, snapshot=None, error=error, configuration_sha256='c'*64)
            with self.subTest(error=error), patch.object(socket, 'socket', return_value=Client(reply=reply)), patch.object(socket, 'SO_PEERCRED', 17, create=True):
                with self.assertRaises(ValueError) as caught:
                    coordinator.control(self.root, 501, 'inventory', configuration_sha256='c'*64)
                self.assertNotIsInstance(caught.exception, coordinator.ControlRefusal)
                self.assertNotIn('private/path', str(caught.exception))
        with patch.object(socket, 'socket', return_value=Client()), patch.object(socket, 'SO_PEERCRED', 17, create=True):
            with self.assertRaises(ValueError): coordinator.control(self.root, 501, 'inventory', configuration_sha256='d'*64)
        with patch.object(socket, 'socket', return_value=Client()), patch.object(socket, 'SO_PEERCRED', 17, create=True):
            coordinator.control(self.root, 501, 'inventory', configuration_sha256='c'*64)

        snapshot['suspended'] = False
        for reply in [dict(ok=True, snapshot=snapshot, error=None, configuration_sha256='c'*64), dict(ok=False, snapshot=snapshot, error='busy', configuration_sha256='c'*64)]:
            with patch.object(socket, 'socket', return_value=Client(reply=reply)), patch.object(socket, 'SO_PEERCRED', 17, create=True):
                with self.assertRaises(ValueError): coordinator.control(self.root, 501, 'quiesce', 'a'*64, configuration_sha256='c'*64)
        client = Client()
        with patch.object(socket, 'socket', return_value=client), patch.object(socket, 'SO_PEERCRED', 17, create=True):
            self.assertEqual(coordinator.control(self.root, 501, 'warm', 'a'*64, configuration_sha256='c'*64), snapshot)
        self.assertEqual(json.loads(client.sent), dict(operation='warm', inventory_sha256='a'*64, configuration_sha256='c'*64))
        with patch.object(socket, 'socket', return_value=Client()), patch.object(socket, 'SO_PEERCRED', 17, create=True):
            with self.assertRaises(ValueError): coordinator.control(self.root, 501, 'warm', 'b'*64, configuration_sha256='c'*64)

    def test_gate_refresh_updates_only_managed_file_and_rolls_back_invalid_nginx(self):
        from types import SimpleNamespace
        path = self.root / 'nginx-update-server.conf'
        path.write_text('# Include in the managed HTTPS server, before location selection.\nold page')
        calls = []
        engine = SimpleNamespace(trusted=lambda _: None, run=lambda command, *args: calls.append(command))
        coordinator.refresh_maintenance_gate('a'*64, engine, path=path)
        self.assertEqual(path.read_text(), coordinator.nginx_probe_configuration('a'*64)[1])
        self.assertEqual(calls, [['nginx','-t'], ['systemctl','reload','nginx.service']])
        coordinator.refresh_maintenance_gate('a'*64, engine, path=path)
        self.assertEqual(len(calls), 2, 'An unchanged gate does not reload Nginx')
        path.write_text('# Include in the managed HTTPS server, before location selection.\nprevious page')
        old = path.read_bytes()
        def fail(command, *args):
            if command == ['nginx','-t']: raise RuntimeError('synthetic invalid config')
        engine.run = fail
        with self.assertRaises(RuntimeError): coordinator.refresh_maintenance_gate('a'*64, engine, path=path)
        self.assertEqual(path.read_bytes(), old)
        backup=path.with_name('.'+path.name+'.previous')
        backup.write_bytes(old)
        path.write_text(coordinator.nginx_probe_configuration('a'*64)[1])
        engine.run=lambda command, *args: calls.append(command)
        coordinator.refresh_maintenance_gate('a'*64, engine, path=path)
        self.assertFalse(backup.exists())
        self.assertEqual(path.read_text(), coordinator.nginx_probe_configuration('a'*64)[1])
        path.write_text('unmanaged config')
        with self.assertRaises(ValueError): coordinator.refresh_maintenance_gate('a'*64, engine, path=path)
        self.assertEqual(path.read_text(), 'unmanaged config')

    def test_verification_smoke_accepts_manual_closure_but_requires_private_probe(self):
        probe = object.__new__(coordinator.Probe)
        def reply(path, **kwargs):
            headers = {'X-Talocart-Update-Probe-Allowed': '1', 'X-Talocart-Verification': '1'}
            if path == '/health/ready': return 200, b'verification-ready\n', headers
            if path == '/assets/ui.css': return 200, b'css', headers
            return 503, coordinator.MAINTENANCE_PAGE.encode(), headers | {'X-Talocart-Maintenance': 'manual'}
        probe._request = reply
        probe.smoke(verification=True)
        for missing in ('X-Talocart-Update-Probe-Allowed', 'X-Talocart-Verification'):
            def invalid(path, **kwargs):
                status, body, headers = reply(path, **kwargs)
                if status == 503: headers.pop(missing)
                return status, body, headers
            probe._request = invalid
            with self.assertRaises(ValueError): probe.smoke(verification=True)

    def test_maintenance_page_fits_one_nginx_token_and_pins_its_inline_code(self):
        import base64, hashlib, re
        _, gate = coordinator.nginx_probe_configuration('a'*64)
        page, policy = coordinator.maintenance_page()
        self.assertEqual((Path(__file__).resolve().parents[2]/'assets/maintenance.csp').read_text(), policy)
        self.assertIn("return 503 '" + page + "';", gate)
        self.assertLess(len(page.encode()), 4000, 'Nginx parses one token within a 4 KiB buffer')
        self.assertFalse(set(page) & set("$'\\"), 'Nginx would expand variables or end the string')
        self.assertTrue(all(lang in page for lang in ('lang="it"', 'lang="en"')))
        self.assertIn('<a href="">Ricarica / Reload</a>', page, 'Native reload works without JavaScript')
        self.assertNotRegex(page, r'(src|href)="(https?:)?//', 'Every route is 503: nothing external or proxied')
        for tag in ('style', 'script'):
            inline = re.search('<' + tag + '>(.*?)</' + tag + '>', page).group(1)
            pin = base64.b64encode(hashlib.sha256(inline.encode()).digest()).decode()
            self.assertIn(tag + "-src 'sha256-" + pin + "'", policy)
        self.assertIn('add_header Content-Security-Policy "' + policy + '" always;', gate)
        self.assertNotIn('unsafe-inline', policy)

    def test_journal_records_intent_before_effects_and_preserves_interrupted_phase(self):
        path = self.root / 'attempt.json'
        trust = lambda _: None
        journal = coordinator.Journal.begin(path, trust, 'a'*64, 'b'*32, dict(enabled={'koala-import': True}))
        with self.assertRaises(ValueError):
            journal.complete('updater_ready')
        journal.intent('updater_ready')
        interrupted = json.loads(path.read_text())
        self.assertEqual(interrupted['phase'], 'prepared')
        self.assertEqual(interrupted['pending'], 'updater_ready')
        self.assertTrue(interrupted['snapshot']['enabled']['koala-import'])
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        with self.assertRaises(ValueError):
            journal.intent('quiescing')
        journal.complete('updater_ready', dict(version='1.0.0'))
        journal.intent('quiescing')
        self.assertEqual(json.loads(path.read_text())['pending'], 'quiescing')
        with self.assertRaises(ValueError):
            coordinator.Journal.begin(path, trust, 'a'*64, 'c'*32, {})


if __name__ == '__main__':
    unittest.main()
