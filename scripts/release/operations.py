#!/usr/bin/env python3
"""Offline backup and empty-target restore. Python 3.11+, PostgreSQL 17 client tools.

No shell interpolation; database credentials never appear in argv or diagnostics.
The operator must fence all writers for the whole maintenance window.
"""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import tomllib
from urllib.parse import urlsplit, unquote, parse_qsl


class OperationError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise OperationError(message)


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def files(root):
    require(root.is_dir() and not root.is_symlink(), 'Expected a real directory')
    result = {}
    for path in sorted(root.rglob('*')):
        require(not path.is_symlink(), 'Symlinks are not supported')
        if path.is_dir():
            continue
        require(stat.S_ISREG(path.stat().st_mode), 'Special files are not supported')
        result[path.relative_to(root).as_posix()] = digest(path)
    return result


def verify(root, manifest_name='backup.json'):
    require(not (root / manifest_name).is_symlink(), 'Invalid manifest')
    manifest = json.loads((root / manifest_name).read_text())
    require(manifest.get('format') == 1 and isinstance(manifest.get('files'), dict), 'Unsupported manifest')
    actual = files(root)
    actual.pop(manifest_name, None)
    require(actual == manifest['files'], 'Checksum mismatch or unexpected/missing files')
    return manifest


def run(command, env=None, timeout=30):
    try:
        result = subprocess.run(command, env=env, capture_output=True, timeout=timeout, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise OperationError('Required command unavailable or timed out; private diagnostics omitted') from error
    require(result.returncode == 0, f'{Path(command[0]).name} failed; private diagnostics omitted')
    return result.stdout


def configuration(path):
    # Refuse silent drift between the file backed up and effective runtime settings.
    require(not any(k.startswith('TALOCART_') and k != 'TALOCART_LOG' for k in os.environ),
            'Unset TALOCART_* overrides; use a complete configuration file for maintenance')
    require(path.is_file() and not path.is_symlink(), 'Configuration must be a regular file')
    config = tomllib.loads(path.read_text())
    media = Path(config.get('media', {}).get('root', 'storage/media'))
    if not media.is_absolute():
        media = path.absolute().parent.resolve() / media
    require(not any(p.is_symlink() for p in [media, *media.parents]), 'Media path contains symlinks')
    media = Path(os.path.abspath(media))
    url = urlsplit(config['database']['url'])
    require(url.scheme in ('postgres', 'postgresql') and url.hostname and url.path.strip('/'), 'Invalid database URL')
    # libpq options must be explicit, never search_path-scoped partial backups.
    query = dict(parse_qsl(url.query))
    require(set(query) <= {'sslmode', 'sslrootcert', 'sslcert', 'sslkey', 'connect_timeout'}, 'Unsupported database URL options')
    env = {k: v for k, v in os.environ.items() if not k.startswith('PG')}
    env.update(PGHOST=url.hostname, PGPORT=str(url.port or 5432), PGDATABASE=unquote(url.path[1:]),
               PGUSER=unquote(url.username or ''), PGPASSWORD=unquote(url.password or ''), PGCONNECT_TIMEOUT='5')
    env.update({'PG' + k.upper(): v for k, v in query.items()})
    return media, env


def packages_path(config):
    value = tomllib.loads(config.read_text()).get('packages', {}).get('root', 'storage/packages')
    require(isinstance(value, str) and bool(value), 'Invalid packages path')
    path = Path(value)
    require('..' not in path.parts, 'Packages path contains parent traversal')
    if not path.is_absolute():
        path = config.absolute().parent.resolve() / path
    require(not any(p.is_symlink() for p in [path, *path.parents]), 'Packages path contains symlinks')
    return Path(os.path.abspath(path))


def smtp_secrets_path(media):
    path = media.with_name(media.name + '.secrets')
    require(not any(p.is_symlink() for p in [path, *path.parents]), 'SMTP secrets path contains symlinks')
    return path


def separate_paths(first, second):
    require(not first.is_relative_to(second) and not second.is_relative_to(first), 'Persistent storage paths must be separate')


def copy_packages(source, target):
    copy_tree(source, target)
    # The signed native executable has this exact layout; settings remain private data.
    for executable in (target / 'installed').glob('*/*/module'):
        require(executable.is_file(), 'Invalid module executable')
        executable.chmod(0o700)


def query(env, sql):
    return run(['psql', '-X', '-A', '-t', '-v', 'ON_ERROR_STOP=1', '-c', sql], env).decode().strip()


def offline(env):
    require(query(env, "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()") == '0',
            'Database has other connections: stop the service and fence writers first')


def copy_private(source, destination):
    source = Path(source)
    require(source.is_file() and not source.is_symlink(), 'Expected a regular source file')
    shutil.copyfile(source, destination)
    destination.chmod(0o600)


def copy_tree(source, target):
    files(source)  # Reject links/special files before copying.
    shutil.copytree(source, target)
    for path in [target, *target.rglob('*')]:
        path.chmod(0o700 if path.is_dir() else 0o600)


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def sync_tree(root):
    """Make a verified private tree durable without following links or devices."""
    def sync(fd):
        info = os.fstat(fd)
        if stat.S_ISDIR(info.st_mode):
            for name in os.listdir(fd):
                child_info = os.stat(name, dir_fd=fd, follow_symlinks=False)
                require(stat.S_ISREG(child_info.st_mode) or stat.S_ISDIR(child_info.st_mode),
                        'Unsafe storage member')
                child = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
                try:
                    sync(child)
                finally:
                    os.close(child)
        else:
            require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1, 'Unsafe storage member')
        os.fsync(fd)
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        sync(fd)
    finally:
        os.close(fd)


