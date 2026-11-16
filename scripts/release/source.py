#!/usr/bin/env python3
"""Export the current sources without Git history, secrets, or unselected modules.

The base profile copies reviewed sources without rewriting application code. It does not touch the working tree or the application database.
"""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
PUBLIC_FILES = Path('scripts/release/base-files.json')


def public_files(root):
    """Only explicitly reviewed paths can enter the public source distribution."""
    root = root.resolve()
    manifest = json.loads((root / PUBLIC_FILES).read_text())
    if manifest.get('format') != 1 or not isinstance(manifest.get('files'), dict):
        raise ValueError('Invalid public source allowlist')
    for target, origin in manifest['files'].items():
        for name in (target, origin):
            path = Path(name)
            if path.is_absolute() or '..' in path.parts or path.as_posix() != name:
                raise ValueError('Invalid public source path: ' + name)
            if any(part in ('.git', '.local', '.claude', 'modules', 'storage', 'target', 'node_modules') for part in path.parts):
                raise ValueError('Private/runtime path in public source allowlist: ' + name)
        path = input_path(root, origin)
        if not path.is_file() or path.is_symlink() or any(p.is_symlink() for p in [path, *path.parents] if p.is_relative_to(root)):
            raise ValueError('Missing or linked public source input: ' + origin)
    return manifest['files']


def check_public(destination, *, no_history=False):
    """Check the snapshot boundary and local documentation links before publication."""
    reviewed = public_files(destination)
    if any(target != origin for target, origin in reviewed.items()):
        raise ValueError('Public snapshot paths must map to themselves')
    expected = set(reviewed)
    if no_history and ((destination / '.git').exists() or (destination / '.git').is_symlink()):
        raise ValueError('Public source snapshot must not contain Git history')
    if (destination / 'modules').exists():
        raise ValueError('Optional module directory in public base')
    # A fresh export has no generated files. In a development checkout only
    # check versioned files, so build/test output cannot become public inputs.
    if (destination / '.git').exists() and not no_history:
        paths = subprocess.check_output(['git', 'ls-files', '-z'], cwd=destination).decode().split('\0')
        actual = {name for name in paths if name}
    else:
        actual = {p.relative_to(destination).as_posix() for p in destination.rglob('*') if p.is_file() or p.is_symlink()}
    if actual != expected:
        raise ValueError(f'Public source file set differs: extra={sorted(actual-expected)}, missing={sorted(expected-actual)}')
    for name in expected:
        path = destination / name
        if path.suffix == '.rs':
            text = path.read_text()
            if 'talocart_koala_import' in text or 'talocart_stripe' in text:
                raise ValueError('Optional module implementation link: ' + name)
            if re.search(r'#\s*\[cfg\([^\n]*feature\s*=\s*"(?:koala-import|stripe-sandbox-tests)"', text):
                raise ValueError('Optional module conditional compilation: ' + name)
        if path.suffix == '.md':
            for link in re.findall(r'(?<!!)\[[^\]]*\]\(([^)\s]+)\)', path.read_text()):
                if re.match(r'[a-zA-Z][a-zA-Z0-9+.-]*:', link) or link.startswith('#'):
                    continue
                target = (path.parent / link.split('#', 1)[0]).resolve()
                if not target.is_relative_to(destination.resolve()) or not target.exists():
                    raise ValueError(f'Broken public documentation link: {name}: {link}')
    for manifest in (destination / 'Cargo.toml', destination / 'Cargo.lock'):
        if any(name in manifest.read_text() for name in ('talocart-koala-import', 'talocart-stripe', 'modules/')):
            raise ValueError('Optional module in public workspace/lockfile')
    return len(expected)

def input_path(root, name):
    # Full has a single authoritative workspace manifest/lockfile above base.
    if name in ('Cargo.toml', 'Cargo.lock') and not (root / 'Cargo.toml').exists():
        if (root.parent / 'integration/Cargo.toml').is_file():
            return root.parent / name
    return root / name


def export(destination, profile="base"):
    if profile != 'base':
        raise ValueError('Optional modules are distributed as separate native packages')
    destination = Path(destination).resolve()
    if destination.exists():
        raise ValueError('Output directory already exists')
    if destination == ROOT or ROOT in destination.parents and destination.parts[len(ROOT.parts)] not in ('.local', 'dist'):
        raise ValueError('Use an external output directory, .local or dist')
    reviewed = public_files(ROOT)
    destination.mkdir(parents=True)
    for name, origin in reviewed.items():
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(input_path(ROOT, origin), target)
    if not (ROOT / 'Cargo.toml').exists():
        # Only workspace metadata changes. Application sources and tests are copied verbatim.
        path = destination / 'Cargo.toml'
        text = path.read_text()
        members = ['apps/talocart', 'crates/platform', 'crates/commerce', 'crates/web', 'crates/package-fixture', 'crates/module-sdk']
        text = re.sub(r'(?m)^members = .*$', 'members = ' + json.dumps(members), text)
        text = re.sub(r'(?m)^default-members = .*$', 'default-members = ' + json.dumps(members[:4]), text)
        text = text.replace('"base/vendor/askama_derive"', '"vendor/askama_derive"')
        path.write_text(text)
    subprocess.run(['cargo', 'update', '--workspace', '--offline'], cwd=destination, stdout=subprocess.DEVNULL, check=True)
    (destination / PUBLIC_FILES).write_text(json.dumps({'format': 1, 'files': {p: p for p in reviewed}}, indent=2) + '\n')
    check_public(destination, no_history=True)
    return destination

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=("base",), default="base")
    parser.add_argument("--output", type=Path)
    parser.add_argument('--check', type=Path, help='Check an existing public base snapshot/checkout')
    parser.add_argument('--no-history', action='store_true', help='Require a snapshot without .git when checking')
    args = parser.parse_args()
    if args.check:
        print(f'Public base: {check_public(args.check.resolve(), no_history=args.no_history)} reviewed files OK')
    elif args.output:
        print(export(args.output, args.profile))
    else:
        parser.error('Choose --output or --check')
