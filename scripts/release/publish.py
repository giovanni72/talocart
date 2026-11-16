#!/usr/bin/env python3
"""Local candidate upload and explicit update approval; never builds or deploys.

Requires gh authentication, OpenSSL 3, clean annotated main release and packages
built/tested locally. Candidates are GitHub draft releases, invisible to updaters.
"""
import argparse
import json
from pathlib import Path
import re
import tempfile
import tarfile
import subprocess

import sys

# Packaging/checking must not create cache files in the reviewed source tree.
sys.dont_write_bytecode = True

import updates_feed as feed
import version
import update_engine
import distribution
import updater_bundle


def remote_tag(repository, tag, commit):
    ref = json.loads(feed.command(['gh', 'api', f'repos/{repository}/git/ref/tags/{tag}']))
    feed.require(ref['object']['type'] == 'tag', 'Remote release tag must be annotated')
    annotated = json.loads(feed.command(['gh', 'api', f"repos/{repository}/git/tags/{ref['object']['sha']}"]))
    feed.require(annotated['object']['type'] == 'commit' and annotated['object']['sha'] == commit,
                 'Remote tag does not match the verified build commit')


def read_bounded(path, limit):
    feed.require(path.is_file() and not path.is_symlink() and path.stat().st_size <= limit, 'Invalid publication input')
    with path.open('rb') as source: raw = source.read(limit + 1)
    feed.require(len(raw) <= limit, 'Publication input exceeds limit')
    return raw


def artifact_records(value):
    records = {}
    for platform in value['platforms']:
        for item in [platform['base'], *platform['modules'], platform['updater']]:
            records[item['asset']] = item['size']
        records[platform['updater']['signature_asset']] = 64
    return records


def check_distribution_artifacts(value, folder, root_key, *, current=None):
    distribution.check_artifacts(value,folder)
    for platform in value['platforms']:
        base=platform['base']
        with tempfile.TemporaryDirectory(prefix='talocart-distribution-check-') as temporary:
            data=update_engine.extract(folder/base['asset'],Path(temporary),base['version'],base['commit'],platform['target'])
        build=data['build']
        feed.require(all(build.get(k) == v for k,v in base['contracts'].items()), 'Base contracts do not match signed distribution')
        if current is not None and base['version'] == current[0]:
            feed.require(base['commit'] == current[1] and build.get('source_fingerprint') == current[2], 'Current base was not built from this reviewed tag')
        updater=platform['updater']
        updater_bundle.verify(folder/updater['asset'],folder/updater['signature_asset'],root_key,updater['version'])


def candidate_distribution(repository, manifest, artifacts, key, root_key, verified):
    feed.require(verified, 'Complete local release checks and pass --verified')
    selected=version.check(release=True)
    feed.require(version.git(version.ROOT,'branch','--show-current') == 'main','Publish from reviewed main')
    commit=version.git(version.ROOT,'rev-parse','HEAD')
    value=distribution.validate(distribution.decode(read_bounded(manifest,distribution.LIMIT)),repository)
    feed.require(value['release']['id'] == 'v'+selected and value['release']['commit'] == commit,'Distribution does not match this reviewed tag')
    feed.require(key.is_file() and not key.is_symlink() and key.stat().st_mode&0o077 == 0,'Signing key must be private')
    remote_tag(repository,value['release']['id'],commit)
    for base in { (p['base']['version'],p['base']['commit']) for p in value['platforms'] }:
        remote_tag(repository,'v'+base[0],base[1])
    with tempfile.TemporaryDirectory(prefix='talocart-distribution-publish-') as temporary:
        folder=Path(temporary)
        public=folder/'public.pem'; public.write_bytes(feed.command(['openssl','pkey','-in',key,'-pubout']))
        feed.require(updater_bundle.public_identity(public) != updater_bundle.public_identity(root_key),'Root code and application signing keys must differ')
        check_distribution_artifacts(value,artifacts,root_key,current=(selected,commit,version.fingerprint()))
        raw=(json.dumps(value,sort_keys=True,separators=(',',':'))+'\n').encode()
        (folder/'distribution.json').write_bytes(raw); (folder/'distribution.sig').write_bytes(feed.sign(raw,key))
        distribution.verify(raw,(folder/'distribution.sig').read_bytes(),public,repository)
        extras=[folder/'distribution.json',folder/'distribution.sig']
        # gh owns one atomic draft creation. No replacement/clobber of an existing release.
        feed.command(['gh','release','create',value['release']['id'],'--repo',repository,'--verify-tag','--draft',
                      '--title','Talocart '+selected,'--notes','Locally verified distribution; update approval pending.',
                      *[artifacts/name for name in sorted(artifact_records(value))],*extras])
    print('Distribution draft uploaded. No server update is authorized until approval and owner confirmation.')