def prepare_directory(path):
    """Persist any newly created parent chain before publishing data below it."""
    missing, current = [], path
    while not current.exists():
        require(not current.is_symlink(), 'Storage path contains symlinks')
        missing.append(current)
        current = current.parent
    require(current.is_dir() and not current.is_symlink(), 'Expected a real storage directory')
    for folder in reversed(missing):
        folder.mkdir(mode=0o700)
    for folder in missing:
        sync_directory(folder)
        sync_directory(folder.parent)


def preflight(config, binary):
    """Read-only online capacity/tool check. No backup, migration or temporary tree."""
    media, env = configuration(config)
    packages = packages_path(config)
    secrets = smtp_secrets_path(media)
    separate_paths(media, packages)
    separate_paths(media, secrets)
    separate_paths(packages, secrets)
    env['PGOPTIONS'] = '-c default_transaction_read_only=on'
    server = int(query(env, "SELECT current_setting('server_version_num')")) // 10000
    require(server == 17, 'This backup workflow is qualified for PostgreSQL 17')
    for tool in ('pg_dump', 'pg_restore'):
        version = re.search(rb'PostgreSQL\)?\s+(\d+)\.', run([tool, '--version']))
        require(version is not None and int(version[1]) == server, 'Matching PostgreSQL client tools are required')
    database = int(query(env, 'SELECT pg_database_size(current_database())'))
    require(database > 0, 'Invalid database capacity estimate')
    stored, count = 0, 0
    for root in (media, packages, secrets):
        if not root.exists() and root in (packages, secrets):
            continue
        require(root.is_dir() and not root.is_symlink(), 'Expected existing storage directory')
        for path in root.rglob('*'):
            require(not path.is_symlink(), 'Storage symlinks are unsupported')
            info = path.stat()
            if stat.S_ISDIR(info.st_mode):
                continue
            require(stat.S_ISREG(info.st_mode), 'Storage special files are unsupported')
            stored += info.st_size
            count += 1
    require(binary.is_file() and not binary.is_symlink(), 'Expected an immutable application binary')
    reserve = database * 3 + stored + binary.stat().st_size + config.stat().st_size + count * 8192 + 256 * 1024**2
    return dict(format=1, postgres_major=server, database_bytes=database, storage_bytes=stored, backup_bytes=reserve)


