#!/usr/bin/env python3
"""Audit the workspace; exempt the known SQLite advisory only while inactive."""
from pathlib import Path
import subprocess
import sys


def audit_arguments(tree, lockfile):
    command = ['cargo', 'audit', '--file', str(lockfile)]
    packages = {line.split()[0] for line in tree.splitlines() if line.strip()}
    if 'libsqlite3-sys' not in packages:
        command.extend(['--ignore', 'RUSTSEC-2026-0459'])
    return command


def main():
    root = Path(subprocess.check_output(
        ['cargo', 'locate-project', '--workspace', '--message-format', 'plain'],
        text=True).strip()).parent
    tree = subprocess.check_output(
        ['cargo', 'tree', '--locked', '--workspace', '--target', 'all',
         '--edges', 'normal,build,dev', '--prefix', 'none', '--format', '{p}'],
        cwd=root, text=True)
    command = audit_arguments(tree, root / 'Cargo.lock')
    if '--ignore' in command:
        print('RUSTSEC-2026-0459: SQLite is absent from the active workspace '
              'normal/build/dev graph on all targets; excluding this advisory only.', flush=True)
    return subprocess.call(command, cwd=root)


if __name__ == '__main__':
    sys.exit(main())
