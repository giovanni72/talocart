#!/usr/bin/python3 -I
"""First server installation: signed base and updater in one administrative command.

Run only from the separately root-signed installation package. PostgreSQL, an
unprivileged account, host TOML, TLS and an operator-restricted Nginx site must
already be provisioned. Never executes application-package scripts as root.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import pwd
import re
import sys
import tempfile
import tomllib

sys.dont_write_bytecode = True


def require(condition, message):
    if not condition:
        raise ValueError(message)


def first_install(config, current_exists, service_state, installed):
    require(config.get('feed_format') == 2 and not config.get('bootstrap_updates', False),
            'First installation requires unified updates')
    require(not current_exists and service_state in ('inactive', 'failed') and not installed,
            'Existing installation: use the backend update or administrative recovery')
    require(config['writer_units'] == [], 'Provision external writers after first installation')


def service_text(config):
    for name in ('current', 'config'):
        require(re.fullmatch(r'/[A-Za-z0-9_./-]+', config[name]) is not None,
                'Unsupported systemd path')
    require(re.fullmatch(r'[a-z_][a-z0-9_-]*', config['service_user']) is not None
            and config['service_user'] != 'root', 'Unprivileged account required')
    return '''[Unit]
Description=Talocart
After=network-online.target
Wants=network-online.target
BindsTo=talocart-installation.service
After=talocart-installation.service
[Service]
Type=simple
User=USER
Group=USER
WorkingDirectory=/var/lib/talocart
ExecStart=CURRENT/bin/talocart --config CONFIG serve
Restart=on-failure
RestartSec=5
TimeoutStopSec=90
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/talocart
ReadWritePaths=-/var/lib/talocart-update/requests
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
[Install]
WantedBy=multi-user.target
'''.replace('USER', config['service_user']).replace('CURRENT', config['current']).replace('CONFIG', config['config'])


def stop_failed_installation(e, l, config, version):
    """Stopping must not depend on writable journal/marker/log storage."""
    import traceback
    try:
        descriptor = os.open(l.STATE/'installation-failure.log',
                             os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, 'w') as log:
            log.write(traceback.format_exc())
    except OSError:
        pass
    try:
        for name in ('maintenance', 'writer-fence'):
            try:
                (l.STATE/name).touch(mode=0o644)
            except OSError:
                pass
    finally:
        try:
            # No preliminary systemctl query: even a failed query must not skip stop.
            e.run(['systemctl', 'stop', config['service'], 'talocart-installation.service'])
        finally:
            try:
                l.atomic_json(l.STATE/'installation.json', dict(format=1, phase='failed', version=version))
            except OSError:
                pass  # The existing installing journal and released lease fail closed.


class Installation:
    def __init__(self, args, engine, launcher, bootstrap, distribution, bundle):
        self.args, self.e, self.l, self.b, self.d, self.bundle = args, engine, launcher, bootstrap, distribution, bundle

    def preflight(self):
        a, e, l, b = self.args, self.e, self.l, self.b
        self.raw = l.read(a.config_source, 65536)
        c = self.config = tomllib.loads(self.raw.decode())
        e.check_config(c)
        require(a.config_source != l.CONFIG, 'Stage configuration outside the installed updater')
        for p in (a.artifacts, a.nginx_site, a.root_public_key, Path(c['public_key'])):
            b.trusted(p)
        if c.get('token_file'):
            token = Path(c['token_file'])
            require(token.stat().st_mode & 0o077 == 0 and l.read(token, 8192).strip(),
                    'Repository token must exist and be root-private')
        require(b.public_key(a.root_public_key, l) != b.public_key(Path(c['public_key']), l),
                'Application and root-code keys must differ')
        account = self.account = pwd.getpwnam(c['service_user'])
        require(account.pw_uid > 0 and account.pw_gid > 0, 'Unprivileged service account required')
        first_install(c, Path(c['current']).exists() or Path(c['current']).is_symlink(),
                      e.run(['systemctl', 'show', '-p', 'ActiveState', '--value', c['service']]),
                      l.CONFIG.exists() or l.CURRENT.exists() or l.CURRENT.is_symlink())
        self.unit = Path('/etc/systemd/system') / c['service']
        require(e.run(['systemctl', 'show', '-p', 'LoadState', '--value', c['service']]) == 'not-found',
                'An existing service unit must not be overwritten')
        require(not self.unit.exists() and not self.unit.is_symlink(), 'Existing application unit')
        self.service = service_text(c)
        self.app = Path(c['config'])
        b.trusted(self.app.parent)
        app_raw = b.bounded(self.app, 1024**2, owner=account.pw_uid)
        require(self.app.stat().st_mode & 0o077 == 0, 'Application TOML must be private')
        app = tomllib.loads(app_raw.decode())
        require(app.get('updates', {}).get('root', str(l.STATE)) == str(l.STATE), 'Wrong application update root')
        # The app resolves omitted/relative roots beside its TOML, outside the
        # service's writable data tree. Never invent different installer defaults.
        for section, folder in (('media', 'media'), ('packages', 'packages'), ('logging', 'logs')):
            require(isinstance(app.get(section), dict)
                    and app[section].get('root') == '/var/lib/talocart/' + folder,
                    'First installation requires explicit storage roots below /var/lib/talocart')
        require(not Path('/var/lib/talocart/packages/registry.json').exists(), 'Existing package data')
        require(not l.incomplete() and not (l.STATE/'installation.json').exists(),
                'Recover the incomplete operation first')
        require(not self.unit.with_name('talocart-installation.service').exists(), 'Existing installation guard')
        # Fail before migration if the provided site cannot strip the probe header.
        site = l.read(a.nginx_site, 1024**2).decode()
        b.proxy_headers(site)
        require('include /etc/talocart/nginx-update-server.conf;' in site, 'Prepare the updater HTTPS include')
        self.manifest = self.d.verify(l.read(a.artifacts/'distribution.json', self.d.LIMIT),
            l.read(a.artifacts/'distribution.sig', 64), Path(c['public_key']), c['repository'])
        require(tuple(map(int, l.VERSION.split('.'))) >= tuple(map(int, self.manifest['minimum']['launcher'].split('.'))),
                'Installer launcher is too old')
        target = {'x86_64': 'x86_64-unknown-linux-gnu', 'aarch64': 'aarch64-unknown-linux-gnu'}.get(platform.machine())
        selected = [p for p in self.manifest['platforms'] if p['target'] == target]
        require(len(selected) == 1, 'No distribution for this server')
        self.platform = selected[0]
        require(not self.platform['modules'], 'First installation must not add optional modules')
        self.base, self.updater = self.platform['base'], self.platform['updater']
        kit = json.loads(l.read(Path(__file__).with_name('installation.json'), 16384))
        require(kit['format'] == 1 and kit['version'] == self.base['version']
                and kit['source_commit'] == self.base['commit'] and kit['target'] == target,
                'Installer and base provenance differ')
        for item in (self.base, self.updater):
            p = a.artifacts/item['asset']
            b.trusted(p)
            require(p.is_file() and p.stat().st_size == item['size'] and e.sha256(p) == item['sha256'],
                    'Installation artifact changed')
        b.trusted(a.artifacts/self.updater['signature_asset'])
        self.bundle.verify(a.artifacts/self.updater['asset'], a.artifacts/self.updater['signature_asset'],
                           a.root_public_key, self.updater['version'])
        root = Path(c['release_root'])
        b.parent_trust(root)
        b.parent_trust(l.STATE)
        b.parent_trust(self.unit.parent)
        require(not (root/self.base['version']).exists(), 'Release directory already exists')
        if 'updates' not in app:
            self.app_original = app_raw
            self.app_raw = app_raw + b'\n[updates]\nroot="/var/lib/talocart-update"\n'
        else:
            require(app['updates'].get('root') == str(l.STATE), 'Set the application update root')
            self.app_original = app_raw
            self.app_raw = app_raw
        return dict(format=1, operation='first-installation', version=self.base['version'],
                    updater_version=self.updater['version'], target=target, updater_required=True)

    def apply(self):
        result = self.preflight()
        e, l, b, c = self.e, self.l, self.b, self.config
        # Exclusive global installation lease. An interrupted attempt stays fenced;
        # retrying never overwrites its release or application data.
        l.STATE.mkdir(mode=0o755, parents=True, exist_ok=True)
        b.trusted(l.STATE)
        l.STATE.chmod(0o755)
        import fcntl
        fd = os.open(l.STATE/'installation.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, 'r+b') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.preflight()
            l.atomic_json(l.STATE/'installation.json', dict(format=1, phase='installing', version=self.base['version']))
            try:
                root = Path(c['release_root'])
                for folder in reversed([root, *root.parents]):
                    if not folder.exists():
                        folder.mkdir(mode=0o755); folder.chmod(0o755)
                    b.trusted(folder)
                dest = root/self.base['version']
                # Extract only the signed base allowlist; application content never
                # supplies the privileged installer, launcher or service unit.
                with tempfile.TemporaryDirectory(prefix='.install-', dir=root) as tmp:
                    stage = Path(tmp)/'release'
                    stage.mkdir(mode=0o755)
                    e.extract(self.args.artifacts/self.base['asset'], stage, self.base['version'],
                              self.base['commit'], self.platform['target'])
                    stage.rename(dest)
                for folder in (Path('/var/lib/talocart'), Path('/var/lib/talocart/media')):
                    if not folder.exists():
                        folder.mkdir(mode=0o700)
                        os.chown(folder, self.account.pw_uid, self.account.pw_gid)
                    require(folder.is_dir() and not folder.is_symlink() and folder.stat().st_uid == self.account.pw_uid,
                            'Untrusted application storage')
                # root-owned state, application-owned private request channel.
                requests = l.STATE/'requests'
                requests.mkdir(mode=0o700)
                os.chown(requests, self.account.pw_uid, self.account.pw_gid)
                if self.app_raw != self.app.read_bytes():
                    fd = os.open(self.app, os.O_WRONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                    with os.fdopen(fd, 'wb') as out:
                        import stat
                        info = os.fstat(out.fileno())
                        require(stat.S_ISREG(info.st_mode) and info.st_uid == self.account.pw_uid
                                and info.st_nlink == 1, 'Application config changed')
                        require(b.bounded(self.app, 1024**2, owner=self.account.pw_uid) == self.app_original, 'Application config changed')
                        out.truncate(0); out.write(self.app_raw); out.flush(); os.fsync(out.fileno())
                binary = dest/'bin/talocart'
                code = ("import sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]); "
                        "import operations as o; _,env=o.configuration(Path(sys.argv[2])); "
                        "query=\"SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace "
                        "WHERE n.nspname NOT LIKE 'pg_%' AND n.nspname <> 'information_schema'\"; "
                        "sys.exit(0 if o.run(['psql','--no-psqlrc','-At','-c',query],env=env).strip()==b'0' else 'Database is not empty')")
                e.as_user(c, ['/usr/bin/python3', '-I', '-c', code, dest/'scripts/release', c['config']])
                for command in ('migrate', 'doctor'):
                    e.as_user(c, [binary, '--config', c['config'], command], 3900)
                # Persist independent root-owned guard before the app can be started.
                import install_guard
                guard = l.STATE/'install_guard.py'
                with guard.open('xb') as out:
                    out.write(l.read(Path(__file__).with_name('install_guard.py'), 16384))
                    out.flush(); os.fsync(out.fileno())
                guard.chmod(0o644)
                with self.unit.with_name('talocart-installation.service').open('x') as out:
                    out.write(install_guard.UNIT)
                    out.flush(); os.fsync(out.fileno())
                self.unit.with_name('talocart-installation.service').chmod(0o644)
                l.sync_directory(l.STATE)
                l.sync_directory(self.unit.parent)
                Path(c['current']).symlink_to(dest)
                with self.unit.open('x') as out:
                    out.write(self.service)
                self.unit.chmod(0o644)
                e.run(['systemctl', 'daemon-reload'])
                e.run(['systemctl', 'start', c['service']])
                e.health(c)
                e.running_binary(c, binary)
                args = dict(config_source=self.args.config_source, engine=self.args.artifacts/self.updater['asset'],
                            signature=self.args.artifacts/self.updater['signature_asset'], engine_version=self.updater['version'],
                            root_public_key=self.args.root_public_key, nginx_site=self.args.nginx_site)
                # Bootstrap retains its own launcher/agent locks and input validation.
                e.run(['/usr/bin/python3', '-I', str(Path(__file__).with_name('bootstrap_updater.py')),
                       *[str(v) for pair in ((('--'+k.replace('_', '-')), v) for k, v in args.items()) for v in pair], '--apply'], 600)
                e.run(['systemctl', 'enable', c['service']])
                for unit in ('talocart-update.timer', 'talocart-update.path'):
                    require(e.run(['systemctl', 'is-active', unit]) == 'active', 'Updater scheduling did not start')
                l.atomic_json(l.STATE/'installation.json', dict(format=1, phase='succeeded', version=self.base['version']))
                return dict(result, installed=True, updater_installed=True)
            except BaseException:
                stop_failed_installation(e, l, c, self.base['version'])
                raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument('--config-source', type=Path, required=True)
    parser.add_argument('--root-public-key', type=Path, required=True)
    parser.add_argument('--nginx-site', type=Path, required=True)
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    try:
        require(sys.platform == 'linux' and os.geteuid() == 0 and sys.flags.isolated,
                'Run as root with /usr/bin/python3 -I')
        os.umask(0o077)
        # Isolated Python must import only the independently root-signed kit.
        here = Path(__file__).resolve().parent
        for path in (here, *here.parents, *[here/n for n in
                ('install_guard.py', 'bootstrap_updater.py', 'update_launcher.py', 'update_engine.py', 'update_coordinator.py',
                 'distribution.py', 'updates_feed.py', 'updater_bundle.py')]):
            info = path.lstat()
            require(not path.is_symlink() and info.st_uid == 0 and not info.st_mode & 0o022, 'Untrusted installer source')
        sys.path.insert(0, str(here))
        import update_engine as engine
        import update_launcher as launcher
        import bootstrap_updater as bootstrap
        import distribution
        import updater_bundle
        job = Installation(args, engine, launcher, bootstrap, distribution, updater_bundle)
        print(json.dumps(job.apply() if args.apply else job.preflight()))
    except Exception:
        parser.exit(1, 'First installation failed. Inspect root-private state and service journal; partial installations remain stopped.\n')


if __name__ == '__main__':
    main()
