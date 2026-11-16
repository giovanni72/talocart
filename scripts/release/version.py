#!/usr/bin/env python3
"""Prepare/check the shared application version and tag a reviewed main commit."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[2]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate(value):
    number = r'(?:0|[1-9][0-9]*)'
    require(isinstance(value, str) and re.fullmatch(
        rf'{number}\.{number}\.{number}(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?', value),
        'Invalid application version')
    if '-' in value:
        require(all(not item.isdigit() or len(item) == 1 or not item.startswith('0')
                    for item in value.split('-', 1)[1].split('.')), 'Invalid prerelease identifier')
    return value


def workspace_root(root):
    if not (root / "Cargo.toml").exists() and (root.parent / "integration/Cargo.toml").is_file():
        return root.parent
    return root


def current(root=ROOT):
    root = workspace_root(root)
    return validate(tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version'])


def git(root, *args):
    return subprocess.check_output(['git', *args], cwd=root, text=True, stderr=subprocess.DEVNULL).strip()


def check(root=ROOT, release=False):
    root = workspace_root(root)
    value = current(root)
    # --no-deps avoids fetching crates for platforms other than the local target.
    # Cargo skips lock validation in that mode: explicitly compare workspace entries.
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'], cwd=root, text=True))
    locked = tomllib.loads((root / 'Cargo.lock').read_text())['package']
    for package in metadata['packages']:
        if package['id'] in metadata['workspace_members']:
            require(package['version'] == value, 'Workspace packages must inherit the application version')
            matches = [p for p in locked if p['name'] == package['name'] and 'source' not in p]
            require(len(matches) == 1 and matches[0]['version'] == value,
                    'Workspace lockfile version is stale; prepare the version first')
    if release:
        require(not git(root, 'status', '--porcelain'), 'Release requires a clean worktree')
        require(git(root, 'tag', '--points-at', 'HEAD', '--list', 'v' + value) == 'v' + value,
                'HEAD tag does not match the Cargo version')
        require(git(root, 'cat-file', '-t', 'refs/tags/v' + value) == 'tag', 'Release tag must be annotated')
    return value


def fingerprint(root=ROOT):
    root = workspace_root(root)
    data = subprocess.check_output(['git', 'diff', '--binary', 'HEAD', '--'], cwd=root)
    paths = subprocess.check_output(['git', 'ls-files', '--others', '--exclude-standard', '-z'], cwd=root)
    for path in filter(None, paths.split(b'\0')):
        digest = git(root, 'hash-object', '--', path.decode())
        data += path + b'\0' + digest.encode() + b'\0'
    return subprocess.check_output(['git', 'hash-object', '--stdin'], input=data, cwd=root, text=False).decode().strip()


def prepare(value, root=ROOT):
    root = workspace_root(root)
    validate(value)
    manifest = root / 'Cargo.toml'
    lock = root / 'Cargo.lock'
    previous = current(root)
    source, locked = manifest.read_bytes(), lock.read_bytes()
    text = source.decode()
    start = text.index('[workspace.package]')
    end = text.find('\n[', start + 1)
    end = len(text) if end < 0 else end
    section, count = re.subn(r'(?m)^version[ \t]*=[ \t]*"' + re.escape(previous) + r'"[ \t]*$',
                             'version = "' + value + '"', text[start:end])
    require(count == 1, 'Expected one workspace version')
    try:
        manifest.write_text(text[:start] + section + text[end:])
        # Update workspace packages only; do not upgrade registry dependencies.
        subprocess.run(['cargo', 'update', '--workspace', '--offline'], cwd=root, check=True)
        check(root)
    except (OSError, ValueError, subprocess.SubprocessError):
        manifest.write_bytes(source)
        lock.write_bytes(locked)
        raise
    return value


def tag(root=ROOT):
    root = workspace_root(root)
    value = check(root)
    require(git(root, 'branch', '--show-current') == 'main', 'Release tags require main')
    require(not git(root, 'status', '--porcelain'), 'Commit and rebase reviewed changes before tagging')
    require(not git(root, 'tag', '--list', 'v' + value), 'Version tag already exists; prepare a new version')
    subprocess.run(['git', 'tag', '-a', 'v' + value, '-m', 'Talocart ' + value], cwd=root, check=True)
    return check(root, release=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('prepare').add_argument('version')
    commands.add_parser('check').add_argument('--release', action='store_true')
    commands.add_parser('tag')
    args = parser.parse_args()
    try:
        print(prepare(args.version) if args.command == 'prepare' else
              tag() if args.command == 'tag' else check(release=args.release))
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, f'Version operation failed: {error}\n')
