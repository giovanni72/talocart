#!/usr/bin/env python3
"""Build reproducible, separately signed root-engine bundles; no installation."""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import tarfile

import sys

# Packaging/checking must not create cache files in the reviewed source tree.
sys.dont_write_bytecode = True

import update_launcher as launcher
import updates_feed as feed


def coordinator_source(source, engine_version=None):
    """Embed the shared public page; no runtime filesystem dependency in root bundles."""
    raw = (source / 'update_coordinator.py').read_bytes()
    # Keep the signed runtime self-contained and its exact member allowlist stable.
    planning = b'from update_plan import canonical, create_plan, digest, require'
    if planning in raw:
        feed.require(raw.count(planning) == 1, 'Invalid planning embedding point')
        raw = raw.replace(planning, (source / 'update_plan.py').read_bytes())
    persistence = b'from update_state import PHASES, Journal, PlanStore, atomic, private_json'
    if persistence in raw:
        feed.require(raw.count(persistence) == 1, 'Invalid state embedding point')
        state = (source / 'update_state.py').read_bytes()
        state = state.replace(b'from update_plan import canonical, digest, require', b'')
        raw = raw.replace(persistence, state)
    host_import = b'from update_host import Operations, root_bytes, sync_folder'
    if host_import in raw:
        feed.require(raw.count(host_import) == 1, 'Invalid host embedding point')
        host = (source / 'update_host.py').read_bytes()
        host = host.replace(b'from update_plan import canonical, require', b'')
        host = host.replace(b'from update_state import atomic', b'')
        raw = raw.replace(host_import, host)
    probe_import = b'from update_probe import MAINTENANCE_PAGE, MAINTENANCE_STYLE, MAINTENANCE_SCRIPT, Probe, maintenance_page, nginx_probe_configuration, refresh_maintenance_gate'
    if probe_import in raw:
        feed.require(raw.count(probe_import) == 1, 'Invalid probe embedding point')
        probe = (source / 'update_probe.py').read_bytes()
        probe = probe.replace(b'from update_plan import require', b'')
        probe = probe.replace(b'from update_host import root_bytes, sync_folder', b'')
        raw = raw.replace(probe_import, probe)
    if engine_version is not None:
        feed.require(re.fullmatch(launcher.SEMVER, engine_version), 'Invalid engine version')
        raw = re.sub(rb"(?m)^ENGINE_VERSION = '[^']+'$", ('ENGINE_VERSION = ' + repr(engine_version)).encode(), raw)
    marker = b'EMBEDDED_MAINTENANCE_PAGE = None'
    if marker not in raw:
        return raw  # Alternate/synthetic engines need not contain this component.
    canonical = source / 'maintenance.html'
    if not canonical.is_file(): canonical = source.parents[1] / 'assets/maintenance.html'
    page = canonical.read_text()
    feed.require(raw.count(marker) == 1, 'Missing canonical maintenance embedding point')
    return raw.replace(marker, ('EMBEDDED_MAINTENANCE_PAGE = ' + repr(page)).encode())


def build(source, version, key, output):
    feed.require(re.fullmatch(launcher.SEMVER, version), 'Invalid engine version')
    feed.require(key.is_file() and not key.is_symlink() and key.stat().st_mode & 0o077 == 0,
                 'Root-code signing key must be private')
    files = {name: coordinator_source(source, version) if name == 'update_coordinator.py' else (source / name).read_bytes() for name in sorted(launcher.FILES)}
    for name, raw in files.items():
        feed.require(0 < len(raw) <= launcher.MAX_BUNDLE, 'Invalid engine file size')
        compile(raw, name, 'exec')
    # Version/contract is data in the signed manifest; runtime self-check must agree.
    value = dict(format=1, version=version, state_format=launcher.STATE_FORMAT,
                 files={name: hashlib.sha256(raw).hexdigest() for name, raw in files.items()})
    files['bundle.json'] = (json.dumps(value, sort_keys=True, separators=(',', ':')) + '\n').encode()
    return write_archive(files, f'talocart-updater-{version}.tar.gz', version, key, output)