def backup(config, binary, output, database_timeout=3600):
    require(not any(p.is_symlink() for p in [output, *output.parents]), 'Backup path contains symlinks')
    output = Path(os.path.abspath(output))
    require(not output.exists() and not output.is_symlink(), 'Output already exists')
    media, env = configuration(config)
    packages = packages_path(config)
    secrets = smtp_secrets_path(media)
    separate_paths(media, packages)
    separate_paths(media, secrets)
    separate_paths(packages, secrets)
    require(media.is_dir(), 'Media directory is missing')
    require(not packages.exists() or packages.is_dir(), 'Packages directory is invalid')
    require(not output.is_relative_to(packages), 'Backup must be outside packages')
    secrets_included = secrets.exists()
    require(not secrets_included or secrets.is_dir(), 'Invalid SMTP secrets directory')
    require(not output.is_relative_to(secrets), 'Backup must be outside SMTP secrets')
    packages_included = packages.exists()
    require(media not in output.absolute().parents and output.absolute() != media, 'Backup must be outside media')
    offline(env)
    # doctor checks this binary against every migration checksum before the dump.
    run([str(binary.absolute()), '--config', str(config.absolute()), 'doctor'])
    prepare_directory(output.parent)
    stage = Path(tempfile.mkdtemp(prefix='.talocart-backup-', dir=output.parent))
    try:
        run(['pg_dump', '--format=custom', '--no-owner', '--no-privileges', '--file', str(stage / 'database.dump')], env, timeout=database_timeout)
        copy_private(config, stage / 'talocart.toml')
        copy_private(binary, stage / 'talocart')
        (stage / 'talocart').chmod(0o700)
        copy_tree(media, stage / 'media')
        if packages_included:
            copy_packages(packages, stage / 'packages')
        if secrets_included:
            copy_tree(secrets, stage / 'smtp-secrets')
        offline(env)
        require(config.read_bytes() == (stage / 'talocart.toml').read_bytes()
                and digest(binary) == digest(stage / 'talocart') and files(media) == files(stage / 'media')
                and packages.exists() == packages_included
                and (not packages_included or files(packages) == files(stage / 'packages'))
                and secrets.exists() == secrets_included
                and (not secrets_included or files(secrets) == files(stage / 'smtp-secrets')),
                'Source changed during maintenance; backup discarded')
        manifest = {'format': 1, 'packages_included': packages_included, 'smtp_secrets_included': secrets_included, 'files': files(stage), 'migration_count': int(query(env, 'SELECT count(*) FROM _sqlx_migrations'))}
        (stage / 'backup.json').write_text(json.dumps(manifest, indent=2) + '\n')
        (stage / 'backup.json').chmod(0o600)
        verify(stage)
        sync_tree(stage)
        stage.rename(output)
        sync_directory(output.parent)
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def restore(source, config, database_timeout=3600):
    manifest = verify(source)
    require({'database.dump', 'talocart.toml', 'talocart'} <= set(manifest['files']), 'Incomplete backup')
    media, env = configuration(config)
    packages = packages_path(config)
    secrets = smtp_secrets_path(media)
    separate_paths(media, packages)
    separate_paths(media, secrets)
    separate_paths(packages, secrets)
    require(not packages.exists() and not packages.is_symlink(), 'Restore requires a new packages path')
    require(not packages.is_relative_to(source.resolve()), 'Restore packages must be outside backup')
    require(not secrets.exists() and not secrets.is_symlink(), 'Restore requires a new SMTP secrets path')
    require(not secrets.is_relative_to(source.resolve()), 'Restore SMTP secrets must be outside backup')
    has_secrets = manifest.get('smtp_secrets_included', False)
    require(isinstance(has_secrets, bool) and has_secrets == (source / 'smtp-secrets').is_dir(), 'Incomplete SMTP secrets backup')
    has_packages = manifest.get('packages_included', (source / 'packages').is_dir())
    require(isinstance(has_packages, bool) and has_packages == (source / 'packages').is_dir(), 'Incomplete package backup')
    require(not media.exists() and not media.is_symlink(), 'Restore requires a new media path')
    offline(env)
    require(query(env, "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname !~ '^pg_toast' AND n.nspname !~ '^pg_temp'") == '0',
            'Restore requires an empty database; existing data will never be overwritten')
    # Stage and verify all trees before publishing either one or starting SQL.
    # On publication failure, retract only directories published by this invocation.
    from contextlib import ExitStack
    published = []
    with ExitStack() as cleanup:
        staged_trees = []
        trees = [('media', media, copy_tree)]
        if has_secrets:
            trees.append(('smtp-secrets', secrets, copy_tree))
        if has_packages:
            trees.append(('packages', packages, copy_packages))
        for name, destination, copier in trees:
            prepare_directory(destination.parent)
            temp = Path(cleanup.enter_context(tempfile.TemporaryDirectory(prefix='.talocart-restore-', dir=destination.parent)))
            staged = temp / name
            copier(source / name, staged)
            expected = {k.removeprefix(name + '/'): v for k, v in manifest['files'].items() if k.startswith(name + '/')}
            require(files(staged) == expected, 'Restored ' + name + ' differs')
            sync_tree(staged)
            staged_trees.append((staged, destination))
        try:
            for staged, destination in staged_trees:
                require(not destination.exists() and not destination.is_symlink(), 'Restore destination appeared during copy')
                staged.rename(destination)
                published.append((destination, staged))
                sync_directory(destination.parent)
                sync_directory(staged.parent)
        except (OSError, OperationError):
            cleanup_error = None
            for destination, staged in reversed(published):
                try:
                    destination.rename(staged)
                    sync_directory(destination.parent)
                    sync_directory(staged.parent)
                except OSError as error:
                    cleanup_error = error  # Still retract the other trees after a failed sync.
            if cleanup_error is not None:
                raise OperationError('Storage publication or cleanup failed. No database restore started; inspect the new destination paths before retrying.') from cleanup_error
            raise
    try:
        run(['pg_restore', '--no-owner', '--no-privileges', '--exit-on-error', '--single-transaction',
             '--dbname', env['PGDATABASE'], str(source / 'database.dump')], env, timeout=database_timeout)
    except OperationError as error:
        # A client failure does not prove server rollback (e.g. lost COMMIT acknowledgement).
        raise OperationError('Database restore failed or outcome uncertain. Media preserved; SMTP secrets and installed packages preserved when included. Inspect database and all restored storage before retrying. / Ripristino database fallito o esito incerto. Media, segreti SMTP e pacchetti inclusi conservati: verificare database e storage ripristinato prima di riprovare.') from error
    # Do not overwrite target configuration or start workers. Validate with compatible binary next.
    require(int(query(env, 'SELECT count(*) FROM _sqlx_migrations')) == manifest['migration_count'], 'Restored migration count differs')


