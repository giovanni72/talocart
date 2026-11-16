#!/usr/bin/python3 -I
"""Installed root-owned pull updater. Timer checks availability; only owner requests install.

Install beside update_engine.py and updates_feed.py. No sudo or SSH from the web
application. Failures preserve the engine's maintenance/recovery state.
"""
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import signal
import stat
import sys
import tomllib
from types import SimpleNamespace

# Bundles are immutable: imported siblings must never create __pycache__ entries.
sys.dont_write_bytecode = True

CONFIG = Path('/etc/talocart/update.toml')
STATE = Path('/var/lib/talocart-update')


def trusted(path):
    for item in [path, *path.parents]:
        info = item.lstat()
        if stat.S_ISLNK(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
            raise ValueError('Updater files must be root-owned and not writable by others')


def sibling(name):
    path = Path(__file__).with_name(name + '.py').resolve(strict=True)
    trusted(path)
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    # Isolated Python excludes the script directory. Register only this verified
    # file so sibling dependencies cannot fall back to cwd/PYTHONPATH/site code.
    missing = object()
    previous = sys.modules.get(name, missing)
    sys.modules[name] = value
    try:
        spec.loader.exec_module(value)
    except BaseException:
        if previous is missing:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = previous
        raise
    return value


def connection(config, feed):
    token = ''
    if config.get('token_file'):
        path = Path(config['token_file'])
        trusted(path)
        feed.require(path.stat().st_mode & 0o077 == 0, 'Repository token must be root-private')
        token = path.read_text().strip()
    trusted(Path(config['public_key']))
    github = feed.GitHub(config['repository'], token)
    return github


def download(github, release, package, archive, feed):
    # Only disposable downloads in the trusted incoming directory are replaced;
    # release directories, backups and recovery state are never removed here.
    if archive.exists() or archive.is_symlink():
        trusted(archive)
        info = archive.lstat()
        feed.require(stat.S_ISREG(info.st_mode), 'Download must be a regular file')
        if info.st_size == package['size'] and feed.digest(archive) == package['sha256']:
            return
        archive.unlink()
    partial = archive.with_name(archive.name + '.partial')
    if partial.exists() or partial.is_symlink():
        trusted(partial)
        feed.require(stat.S_ISREG(partial.lstat().st_mode), 'Partial download must be a regular file')
        partial.unlink()
    try:
        github.asset(release, package['asset'], package['size'], partial)
        feed.require(partial.stat().st_size == package['size'] and feed.digest(partial) == package['sha256'],
                     'Downloaded package checksum mismatch')
        os.replace(partial, archive)
    finally:
        partial.unlink(missing_ok=True)


def self_check():
    # Offline, fixed contract; no config, network, database or service operations.
    folder = Path(__file__).resolve().parent
    for name in ('update_agent.py', 'update_engine.py', 'updates_feed.py',
                 'distribution.py', 'update_coordinator.py'):
        compile((folder / name).read_bytes(), name, 'exec')
    # Execute the same dependency graph as the installed -I entry point, not
    # merely syntax-check distribution.py while hiding a missing sibling import.
    sibling('updates_feed')
    sibling('update_engine')
    coordinator = sibling('update_coordinator')
    sibling('distribution')
    print(json.dumps({'format': 1, 'state_format': 1, 'engine_version': coordinator.ENGINE_VERSION}))


def main():
    if sys.argv[1:] == ['--self-check']:
        self_check()
        return
    if sys.argv[1:]:
        raise ValueError('Unexpected updater arguments')
    try:
        if sys.platform != 'linux' or os.geteuid() != 0:
            raise ValueError('Requires the installed Linux updater')
        trusted(Path(__file__).resolve())
        trusted(CONFIG)
        trusted(STATE)
        feed, engine = sibling('updates_feed'), sibling('update_engine')
        config = tomllib.loads(CONFIG.read_text())
        engine.check_config(config)
        with (STATE / 'agent.lock').open('a') as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return
            engine.STATE = STATE
            def interrupted(*_):
                raise RuntimeError('Updater interrupted')
            for signum in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
                signal.signal(signum, interrupted)
            coordinator, distribution = sibling('update_coordinator'), sibling('distribution')
            result = coordinator.cycle(config, feed, distribution, SimpleNamespace(connection=connection, download=download), engine)
            if result:
                raise SystemExit(result)
    except Exception:
        print('Update check/install failed; inspect private server state. No automatic rollback.', file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