def write_archive(files, name, version, key, output, *, limit=launcher.MAX_BUNDLE):
    archive = io.BytesIO()
    with gzip.GzipFile(fileobj=archive, mode='wb', filename='', mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode='w', format=tarfile.USTAR_FORMAT) as bundle:
            for filename, raw in sorted(files.items()):
                info = tarfile.TarInfo(filename)
                info.size, info.mode, info.mtime = len(raw), 0o644, 0
                bundle.addfile(info, io.BytesIO(raw))
    raw = archive.getvalue()
    feed.require(len(raw) <= limit, 'Privileged archive too large')
    signature = feed.sign(raw, key)
    output.mkdir(mode=0o700)
    (output / name).write_bytes(raw)
    (output / (name + '.sig')).write_bytes(signature)
    return dict(version=version, asset=name, size=len(raw), sha256=hashlib.sha256(raw).hexdigest(),
                signature_asset=name + '.sig')


def build_bootstrap(repository, version, commit, tag, key, output):
    """Signed administrative kit; deliberately incompatible with launcher bundles."""
    feed.require(version == launcher.VERSION and re.fullmatch(r'[0-9a-f]{40}',commit)
                 and re.fullmatch('v'+feed.VERSION,tag), 'Invalid administrative provenance')
    feed.require(key.is_file() and not key.is_symlink() and key.stat().st_mode&0o077 == 0,'Root signing key must be private')
    scripts=repository/'scripts/release'
    files={name:coordinator_source(scripts) if name == 'update_coordinator.py' else (scripts/name).read_bytes() for name in ('bootstrap_updater.py','update_launcher.py','update_engine.py','update_coordinator.py')}
    for name,raw in files.items():
        feed.require(0 < len(raw) <= launcher.MAX_BUNDLE,'Invalid administrative member')
        compile(raw,name,'exec')
    files['update-unified.toml']=(repository/'distribution/update-unified.toml').read_bytes()
    files['README.md']=(repository/'distribution/bootstrap-README.md').read_text().replace('{{source_tag}}',tag).replace('{{source_commit}}',commit).encode()
    value=dict(format=1,version=tag[1:],launcher_version=version,source_commit=commit,source_tag=tag,
               files={name:hashlib.sha256(raw).hexdigest() for name,raw in files.items()})
    files['bootstrap.json']=(json.dumps(value,sort_keys=True,separators=(',',':'))+'\n').encode()
    return write_archive(files,f'talocart-bootstrap-{tag[1:]}.tar.gz',tag[1:],key,output)


def build_installation(repository, commit, tag, target, artifacts, public_key, root_public_key, key, output):
    """One separately root-signed first-install package; no deployment or secrets."""
    feed.require(key.is_file() and not key.is_symlink() and key.stat().st_mode & 0o077 == 0,
                 'Root-code signing key must be private')
    import distribution
    import update_engine
    feed.require(public_identity(root_public_key) != public_identity(public_key), 'Use distinct signing roots')
    actual = feed.command(['openssl', 'pkey', '-in', key, '-pubout', '-outform', 'DER'])
    feed.require(actual == public_identity(root_public_key), 'Wrong root signing key')
    raw = (artifacts/'distribution.json').read_bytes()
    decoded = distribution.decode(raw)
    value = distribution.verify(raw, (artifacts/'distribution.sig').read_bytes(), public_key, decoded['repository'])
    feed.require(feed.newer_or_equal(launcher.VERSION, value['minimum']['launcher']), 'Installer launcher is too old')
    selected = [p for p in value['platforms'] if p['target'] == target]
    feed.require(len(selected) == 1, 'No installation target')
    platform = selected[0]
    base, engine = platform['base'], platform['updater']
    feed.require(value['release']['id'] == tag and value['release']['commit'] == commit
                 and base['version'] == tag[1:] and base['commit'] == commit and not platform['modules'],
                 'First installation requires this base and no optional modules')
    distribution.check_artifacts(value, artifacts)
    verify(artifacts/engine['asset'], artifacts/engine['signature_asset'], root_public_key, engine['version'])
    import tempfile
    with tempfile.TemporaryDirectory(prefix='talocart-install-check-') as temporary:
        metadata = update_engine.extract(artifacts/base['asset'], Path(temporary), base['version'], base['commit'], target)
        feed.require(metadata['build']['tagged'] and not metadata['source_dirty'], 'Use a tagged base')
        feed.require(all(metadata['build'].get(k) == v for k, v in base['contracts'].items()), 'Base contracts differ')
    names = ('install_server.py', 'install_guard.py', 'bootstrap_updater.py', 'update_launcher.py', 'update_engine.py',
             'update_coordinator.py', 'distribution.py', 'updates_feed.py', 'updater_bundle.py')
    files = {name: coordinator_source(repository/'scripts/release') if name == 'update_coordinator.py' else (repository/'scripts/release'/name).read_bytes() for name in names}
    for name, raw in files.items(): compile(raw, name, 'exec')
    for name in ('distribution.json', 'distribution.sig', base['asset'], engine['asset'], engine['signature_asset']):
        files[name] = (artifacts/name).read_bytes()
    files['update-unified.toml'] = (repository/'distribution/update-unified.toml').read_bytes()
    files['README.md'] = (repository/'distribution/install-README.md').read_bytes()
    files['installation.json'] = (json.dumps(dict(format=1, version=base['version'], source_commit=commit,
        target=target, files={n: hashlib.sha256(raw).hexdigest() for n, raw in files.items()}), sort_keys=True)+'\n').encode()
    return write_archive(files, f"talocart-install-{tag[1:]}-{target}.tar.gz", tag[1:], key, output,
                         limit=512*1024**2)


