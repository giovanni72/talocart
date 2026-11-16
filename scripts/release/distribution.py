#!/usr/bin/env python3
"""Format-2 distribution metadata. Local preparation/inspection only; never deploys."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

import updates_feed as feed

LIMIT = 65536
HEX = r'[0-9a-f]{64}'
ID = r'[a-z][a-z0-9-]{0,47}'


def fields(value, names):
    feed.require(isinstance(value, dict) and set(value) == set(names.split()), 'Invalid metadata fields')


def matches(value, pattern):
    return isinstance(value, str) and re.fullmatch(pattern, value) is not None


def positive(value, maximum):
    return type(value) is int and 0 < value <= maximum


def version(value):
    feed.require(matches(value, feed.VERSION), 'Invalid stable version')


def artifact(value, names=''):
    fields(value, 'asset size sha256 ' + names)
    feed.require(matches(value['asset'], r'[A-Za-z0-9][A-Za-z0-9_.-]{0,159}')
                 and '..' not in value['asset'] and matches(value['sha256'], HEX)
                 and positive(value['size'], 2 * 1024**3), 'Invalid artifact')


def validate(value, repository):
    fields(value, 'format channel repository release minimum platforms')
    feed.require(type(value['format']) is int and value['format'] == 2
                 and value['channel'] == 'stable' and value['repository'] == repository
                 and matches(repository, r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+'), 'Invalid distribution identity')
    release = value['release']
    fields(release, 'id sequence commit')
    feed.require(matches(release['id'], 'v' + feed.VERSION)
                 and positive(release['sequence'], 2**53 - 1)
                 and matches(release['commit'], r'[0-9a-f]{40}'), 'Invalid distribution release')
    fields(value['minimum'], 'launcher engine')
    for item in value['minimum'].values():
        version(item)
    platforms = value['platforms']
    feed.require(isinstance(platforms, list) and 1 <= len(platforms) <= len(feed.TARGETS), 'Invalid platforms')
    targets, assets = set(), {}
    for platform in platforms:
        fields(platform, 'target base modules updater')
        target = platform['target']
        feed.require(target in feed.TARGETS and target not in targets, 'Duplicate or invalid target')
        targets.add(target)
        base = platform['base']
        artifact(base, 'version commit contracts')
        version(base['version'])
        feed.require(matches(base['commit'], r'[0-9a-f]{40}')
                     and base['asset'] == f"talocart-{base['version']}-{target}.tar.gz", 'Invalid base identity')
        contracts = base['contracts']
        fields(contracts, 'sdk_version protocol database')
        version(contracts['sdk_version'])
        feed.require(positive(contracts['protocol'], 2**32-1), 'Invalid protocol')
        fields(contracts['database'], 'version migrations sha256')
        database = contracts['database']
        feed.require(positive(database['version'], 2**32-1) and positive(database['migrations'], 10000)
                     and matches(database['sha256'], HEX), 'Invalid database checkpoint')
        modules = platform['modules']
        feed.require(isinstance(modules, list) and len(modules) <= 32, 'Invalid module catalogue')
        ids = set()
        for module in modules:
            artifact(module, 'id version' + (' publisher' if isinstance(module, dict) and 'publisher' in module else ''))
            version(module['version'])
            feed.require(matches(module['id'], ID) and module['id'] != 'base' and module['id'] not in ids
                         and (module.get('publisher', '') == '' or matches(module['publisher'], HEX))
                         and module['asset'].endswith('.talopkg')
                         and module['size'] <= 128 * 1024**2 + 32768 + 77, 'Invalid module')
            ids.add(module['id'])
        updater = platform['updater']
        artifact(updater, 'version signature_asset')
        version(updater['version'])
        feed.require(tuple(map(int, updater['version'].split('.'))) >=
                     tuple(map(int, value['minimum']['engine'].split('.'))), 'Updater below minimum engine')
        feed.require(updater['signature_asset'] == updater['asset'] + '.sig'
                     and len(updater['signature_asset']) <= 164, 'Invalid updater signature asset')
        for item in [base, *modules]:
            feed.require(item['asset'] not in assets, 'Duplicate application asset')
            assets[item['asset']] = ('application', item)
        # The Python engine is architecture-independent. Shared names are valid
        # only for the exact same updater record and its detached signature.
        for name, kind in [(updater['asset'], 'updater'), (updater['signature_asset'], 'signature')]:
            expected = (kind, updater)
            feed.require(name not in assets or assets[name] == expected, 'Conflicting shared updater asset')
            assets[name] = expected
    return value


def no_duplicates(pairs):
    value = {}
    for name, item in pairs:
        feed.require(name not in value, 'Duplicate JSON field')
        value[name] = item
    return value


def decode(raw):
    feed.require(0 < len(raw) <= LIMIT, 'Distribution exceeds limit')
    return json.loads(raw, object_pairs_hook=no_duplicates,
                      parse_constant=lambda _: feed.require(False, 'Invalid JSON constant'))


def verify(raw, signature, key, repository):
    # Verify the signed unified distribution before accepting its contents.
    feed.verify_signature(raw, signature, key, LIMIT)
    return validate(decode(raw), repository)


def check_sequence(value, digest, previous):
    """Caller supplies trusted root-owned state; same exact release may be retried."""
    feed.require(matches(digest, HEX), 'Invalid manifest digest')
    fields(previous, 'sequence digest')
    feed.require(type(previous['sequence']) is int and 0 <= previous['sequence'] <= 2**53-1
                 and matches(previous['digest'], HEX), 'Invalid sequence state')
    sequence = value['release']['sequence']
    feed.require(sequence > previous['sequence'] or
                 sequence == previous['sequence'] and digest == previous['digest'], 'Release replay or equivocation')


def artifacts(value):
    for platform in value['platforms']:
        yield platform['base']
        yield from platform['modules']
        yield platform['updater']


def check_artifacts(value, folder):
    """Checks bytes only; module content/target contracts belong to the target Rust host.

    The updater signature must separately be checked against the privileged key by
    the launcher. A release signature does not authorize execution of root code.
    """
    for item in artifacts(value):
        path = folder / item['asset']
        feed.require(path.is_file() and not path.is_symlink() and path.stat().st_size == item['size']
                     and feed.digest(path) == item['sha256'], 'Artifact length or digest mismatch')
    for platform in value['platforms']:
        sig = folder / platform['updater']['signature_asset']
        feed.require(sig.is_file() and not sig.is_symlink() and sig.stat().st_size == 64,
                     'Missing privileged updater signature')


def approved(github, public_key, repository):
    release = github.latest()
    raw = github.asset(release, 'distribution.json', LIMIT)
    signature = github.asset(release, 'distribution.sig', 64)
    value = verify(raw, signature, public_key, repository)
    feed.require(release.get('draft') is False and release.get('prerelease') is False
                 and release.get('tag_name') == value['release']['id'], 'Distribution is not approved')
    return release, value, hashlib.sha256(raw).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='operation', required=True)
    inspect = sub.add_parser('verify')
    inspect.add_argument('--manifest', type=Path, required=True)
    inspect.add_argument('--signature', type=Path, required=True)
    inspect.add_argument('--public-key', type=Path, required=True)
    prepare = sub.add_parser('sign')
    prepare.add_argument('--manifest', type=Path, required=True)
    prepare.add_argument('--key', type=Path, required=True)
    prepare.add_argument('--artifacts', type=Path, required=True)
    prepare.add_argument('--output', type=Path, required=True)
    prepare.add_argument('--verified', action='store_true')
    for cmd in [inspect, prepare]:
        cmd.add_argument('--repo', required=True)
    args = parser.parse_args()
    try:
        # Bounded reads even for local inputs.
        with args.manifest.open('rb') as stream:
            raw = stream.read(LIMIT + 1)
        if args.operation == 'verify':
            with args.signature.open('rb') as stream:
                signature = stream.read(65)
            value = verify(raw, signature, args.public_key, args.repo)
        else:
            feed.require(args.verified, 'Complete local checks and pass --verified')
            feed.require(args.key.is_file() and not args.key.is_symlink()
                         and args.key.stat().st_mode & 0o077 == 0, 'Signing key must be private')
            value = validate(decode(raw), args.repo)
            check_artifacts(value, args.artifacts)
            raw = (json.dumps(value, sort_keys=True, separators=(',', ':')) + '\n').encode()
            signature = feed.sign(raw, args.key)
            with tempfile.TemporaryDirectory() as temporary:
                public = Path(temporary) / 'public.pem'
                public.write_bytes(feed.command(['openssl', 'pkey', '-in', args.key, '-pubout']))
                verify(raw, signature, public, args.repo)
            # A new output directory prevents overwriting an earlier signed candidate.
            args.output.mkdir(mode=0o700)
            (args.output / 'distribution.json').write_bytes(raw)
            (args.output / 'distribution.sig').write_bytes(signature)
        print(json.dumps({'release': value['release'], 'digest': hashlib.sha256(raw).hexdigest()}))
    except (OSError, ValueError, TypeError, KeyError, RecursionError, subprocess.SubprocessError):
        parser.exit(1, 'Distribution preparation/verification failed; nothing was published or deployed.\n')


if __name__ == '__main__':
    main()
