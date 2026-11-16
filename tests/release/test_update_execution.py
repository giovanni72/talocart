"""Global phase ordering and fail-closed behavior; real OS cases live in systemd fixtures."""
import copy
import json
from pathlib import Path
import unittest
from unittest.mock import patch

import test_update_preparation
import update_coordinator as c


class Execution(unittest.TestCase):
    def setUp(self):
        f = test_update_preparation.Preparation('test_preparation_binds_original_state_and_reuses_cached_verified_artifacts')
        f.setUp()
        self.addCleanup(f.doCleanups)
        self.f = f
        self.prepared = f.prepare()
        self.events, self.failure, self.mode = [], None, False
        self.refusals = {}
        p, events = self.prepared, self.events
        p.engine.RUNTIME = f.root / 'run'
        p.engine.RUNTIME.mkdir()
        self.states = {'fixture.service': 'active'}
        def run(argv, *args):
            if argv[0] != 'systemctl': return ''
            if argv[1] in ('stop', 'start'):
                events.append(argv[1])
                for name in argv[2:]: self.states[name] = 'active' if argv[1] == 'start' else 'inactive'
                return ''
            if 'ActiveState' in argv: return self.states.get(argv[-1], 'inactive')
            if 'MainPID' in argv: return '0'
            return ''
        p.engine.run = run
        p.engine.health = lambda *_args, **kwargs: self.assertEqual(kwargs.get('verification', False), self.mode)
        p.engine.running_binary = lambda *_args, **kwargs: self.assertEqual(kwargs.get('verification', False), self.mode)
        def recover(_):
            events.append('recover')
            for name in ('maintenance', 'writer-fence'): (p.state / name).touch()
            for name in ('application-start', 'writers-start'): (p.engine.RUNTIME / name).unlink(missing_ok=True)
            self.states['fixture.service'] = 'inactive'
        p.engine.recover = recover
        class Ops:
            uid, gid = 501, 501
            def backup_preflight(inner, *args): return json.dumps(dict(backup_bytes=1024))
            def backup(inner, old, output, verify=False):
                events.append('backup-verify' if verify else 'backup')
                if not verify:
                    output.mkdir()
                    (output / 'talocart.toml').write_bytes(Path(f.config['config']).read_bytes())
                    (output / 'backup.json').write_text('{}')
                return 'OK'
            def application(inner, binary, command, **kwargs):
                events.append(command)
                if self.failure == command: raise ValueError('Synthetic command failure')
                if command == 'package-update-suspend': return json.dumps(p.original)
                if command in ('package-update-verify', 'package-update-restore'):
                    enabled = command == 'package-update-restore'
                    return json.dumps(dict(inventory_sha256='b'*64, suspended=False, modules=[dict(
                        id=m['id'],version=m['to'],sha256=m['sha256'],enabled=m['enabled'] if enabled else False)
                        for m in p.plan['modules']]))
                return '{}'
        p.ops = Ops()
        class Probe:
            def closed(inner):
                events.append('public-closed')
                self.assertTrue((p.state / 'maintenance').exists())
            def check(inner, *args): pass
            def smoke(inner, verification=False):
                events.append('verify-https' if verification else 'normal-https')
                self.assertEqual(verification, self.mode)
                self.assertTrue((p.state / 'maintenance').exists())
                if self.failure == 'verify-https' and verification: raise ValueError('Synthetic TLS failure')
            def _request(inner, *args, **kwargs):
                events.append('public-open')
                self.assertFalse((p.state / 'maintenance').exists())
                self.assertFalse(self.mode)
                return 200, b'ready\n', {}
        p.probe = Probe()
        def control(*args, **kwargs):
            operation = args[2]
            events.append(operation)
            if operation in self.refusals: raise c.ControlRefusal(self.refusals[operation])
            if self.failure == operation: raise ValueError('Synthetic control refusal')
            if operation == 'quiesce': return p.original | {'suspended': True}
            if operation == 'resume': return p.original
            return dict(inventory_sha256='b'*64, suspended=False, modules=[dict(
                id=m['id'],version=m['to'],sha256=m['sha256'],enabled=m['enabled']) for m in p.plan['modules']])
        def override(_execution, enable):
            self.mode = enable
            events.append('override-verify' if enable else 'override-normal')
        for name, value in [('control', control), ('seal_backup', lambda *_: events.append('seal')),
                            ('marker', lambda path, _: path.touch())]:
            context = patch.object(c, name, value)
            context.start(); self.addCleanup(context.stop)
        context = patch.object(c.Execution, 'verification_override', override)
        context.start(); self.addCleanup(context.stop)

    def test_independent_intent_precedes_the_first_durable_journal(self):
        begin=c.Journal.begin
        def guarded(path,trusted,identity,attempt,snapshot):
            processing=self.prepared.state/'processing.json'
            self.assertTrue(processing.is_file(), 'Crash after journal creation needs an independent intent')
            self.assertEqual(json.loads(processing.read_text()),dict(format=1,attempt=attempt,plan_sha256=identity))
            return begin(path,trusted,identity,attempt,snapshot)
        with patch.object(c.Journal,'begin',side_effect=guarded):
            c.Execution(self.prepared).run()

    def test_coherent_backup_precedes_flags_and_sql_and_traffic_opens_after_resume(self):
        result = c.Execution(self.prepared).run()
        e = self.events
        self.assertLess(e.index('backup-verify'), e.index('package-update-suspend'))
        self.assertLess(e.index('seal'), e.index('package-update-migrate'))
        self.assertLess(e.index('package-update-apply'), e.index('verify-https'))
        self.assertLess(e.index('verify-https'), e.index('package-update-restore'))
        self.assertLess(e.index('package-update-restore'), e.index('warm'))
        self.assertLess(e.index('warm'), e.index('public-open'))
        self.assertEqual(json.loads(next((self.prepared.state / 'attempts').glob('*.json')).read_text())['phase'], 'succeeded')
        self.assertEqual(result['release'], self.prepared.plan['release'])
        self.assertTrue((self.prepared.state / 'processing.json').exists(), 'Agent status acknowledgement is still required')

    def test_quiesce_refusal_resumes_old_host_without_migration_or_backup(self):
        self.failure = 'quiesce'
        with self.assertRaises(ValueError): c.Execution(self.prepared).run()
        self.assertIn('resume', self.events)
        self.assertNotIn('recover', self.events)
        self.assertNotIn('backup', self.events)
        self.assertEqual(self.states['fixture.service'], 'active')
        self.assertFalse((self.prepared.state / 'maintenance').exists())
        self.assertFalse((self.prepared.state / 'processing.json').exists())
        self.assertEqual(json.loads(next((self.prepared.state / 'attempts').glob('*.json')).read_text())['phase'], 'aborted')

    def test_typed_busy_or_conflict_retains_reason_and_cancels_quiesce_before_backup(self):
        for code in ('busy', 'conflict', 'unavailable'):
            self.refusals = {'quiesce': code}
            with self.subTest(code=code), self.assertRaises(c.ControlRefusal) as caught:
                c.Execution(self.prepared).run()
            self.assertEqual(caught.exception.code, code)
            self.assertIn('resume', self.events)
            self.assertNotIn('recover', self.events)
            self.assertNotIn('backup', self.events)
            self.assertNotIn('package-update-migrate', self.events)
            self.assertEqual(self.states['fixture.service'], 'active')
            self.assertFalse((self.prepared.state / 'maintenance').exists())
            self.assertFalse((self.prepared.state / 'processing.json').exists())

    def test_changed_loaded_configuration_that_cannot_resume_keeps_recovery_fence(self):
        self.refusals = {'quiesce': 'configuration', 'resume': 'configuration'}
        with self.assertRaises(c.ControlRefusal) as caught:
            c.Execution(self.prepared).run()
        self.assertEqual(caught.exception.code, 'configuration')
        self.assertIn('recover', self.events)
        self.assertNotIn('backup', self.events)
        self.assertNotIn('package-update-migrate', self.events)
        self.assertEqual(self.states['fixture.service'], 'inactive')
        self.assertTrue((self.prepared.state / 'maintenance').exists())
        self.assertTrue((self.prepared.state / 'writer-fence').exists())

    def test_uncertain_migration_keeps_backup_and_fence_without_restart_or_rollback(self):
        self.failure = 'package-update-migrate'
        with self.assertRaises(ValueError): c.Execution(self.prepared).run()
        self.assertIn('recover', self.events)
        self.assertNotIn('start', self.events)
        self.assertNotIn('package-update-restore', self.events)
        self.assertTrue((self.prepared.state / 'writer-fence').exists())
        journal = json.loads(next((self.prepared.state / 'attempts').glob('*.json')).read_text())
        self.assertEqual(journal['pending'], 'applying')
        self.assertTrue(Path(journal['snapshot']['backup']).is_dir())

    def test_failed_verification_does_not_restore_flags_start_workers_or_open_traffic(self):
        self.failure = 'verify-https'
        with self.assertRaises(ValueError): c.Execution(self.prepared).run()
        self.assertNotIn('package-update-restore', self.events)
        self.assertNotIn('warm', self.events)
        self.assertNotIn('public-open', self.events)
        self.assertEqual(self.states['fixture.service'], 'inactive')
        self.assertTrue((self.prepared.state / 'maintenance').exists())

    def test_modified_confirmed_plan_is_rejected_before_any_effect(self):
        self.prepared.plan['modules'][0]['enabled'] = True
        with self.assertRaises(ValueError): c.Execution(self.prepared).run()
        self.assertEqual(self.events, [])
        self.assertFalse((self.prepared.state / 'processing.json').exists())


if __name__ == '__main__': unittest.main()