def positive_seconds(value):
    try:
        seconds = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError('Use positive integer seconds / Usare secondi interi positivi') from error
    if seconds <= 0:
        raise argparse.ArgumentTypeError('Use positive integer seconds / Usare secondi interi positivi')
    return seconds


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    p = commands.add_parser('preflight', help='Read-only online capacity and PostgreSQL tool check')
    p.add_argument('--config', type=Path, required=True)
    p.add_argument('--binary', type=Path, required=True)
    b = commands.add_parser('backup', help='Offline only: service stopped, all writers fenced')
    b.add_argument('--config', type=Path, required=True)
    b.add_argument('--binary', type=Path, required=True)
    b.add_argument('--output', type=Path, required=True)
    r = commands.add_parser('restore', help='Restore into empty database and new media/packages directories only')
    r.add_argument('--backup', type=Path, required=True)
    r.add_argument('--config', type=Path, required=True, help='Explicit target credentials and new media/packages paths')
    for command in (b, r):
        command.add_argument('--database-timeout', type=positive_seconds, default=3600,
                             help='Dump/restore timeout in seconds (default: 3600) / Limite dump/ripristino in secondi (predefinito: 3600)')
    v = commands.add_parser('verify', help='Verify complete backup contents before restoration')
    v.add_argument('directory', type=Path)
    args = parser.parse_args()
    try:
        os.umask(0o077)
        if args.command == 'preflight':
            print(json.dumps(preflight(args.config, args.binary)))
            return 0
        if args.command == 'backup':
            backup(args.config, args.binary, args.output, args.database_timeout)
        elif args.command == 'restore':
            restore(args.backup, args.config, args.database_timeout)
        else:
            verify(args.directory)
        print('OK')
        return 0
    except OperationError as error:
        print(str(error), file=sys.stderr)
        return 1
    except (OSError, ValueError, KeyError, TypeError):
        # Syntax errors and libpq diagnostics can contain credentials or personal data.
        print('Operation failed; check paths, offline/empty-target requirements and checksums. Private diagnostics omitted.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