def verify(archive, signature, public_key, expected_version):
    """Unprivileged byte/structure verification; never executes downloaded code."""
    feed.require(archive.is_file() and not archive.is_symlink() and archive.stat().st_size <= launcher.MAX_BUNDLE,
                 'Invalid updater archive')
    raw = archive.read_bytes()
    feed.require(signature.is_file() and not signature.is_symlink() and signature.stat().st_size == 64,'Invalid updater signature input')
    with signature.open('rb') as source: signed = source.read(65)
    feed.verify_signature(raw, signed, public_key, launcher.MAX_BUNDLE)
    files, total = {}, 0
    with tarfile.open(fileobj=io.BytesIO(raw), mode='r:gz') as bundle:
        for member in bundle:
            total += member.size
            feed.require(member.name in launcher.FILES | {'bundle.json'} and member.name not in files
                         and member.isfile() and not member.pax_headers and 0 < member.size <= launcher.MAX_BUNDLE
                         and total <= launcher.MAX_BUNDLE, 'Unsafe updater archive')
            files[member.name] = bundle.extractfile(member).read(launcher.MAX_BUNDLE + 1)
    feed.require(set(files) == launcher.FILES | {'bundle.json'}, 'Incomplete updater bundle')
    feed.require(len(files['bundle.json']) <= 8192,'Updater manifest exceeds launcher limit')
    value = launcher.manifest(files['bundle.json'], expected_version)
    for name, identity in value['files'].items():
        feed.require(hashlib.sha256(files[name]).hexdigest() == identity, 'Updater member changed')
        compile(files[name], name, 'exec')
    return value


def public_identity(path):
    raw = feed.command(['openssl','pkey','-pubin','-in',path,'-pubout','-outform','DER'])
    feed.require(len(raw) == 44 and raw[:12].hex() == '302a300506032b6570032100', 'Ed25519 public key required')
    return raw

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument('--version', required=True)
    parser.add_argument('--key', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--installation', action='store_true', help='Build one root-signed first-installation package')
    parser.add_argument('--artifacts', type=Path)
    parser.add_argument('--public-key', type=Path)
    parser.add_argument('--root-public-key', type=Path)
    parser.add_argument('--target', choices=feed.TARGETS)
    parser.add_argument('--bootstrap',action='store_true',help='Administrative kit from reviewed clean main/tag; never an engine bundle')
    parser.add_argument('--verified',action='store_true')
    args = parser.parse_args()
    try:
        if args.bootstrap or args.installation:
            feed.require(not (args.bootstrap and args.installation), 'Select one package kind')
            import version as release_version
            feed.require(args.verified,'Complete local qualification and pass --verified')
            feed.require(args.source.resolve() == Path(__file__).resolve().parent,'Administrative kit must use this reviewed checkout')
            selected=release_version.check(release=True)
            feed.require(release_version.git(release_version.ROOT,'branch','--show-current') == 'main','Build administrative kit from main')
            if args.installation:
                feed.require(args.version == launcher.VERSION and all((args.artifacts, args.public_key, args.root_public_key, args.target)), 'Installation inputs required; launcher version must match')
                record=build_installation(release_version.ROOT,release_version.git(release_version.ROOT,'rev-parse','HEAD'),
                    'v'+selected,args.target,args.artifacts,args.public_key,args.root_public_key,args.key,args.output)
            else:
                record=build_bootstrap(release_version.ROOT,args.version,release_version.git(release_version.ROOT,'rev-parse','HEAD'),'v'+selected,args.key,args.output)
        else:
            record=build(args.source,args.version,args.key,args.output)
        print(json.dumps(record))
    except (OSError, ValueError, SyntaxError, KeyError, TypeError):
        parser.exit(1, 'Engine bundle preparation failed; nothing was installed or published.\n')


if __name__ == '__main__':
    main()
