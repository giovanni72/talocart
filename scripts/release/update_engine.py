#!/usr/bin/python3 -I
"""Root-owned Linux update engine, called only by the installed pull agent.

Configuration is supplied by the root-owned agent. Failure after fencing leaves
maintenance enabled. No automatic database rollback or deletion of prior state.
"""
import hashlib
import fcntl
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import tarfile
import time
import tomllib
import urllib.request
from urllib.parse import urlsplit

STATE = Path('/var/lib/talocart-update')
RUNTIME = Path('/run/talocart-update')
VERSION = r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'


class DeployError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise DeployError(message)


def sha256(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def run(argv, timeout=120):
    try:
        result = subprocess.run(argv, capture_output=True, timeout=timeout, check=False,
                                env={'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C.UTF-8'})
    except (OSError, subprocess.TimeoutExpired) as error:
        diagnostics(getattr(error, 'stdout', b'') or b'', getattr(error, 'stderr', b'') or b'')
        raise DeployError('Command unavailable or timed out; private diagnostics omitted') from error
    if result.returncode != 0:
        diagnostics(result.stdout, result.stderr)
    require(result.returncode == 0, 'Deployment command failed; inspect private server logs')
    return result.stdout.decode().strip()


def diagnostics(stdout, stderr):
    # CLI diagnostics may contain secrets: retain only in root-private server
    # storage, never echo them to the SSH client/Actions or public journal.
    path = STATE / 'failure.log'
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as log:
        os.fchmod(log.fileno(), 0o600)
        log.write((stdout + b'\n' + stderr)[-256 * 1024:])


def trusted(path):
    for item in [path, *path.parents]:
        info = item.lstat()
        require(not stat.S_ISLNK(info.st_mode) and info.st_uid == 0
                and info.st_mode & 0o022 == 0, 'Deployment paths must be root-owned and not writable by others')


def extract(archive, destination, version, commit, target):
    """Extract only regular files/directories under exactly one expected root."""
    prefix = f'talocart-{version}-{target}'
    with tarfile.open(archive, 'r:gz') as bundle:
        members, total = [], 0
        for member in bundle:
            members.append(member)
            total += member.size
            require(len(members) <= 50000 and total <= 4 * 1024**3,
                    'Archive exceeds deployment limits')
        names = set()
        for member in members:
            path = PurePosixPath(member.name)
            require(not path.is_absolute() and '..' not in path.parts and path.parts
                    and path.parts[0] == prefix and (member.isfile() or member.isdir())
                    and member.name not in names, 'Unsafe archive member')
            names.add(member.name)
        # Validation above rejects links, devices, traversal and duplicate names.
        for member in members:
            relative = PurePosixPath(member.name).relative_to(prefix)
            output = destination.joinpath(*relative.parts)
            if member.isdir():
                output.mkdir(parents=True, exist_ok=True)
                output.chmod(0o755)
            else:
                output.parent.mkdir(parents=True, exist_ok=True)
                with bundle.extractfile(member) as source, output.open('xb') as sink:
                    shutil.copyfileobj(source, sink)
                output.chmod(0o755 if str(relative) == 'bin/talocart' else 0o644)
    require((destination / 'release.json').stat().st_size <= 8 * 1024**2, 'Release metadata exceeds limit')
    manifest = json.loads((destination / 'release.json').read_text())
    build = manifest.get('build', {})
    require(manifest.get('format') == 1 and manifest.get('profile') == 'base'
            and manifest.get('version') == version and manifest.get('source_commit') == commit
            and manifest.get('target') == target and manifest.get('source_dirty') is False
            and build.get('version') == version and build.get('commit') == commit
            and build.get('source_dirty') is False and build.get('tagged') is True,
            'Release identity is not a clean, tagged base build')
    actual = {p.relative_to(destination).as_posix(): sha256(p)
              for p in destination.rglob('*') if p.is_file() and p != destination / 'release.json'}
    require(actual == manifest.get('files'), 'Release manifest checksum mismatch')
    require({'bin/talocart', 'scripts/release/operations.py'} <= actual.keys(), 'Incomplete release')
    return manifest


def check_config(config):
    require(type(config.get('feed_format')) is int and config['feed_format'] == 2,
            'Invalid updater feed format')
    required = {'service_user', 'service', 'release_root', 'current', 'config', 'backup_root',
                'incoming', 'public_origin', 'ready_url', 'writer_units', 'writers_fenced', 'environment'}
    require(required <= config.keys(), 'Incomplete deployment configuration')
    require(config['writers_fenced'] is True, 'External writers must be fenced before enabling deployment')
    require('bootstrap_updates' not in config, 'Retired bootstrap_updates option')
    require(config['environment'] in ('staging', 'production'), 'Invalid deployment environment')
    require(re.fullmatch(r'[a-z_][a-z0-9_-]*', config['service_user']) is not None
            and config['service_user'] != 'root', 'Invalid service account')
    units = [config['service'], *config['writer_units']]
    require(len(units) <= 32 and len(units) == len(set(units)) and all(re.fullmatch(r'[A-Za-z0-9_-]+\.service', u) for u in units),
            'Invalid or duplicate service units')
    require('talocart-update.service' not in units, 'The updater cannot fence its own service')
    for name in ('release_root', 'current', 'config', 'backup_root', 'incoming'):
        path = Path(config[name])
        require(path.is_absolute() and '..' not in path.parts, 'Deployment paths must be absolute')
    require(Path(config['current']).parent == Path(config['release_root']).parent,
            'current must be beside release_root')
    origin, ready = urlsplit(config['public_origin']), urlsplit(config['ready_url'])
    require(origin.scheme == 'https' and origin.hostname and not origin.username
            and not origin.password and origin.path in ('', '/') and not origin.query and not origin.fragment,
            'An exact HTTPS public origin is required')
    require(ready.scheme == 'http' and ready.hostname == '127.0.0.1' and not ready.username
            and not ready.password and ready.path == '/health/ready' and not ready.query and not ready.fragment,
            'Readiness must use the loopback health endpoint')


def as_user(config, argv, timeout=120):
    return run(['runuser', '-u', config['service_user'], '--', *map(str, argv)], timeout)


def runtime_configuration(config):
    app = tomllib.loads(Path(config['config']).read_text())
    settings = app.get('server', {})
    require(settings.get('listen') == urlsplit(config['ready_url']).netloc
            and settings.get('public_origin', '').rstrip('/') == config['public_origin'].rstrip('/')
            and settings.get('secure_cookies') is True,
            'Deployment health/origin must match the complete HTTPS runtime configuration')


def health(config, verification=False):
    request = urllib.request.Request(config['ready_url'] + '?all_modules=true', headers={'Host': urlsplit(config['public_origin']).netloc})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    for _ in range(30):
        try:
            with opener.open(request, timeout=2) as response:
                if response.status == 200 and response.read(64) == (b'verification-ready\n' if verification else b'ready\n'):
                    return
        except OSError:
            pass
        time.sleep(1)
    raise DeployError('Readiness failed')


def running_binary(config, binary, verification=False):
    pid = run(['systemctl', 'show', '-p', 'MainPID', '--value', config['service']])
    require(pid.isdigit() and pid != '0', 'Service has no running main process')
    process = Path('/proc') / pid
    require(sha256(process / 'exe') == sha256(binary), 'Running process does not match the selected executable')
    require((process / 'cmdline').read_bytes().split(b'\0')[:-1] ==
            [os.fsencode(str(Path(config['current']) / 'bin/talocart')), b'--config', os.fsencode(config['config']), b'serve-verify' if verification else b'serve'],
            'Running service does not use the managed executable/configuration')
    require(not any((item.startswith(b'TALOCART_') and not item.startswith(b'TALOCART_LOG=')) or item.startswith(b'PG')
                    for item in (process / 'environ').read_bytes().split(b'\0')),
            'Runtime configuration overrides are unsupported')


def recover(config):
    """Fail closed after exceptions, SIGKILL or reboot; never roll back data."""
    (STATE / 'maintenance').touch(mode=0o644)
    (STATE / 'writer-fence').touch(mode=0o644)
    (RUNTIME / 'application-start').unlink(missing_ok=True)
    (RUNTIME / 'writers-start').unlink(missing_ok=True)
    units = [config['service'], *config['writer_units']]
    run(['systemctl', 'stop', *units])


def application_start_allowed():
    """ExecCondition: a persistent fence permits only an updater-held activation.

    /run is cleared at boot. The agent lock also expires on SIGKILL, so a stale
    permit cannot authorize automatic restarts after the updater has died.
    This predicate is read-only and runs before the application, as root.
    """
    if not (STATE / 'writer-fence').exists():
        return True
    if not (RUNTIME / 'application-start').is_file():
        return False
    try:
        with (STATE / 'agent.lock').open('rb') as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return True
    except OSError:
        pass
    return False


if __name__ == '__main__':
    import sys
    # The installed root-owned predicate never executes deployment from argv.
    if sys.argv[1:] != ['--application-start-allowed']:
        sys.exit(1)
    sys.exit(0 if application_start_allowed() else 1)
