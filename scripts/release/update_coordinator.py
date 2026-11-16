"""Unified-update planning and durable attempt journal, shared by trusted engine phases.

No service or application mutations at import time. The target Rust binary owns
module validation; this module binds its result to signed distribution metadata.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import tempfile

ENGINE_VERSION = '1.0.0'


from update_plan import canonical, create_plan, digest, require


from update_state import PHASES, Journal, PlanStore, atomic, private_json


from update_host import Operations, root_bytes, sync_folder
from update_probe import MAINTENANCE_PAGE, MAINTENANCE_STYLE, MAINTENANCE_SCRIPT, Probe, maintenance_page, nginx_probe_configuration, refresh_maintenance_gate


class ControlRefusal(ValueError):
    """A fixed, non-sensitive reason from the authenticated live application."""
    CODES = ('busy', 'conflict', 'configuration', 'unavailable')

    def __init__(self, code):
        require(isinstance(code, str) and code in self.CODES, 'Invalid update control refusal')
        self.code = code
        super().__init__('Application refused update control: ' + code)


def control(root, service_uid, operation, inventory_sha256=None, *, configuration_sha256):
    """Root updater talks to the live host; authenticate its OS peer before sending.

    Linux SO_PEERCRED is a kernel credential, not a header or token from the app.
    This channel cannot name commands, executables, files or arbitrary actions.
    """
    import socket
    import struct
    require(operation in ('inventory', 'quiesce', 'resume', 'warm'), 'Invalid update control operation')
    require(isinstance(configuration_sha256, str) and re.fullmatch(r'[0-9a-f]{64}', configuration_sha256), 'Invalid expected startup configuration')
    message = {'operation': operation, 'configuration_sha256': configuration_sha256}
    if operation in ('quiesce', 'warm'):
        require(isinstance(inventory_sha256, str) and re.fullmatch(r'[0-9a-f]{64}', inventory_sha256),
                'Invalid expected inventory')
        message['inventory_sha256'] = inventory_sha256
    require(type(service_uid) is int and service_uid > 0, 'Invalid application service UID')
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(600)
        client.connect(str(root / 'requests/control.sock'))
        _, uid, _ = struct.unpack('3i', client.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
        require(uid == service_uid, 'Update control peer is not the application account')
        client.sendall(canonical(message))
        raw = bytearray()
        while not raw.endswith(b'\n'):
            chunk = client.recv(min(4096, 32769 - len(raw)))
            require(chunk, 'Update control connection closed')
            raw.extend(chunk)
            require(len(raw) <= 32768, 'Update control response exceeds limit')
    value = json.loads(raw)
    require(isinstance(value, dict) and set(value) == {'ok', 'snapshot', 'error', 'configuration_sha256'}
            and type(value['ok']) is bool, 'Invalid update control response')
    source = value['configuration_sha256']
    require(source is None or isinstance(source, str) and re.fullmatch(r'[0-9a-f]{64}', source), 'Invalid startup configuration fingerprint')
    if source != configuration_sha256:
        raise ControlRefusal('configuration')
    if not value['ok']:
        require(isinstance(value['error'], str) and value['error'] in ControlRefusal.CODES, 'Invalid update control refusal')
        raise ControlRefusal(value['error'])
    require(value['error'] is None, 'Invalid successful update control response')
    snapshot = value['snapshot']
    require(isinstance(snapshot, dict) and set(snapshot) == {'inventory_sha256', 'modules', 'suspended'}
            and isinstance(snapshot['inventory_sha256'], str)
            and re.fullmatch(r'[0-9a-f]{64}', snapshot['inventory_sha256'])
            and type(snapshot['suspended']) is bool and isinstance(snapshot['modules'], list)
            and len(snapshot['modules']) <= 32, 'Invalid application inventory response')
    ids = set()
    for module in snapshot['modules']:
        require(isinstance(module, dict) and set(module) == {'id', 'version', 'enabled', 'sha256'}
                and isinstance(module['id'], str) and re.fullmatch(r'[a-z][a-z0-9-]{0,47}', module['id'])
                and module['id'] not in ids and type(module['enabled']) is bool
                and isinstance(module['version'], str) and re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', module['version'])
                and isinstance(module['sha256'], str) and re.fullmatch(r'[0-9a-f]{64}', module['sha256']),
                'Invalid module inventory response')
        ids.add(module['id'])
    if operation == 'quiesce':
        require(snapshot['suspended'] is True and snapshot['inventory_sha256'] == inventory_sha256,
                'Application did not quiesce the expected inventory')
    if operation in ('resume', 'warm'):
        require(snapshot['suspended'] is False, 'Application did not resume')
    if operation == 'warm':
        require(snapshot['inventory_sha256'] == inventory_sha256, 'Activation inventory changed')
    return snapshot


def check_tree(folder, engine):
    """Root-owned release tree, including previously prepared/cached extractions."""
    from pathlib import PurePosixPath
    engine.trusted(folder)
    value = json.loads(root_bytes(folder / 'release.json', engine))
    require(value.get('format') == 1 and value.get('profile') == 'base' and value.get('source_dirty') is False
            and isinstance(value.get('files'), dict) and {'bin/talocart','scripts/release/operations.py'} <= value['files'].keys() and len(value['files']) <= 50000
            and isinstance(value.get('build'), dict) and value['build'].get('tagged') is True
            and value['build'].get('source_dirty') is False, 'Unsupported installed release')
    require(value.get('version') == value['build'].get('version') and value.get('source_commit') == value['build'].get('commit'), 'Incoherent release identity')
    for name, identity in value['files'].items():
        relative = PurePosixPath(name)
        require(relative.parts and not relative.is_absolute() and '..' not in relative.parts
                and relative.as_posix() == name and name != 'release.json'
                and isinstance(identity, str) and re.fullmatch(r'[0-9a-f]{64}', identity), 'Invalid release file list')
        engine.trusted(folder / name)
        require(engine.sha256(folder / name) == identity, 'Prepared/installed release changed')
    actual = set()
    for path in folder.rglob('*'):
        require(not path.is_symlink(), 'Release tree contains links')
        if path.is_file() and path != folder / 'release.json':
            actual.add(path.relative_to(folder).as_posix())
    require(actual == set(value['files']), 'Unexpected/missing release files')
    return value


def require_capacity(budgets):
    """Sum reservations on shared filesystems, instead of checking each path alone."""
    import shutil
    devices = {}
    for path, amount in budgets:
        require(type(amount) is int and amount >= 0, 'Invalid capacity budget')
        device = path.stat().st_dev
        if device in devices:
            devices[device][1] += amount
        else:
            devices[device] = [path, amount]
    for path, amount in devices.values():
        require(shutil.disk_usage(path).free >= amount, 'Insufficient capacity for update and coordinated backup')


def check_units(config, engine, *, guards=True):
    material = {}
    for unit in [config['service'], *config['writer_units']]:
        require(engine.run(['systemctl', 'show', '-p', 'LoadState', '--value', unit]) == 'loaded', 'Managed writer unit is missing')
        kind = engine.run(['systemctl', 'show', '-p', 'Type', '--value', unit])
        supported = ('simple','exec') if unit == config['service'] else ('simple','exec','notify','dbus','forking')
        require(kind in supported and engine.run(['systemctl','show','-p','RemainAfterExit','--value',unit]) == 'no',
                'Unsupported writer lifecycle: oneshot and RemainAfterExit units cannot be resumed automatically')
        for name in ('Environment', 'EnvironmentFiles'):
            require(not engine.run(['systemctl', 'show', '-p', name, '--value', unit]), 'Service configuration overrides are unsupported')
        if unit == config['service']:
            require(engine.run(['systemctl', 'show', '-p', 'User', '--value', unit]) == config['service_user'], 'Managed application UID differs')
            for name in ('ExecStartPre', 'ExecStartPost'):
                require(not engine.run(['systemctl', 'show', '-p', name, '--value', unit]), 'Startup hooks are unsupported in workerless verification')
        conditions = engine.run(['systemctl','show','-p','ExecCondition','--value',unit])
        if conditions:
            option = '--application-start-allowed' if unit == config['service'] else '--writer-start-allowed'
            scripts = ('update_launcher.py',) if guards else ('update_launcher.py','update_engine.py')
            require(conditions.count('argv[]=') == 1 and any(
                'argv[]=/usr/bin/python3 -I /usr/local/libexec/talocart/' + script + ' ' + option + ' ;' in conditions
                for script in scripts), 'Additional execution conditions are unsupported')
        elif guards:
            raise ValueError('Managed launcher condition is not loaded')
        if guards:
            guard = Path('/etc/systemd/system') / (unit + '.d') / '90-talocart-update.conf'
            require(root_bytes(guard, engine, 4096).decode() == unit_guard(unit == config['service']), 'Install the independent launcher guards before updating')
        material[unit] = engine.run(['systemctl', 'show', '-p', 'Type,RemainAfterExit,ExecStart,ExecStartPre,ExecStartPost,ExecStop,ExecCondition,User,Group,Environment,EnvironmentFiles,Restart,TimeoutStopUSec', unit])
    return material


def unit_guard(application):
    return '[Service]\nExecCondition=+/usr/bin/python3 -I /usr/local/libexec/talocart/update_launcher.py --' + ('application' if application else 'writer') + '-start-allowed\n'


class Preparation:
    """Resolve the complete approved host-specific proposal before downtime."""
    def __init__(self, config, feed, distribution, agent, engine):
        self.config, self.feed, self.distribution, self.agent, self.engine = config, feed, distribution, agent, engine
        self.state = engine.STATE
        self.store = PlanStore(self.state, engine.trusted)
        self.ops = Operations(config, engine)
        self.probe = Probe(config, engine)

    def run(self):
        import platform as os_platform
        import shutil
        import tomllib
        config, engine = self.config, self.engine
        engine.check_config(config)
        require(config.get('feed_format') == 2, 'Unified preparation requires format 2')
        engine.runtime_configuration(config)
        self.probe.refresh_gate(engine)
        app_raw = root_bytes(Path(config['config']), engine, 1024**2, app_config=True)
        self.app_sha256 = hashlib.sha256(app_raw).hexdigest()
        app = tomllib.loads(app_raw.decode())
        require(app.get('updates', {}).get('root') == str(self.state), 'Managed application update channel is required')
        release_root, incoming, backup_root = [Path(config[name]) for name in ('release_root', 'incoming', 'backup_root')]
        for path in (release_root, incoming, backup_root): engine.trusted(path)
        current = Path(config['current'])
        self.old = current.resolve(strict=True)
        require(current.is_symlink() and self.old.parent == release_root, 'Invalid managed current release')
        old = check_tree(self.old, engine)
        engine.running_binary(config, self.old / 'bin/talocart')
        require(json.loads(self.ops.application(self.old / 'bin/talocart', 'build-info')) == old['build'], 'Current executable provenance mismatch')
        self.original = control(self.state, self.ops.uid, 'inventory', configuration_sha256=self.app_sha256)
        require(not self.original['suspended'], 'A suspended host requires recovery')
        self.unit_material = check_units(config, engine)
        self.ops.application(self.old / 'bin/talocart', 'doctor')
        self.probe.check('/health/ready')
        github = self.agent.connection(config, self.feed)
        release, manifest, manifest_digest = self.distribution.approved(github, config['public_key'], config['repository'])
        self.store.accept_manifest(manifest, manifest_digest, self.distribution)
        bootstrap = private_json(self.state / 'bootstrap.json', engine.trusted, 4096)
        require(bootstrap.get('format') == 1 and isinstance(bootstrap.get('launcher_version'), str)
                and self.feed.newer_or_equal(bootstrap['launcher_version'], manifest['minimum']['launcher']), 'Launcher requires administrative bootstrap')
        target = {'x86_64': self.feed.TARGETS[0], 'aarch64': self.feed.TARGETS[1]}.get(os_platform.machine())
        selected = next((p for p in manifest['platforms'] if p['target'] == target), None)
        require(selected is not None, 'No approved artifacts for this architecture')
        self.manifest, self.platform, self.github, self.release = manifest, selected, github, release
        installed = {m['id'] for m in self.original['modules']}
        needed = [m for m in selected['modules'] if m['id'] in installed]
        artifacts = [selected['base'], selected['updater'], *needed]
        # Bounded extraction plus all compressed artifacts before any service stop.
        require_capacity([(incoming, sum(a['size'] for a in artifacts)), (release_root, 4*1024**3 + sum(m['size'] for m in needed))])
        for artifact in artifacts:
            self.agent.download(github, release, artifact, incoming / artifact['asset'], self.feed)
        updater = selected['updater']
        signature = incoming / updater['signature_asset']
        if signature.exists() or signature.is_symlink():
            engine.trusted(signature)
            require(signature.is_file() and not signature.is_symlink(), 'Invalid updater signature input')
            signature.unlink()
        github.asset(release, updater['signature_asset'], 64, signature)
        require(signature.stat().st_size == 64, 'Invalid updater signature length')
        engine.trusted(Path(config['updater_public_key']))
        engine.run(['openssl', 'pkeyutl', '-verify', '-rawin', '-pubin', '-inkey', config['updater_public_key'],
                    '-in', incoming / updater['asset'], '-sigfile', signature])
        app_key = engine.run(['openssl', 'pkey', '-pubin', '-in', config['public_key'], '-pubout'])
        root_key = engine.run(['openssl', 'pkey', '-pubin', '-in', config['updater_public_key'], '-pubout'])
        require(app_key != root_key, 'Root code and application release keys must be distinct')
        self.needs_engine = tuple(map(int, updater['version'].split('.'))) > tuple(map(int, ENGINE_VERSION.split('.')))
        require(self.needs_engine or self.feed.newer_or_equal(ENGINE_VERSION, manifest['minimum']['engine']), 'Engine minimum cannot be satisfied')
        receipt = private_json(self.state / 'engine-receipts' / (ENGINE_VERSION + '.json'), engine.trusted, 1024)
        require(isinstance(receipt.get('sha256'), str) and re.fullmatch(r'[0-9a-f]{64}', receipt['sha256']), 'Invalid current engine receipt')
        self.active_updater = dict(version=ENGINE_VERSION, sha256=receipt['sha256'])
        if updater['version'] == ENGINE_VERSION:
            require(receipt.get('sha256') == updater['sha256'], 'Same engine version has different provenance')
        base = selected['base']
        self.target = release_root / ('.prepared-' + base['sha256'])
        if self.target.exists() or self.target.is_symlink():
            checked = check_tree(self.target, engine)
        else:
            with tempfile.TemporaryDirectory(prefix='.prepare-', dir=release_root) as temporary:
                staging = Path(temporary) / 'release'
                staging.mkdir(mode=0o755)
                checked = engine.extract(incoming / base['asset'], staging, base['version'], base['commit'], target)
                for directory in [staging, *(p for p in staging.rglob('*') if p.is_dir())]: directory.chmod(0o755)
                staging.rename(self.target)
        require(checked['version'] == base['version'] and checked['source_commit'] == base['commit'] and checked['target'] == target,
                'Prepared base does not match the signed distribution')
        if base['version'] == old['version']:
            require(checked['files'] == old['files'] and checked['build'] == old['build'], 'Same base version has different files')
        self.binary = self.target / 'bin/talocart'
        require(json.loads(self.ops.application(self.binary, 'build-info')) == checked['build'], 'Target executable identity mismatch')
        modules = release_root / ('.modules-' + manifest_digest)
        modules.mkdir(mode=0o750, exist_ok=True)
        engine.trusted(modules)
        os.chown(modules, 0, self.ops.gid)
        modules.chmod(0o750)
        self.packages = []
        for item in needed:
            path = modules / item['asset']
            if path.exists() or path.is_symlink():
                engine.trusted(path)
                require(path.is_file() and not path.is_symlink() and engine.sha256(path) == item['sha256'], 'Prepared module changed')
            else:
                with path.open('xb') as output, (incoming / item['asset']).open('rb') as source:
                    shutil.copyfileobj(source, output)
                    output.flush()
                    os.fsync(output.fileno())
                path.chmod(0o640)
                os.chown(path, 0, self.ops.gid)
            self.packages.append(path)
        native = json.loads(self.ops.application(self.binary, 'package-update-plan', packages=self.packages))
        require(native['inventory_sha256'] == self.original['inventory_sha256'], 'Live/disk inventories disagree')
        self.capacity = json.loads(self.ops.backup_preflight(self.target, self.old / 'bin/talocart'))
        require(self.capacity.get('format') == 1 and type(self.capacity.get('backup_bytes')) is int
                and self.capacity['backup_bytes'] > 0, 'Invalid backup capacity attestation')
        def app_path(section, default):
            path = Path(app.get(section, {}).get('root', default))
            return path if path.is_absolute() else Path(config['config']).parent / path
        package_root = app_path('packages', 'storage/packages')
        require_capacity([(incoming, sum(a['size'] for a in artifacts)), (release_root, 4*1024**3),
                          (backup_root, self.capacity['backup_bytes']),
                          (package_root, sum(m['size'] * 2 for m in needed))])
        configuration = self.store.configuration_fingerprint(dict(agent=config, units=self.unit_material, app_sha256=self.app_sha256,
                    root_key_sha256=hashlib.sha256(root_key.encode()).hexdigest(), probe_token=self.probe.token))
        self.plan, self.identity = create_plan(manifest, manifest_digest, selected, native, configuration,
                    dict(version=old['version'], commit=old['source_commit'], files_sha256=digest(old['files'])))
        require(root_bytes(Path(config['config']), engine, 1024**2, app_config=True) == app_raw, 'Configuration changed during preparation')
        self.store.save(self.plan)
        return self


def marker(path, engine):
    engine.trusted(path.parent)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o644)
    with os.fdopen(fd, 'wb') as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_uid == 0 and info.st_mode & 0o022 == 0, 'Invalid update marker')
        os.fsync(stream.fileno())
    sync_folder(path.parent)


def seal_backup(parent, gid, engine):
    """Pin directories with openat before chown; never follow application-owned links."""
    engine.trusted(parent.parent)
    def seal(fd):
        info = os.fstat(fd)
        if stat.S_ISDIR(info.st_mode):
            os.fchown(fd, 0, gid)
            os.fchmod(fd, 0o750)
            for name in os.listdir(fd):
                child = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW, dir_fd=fd)
                try: seal(child)
                finally: os.close(child)
            os.fsync(fd)
        else:
            require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1, 'Unsafe backup member')
            os.fchown(fd, 0, gid)
            os.fchmod(fd, 0o750 if info.st_mode & 0o111 else 0o640)
            os.fsync(fd)
    fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try: seal(fd)
    finally: os.close(fd)
    sync_folder(parent.parent)


def check_activation(snapshot, plan, enabled):
    require(isinstance(snapshot, dict) and isinstance(snapshot.get('modules'), list)
            and snapshot.get('suspended') is False, 'Invalid offline activation report')
    expected = sorted([dict(id=m['id'], version=m['to'], sha256=m['sha256'], enabled=m['enabled'] if enabled else False)
                       for m in plan['modules']], key=lambda m: m['id'])
    require(snapshot['modules'] == expected, 'Applied module identity/activation disagrees with confirmed plan')


class Execution:
    """Only a freshly revalidated, explicitly confirmed proposal reaches this class."""
    def __init__(self, prepared, progress=None):
        self.p = prepared
        self.progress = progress
        self.config, self.engine, self.state = prepared.config, prepared.engine, prepared.state
        self.binary = prepared.binary
        self.units = [self.config['service'], *self.config['writer_units']]
        self.stopped = False

    def phase(self, name):
        self.journal.intent(name)
        if self.progress is not None: self.progress(name)

    def revoke(self):
        for name in ('application-start', 'writers-start'):
            (self.engine.RUNTIME / name).unlink(missing_ok=True)
        sync_folder(self.engine.RUNTIME)

    def permit(self, name):
        path = self.engine.RUNTIME / name
        atomic(path, dict(attempt=self.attempt), self.engine.trusted)

    def verification_override(self, enable):
        folder = Path('/run/systemd/system') / (self.config['service'] + '.d')
        folder.mkdir(mode=0o755, exist_ok=True)
        self.engine.trusted(folder)
        path = folder / '95-talocart-update-verify.conf'
        if enable:
            args = [str(Path(self.config['current']) / 'bin/talocart'), '--config', self.config['config'], 'serve-verify']
            require(not any(any(c in str(a) for c in '\r\n\0') for a in args), 'Invalid service arguments')
            quoted = ' '.join('"' + str(a).replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%').replace('$', '$$') + '"' for a in args)
            content = '[Service]\nExecStart=\nExecStart=' + quoted + '\n'
            fd, temporary = tempfile.mkstemp(prefix='.verify-', dir=folder)
            try:
                with os.fdopen(fd, 'w') as output:
                    os.fchmod(output.fileno(), 0o600)
                    output.write(content); output.flush(); os.fsync(output.fileno())
                os.replace(temporary, path)
            finally:
                Path(temporary).unlink(missing_ok=True)
        else:
            if path.exists() or path.is_symlink():
                self.engine.trusted(path)
                require(path.is_file() and not path.is_symlink(), 'Invalid verification override')
                path.unlink()
        sync_folder(folder)
        self.engine.run(['systemctl', 'daemon-reload'])

    def stop(self):
        self.revoke()
        self.stopped = True  # A partial systemctl stop is already an uncertain boundary.
        self.engine.run(['systemctl', 'stop', *self.units])
        require(all(self.engine.run(['systemctl', 'show', '-p', 'ActiveState', '--value', u]) in ('inactive', 'failed')
                    and self.engine.run(['systemctl', 'show', '-p', 'MainPID', '--value', u]) == '0' for u in self.units),
                'Writer cgroups did not stop')

    def start_application(self, verification):
        self.permit('application-start')
        self.engine.run(['systemctl', 'start', self.config['service']])
        self.engine.health(self.config, verification=verification)
        self.engine.running_binary(self.config, self.binary, verification=verification)

    def resume_writers(self):
        self.permit('writers-start')
        if self.resume:
            self.engine.run(['systemctl', 'start', *self.resume])
            require(all(self.engine.run(['systemctl', 'show', '-p', 'ActiveState', '--value', u]) == 'active' for u in self.resume),
                    'External writer did not resume')

    def safe_abort(self):
        require(not self.stopped, 'Stopped host requires recovery')
        snapshot = control(self.state, self.p.ops.uid, 'resume', configuration_sha256=self.p.app_sha256)
        require(snapshot['suspended'] is False, 'Quiesce cancellation was not confirmed')
        self.engine.running_binary(self.config, self.p.old / 'bin/talocart')
        self.engine.health(self.config)
        self.p.probe.smoke()
        self.resume_writers()
        self.journal.abort()
        for name in ('maintenance', 'writer-fence', 'processing.json'):
            (self.state / name).unlink(missing_ok=True)
        sync_folder(self.state)
        self.revoke()

    def run(self):
        import secrets
        require(not self.p.needs_engine and not self.p.plan['blocked'], 'Updater and complete plan must be ready')
        require(digest(self.p.plan) == self.p.identity and self.p.store.load(self.p.identity) == self.p.plan, 'Confirmed plan changed')
        require(not any((self.state / name).exists() or (self.state / name).is_symlink()
                        for name in ('processing.json', 'maintenance', 'writer-fence')), 'Previous attempt requires recovery')
        self.attempt = secrets.token_hex(16)
        attempts = self.state / 'attempts'
        attempts.mkdir(mode=0o700, exist_ok=True)
        self.engine.trusted(attempts)
        self.resume = [u for u in self.config['writer_units'] if self.engine.run(['systemctl', 'show', '-p', 'ActiveState', '--value', u]) == 'active']
        backup_parent = Path(self.config['backup_root']) / self.attempt
        self.backup = backup_parent / 'data'
        self.revoke()
        # The independent launcher must see the attempt even if the engine dies
        # immediately after creating its first journal, before any maintenance.
        atomic(self.state / 'processing.json', dict(format=1, attempt=self.attempt, plan_sha256=self.p.identity), self.engine.trusted)
        self.journal = Journal.begin(attempts / (self.attempt + '.json'), self.engine.trusted,
            self.p.identity, self.attempt, dict(original=self.p.original, previous=str(self.p.old), backup=str(self.backup), resume_units=self.resume))
        try:
            self.phase('updater_ready')
            self.journal.complete('updater_ready', dict(version=ENGINE_VERSION))
            self.phase('quiescing')
            marker(self.state / 'maintenance', self.engine)
            marker(self.state / 'writer-fence', self.engine)
            self.p.probe.closed()
            self.p.probe.check('/health/ready')
            if self.config['writer_units']:
                self.engine.run(['systemctl', 'stop', *self.config['writer_units']])
            quiesced = control(self.state, self.p.ops.uid, 'quiesce', self.p.plan['inventory_sha256'], configuration_sha256=self.p.app_sha256)
            self.journal.complete('quiescing', quiesced)
            self.phase('fenced')
            self.stop()
            self.journal.complete('fenced')
            self.phase('backed_up')
            capacity = json.loads(self.p.ops.backup_preflight(self.p.target, self.p.old / 'bin/talocart'))
            require(type(capacity.get('backup_bytes')) is int and capacity['backup_bytes'] > 0, 'Invalid offline capacity check')
            require_capacity([(Path(self.config['backup_root']), capacity['backup_bytes'])])
            backup_parent.mkdir(mode=0o700)
            os.chown(backup_parent, self.p.ops.uid, self.p.ops.gid)
            require(self.p.ops.backup(self.p.old, self.backup) == 'OK', 'Backup did not complete')
            require(self.p.ops.backup(self.p.old, self.backup, verify=True) == 'OK', 'Backup integrity failed')
            seal_backup(backup_parent, self.p.ops.gid, self.engine)
            require(self.p.ops.backup(self.p.old, self.backup, verify=True) == 'OK', 'Sealed backup integrity failed')
            require(hashlib.sha256(root_bytes(self.backup / 'talocart.toml', self.engine)).hexdigest() == self.p.app_sha256,
                    'Backup configuration changed')
            self.journal.complete('backed_up', dict(id=self.attempt, manifest_sha256=self.engine.sha256(self.backup / 'backup.json')))
            self.phase('applying')
            original = json.loads(self.p.ops.application(self.binary, 'package-update-suspend', inventory=self.p.plan['inventory_sha256'], snapshot=quiesced))
            require(original['modules'] == self.p.original['modules'] and original['inventory_sha256'] == self.p.plan['inventory_sha256'], 'Original activation changed')
            self.p.ops.application(self.binary, 'package-update-migrate')
            self.p.ops.application(self.binary, 'package-update-apply', packages=self.p.packages)
            check_activation(json.loads(self.p.ops.application(self.binary, 'package-update-verify')), self.p.plan, False)
            destination = Path(self.config['release_root']) / self.p.plan['base']['version']
            if destination != self.p.old:
                if destination.exists() or destination.is_symlink():
                    require(check_tree(destination, self.engine) == check_tree(self.p.target, self.engine), 'Existing release directory differs')
                else:
                    self.p.target.rename(destination)
                    sync_folder(destination.parent)
                pending = Path(self.config['current']).with_name('.current-' + self.attempt)
                pending.symlink_to(destination)
                os.replace(pending, self.config['current'])
                sync_folder(destination.parent.parent)
            self.binary = destination / 'bin/talocart'
            self.journal.complete('applying')
            self.phase('verifying')
            self.p.ops.application(self.binary, 'doctor')
            self.verification_override(True)
            self.start_application(True)
            self.p.probe.closed()
            self.p.probe.smoke(True)
            self.stop()
            self.verification_override(False)
            restored = json.loads(self.p.ops.application(self.binary, 'package-update-restore', snapshot=original))
            check_activation(restored, self.p.plan, True)
            self.journal.complete('verifying', dict(inventory_sha256=restored['inventory_sha256']))
            self.phase('resuming')
            self.start_application(False)
            warmed = control(self.state, self.p.ops.uid, 'warm', restored['inventory_sha256'], configuration_sha256=self.p.app_sha256)
            check_activation(warmed, self.p.plan, True)
            self.p.probe.smoke()
            self.resume_writers()
            self.p.probe.closed()
            self.journal.complete('resuming')
            self.phase('succeeded')
            (self.state / 'maintenance').unlink()
            (self.state / 'writer-fence').unlink()
            sync_folder(self.state)
            status, body, _ = self.p.probe._request('/health/ready', private=False)
            require(status == 200 and body == b'ready\n', 'Public activation failed')
            self.journal.complete('succeeded')
            result = dict(format=2, release=self.p.plan['release'], base=self.p.plan['base'], modules=restored['modules'],
                          updater=self.p.active_updater, plan_sha256=self.p.identity, backup_id=self.attempt)
            atomic(self.state / 'deployed.json', result, self.engine.trusted)
            # Status is published by the agent, then it removes the last processing
            # marker. A failure before that acknowledgement remains recoverable.
            return result
        except BaseException:
            if not self.stopped and self.journal.value['phase'] in ('prepared', 'updater_ready'):
                try:
                    self.safe_abort()
                except BaseException:
                    self.engine.recover(self.config)
            else:
                self.engine.recover(self.config)
            raise


def public_plan(plan):
    return dict(release=plan['release']['id'], sequence=plan['release']['sequence'],
                base_from=plan['current_base']['version'], base_to=plan['base']['version'],
                base_action='keep' if plan['current_base']['version'] == plan['base']['version'] else 'update', blocked=plan['blocked'],
                modules=[{k:m[k] for k in ('id','from','to','enabled','action','blocker')} for m in plan['modules']])


def candidate(plan, identity):
    return dict(version=plan['release']['id'][1:], commit=plan['release']['commit'], digest=identity)


def publish_status(config, engine, phase, plan=None, identity=None, *, stage=None, recovery=False, backup=None, notice=None, engine_blocked=False):
    import datetime
    import pwd
    import time
    projection = public_plan(plan) if plan is not None else None
    if engine_blocked:
        require(projection is not None and phase == 'blocked' and identity is None, 'Invalid rejected-engine projection')
        projection['blocked'] = True
    value = dict(format=2, phase=phase, checked_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                 checked_epoch=int(time.time()), candidate=candidate(plan, identity) if plan is not None and identity is not None else None,
                 plan=projection, stage=stage,
                 recovery_required=recovery, backup_id=backup, notice=notice)
    data = canonical(value)
    require(len(data) <= 65536, 'Public update status exceeds limit')
    engine.trusted(engine.STATE)
    fd, name = tempfile.mkstemp(prefix='.status-', dir=engine.STATE)
    try:
        with os.fdopen(fd, 'wb') as stream:
            os.fchmod(stream.fileno(), 0o640)
            os.fchown(stream.fileno(), 0, pwd.getpwnam(config['service_user']).pw_gid)
            stream.write(data); stream.flush(); os.fsync(stream.fileno())
        os.replace(name, engine.STATE / 'status.json')
        sync_folder(engine.STATE)
    finally:
        Path(name).unlink(missing_ok=True)


def read_request(path, uid):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_uid == uid and info.st_mode & 0o077 == 0 and info.st_size <= 4096,
                'Invalid install request ownership or size')
        raw = stream.read(4097)
    require(len(raw) <= 4096, 'Install request exceeds limit')
    value = json.loads(raw)
    require(isinstance(value, dict) and set(value) == {'version','commit','digest'}
            and isinstance(value['version'], str) and re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', value['version'])
            and all(isinstance(value[k], str) and re.fullmatch(r'[0-9a-f]{'+str(n)+'}', value[k]) for k,n in [('commit',40),('digest',64)]),
            'Invalid confirmed proposal')
    return value


def recent_status(state, engine, window=3600):
    import time
    try:
        value = json.loads(root_bytes(state / 'status.json', engine, 65536))
    except (OSError, ValueError):
        return False
    if not isinstance(value, dict) or value.get('format') != 2:
        return False
    if value.get('phase') == 'failed' and value.get('candidate') is None:
        return False  # A failed request without a candidate must not hide the next fresh proposal.
    epoch = value.get('checked_epoch')
    return (type(epoch) is int and 0 <= time.time() - epoch < window
            and value.get('phase') in ('current','available','blocked','succeeded','failed','check_failed'))


def consume_check(state):
    """Claim without reading/following content, outside the systemd path watch."""
    try:
        os.replace(state / 'requests/check.json', state / 'pending-check.json')
    except FileNotFoundError:
        return (state / 'pending-check.json').exists() or (state / 'pending-check.json').is_symlink()
    sync_folder(state / 'requests')
    sync_folder(state)
    return True


def cycle(config, feed, distribution, agent, engine):
    """One host lock is held by the agent; a web request names only a proposal."""
    import pwd
    state = engine.STATE
    store = PlanStore(state, engine.trusted)
    request, working = state / 'requests/install.json', state / 'preparing-request.json'
    p, wanted, previous, run, retain = None, None, None, None, False
    try:
        # Claim first so recovery or throttling cannot leave the path unit retriggering.
        forced = consume_check(state)
        recovery = any((state / name).exists() or (state / name).is_symlink() for name in ('processing.json','maintenance','writer-fence'))
        if recovery:
            (state / 'pending-check.json').unlink(missing_ok=True)
            sync_folder(state)
        require(not recovery,
                'Previous update requires recovery')
        # A manual check skips the hourly cache but never queries more than once a minute.
        if not any(path.exists() or path.is_symlink() for path in (request, working, state / 'engine-handoff-failed.json')) and recent_status(state, engine, 60 if forced else 3600):
            return 0
        (state / 'pending-check.json').unlink(missing_ok=True)
        sync_folder(state)
        if not working.exists() and not working.is_symlink() and (request.exists() or request.is_symlink()):
            os.rename(request, working)
            sync_folder(state / 'requests'); sync_folder(state)
        if working.exists() or working.is_symlink():
            wanted = read_request(working, pwd.getpwnam(config['service_user']).pw_uid)
            previous = store.load(wanted['digest'])
            require(wanted == candidate(previous, wanted['digest']), 'Request does not match stored proposal')
            publish_status(config, engine, 'installing', previous, wanted['digest'], stage='prepared')
        failure = state / 'engine-handoff-failed.json'
        if failure.exists() or failure.is_symlink():
            failed = private_json(failure, engine.trusted, 4096)
            updater = failed.get('updater')
            require(isinstance(updater, dict) and isinstance(updater.get('sha256'), str)
                    and re.fullmatch(r'[0-9a-f]{64}', updater['sha256']), 'Invalid failed engine identity')
            atomic(state / 'engine-rejected.json', dict(format=1, sha256=updater['sha256']), engine.trusted)
            if wanted is not None and failed.get('plan_sha256') == wanted['digest']:
                working.unlink(); wanted = None
            failure.unlink(); sync_folder(state)
        p = Preparation(config, feed, distribution, agent, engine).run()
        rejected = state / 'engine-rejected.json'
        if p.needs_engine and (rejected.exists() or rejected.is_symlink()):
            value = private_json(rejected, engine.trusted, 1024)
            require(set(value) == {'format','sha256'} and type(value['format']) is int and value['format'] == 1
                    and isinstance(value['sha256'], str) and re.fullmatch(r'[0-9a-f]{64}',value['sha256']), 'Invalid engine rejection record')
            if value['sha256'] == p.platform['updater']['sha256']:
                publish_status(config, engine, 'blocked', p.plan, notice='engine_failed', engine_blocked=True)
                return 0
        no_change = not p.needs_engine and p.plan['base']['version'] == p.plan['current_base']['version'] and all(m['action'] == 'keep' for m in p.plan['modules'])
        if wanted == candidate(p.plan, p.identity) and no_change and not p.plan['blocked']:
            # A root-engine-only handoff has finished; no application backup or SQL
            # is needed when every application component was freshly verified unchanged.
            atomic(state / 'deployed.json', dict(format=2, release=p.plan['release'], base=p.plan['base'],
                   modules=p.original['modules'], updater=p.active_updater, plan_sha256=p.identity, backup_id=None), engine.trusted)
            publish_status(config, engine, 'succeeded', p.plan)
            return 0
        if wanted is None or wanted != candidate(p.plan, p.identity) or p.plan['blocked'] or no_change:
            publish_status(config, engine, 'blocked' if p.plan['blocked'] else 'current' if no_change else 'available',
                           p.plan, None if no_change else p.identity, notice='plan_changed' if wanted is not None else None)
            return 0
        if p.needs_engine:
            atomic(state / 'engine-handoff.json', dict(format=1, updater=p.platform['updater'], plan_sha256=p.identity,
                    minimum_launcher=p.manifest['minimum']['launcher']), engine.trusted)
            retain = True
            return 75
        run = Execution(p, lambda stage: publish_status(config, engine, 'installing', p.plan, p.identity, stage=stage))
        result = run.run()
        publish_status(config, engine, 'succeeded', p.plan, backup=result['backup_id'])
        (state / 'processing.json').unlink()
        sync_folder(state)
        run.revoke()
        return 0
    except BaseException as error:
        recovery = any((state / name).exists() or (state / name).is_symlink() for name in ('processing.json','maintenance','writer-fence'))
        if recovery:
            engine.recover(config)
        plan = p.plan if p is not None else previous
        identity = p.identity if p is not None else wanted['digest'] if previous is not None else None
        verified_backup = run.journal.value['results'].get('backed_up') if run is not None and hasattr(run, 'journal') else None
        publish_status(config, engine, 'failed' if wanted is not None else 'check_failed', plan, identity,
                       recovery=recovery, backup=verified_backup['id'] if verified_backup else None, notice='control_'+error.code if isinstance(error, ControlRefusal) else 'preparation_failed')
        raise
    finally:
        if not retain:
            working.unlink(missing_ok=True)
            sync_folder(state)
