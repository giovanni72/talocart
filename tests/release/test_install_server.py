"""First-install safety boundaries; real Linux installation has a systemd smoke."""
from pathlib import Path
import sys
import unittest
import tempfile
from unittest.mock import Mock, patch
from types import SimpleNamespace
sys.path.insert(0, str(Path(__file__).resolve().parents[2]/'scripts/release'))
import install_server as install
import install_guard as guard
import bootstrap_updater as bootstrap
import updater_bundle


class InstallServer(unittest.TestCase):
    def setUp(self):
        self.config = dict(feed_format=2, writer_units=[],
            service_user='talocart', current='/opt/talocart/current', config='/etc/talocart/talocart.toml')

    def test_only_empty_servers_can_use_first_installation(self):
        install.first_install(self.config, False, 'inactive', False)
        for current, state, installed in [(True,'inactive',False),(False,'active',False),(False,'inactive',True)]:
            with self.subTest(current=current,state=state,installed=installed), self.assertRaises(ValueError):
                install.first_install(self.config, current, state, installed)
        for edit in [dict(feed_format=1),dict(bootstrap_updates=True),dict(writer_units=['import.service'])]:
            with self.subTest(edit=edit), self.assertRaises(ValueError):
                install.first_install(dict(self.config, **edit),False,'inactive',False)

    def test_unit_cannot_inject_systemd_commands_or_expand_specifiers(self):
        unit=install.service_text(self.config)
        self.assertIn('User=talocart\n',unit)
        self.assertIn('ReadWritePaths=-/var/lib/talocart-update/requests',unit)
        for name,value in [('current','/opt/%n'),('config','/etc/$HOME'),('config','/etc/a\nExecStartPost=/bin/true'),('service_user','root')]:
            with self.subTest(name=name,value=value), self.assertRaises(ValueError):
                install.service_text(dict(self.config, **{name:value}))

    def test_application_lifetime_is_bound_to_installation_guard(self):
        unit = install.service_text(self.config)
        self.assertIn('BindsTo=talocart-installation.service\n', unit)
        self.assertIn('After=talocart-installation.service\n', unit)

    def test_preflight_requires_explicit_storage_paths_before_artifact_work(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app_path = root / 'talocart.toml'
            app_path.touch(mode=0o600)
            args = SimpleNamespace(config_source=root/'update-source.toml', artifacts=root/'artifacts',
                nginx_site=root/'nginx.conf', root_public_key=root/'root.pem')
            config = self.config | dict(config=str(app_path), current=str(root/'current'),
                service='talocart-storage-fixture.service', public_key=str(root/'app.pem'), repository='fixture/repo')
            raw = '\n'.join(f'{key}={value!r}' for key,value in config.items()
                            if isinstance(value,str)).encode() + b'\nfeed_format=2\nwriter_units=[]\n'
            engine = Mock()
            engine.run.side_effect = lambda argv: 'inactive' if 'ActiveState' in argv else 'not-found'
            launcher = SimpleNamespace(CONFIG=root/'installed.toml', CURRENT=root/'engine-current',
                STATE=root/'state', incomplete=lambda: False,
                read=lambda path, limit: raw if path == args.config_source else b'include /etc/talocart/nginx-update-server.conf;')
            bootstrap = Mock()
            bootstrap.public_key.side_effect = lambda path, _: str(path)
            distribution = Mock(LIMIT=1024)
            distribution.verify.side_effect = RuntimeError('artifact validation reached')
            installer = install.Installation(args, engine, launcher, bootstrap, distribution, Mock())
            expected = dict(media='/var/lib/talocart/media', packages='/var/lib/talocart/packages',
                            logging='/var/lib/talocart/logs')
            cases = [(name, value) for name in expected for value in (None, {}, {'root':'storage/'+name}, {'root':'/outside/'+name})]
            account = SimpleNamespace(pw_uid=501, pw_gid=501)
            original_exists = Path.exists
            registry = Path('/var/lib/talocart/packages/registry.json')
            def empty_target_exists(path):
                # Model a fresh server without inspecting the developer's installation.
                return original_exists(path) if path.is_relative_to(root) else False
            with patch.object(install.pwd, 'getpwnam', return_value=account), \
                    patch.object(Path, 'exists', empty_target_exists):
                for name, value in cases:
                    with self.subTest(section=name,value=value):
                        distribution.verify.reset_mock()
                        values = {key: {'root':path} for key,path in expected.items()}
                        if value is None:
                            del values[name]
                        else:
                            values[name] = value
                        bootstrap.bounded.return_value = '\n'.join(
                            f'[{key}]\n' + '\n'.join(f'{field}={item!r}' for field,item in row.items())
                            for key,row in values.items()).encode()
                        with self.assertRaisesRegex(ValueError, 'explicit.*storage|persistent data'):
                            installer.preflight()
                        distribution.verify.assert_not_called()
                distribution.verify.reset_mock()
                bootstrap.bounded.return_value = '\n'.join(f'[{key}]\nroot={path!r}' for key,path in expected.items()).encode()
                with self.assertRaisesRegex(RuntimeError, 'artifact validation reached'):
                    installer.preflight()
                distribution.verify.assert_called_once()
                distribution.verify.reset_mock()
                with patch.object(Path, 'exists', lambda path: path == registry or empty_target_exists(path)):
                    with self.assertRaisesRegex(ValueError, 'Existing package data'):
                        installer.preflight()
                distribution.verify.assert_not_called()

    def test_disk_full_cannot_skip_service_stop(self):
        state = Mock()
        state.__truediv__ = Mock(return_value=state)
        state.touch.side_effect = OSError('ENOSPC')
        launcher = SimpleNamespace(STATE=state, atomic_json=Mock(side_effect=OSError('ENOSPC')))
        engine = SimpleNamespace(run=Mock(return_value='loaded'))
        with patch('os.open', side_effect=OSError('ENOSPC')):
            install.stop_failed_installation(engine, launcher, {'service': 'talocart.service'}, '11.12.0')
        engine.run.assert_any_call(['systemctl', 'stop', 'talocart.service', 'talocart-installation.service'])

    def test_guard_rejects_lost_lease_and_failed_journal(self):
        for status in ('installing', 'failed'):
            with self.subTest(status=status), patch.object(guard, 'phase', return_value=status), patch.object(guard.fcntl, 'flock'):
                with self.assertRaises(ValueError): guard.check(42)
        with patch.object(guard, 'phase', return_value='installing'), patch.object(guard.fcntl, 'flock', side_effect=BlockingIOError):
            self.assertFalse(guard.check(42))
        with patch.object(guard, 'phase', side_effect=['installing','succeeded']), patch.object(guard.fcntl, 'flock'):
            self.assertTrue(guard.check(42))
        with patch.object(guard, 'phase', return_value='succeeded'), patch.object(guard.fcntl, 'flock') as lock:
            self.assertTrue(guard.check(42))
            lock.assert_not_called()

    def test_installation_packaging_refuses_readable_or_linked_private_keys_before_signing(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary); key=root/'key';key.write_text('synthetic');key.chmod(0o644)
            link=root/'link';link.symlink_to(key)
            for candidate in (key,link):
                with self.subTest(candidate=candidate),patch.object(updater_bundle.feed,'command') as command:
                    with self.assertRaises(ValueError):
                        updater_bundle.build_installation(root,'a'*40,'v11.12.0','x86_64-unknown-linux-gnu',root,root/'public',root/'root-public',candidate,root/'out')
                    command.assert_not_called()

    def test_first_nginx_reload_waits_for_old_workers_but_never_accepts_a_bad_probe(self):
        probe=SimpleNamespace(check=Mock(side_effect=[ValueError('Private HTTPS probe failed'),None]))
        with patch('time.sleep'):
            bootstrap.gateway_ready(probe)
        self.assertEqual(probe.check.call_count,2)
        probe.check=Mock(side_effect=ValueError('Private HTTPS probe failed'))
        with patch('time.monotonic',side_effect=[0,5]),patch('time.sleep') as sleep:
            with self.assertRaises(ValueError):bootstrap.gateway_ready(probe)
            sleep.assert_not_called()

if __name__=='__main__':unittest.main()