def download_draft(repository,tag,folder,names):
    argv=['gh','release','download',tag,'--repo',repository,'--dir',folder]
    for name in sorted(names): argv += ['--pattern',name]
    feed.command(argv)


def approve_distribution(repository,tag,public_key,root_key):
    feed.require(re.fullmatch('v'+feed.VERSION,tag),'Expected vX.Y.Z')
    feed.require(updater_bundle.public_identity(public_key) != updater_bundle.public_identity(root_key),'Root code and application signing keys must differ')
    release=json.loads(feed.command(['gh','release','view',tag,'--repo',repository,'--json','isDraft,isPrerelease,tagName,assets']))
    feed.require(release['isDraft'] is True and release['isPrerelease'] is False and release['tagName'] == tag,'Approval requires an unpublished stable draft')
    assets=release['assets']
    feed.require(isinstance(assets,list) and len(assets)<=256 and all(isinstance(a,dict) and isinstance(a.get('name'),str)
                 and type(a.get('size')) is int for a in assets),'Invalid draft asset inventory')
    sizes={a['name']:a['size'] for a in assets}
    feed.require(len(sizes)==len(assets) and 0 < sizes.get('distribution.json',0)<=distribution.LIMIT
                 and sizes.get('distribution.sig')==64,'Invalid distribution metadata assets')
    with tempfile.TemporaryDirectory(prefix='talocart-distribution-approve-') as temporary:
        folder=Path(temporary)
        download_draft(repository,tag,folder,['distribution.json','distribution.sig'])
        value=distribution.verify(read_bounded(folder/'distribution.json',distribution.LIMIT),read_bounded(folder/'distribution.sig',64),public_key,repository)
        feed.require(value['release']['id']==tag,'Draft/tag mismatch')
        remote_tag(repository,tag,value['release']['commit'])
        expected=artifact_records(value)
        feed.require(all(sizes.get(name)==size for name,size in expected.items()),'Draft component length changed')
        download_draft(repository,tag,folder,expected)
        check_distribution_artifacts(value,folder,root_key)
        for base in {(p['base']['version'],p['base']['commit']) for p in value['platforms']}:
            remote_tag(repository,'v'+base[0],base[1])
        feed.require('update.json' not in sizes and 'update.sig' not in sizes,
                     'Format-1 bridge assets are not supported in a unified distribution')
    feed.command(['gh','release','edit',tag,'--repo',repository,'--draft=false','--prerelease=false','--latest'])
    print('Distribution approved. Each installed server still requires its owner-confirmed local plan.')

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='operation', required=True)
    unified = sub.add_parser('candidate-unified')
    unified.add_argument('--repo',required=True)
    unified.add_argument('--manifest',type=Path,required=True)
    unified.add_argument('--artifacts',type=Path,required=True)
    unified.add_argument('--key',type=Path,required=True)
    unified.add_argument('--root-public-key',type=Path,required=True)
    unified.add_argument('--verified',action='store_true')
    promote = sub.add_parser('approve-unified')
    promote.add_argument('--repo',required=True)
    promote.add_argument('--tag',required=True)
    promote.add_argument('--public-key',type=Path,required=True)
    promote.add_argument('--root-public-key',type=Path,required=True)
    args = parser.parse_args()
    try:
        feed.require(re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', args.repo), 'Invalid repository')
        if args.operation == 'candidate-unified':
            candidate_distribution(args.repo,args.manifest,args.artifacts,args.key.resolve(),args.root_public_key.resolve(),args.verified)
        else:
            approve_distribution(args.repo,args.tag,args.public_key.resolve(),args.root_public_key.resolve())
    except (OSError, ValueError, TypeError, KeyError, tarfile.TarError, subprocess.SubprocessError, SyntaxError, EOFError, RecursionError, update_engine.DeployError):
        parser.exit(1, 'Release operation failed. Nothing was deployed; inspect the draft before retrying.\n')


if __name__ == '__main__':
    main()
