"""Proposal confirmation, handoff and acknowledgement; no OS/database simulation claims."""
import copy
import json
import os
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import test_update_preparation
import update_coordinator as c


class Cycle(unittest.TestCase):
    def setUp(self):
        f = test_update_preparation.Preparation('test_preparation_binds_original_state_and_reuses_cached_verified_artifacts')
        f.setUp(); self.addCleanup(f.doCleanups)
        self.f, self.p = f, f.prepare()
        self.requests = f.state / 'requests'; self.requests.mkdir(mode=0o700)
        self.status, self.executions, self.revoked = [], 0, False
        self.prepare_error = None
        self.preparation = self.p
        def prepare():
            if self.prepare_error is not None: raise self.prepare_error
            return self.preparation
        def publish(*args, **kwargs):
            self.status.append((args[2], args[3:], kwargs))
            if args[2] == 'succeeded' and self.executions: self.assertTrue((f.state / 'processing.json').exists())
        parent = self
        class Execute:
            def __init__(inner, prepared, progress):
                parent.assertIs(prepared, parent.preparation)
                inner.progress = progress
            def run(inner):
                parent.executions += 1
                c.atomic(f.state / 'processing.json', {}, lambda _:None)
                inner.progress('backed_up')
                return dict(backup_id='b'*32)
            def revoke(inner):
                parent.assertFalse((f.state / 'processing.json').exists())
                parent.revoked = True
        for module, name, value in [(c, 'Preparation', lambda *_:SimpleNamespace(run=prepare)),
                                    (c, 'Execution', Execute), (c, 'publish_status', publish)]:
            ctx = patch.object(module, name, value); ctx.start(); self.addCleanup(ctx.stop)
        import pwd
        ctx = patch.object(pwd, 'getpwnam', return_value=SimpleNamespace(pw_uid=os.getuid(),pw_gid=os.getgid()))
        ctx.start(); self.addCleanup(ctx.stop)
        self.recovery = []
        self.f.engine.recover = lambda _:self.recovery.append(True)

    def request(self, value=None):
        path = self.requests / 'install.json'
        path.write_bytes(c.canonical(value or c.candidate(self.p.plan, self.p.identity)))
        path.chmod(0o600)
        return path

    def run_cycle(self):
        return c.cycle(self.f.config, None, None, None, self.f.engine)

    def test_hourly_check_cache_never_delays_a_confirmed_install_request(self):
        import time
        c.atomic(self.f.state / 'status.json',dict(format=2,phase='available',checked_epoch=int(time.time())),lambda _:None)
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.status, [])
        self.request()
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.executions, 1)

    def test_manual_check_skips_hourly_cache_but_keeps_a_one_minute_floor(self):
        import time
        marker = self.requests / 'check.json'
        c.atomic(self.f.state / 'status.json',dict(format=2,phase='current',checked_epoch=int(time.time())-120),lambda _:None)
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.status, [], 'Without a marker the hourly cache still applies')
        marker.touch(0o600)
        self.assertEqual(self.run_cycle(), 0)
        self.assertFalse(marker.exists())
        self.assertEqual(self.status[-1][0], 'available')
        self.assertEqual(self.executions, 0, 'A manual check never installs')
        self.status.clear()
        c.atomic(self.f.state / 'status.json',dict(format=2,phase='current',checked_epoch=int(time.time())),lambda _:None)
        marker.touch(0o600)
        self.assertEqual(self.run_cycle(), 0)
        self.assertFalse(marker.exists(), 'A throttled marker is consumed so the path unit settles')
        self.assertEqual(self.status, [])
        with patch('time.time', return_value=time.time() + 70):
            self.assertEqual(self.run_cycle(), 0)
        self.assertTrue(self.status, 'A throttled check runs on the next timer')
        self.assertEqual(self.status[-1][0], 'available')
        self.assertFalse((self.f.state / 'pending-check.json').exists())
        self.assertEqual(self.executions, 0)

    def test_manual_check_marker_is_consumed_before_recovery_refusal(self):
        marker = self.requests / 'check.json'
        marker.touch(0o600)
        (self.f.state / 'pending-check.json').touch()
        (self.f.state / 'maintenance').touch()
        with self.assertRaises(ValueError): self.run_cycle()
        self.assertFalse(marker.exists())
        self.assertFalse((self.f.state / 'pending-check.json').exists())
        self.assertTrue(self.recovery)
        self.assertEqual(self.executions, 0)

    def test_future_timestamps_or_interrupted_preparation_do_not_hide_a_new_check(self):
        import time
        for phase, epoch in [('available',int(time.time())+3600),('installing',int(time.time()))]:
            c.atomic(self.f.state / 'status.json',dict(format=2,phase=phase,checked_epoch=epoch),lambda _:None)
            self.assertEqual(self.run_cycle(), 0)
            self.assertEqual(self.status[-1][0], 'available')

    def test_cached_check_cannot_bypass_an_incomplete_attempt(self):
        import time
        c.atomic(self.f.state / 'status.json',dict(format=2,phase='available',checked_epoch=int(time.time())),lambda _:None)
        (self.f.state / 'maintenance').touch()
        with self.assertRaises(ValueError): self.run_cycle()
        self.assertTrue(self.recovery)
        self.assertTrue(self.status[-1][2]['recovery'])
        self.assertEqual(self.executions, 0)

    def test_consumed_obsolete_request_does_not_delay_the_next_fresh_proposal(self):
        import time
        c.atomic(self.f.state / 'status.json',dict(format=2,phase='failed',checked_epoch=int(time.time()),candidate=None),lambda _:None)
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.status[-1][0], 'available')
        self.assertEqual(self.executions, 0)

    def test_available_proposal_does_not_install_without_confirmation(self):
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.status[-1][0], 'available')
        self.assertEqual(self.executions, 0)

    def test_exact_confirmation_publishes_success_before_final_acknowledgement(self):
        self.request()
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.executions, 1)
        self.assertTrue(self.revoked)
        self.assertFalse((self.f.state / 'preparing-request.json').exists())
        self.assertEqual([row[0] for row in self.status], ['installing','installing','succeeded'])
        self.assertFalse(self.recovery)

    def test_changed_plan_requires_fresh_confirmation_without_maintenance(self):
        self.request()
        self.preparation = copy.copy(self.p)
        self.preparation.plan = copy.deepcopy(self.p.plan)
        self.preparation.plan['configuration_sha256'] = 'f'*64
        self.preparation.identity = self.p.store.save(self.preparation.plan)
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.executions, 0)
        self.assertEqual(self.status[-1][2]['notice'], 'plan_changed')
        self.assertFalse((self.f.state / 'processing.json').exists())

    def test_handoff_keeps_confirmation_and_new_engine_revalidates_it(self):
        self.request(); self.p.needs_engine = True
        self.assertEqual(self.run_cycle(), 75)
        self.assertEqual(self.executions, 0)
        self.assertTrue((self.f.state / 'preparing-request.json').exists())
        handoff = json.loads((self.f.state / 'engine-handoff.json').read_text())
        self.assertEqual(handoff['plan_sha256'], self.p.identity)
        (self.f.state / 'engine-handoff.json').unlink()
        self.p.needs_engine = False
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.executions, 1)

    def test_failed_handoff_blocks_same_bytes_but_allows_a_corrected_approved_engine(self):
        self.request()
        c.atomic(self.f.state / 'engine-handoff-failed.json',dict(plan_sha256=self.p.identity,updater=self.p.plan['updater']),lambda _:None)
        self.p.needs_engine = True
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.status[-1][0], 'blocked')
        self.assertTrue(self.status[-1][2]['engine_blocked'])
        self.assertEqual(self.status[-1][2]['notice'],'engine_failed')
        self.assertFalse((self.f.state / 'preparing-request.json').exists())
        self.request()
        self.assertEqual(self.run_cycle(), 0, 'Another confirmation cannot authorize rejected engine bytes')
        self.assertFalse((self.f.state / 'engine-handoff.json').exists())
        self.p.plan['updater']['sha256']='f'*64
        self.p.identity=self.p.store.save(self.p.plan)
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.status[-1][0], 'available')
        self.request()
        self.assertEqual(self.run_cycle(), 75)

    def test_engine_only_handoff_finishes_without_application_backup_or_sql(self):
        row = self.p.plan['modules'][0]
        row.update(action='keep', to=row['from'], sha256=self.p.original['modules'][0]['sha256'], package_sha256=None)
        self.p.identity = self.p.store.save(self.p.plan)
        self.request(); self.p.needs_engine = True
        self.assertEqual(self.run_cycle(), 75)
        (self.f.state / 'engine-handoff.json').unlink()
        self.p.needs_engine = False
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.executions, 0)
        self.assertEqual(self.status[-1][0], 'succeeded')
        self.assertEqual(json.loads((self.f.state / 'deployed.json').read_text())['updater'], self.p.active_updater)
        self.assertFalse((self.f.state / 'preparing-request.json').exists())

    def test_download_failure_clears_request_without_fencing_healthy_shop(self):
        self.request(); self.prepare_error = ValueError('Synthetic download failure')
        with self.assertRaises(ValueError): self.run_cycle()
        self.assertEqual(self.status[-1][0], 'failed')
        self.assertFalse(self.status[-1][2]['recovery'])
        self.assertFalse(self.recovery)
        self.assertFalse((self.f.state / 'preparing-request.json').exists())
        self.assertFalse((self.f.state / 'maintenance').exists())
        self.prepare_error = None
        self.request()
        self.assertEqual(self.run_cycle(), 0)
        self.assertEqual(self.executions, 1)

    def test_typed_preflight_refusals_reach_public_notice_without_fencing_or_diagnostics(self):
        for code in ('busy', 'conflict', 'configuration', 'unavailable'):
            self.request(); self.prepare_error = c.ControlRefusal(code)
            with self.subTest(code=code), self.assertRaises(c.ControlRefusal):
                self.run_cycle()
            self.assertEqual(self.status[-1][2]['notice'], 'control_'+code)
            self.assertFalse(self.status[-1][2]['recovery'])
            self.assertFalse(self.recovery)
            self.assertFalse((self.f.state / 'maintenance').exists())
            self.assertFalse((self.f.state / 'preparing-request.json').exists())
        self.assertEqual(self.executions, 0)

    def test_invalid_request_is_consumed_and_reported_without_effects(self):
        self.request(dict(version='11.12.0',commit='a'*40,digest='../private'))
        with self.assertRaises(ValueError): self.run_cycle()
        self.assertFalse((self.f.state / 'preparing-request.json').exists())
        self.assertEqual(self.status[-1][0], 'check_failed')
        self.assertEqual(self.executions, 0)

    def test_unknown_proposal_is_rejected_and_reported_without_loading_it_twice(self):
        self.request(dict(version='11.12.0',commit='a'*40,digest='f'*64))
        with self.assertRaises(FileNotFoundError): self.run_cycle()
        self.assertFalse((self.f.state / 'preparing-request.json').exists())
        self.assertEqual(self.status[-1][0], 'failed')
        self.assertEqual(self.executions, 0)

    def test_public_projection_contains_no_paths_configuration_or_signing_material(self):
        value = c.public_plan(self.p.plan)
        self.assertEqual(value['modules'][0]['action'], 'update')
        self.assertFalse(value['modules'][0]['enabled'])
        raw = json.dumps(value)
        for secret in ('configuration_sha256','publisher','asset','manifest_sha256',str(self.f.root)):
            self.assertNotIn(secret, raw)


if __name__ == '__main__': unittest.main()
