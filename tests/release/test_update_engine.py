"""Deployment orchestration with synthetic archives and process/network boundaries.

No SSH connection, real systemd unit, shop database or application process is used.
"""
import contextlib
import importlib.util
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


def module(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / (name + '.py'))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


server = module('update_engine')


class DeployContracts(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.commit = 'a' * 40
        self.target = 'x86_64-unknown-linux-gnu'
        self.version = '1.2.3'

    def archive(self, bad=None):
        tree = self.root / f'talocart-{self.version}-{self.target}'
        tree.mkdir(exist_ok=True)
        (tree / 'bin').mkdir(exist_ok=True)
        (tree / 'bin/talocart').write_bytes(b'synthetic executable')
        (tree / 'scripts/release').mkdir(parents=True, exist_ok=True)
        (tree / 'scripts/release/operations.py').write_text('# synthetic maintenance\n')
        build = {'version': self.version, 'commit': self.commit, 'source_dirty': False, 'tagged': True}
        manifest = {'format': 1, 'profile': 'base', 'version': self.version, 'source_commit': self.commit,
                    'source_dirty': False, 'target': self.target, 'build': build,
                    'files': {p.relative_to(tree).as_posix(): server.sha256(p) for p in tree.rglob('*') if p.is_file()}}
        if bad == 'identity':
            manifest['build']['tagged'] = False
        if bad == 'checksum':
            manifest['files']['bin/talocart'] = '0' * 64
        (tree / 'release.json').write_text(json.dumps(manifest))
        archive = self.root / (tree.name + '.tar.gz')
        with tarfile.open(archive, 'w:gz') as bundle:
            bundle.add(tree, arcname=tree.name)
            if bad in ('traversal', 'link', 'device', 'duplicate'):
                info = tarfile.TarInfo(tree.name + '/../../escape' if bad == 'traversal' else tree.name + '/extra')
                if bad == 'link':
                    info.type, info.linkname = tarfile.SYMTYPE, '/etc/passwd'
                if bad == 'device':
                    info.type = tarfile.CHRTYPE
                if bad == 'duplicate':
                    info.name = tree.name + '/bin/talocart'
                bundle.addfile(info)
        return archive, build

    def test_only_explicit_unified_feed_is_accepted(self):
        _, _, config, _, _ = self.setup_deploy()
        for value in (None, 1, True, "2", 3):
            with self.assertRaises(server.DeployError):
                server.check_config(config | {'feed_format': value})
        server.check_config(config | {'feed_format': 2})
        with self.assertRaises(server.DeployError):
            server.check_config(config | {'feed_format': 2, 'bootstrap_updates': True})

    def test_archive_rejects_traversal_links_devices_duplicates_and_wrong_identity(self):
        for bad in ('traversal', 'link', 'device', 'duplicate', 'identity', 'checksum'):
            with self.subTest(bad=bad):
                archive, _ = self.archive(bad)
                with tempfile.TemporaryDirectory(dir=self.root) as output:
                    with self.assertRaises(server.DeployError):
                        server.extract(archive, Path(output), self.version, self.commit, self.target)
                self.assertFalse((self.root / 'escape').exists())

    def test_extract_verifies_all_manifest_files_and_keeps_binary_executable(self):
        archive, build = self.archive()
        output = self.root / 'extracted'
        output.mkdir()
        manifest = server.extract(archive, output, self.version, self.commit, self.target)
        self.assertEqual(manifest['build'], build)
        self.assertEqual((output / 'bin/talocart').stat().st_mode & 0o777, 0o755)

    def setup_deploy(self):
        archive, build = self.archive()
        incoming = self.root / 'incoming'
        incoming.mkdir()
        archive = archive.rename(incoming / archive.name)
        releases = self.root / 'releases'
        old = releases / '1.2.2'
        old.mkdir(parents=True)
        (old / 'release.json').write_text('{"version":"1.2.2"}')
        current = self.root / 'current'
        current.symlink_to(old)
        state = self.root / 'state'
        state.mkdir()
        config_path = self.root / 'app.toml'
        config_path.write_text('[packages]\nroot="packages"\n[server]\nlisten="127.0.0.1:3000"\n'
                               'public_origin="https://shop.example.com"\nsecure_cookies=true\n')
        config = dict(service_user='talocart', service='talocart.service', release_root=str(releases),
                      current=str(current), config=str(config_path), backup_root=str(self.root / 'backups'),
                      incoming=str(incoming), public_origin='https://shop.example.com',
                      ready_url='http://127.0.0.1:3000/health/ready', writer_units=['import.service'],
                      writers_fenced=True, environment='staging')
        return archive, build, config, state, old









    def test_activation_requires_live_lock_and_boot_scoped_permit(self):
        state, runtime = self.root / 'state', self.root / 'run'
        state.mkdir(); runtime.mkdir()
        lock = state / 'agent.lock'
        lock.touch()
        with patch.object(server, 'STATE', state), patch.object(server, 'RUNTIME', runtime):
            self.assertTrue(server.application_start_allowed())
            (state / 'writer-fence').touch()
            self.assertFalse(server.application_start_allowed())
            permit = runtime / 'application-start'
            permit.touch()
            self.assertFalse(server.application_start_allowed(), 'Stale permit alone must not authorize restart')
            child = subprocess.Popen([sys.executable, '-c',
                'import fcntl,sys; f=open(sys.argv[1]); fcntl.flock(f,fcntl.LOCK_EX); '
                'print("locked",flush=True); sys.stdin.read()', str(lock)],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
            try:
                self.assertEqual(child.stdout.readline().strip(), 'locked')
                self.assertTrue(server.application_start_allowed())
                permit.unlink()  # /run is empty after reboot, even if another check holds the lock.
                self.assertFalse(server.application_start_allowed())
                permit.touch()
                child.kill(); child.wait(timeout=5)
                self.assertFalse(server.application_start_allowed(), 'SIGKILL must revoke the activation permit')
            finally:
                if child.poll() is None:
                    child.kill()
                child.communicate(timeout=5)

    def test_recovery_restores_markers_revokes_permit_and_stops_all_writers(self):
        runtime = self.root / 'run'; runtime.mkdir()
        permit = runtime / 'application-start'; permit.touch()
        config = dict(service='talocart.service', writer_units=['import.service'])
        def stopped(argv):
            self.assertTrue((self.root / 'maintenance').exists())
            self.assertTrue((self.root / 'writer-fence').exists())
            self.assertFalse(permit.exists())
            self.assertEqual(argv, ['systemctl', 'stop', 'talocart.service', 'import.service'])
        with patch.object(server, 'STATE', self.root), patch.object(server, 'RUNTIME', runtime), \
             patch.object(server, 'run', side_effect=stopped) as command:
            server.recover(config)
            server.recover(config)
            self.assertEqual(command.call_count, 2)

    def test_command_failure_keeps_diagnostics_private_and_out_of_client_error(self):
        with patch.object(server, 'STATE', self.root):
            with self.assertRaises(server.DeployError) as error:
                server.run([sys.executable, '-c', 'import sys; print("synthetic-private-value"); sys.exit(2)'])
        self.assertNotIn('synthetic-private-value', str(error.exception))
        log = self.root / 'failure.log'
        self.assertIn(b'synthetic-private-value', log.read_bytes())
        self.assertEqual(log.stat().st_mode & 0o777, 0o600)



class StrictModuleReadiness(unittest.TestCase):
    def test_local_update_probe_requires_every_enabled_module(self):
        from unittest.mock import MagicMock
        response = MagicMock()
        response.__enter__.return_value = response
        response.status = 200
        response.read.return_value = b'ready\n'
        opener = MagicMock()
        opener.open.return_value = response
        with patch.object(server.urllib.request, 'build_opener', return_value=opener):
            server.health(dict(ready_url='http://127.0.0.1:3000/health/ready', public_origin='https://shop.example.test'))
        request = opener.open.call_args.args[0]
        self.assertEqual(request.full_url, 'http://127.0.0.1:3000/health/ready?all_modules=true')

    def test_https_update_probe_requires_every_enabled_module(self):
        import http.client
        import ssl
        from urllib.parse import urlsplit
        from unittest.mock import MagicMock
        with patch.object(sys, 'path', [str(SCRIPTS), *sys.path]):
            update_probe = module('update_probe')
        probe = object.__new__(update_probe.Probe)
        probe.origin = urlsplit('https://shop.example.test')
        probe.context = ssl.create_default_context()
        probe.token = 'a' * 64
        probe.timeout = 1
        response = MagicMock(status=200)
        response.read.return_value = b'ready\n'
        response.getheaders.return_value = [('X-Talocart-Update-Probe-Allowed', '1')]
        with patch.object(http.client.HTTPSConnection, 'request') as request, patch.object(http.client.HTTPSConnection, 'getresponse', return_value=response):
            probe.check('/health/ready')
        self.assertEqual(request.call_args.args[:2], ('GET', '/health/ready?all_modules=true'))


if __name__ == '__main__':
    unittest.main()
