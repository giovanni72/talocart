"""Pure signed-distribution/native-report binding; no host operations."""
import hashlib
import json
import re

def require(value, message):
    if not value:
        raise ValueError(message)


def canonical(value):
    return (json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True) + '\n').encode()


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def create_plan(manifest, manifest_digest, platform, native, configuration_digest, current_base):
    """Bind target validation and actual ledgers; unchecked proposals remain blocked."""
    require(isinstance(native, dict) and set(native) == {
        'format', 'inventory_sha256', 'host_version', 'host_commit', 'sdk_version', 'protocol',
        'database', 'database_checked', 'database_sha256', 'modules', 'ignored'}, 'Invalid native planning fields')
    require(isinstance(native['modules'], list) and len(native['modules']) <= 32
            and isinstance(native['ignored'], list) and len(native['ignored']) <= 32, 'Invalid native planning lists')
    require(all(isinstance(v, str) and re.fullmatch(r'[0-9a-f]{64}', v)
                for v in (manifest_digest, configuration_digest, native['inventory_sha256'])),
            'Invalid planning fingerprint')
    require(platform in manifest['platforms'] and type(native['format']) is int and native['format'] == 1
            and type(native['database_checked']) is bool, 'Invalid target planning report')
    database_digest = native['database_sha256']
    require((native['database_checked'] and isinstance(database_digest, str) and re.fullmatch(r'[0-9a-f]{64}', database_digest))
            or (not native['database_checked'] and database_digest is None), 'Invalid ledger attestation')
    base = platform['base']
    require(native['host_version'] == base['version'] and native['host_commit'] == base['commit']
            and all(canonical(native[k]) == canonical(base['contracts'][k]) for k in ('database', 'sdk_version', 'protocol')),
            'Target binary does not match signed base')
    # Do not downgrade even when distribution sequence itself is increasing.
    require(tuple(map(int, base['version'].split('.'))) >= tuple(map(int, current_base['version'].split('.'))),
            'Base downgrade is not supported')
    if base['version'] == current_base['version']:
        require(base['commit'] == current_base['commit'], 'Same base version with different provenance')
    catalogue = {m['id']: m for m in platform['modules']}
    ids = set()
    modules = []
    for item in native['modules']:
        require(isinstance(item, dict) and set(item) == {'id', 'from', 'to', 'enabled', 'publisher', 'sha256', 'action', 'blocker', 'package_sha256'},
                'Invalid module planning fields')
        require(isinstance(item['id'], str) and re.fullmatch(r'[a-z][a-z0-9-]{0,47}', item['id'])
                and all(isinstance(item[k], str) and re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', item[k]) for k in ('from', 'to'))
                and isinstance(item['sha256'], str) and re.fullmatch(r'[0-9a-f]{64}', item['sha256'])
                and (item['publisher'] == '' or isinstance(item['publisher'], str) and re.fullmatch(r'[0-9a-f]{64}', item['publisher']))
                and (item['blocker'] in ('incomplete', 'incompatible', 'publisher', 'version', 'schema', 'conflict') if item['action'] == 'blocked' else item['blocker'] is None)
                and (item['package_sha256'] is None or isinstance(item['package_sha256'], str) and re.fullmatch(r'[0-9a-f]{64}', item['package_sha256'])),
                'Invalid module planning values')
        require(item['id'] not in ids and type(item['enabled']) is bool
                and item['action'] in ('keep', 'update', 'blocked'), 'Invalid module plan')
        ids.add(item['id'])
        if item['action'] == 'update':
            available = catalogue.get(item['id'])
            require(available is not None and available['version'] == item['to']
                    and available['sha256'] == item['package_sha256'], 'Module is outside signed release')
        if item['action'] == 'keep' and item['id'] in catalogue:
            available = catalogue[item['id']]
            require(available['version'] == item['to'],
                    'Kept module disagrees with signed catalogue')
        modules.append(dict(item))
    modules.sort(key=lambda m: m['id'])
    plan = dict(format=1, release=manifest['release'], manifest_sha256=manifest_digest,
                target=platform['target'], configuration_sha256=configuration_digest,
                inventory_sha256=native['inventory_sha256'], database_sha256=database_digest, current_base=current_base,
                base=base, modules=modules, updater=platform['updater'],
                blocked=not native['database_checked'] or any(m['action'] == 'blocked' for m in modules))
    return plan, digest(plan)
