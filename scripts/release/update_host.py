"""Fixed unprivileged CLI execution and bounded private host I/O."""
import json
import os
from pathlib import Path
import re
import stat

from update_plan import canonical, require
from update_state import atomic

class Operations:
    """Fixed application CLI as the service account, contained in transient cgroups.

    Root executes OS orchestration only. Even signed application backup scripts
    run unprivileged; killing the updater cannot detach a database-writing CLI.
    """
    def __init__(self, config, engine):
        import pwd
        self.config, self.engine, self.state = config, engine, engine.STATE
        account = pwd.getpwnam(config['service_user'])
        require(account.pw_uid > 0 and account.pw_gid > 0, 'Application requires an unprivileged account')
        self.uid, self.gid = account.pw_uid, account.pw_gid

    def application(self, binary, command, *, packages=(), inventory=None, snapshot=None):
        readonly = {'build-info', 'package-update-plan', 'doctor'}
        offline = {'package-update-suspend', 'package-update-migrate', 'package-update-apply',
                   'package-update-verify', 'package-update-restore'}
        require(command in readonly | offline, 'Invalid application update command')
        self.engine.trusted(binary)
        argv = [str(binary)]
        if command != 'build-info':
            argv += ['--config', self.config['config']]
        argv.append(command)
        require(not packages or command in ('package-update-plan', 'package-update-apply'), 'Unexpected update packages')
        require(len(packages) <= 32, 'Too many update packages')
        for path in packages:
            self.engine.trusted(path)
            require(path.is_absolute(), 'Update artifact path must be absolute')
            argv += ['--package', str(path)]
        if command == 'package-update-plan':
            argv += ['--check-database']
        require((command == 'package-update-suspend') == (inventory is not None), 'Invalid inventory argument')
        if inventory is not None:
            require(isinstance(inventory, str) and re.fullmatch(r'[0-9a-f]{64}', inventory), 'Invalid inventory argument')
            argv += ['--inventory-sha256', inventory]
        require(snapshot is None or command in ('package-update-suspend', 'package-update-restore'), 'Unexpected activation snapshot')
        require(command != 'package-update-restore' or snapshot is not None, 'Missing activation snapshot')
        if command == 'package-update-suspend' and snapshot is not None:
            argv += ['--quiesced']
        payload = canonical(snapshot) if snapshot is not None else None
        require(payload is None or len(payload) <= 16384, 'Activation snapshot exceeds limit')
        return self._run(argv, 600 if command in readonly else 3900, command in offline, payload, accept_blocked_plan=command == 'package-update-plan')

    def backup_preflight(self, release, binary):
        script = release / 'scripts/release/operations.py'
        self.engine.trusted(script)
        return self._run(['/usr/bin/python3', '-I', str(script), 'preflight', '--config', self.config['config'],
                          '--binary', str(binary)], 600, False)

    def backup(self, old, output, verify=False):
        script = old / 'scripts/release/operations.py'
        self.engine.trusted(script)
        argv = ['/usr/bin/python3', '-I', str(script)]
        if verify:
            argv += ['verify', str(output)]
        else:
            argv += ['backup', '--config', self.config['config'], '--binary', str(old / 'bin/talocart'), '--output', str(output)]
        return self._run(argv, 3900, True)

    def _run(self, argv, timeout, mutating, payload=None, *, accept_blocked_plan=False):
        import secrets
        import selectors
        import subprocess
        import time
        path = self.state / 'operation.json'
        require(not path.exists() and not path.is_symlink(), 'An application operation requires recovery')
        if mutating:
            require(all((self.state / name).is_file() for name in ('processing.json', 'maintenance', 'writer-fence')),
                    'Application mutations require the durable attempt and writer fence')
            require(not any((self.engine.RUNTIME / name).exists() or (self.engine.RUNTIME / name).is_symlink()
                            for name in ('application-start', 'writers-start')), 'Revoke activation permits before offline commands')
            require(all(self.engine.run(['systemctl', 'show', '-p', 'ActiveState', '--value', unit]) in ('inactive', 'failed')
                        for unit in [self.config['service'], *self.config['writer_units']]), 'Writers must stop before offline commands')
        unit = 'talocart-update-op-' + secrets.token_hex(16) + '.service'
        atomic(path, dict(format=1, unit=unit, mutating=mutating), self.engine.trusted)
        process, stopped = None, False
        output, errors = bytearray(), bytearray()
        try:
            process = subprocess.Popen([
                'systemd-run', '--quiet', '--wait', '--pipe', '--collect', '--unit=' + unit,
                '--uid=' + str(self.uid), '--gid=' + str(self.gid),
                '--property=Type=exec', '--property=KillMode=control-group',
                '--property=TimeoutStopSec=30s', '--property=RuntimeMaxSec=' + str(timeout) + 's',
                '--property=NoNewPrivileges=yes', '--', '/usr/bin/env', '-i',
                'PATH=/usr/sbin:/usr/bin:/sbin:/bin', 'LANG=C.UTF-8', *map(str, argv)],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                env={'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C.UTF-8'})
            deadline, cursor = time.monotonic() + timeout + 45, 0
            payload = payload or b''
            with selectors.DefaultSelector() as selector:
                for stream in (process.stdout, process.stderr):
                    os.set_blocking(stream.fileno(), False)
                    selector.register(stream, selectors.EVENT_READ)
                if payload:
                    os.set_blocking(process.stdin.fileno(), False)
                    selector.register(process.stdin, selectors.EVENT_WRITE)
                else:
                    process.stdin.close()
                while selector.get_map():
                    require(time.monotonic() < deadline, 'Application operation timed out')
                    for key, _ in selector.select(0.5):
                        stream = key.fileobj
                        if stream is process.stdin:
                            cursor += os.write(stream.fileno(), payload[cursor:])
                            if cursor == len(payload):
                                selector.unregister(stream)
                                stream.close()
                            continue
                        chunk = os.read(stream.fileno(), 65536)
                        if not chunk:
                            selector.unregister(stream)
                            stream.close()
                            continue
                        buffer, limit = (output, 1048576) if stream is process.stdout else (errors, 262144)
                        require(len(buffer) + len(chunk) <= limit, 'Application output exceeds limit')
                        buffer.extend(chunk)
            code = process.wait(timeout=5)
            blocked = False
            if accept_blocked_plan and code == 1:
                report = json.loads(output)
                blocked = isinstance(report, dict) and report.get('format') == 1 and isinstance(report.get('modules'), list) and any(isinstance(m, dict) and m.get('action') == 'blocked' for m in report['modules'])
            require(code == 0 or blocked, 'Application update command failed')
            stopped = True  # --wait/--collect waits for service and cgroup cleanup.
            return bytes(output).decode().strip()
        except BaseException:
            try:
                self.engine.diagnostics(bytes(output)[-262144:], bytes(errors)[-262144:])
            except Exception:
                pass  # A full/logging disk must never prevent cgroup cleanup.
            try:
                loaded = self.engine.run(['systemctl', 'show', '-p', 'LoadState', '--value', unit])
                if loaded != 'not-found':
                    try:
                        self.engine.run(['systemctl', 'stop', unit], 45)
                    except Exception:
                        # --collect can remove a completed unit after the first
                        # read. Only a fresh positive absence permits cleanup.
                        require(self.engine.run(['systemctl', 'show', '-p', 'LoadState', '--value', unit]) == 'not-found',
                                'Application operation stop was not confirmed')
                stopped = True
            except Exception:
                pass  # Keep the operation marker so the independent launcher stops it.
            raise
        finally:
            if process is not None and process.poll() is None:
                process.kill()
                process.wait(timeout=10)
            if stopped:
                path.unlink()
                directory = os.open(self.state, os.O_RDONLY | os.O_DIRECTORY)
                try: os.fsync(directory)
                finally: os.close(directory)


def root_bytes(path, engine, limit=8*1024**2, app_config=False):
    engine.trusted(path.parent if app_config else path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= limit and info.st_mode & 0o022 == 0,
                'Invalid immutable update input')
        value = stream.read(limit + 1)
    require(len(value) <= limit, 'Update input exceeds limit')
    return value


def sync_folder(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)
