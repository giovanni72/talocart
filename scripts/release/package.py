#!/usr/bin/env python3
"""Build a deterministic, allowlisted release archive; never include runtime data."""
import argparse
import gzip
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tarfile
import tempfile
import tomllib
from operations import digest, files, require, verify, OperationError
import version as versions

ROOT = Path(__file__).resolve().parents[2]
TARGETS = {'x86_64-unknown-linux-gnu': (2, 62), 'aarch64-unknown-linux-gnu': (2, 183),
           'aarch64-apple-darwin': None}


def check_binary(binary, target):
    require(target in TARGETS, 'Unsupported target')
    with binary.open('rb') as stream:
        header = stream.read(64)
    require(len(header) >= 64, 'Invalid executable header')
    if TARGETS[target]:
        require(header[:4] == b'\x7fELF' and header[4:6] == b'\x02\x01' and
                struct.unpack('<H', header[18:20])[0] == TARGETS[target][1], 'Binary does not match target')
    else:
        require(header[:4] == b'\xcf\xfa\xed\xfe' and struct.unpack('<I', header[4:8])[0] == 0x0100000c,
                'Binary does not match target')


def licenses(metadata, destination, local_packages=frozenset()):
    destination.mkdir()
    records = []
    available = {p['name'] for p in metadata['packages'] if p['source'] is None}
    require(set(local_packages) <= available, 'Selected module missing from metadata')
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        vendored = Path(package.get('manifest_path', '')).resolve().is_relative_to((ROOT / 'vendor').resolve())
        if package['source'] is None and package['name'] not in local_packages and not vendored:
            continue
        folder = Path(package['manifest_path']).parent
        candidates = [p for p in folder.iterdir() if p.is_file() and
                      p.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE', 'COPYRIGHT'))]
        if package.get('license_file'):
            candidates.append(folder / package['license_file'])
        fallback = ROOT / 'distribution' / 'licenses' / package['name']
        if not candidates and fallback.is_dir():
            candidates = list(fallback.iterdir())
        require(bool(candidates), 'Dependency has no license text: ' + package['name'])
        output = destination / (package['name'] + '-' + package['version'])
        output.mkdir()
        for source in set(candidates):
            require(source.is_file() and not source.is_symlink(), 'Invalid license source')
            shutil.copyfile(source, output / source.name)
        record = {k: package[k] for k in ('name', 'version', 'license', 'repository')}
        if package['source'] is not None:
            record['source_archive'] = f"https://static.crates.io/crates/{package['name']}/{package['name']}-{package['version']}.crate"
        records.append(record)
    (destination / 'dependencies.json').write_text(json.dumps(records, indent=2) + '\n')


def package_versions(metadata):
    pinned = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    compiler = subprocess.check_output(['rustc', '--version'], cwd=ROOT, text=True).split()
    require(len(compiler) >= 2 and compiler[0] == 'rustc' and compiler[1] == pinned,
            'Active Rust compiler does not match rust-toolchain.toml')
    apps = [p for p in metadata['packages'] if p['name'] == 'talocart'
            and p['id'] in metadata['workspace_members'] and p['source'] is None]
    require(len(apps) == 1, 'Expected one Talocart workspace package in metadata')
    return pinned, apps[0]['version']


def build_identity(binary, version, commit, dirty, fingerprint, release=False):
    result = subprocess.run([str(binary.resolve()), 'build-info'], cwd=ROOT, check=True,
                            capture_output=True, text=True, timeout=30)
    info = json.loads(result.stdout)
    require(isinstance(info, dict), 'Invalid binary build identity')
    require(info.get('version') == version, 'Binary version does not match Cargo/package version')
    require(info.get('commit') == commit and info.get('source_dirty') is dirty,
            'Binary source identity does not match the worktree; rebuild first')
    require(info.get('source_fingerprint') == fingerprint, 'Binary sources have changed; rebuild first')
    if release:
        require(not dirty and info.get('tagged') is True, 'Release requires a clean build from its matching tag')
        versions.check(ROOT, release=True)
    return info


def package(binary, target, version, metadata, output, release=False, profile="base"):
    require(profile == "base", "Unknown distribution profile")
    check_binary(binary, target)
    toolchain, cargo_version = package_versions(metadata)
    version = versions.validate(version or cargo_version)
    require(version == cargo_version == versions.current(ROOT), 'Package version must match the workspace Cargo version')
    output.mkdir(parents=True, exist_ok=True)
    name = f'talocart-{version}-{target}'
    archive = output / (name + '.tar.gz')
    checksum = output / (name + '.tar.gz.sha256')
    require(not archive.exists() and not checksum.exists(), 'Release output already exists')
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    epoch = int(os.environ.get('SOURCE_DATE_EPOCH') or subprocess.check_output(['git', 'show', '-s', '--format=%ct', 'HEAD'], cwd=ROOT, text=True))
    dirty = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=versions.workspace_root(ROOT)))
    info = build_identity(binary, version, commit, dirty, versions.fingerprint(ROOT), release)
    require(info.get("compiled_modules")==[],"Binary optional modules do not match distribution profile")
    with tempfile.TemporaryDirectory(prefix='talocart-package-') as temp:
        stage = Path(temp) / name
        stage.mkdir()
        (stage / 'bin').mkdir()
        shutil.copyfile(binary, stage / 'bin' / 'talocart')
        (stage / 'bin' / 'talocart').chmod(0o755)
        # These paths are the complete contract. No globs over the worktree.
        for source in ['LICENSE', 'CONTRIBUTING.md', 'SECURITY.md', 'talocart.example.toml',
                       'distribution/talocart.service', 'distribution/nginx.conf', 'distribution/talocart.https.toml',
                       'docs/release-operations.md', 'docs/operational-readiness-checklist.md', 'docs/guides/versioning.md', 'scripts/release/operations.py',
                       'docs/guides/application-updates.md']:
            destination = stage / source
            destination.parent.mkdir(parents=True, exist_ok=True)
            require((ROOT / source).is_file() and not (ROOT / source).is_symlink(), 'Invalid package source')
            shutil.copyfile(ROOT / source, destination)
        files(ROOT / 'assets/vendor')
        shutil.copytree(ROOT / 'assets/vendor', stage / 'third-party' / 'frontend')
        licenses(metadata, stage / 'third-party' / 'rust')
        # libstd is linked into the executable but is absent from cargo metadata.
        sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
        notices = sysroot / 'share/doc/rust'
        runtime = stage / 'third-party/rust-runtime'
        runtime.mkdir()
        require((notices / 'COPYRIGHT-library.html').is_file(), 'Rust standard-library notices missing')
        files(notices / 'licenses')
        shutil.copyfile(notices / 'COPYRIGHT-library.html', runtime / 'COPYRIGHT-library.html')
        shutil.copytree(notices / 'licenses', runtime / 'licenses')
        manifest = {'format': 1, 'profile': profile, 'modules': [], 'version': version, 'target': target, 'source_commit': commit,
                    'source_dirty': dirty, 'source_date_epoch': epoch,
                    'rust_toolchain': toolchain, 'cargo_package_version': cargo_version,
                    'build': info, 'files': files(stage)}
        (stage / 'release.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
        verify(stage, 'release.json')
        pending = output / (name + '.pending')
        try:
            with pending.open('xb') as raw:
                with gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=epoch) as compressed:
                    with tarfile.open(fileobj=compressed, mode='w', format=tarfile.PAX_FORMAT) as tar:
                        for path in [stage, *sorted(stage.rglob('*'))]:
                            info = tar.gettarinfo(str(path), arcname=path.relative_to(stage.parent).as_posix())
                            info.uid = info.gid = 0
                            info.uname = info.gname = ''
                            info.mtime = epoch
                            info.mode = 0o755 if path.is_dir() or path == stage / 'bin/talocart' else 0o644
                            if path.is_file():
                                with path.open('rb') as data:
                                    tar.addfile(info, data)
                            else:
                                tar.addfile(info)
            pending.rename(archive)
            checksum.write_text(f'{digest(archive)}  {archive.name}\n')
        finally:
            pending.unlink(missing_ok=True)
    return archive


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=('base',), default='base')
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--target', choices=TARGETS, required=True)
    parser.add_argument('--version', help='Optional assertion; must match workspace Cargo version')
    parser.add_argument('--release', action='store_true', help='Require a clean build from the matching annotated Git tag')
    parser.add_argument('--metadata', type=Path, required=True, help='cargo metadata --locked --filter-platform TARGET --format-version 1')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        print(package(args.binary, args.target, args.version, json.loads(args.metadata.read_text()), args.output, args.release, args.profile))
    except (OperationError, OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, f'Packaging failed: {error}\n')
