#!/usr/bin/python3 -I
"""Small installed trust/recovery boundary for versioned updater engines (Linux/root).

No application artifact or module publisher authorizes root code. Install this file
and its dedicated public key administratively; only engine bundles self-update.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib

VERSION = '1.0.0'
STATE_FORMAT = 1
CONFIG = Path('/etc/talocart/update.toml')
STATE = Path('/var/lib/talocart-update')
RUNTIME = Path('/run/talocart-update')
BUNDLES = Path('/usr/local/libexec/talocart/engines')
CURRENT = Path('/usr/local/libexec/talocart/engine-current')
# Deliberately excludes launcher, keys, units and arbitrary executable hooks.
FILES = frozenset({'update_agent.py', 'update_engine.py', 'updates_feed.py',
                   'distribution.py', 'update_coordinator.py'})
MAX_BUNDLE = 8 * 1024**2
HANDOFF_EXIT = 75
SEMVER = r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'


def require(value, message):
    if not value:
        raise ValueError(message)


def trusted(path):
    for item in [path, *path.parents]:
        info = item.lstat()
        require(not stat.S_ISLNK(info.st_mode) and info.st_uid == 0
                and info.st_mode & 0o022 == 0, 'Untrusted updater path')


def read(path, limit):
    trusted(path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as source:
        info = os.fstat(source.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= limit, 'Invalid updater input')
        data = source.read(limit + 1)
    require(len(data) <= limit, 'Updater input exceeds limit')
    return data


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic_json(path, value, group=None):
    trusted(path.parent)
    # Temporary created exclusively, never follows an application-owned link.
    fd, temporary = tempfile.mkstemp(prefix='.' + path.name + '-', dir=path.parent)
    temporary = Path(temporary)
    try:
        with os.fdopen(fd, 'w') as stream:
            os.fchmod(stream.fileno(), 0o600 if group is None else 0o640)
            if group is not None: os.fchown(stream.fileno(), 0, group)
            json.dump(value, stream, sort_keys=True, separators=(',', ':'))
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        temporary.unlink(missing_ok=True)


def command(argv, timeout=120):
    result = subprocess.run(list(map(str, argv)), capture_output=True, timeout=timeout, check=False,
                            env={'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C.UTF-8'})
    require(result.returncode == 0, 'Privileged updater command failed')
    return result.stdout


def unique(pairs):
    result = {}
    for name, value in pairs:
        require(name not in result, 'Duplicate updater field')
        result[name] = value
    return result


def manifest(raw, expected_version):
    value = json.loads(raw, object_pairs_hook=unique)
    require(isinstance(value, dict) and set(value) == {'format', 'version', 'state_format', 'files'}
            and type(value['format']) is int and value['format'] == 1
            and type(value['state_format']) is int and value['state_format'] == STATE_FORMAT
            and value['version'] == expected_version, 'Unsupported updater bundle')
    require(isinstance(value['files'], dict) and set(value['files']) == FILES
            and all(isinstance(v, str) and re.fullmatch(r'[0-9a-f]{64}', v)
                    for v in value['files'].values()), 'Invalid updater file list')
    return value


def inspect(folder, expected_version):
    """Validate root-owned published files again before every engine execution."""
    trusted(folder)
    value = manifest(read(folder / 'bundle.json', 8192), expected_version)
    require({p.name for p in folder.iterdir()} == FILES | {'bundle.json'}, 'Unexpected updater files')
    for name, digest in value['files'].items():
        require(hashlib.sha256(read(folder / name, MAX_BUNDLE)).hexdigest() == digest,
                'Installed updater changed')
    return value


def selected():
    trusted(CURRENT.parent)
    require(CURRENT.is_symlink(), 'Updater engine has not been bootstrapped')
    path = CURRENT.resolve(strict=True)
    require(path.parent == BUNDLES and re.fullmatch(SEMVER, path.name), 'Invalid engine selection')
    inspect(path, path.name)
    return path


def incomplete():
    # Durable stop/recovery markers block engine replacement during an operation.
    if any((STATE / name).exists() or (STATE / name).is_symlink()
           for name in ('processing.json', 'maintenance', 'writer-fence')):
        return True
    try:
        value = operation()
        return value is not None and value['mutating']
    except (OSError, ValueError):
        return True


def operation():
    path = STATE / 'operation.json'
    if not path.exists() and not path.is_symlink():
        return None
    value = json.loads(read(path, 1024), object_pairs_hook=unique)
    require(path.stat().st_mode & 0o077 == 0 and isinstance(value, dict)
            and set(value) == {'format', 'unit', 'mutating'} and type(value['format']) is int
            and value['format'] == 1 and type(value['mutating']) is bool
            and isinstance(value['unit'], str)
            and re.fullmatch(r'talocart-update-op-[0-9a-f]{32}\.service', value['unit']),
            'Invalid application operation marker')
    return value


def reap_operation():
    value = operation()
    if value is None:
        return
    unit = value['unit']
    loaded = command(['systemctl', 'show', '-p', 'LoadState', '--value', unit]).strip()
    if loaded != b'not-found':
        try:
            command(['systemctl', 'stop', unit])
            active = command(['systemctl', 'show', '-p', 'ActiveState', '--value', unit]).strip()
            require(active in (b'inactive', b'failed'), 'Application operation did not stop')
        except Exception:
            # --collect may remove the completed unit after either read. Keep
            # recovery independent from the engine and require positive absence.
            require(command(['systemctl', 'show', '-p', 'LoadState', '--value', unit]).strip() == b'not-found',
                    'Application operation stop was not confirmed')
    (STATE / 'operation.json').unlink()
    sync_directory(STATE)


def lock_alive():
    try:
        fd = os.open(STATE / 'agent.lock', os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, 'rb') as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
                return False
            try:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return True
    except OSError:
        pass
    return False


def application_start_allowed():
    if not incomplete():
        return True
    try:
        read(RUNTIME / 'application-start', 64)
        return lock_alive()
    except (OSError, ValueError):
        return False


def writer_start_allowed():
    if not incomplete():
        return True
    try:
        read(RUNTIME / 'writers-start', 64)
        return lock_alive()
    except (OSError, ValueError):
        return False


def marker(name):
    path = STATE / name
    trusted(STATE)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o644)
    with os.fdopen(fd, 'wb') as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_uid == 0 and info.st_mode & 0o022 == 0,
                'Invalid recovery marker')
        os.fsync(stream.fileno())
    sync_directory(STATE)


def recovery_metadata():
    """Recover only bounded public identifiers from independent trusted journals.

    A previous status cannot prove that this attempt has a verified backup.
    Never read paths, commands, configuration or diagnostics out of the engine.
    """
    def record(path, limit=65536):
        try:
            value=json.loads(read(path,limit))
        except (OSError,ValueError,UnicodeError,RecursionError):
            return {}
        return value if isinstance(value,dict) else {}
    stages=('prepared','updater_ready','quiescing','fenced','backed_up','applying','verifying','resuming','succeeded')
    notices=('engine_failed','plan_changed','preparation_failed','control_busy','control_conflict','control_configuration','control_unavailable')
    value=dict(stage=None,backup_id=None,notice=None)
    status=record(STATE/'status.json')
    if type(status.get('format')) is int and status['format']==2:
        for name,allowed in [('stage',stages),('notice',notices)]:
            if isinstance(status.get(name),str) and status[name] in allowed:
                value[name]=status[name]
    processing=record(STATE/'processing.json',4096)
    attempt=processing.get('attempt')
    if type(processing.get('format')) is not int or processing['format']!=1 or not isinstance(attempt,str) or not re.fullmatch(r'[0-9a-f]{32}',attempt):
        return value
    journal=record(STATE/'attempts'/(attempt+'.json'))
    if type(journal.get('format')) is not int or journal['format']!=1 or journal.get('id')!=attempt:
        return value
    stage=journal.get('pending') or journal.get('phase')
    if isinstance(stage,str) and stage in stages:value['stage']=stage
    results=journal.get('results')
    backup=results.get('backed_up') if isinstance(results,dict) else None
    if isinstance(backup,dict) and backup.get('id')==attempt and isinstance(backup.get('manifest_sha256'),str) and re.fullmatch(r'[0-9a-f]{64}',backup['manifest_sha256']):
        value['backup_id']=attempt
    return value


def recovery(config):
    """Does not import the selected engine, execute app code or modify any data."""
    marker('maintenance')
    marker('writer-fence')
    (RUNTIME / 'application-start').unlink(missing_ok=True)
    (RUNTIME / 'writers-start').unlink(missing_ok=True)
    units = [config['service'], *config['writer_units']]
    require(len(units) <= 32 and len(units) == len(set(units))
            and all(isinstance(u, str) and re.fullmatch(r'[A-Za-z0-9_-]+\.service', u) for u in units)
            and 'talocart-update.service' not in units, 'Invalid recovery service list')
    if config.get('feed_format') == 2:
        import datetime
        import pwd
        # Fixed public contract, independent of all selected-engine imports.
        atomic_json(STATE / 'status.json', dict(format=2, phase='failed',
            checked_at=datetime.datetime.now(datetime.timezone.utc).isoformat(), candidate=None,
            plan=None, recovery_required=True, **recovery_metadata()),
            group=pwd.getpwnam(config['service_user']).pw_gid)
    command(['systemctl', 'stop', *units])
    reap_operation()


def install_bundle(archive, signature, public_key, expected_version):
    """Only before application mutation; switch code as one immutable directory."""
    require(isinstance(expected_version, str) and re.fullmatch(SEMVER, expected_version), 'Invalid engine version')
    require(not incomplete(), 'Resolve interrupted deployment before changing engines')
    trusted(BUNDLES)
    trusted(CURRENT.parent)
    old = selected() if CURRENT.exists() or CURRENT.is_symlink() else None
    if old:
        require(tuple(map(int, expected_version.split('.'))) > tuple(map(int, old.name.split('.'))),
                'Engine upgrades must be strictly increasing')
    published = BUNDLES / expected_version
    archive_digest = hashlib.sha256(read(archive, MAX_BUNDLE)).hexdigest()
    receipt = STATE / 'engine-receipts' / (expected_version + '.json')
    if published.exists() or published.is_symlink():
        # A crash may have published verified files before switching the pointer.
        require(engine_receipt(expected_version) == archive_digest, 'Existing engine provenance mismatch')
        inspect(published, expected_version)
        switch_engine(published)
        return {'version': expected_version, 'previous': old.name if old else None}
    # Copy into a trusted private staging directory first. Verify exactly copied bytes,
    # and never execute or extract from a mutable application-controlled input.
    data, sig = read(archive, MAX_BUNDLE), read(signature, 64)
    require(len(sig) == 64, 'Invalid engine signature')
    trusted(public_key)
    with tempfile.TemporaryDirectory(prefix='.engine-', dir=BUNDLES) as temporary:
        stage = Path(temporary)
        source, signed = stage / 'archive', stage / 'signature'
        source.write_bytes(data)
        signed.write_bytes(sig)
        command(['openssl', 'pkeyutl', '-verify', '-rawin', '-pubin', '-inkey', public_key,
                 '-in', source, '-sigfile', signed])
        destination = stage / 'payload'
        destination.mkdir(mode=0o755)
        seen, total = set(), 0
        with tarfile.open(source, 'r:gz') as bundle:
            for member in bundle:
                total += member.size
                require(member.name in FILES | {'bundle.json'} and member.name not in seen
                        and member.isfile() and 0 < member.size <= MAX_BUNDLE
                        and total <= MAX_BUNDLE and not member.pax_headers, 'Unsafe engine bundle')
                seen.add(member.name)
                with bundle.extractfile(member) as stream, (destination / member.name).open('xb') as output:
                    output.write(stream.read(MAX_BUNDLE + 1))
                    output.flush()
                    os.fsync(output.fileno())
                (destination / member.name).chmod(0o644)
        inspect(destination, expected_version)
        # Compile only; do not run arbitrary self-check hooks from metadata.
        for name in FILES:
            compile(read(destination / name, MAX_BUNDLE), name, 'exec')
        # Engine provides one fixed, offline self-check. It runs only after the
        # dedicated root-code signature and exact file list have been verified.
        report = json.loads(command(['/usr/bin/python3', '-I', destination / 'update_agent.py', '--self-check']))
        require(report == {'format': 1, 'state_format': STATE_FORMAT, 'engine_version': expected_version},
                'Engine self-check contract mismatch')
        receipts = STATE / 'engine-receipts'
        receipts.mkdir(mode=0o700, exist_ok=True)
        trusted(receipts)
        if receipt.exists() or receipt.is_symlink():
            require(engine_receipt(expected_version) == hashlib.sha256(data).hexdigest(), 'Engine receipt mismatch')
        else:
            atomic_json(receipt, {'format': 1, 'version': expected_version, 'sha256': hashlib.sha256(data).hexdigest()})
        sync_directory(destination)
        destination.rename(published)
        sync_directory(BUNDLES)
        switch_engine(published)
    return {'version': expected_version, 'previous': old.name if old else None}


def engine_receipt(version):
    value = json.loads(read(STATE / 'engine-receipts' / (version + '.json'), 1024), object_pairs_hook=unique)
    require(isinstance(value, dict) and set(value) == {'format', 'version', 'sha256'}
            and type(value['format']) is int and value['format'] == 1 and value['version'] == version
            and isinstance(value['sha256'], str) and re.fullmatch(r'[0-9a-f]{64}', value['sha256']),
            'Invalid engine receipt')
    return value['sha256']


def switch_engine(published):
    # This symlink is disposable root state; a crash before replace may leave it.
    pending = CURRENT.with_name('.engine-current-pending')
    if pending.exists() or pending.is_symlink():
        require(pending.is_symlink() and pending.resolve() == published, 'Unexpected pending engine switch')
        pending.unlink()
    pending.symlink_to(published)
    os.replace(pending, CURRENT)
    sync_directory(CURRENT.parent)


def consume_handoff(config):
    """Fixed root-only transition between separate engine processes, before app mutation."""
    require(not incomplete(), 'Cannot change updater during an application attempt')
    path = STATE / 'engine-handoff.json'
    raw = read(path, 4096)
    require(path.stat().st_mode & 0o077 == 0, 'Engine handoff must be private')
    value = json.loads(raw, object_pairs_hook=unique)
    require(isinstance(value, dict) and set(value) == {'format', 'updater', 'plan_sha256', 'minimum_launcher'}
            and type(value['format']) is int and value['format'] == 1 and isinstance(value['plan_sha256'], str)
            and re.fullmatch(r'[0-9a-f]{64}', value['plan_sha256'])
            and isinstance(value['minimum_launcher'], str) and re.fullmatch(SEMVER, value['minimum_launcher'])
            and tuple(map(int, VERSION.split('.'))) >= tuple(map(int, value['minimum_launcher'].split('.'))),
            'Unsupported engine handoff')
    item = value['updater']
    require(isinstance(item, dict) and set(item) == {'asset', 'signature_asset', 'version', 'size', 'sha256'}
            and isinstance(item['asset'], str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,159}', item['asset'])
            and '..' not in item['asset'] and item['signature_asset'] == item['asset'] + '.sig'
            and isinstance(item['version'], str) and re.fullmatch(SEMVER, item['version'])
            and type(item['size']) is int and 0 < item['size'] <= MAX_BUNDLE
            and isinstance(item['sha256'], str) and re.fullmatch(r'[0-9a-f]{64}', item['sha256']),
            'Invalid engine handoff artifact')
    incoming, key = Path(config['incoming']), Path(config['updater_public_key'])
    require(incoming.is_absolute() and key.is_absolute(), 'Invalid configured updater paths')
    archive, signature = incoming / item['asset'], incoming / item['signature_asset']
    content = read(archive, MAX_BUNDLE)
    require(len(content) == item['size'] and hashlib.sha256(content).hexdigest() == item['sha256'],
            'Handoff artifact changed')
    old = selected()
    if old.name == item['version']:
        # Crash after selection but before acknowledgement: never reinstall or downgrade.
        require(engine_receipt(old.name) == item['sha256'], 'Selected engine provenance mismatch')
    else:
        install_bundle(archive, signature, key, item['version'])
    path.unlink()
    sync_directory(STATE)
    return value['plan_sha256']


def main():
    config = None
    handoff_started = False
    try:
        require(sys.platform == 'linux' and os.geteuid() == 0, 'Requires installed Linux root launcher')
        trusted(Path(__file__).resolve())
        trusted(STATE)
        if sys.argv[1:] == ['--application-start-allowed']:
            return 0 if application_start_allowed() else 1
        if sys.argv[1:] == ['--writer-start-allowed']:
            return 0 if writer_start_allowed() else 1
        require(not sys.argv[1:], 'Launcher takes no deployment parameters')
        config = tomllib.loads(read(CONFIG, 65536).decode())
        fd = os.open(STATE / 'launcher.lock', os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, 'wb') as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return 0
            # No dependency on an executable/valid current engine during recovery.
            if incomplete():
                recovery(config)
                return 1
            # Safe preparation CLIs may survive a dead engine; stop their cgroup
            # without taking an otherwise healthy shop offline.
            reap_operation()
            handoff = STATE / 'engine-handoff.json'
            if handoff.exists() or handoff.is_symlink():
                handoff_started = True
                consume_handoff(config)
                handoff_started = False
            for _ in range(2):
                engine = selected()
                result = subprocess.run(['/usr/bin/python3', '-I', str(engine / 'update_agent.py')],
                                        check=False, timeout=21600)
                if result.returncode == HANDOFF_EXIT:
                    handoff_started = True
                    consume_handoff(config)
                    handoff_started = False
                    continue
                if result.returncode != 0 and incomplete():
                    recovery(config)
                return 0 if result.returncode == 0 else 1
            raise ValueError('Repeated engine handoff')
    except Exception:
        if handoff_started and not incomplete():
            pending = STATE / 'engine-handoff.json'
            if pending.exists() or pending.is_symlink():
                os.replace(pending, STATE / 'engine-handoff-failed.json')
                sync_directory(STATE)
        if config is not None and incomplete():
            try:
                recovery(config)
            except Exception:
                pass
        # Root-private server journal is authoritative; never echo configs/argv/secrets.
        print('Updater launcher failed; inspect private state. No automatic data rollback.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
