#!/usr/bin/env python3
"""Serial test groups with explicit selection, timing and opt-in evidence reuse."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
import uuid
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
PROFILES = {'rust', 'tooling', 'postgres', 'browser', 'packages', 'full'}


class UsageError(ValueError):
    """Safe, actionable diagnostics containing no environment values."""


def catalog(root):
    catalog_path = Path(os.environ.get('TALOCART_TEST_CATALOG', root / 'scripts/test-suite.json'))
    data = json.loads(catalog_path.read_text())
    if 'extends' in data:
        inherited = json.loads((catalog_path.parent / data['extends']).read_text())
        overrides = data.get('overrides', {})
        data['groups'] = [dict(g, **overrides.get(g['id'], {})) for g in inherited['groups']] + data['groups']
    if data['version'] != 1:
        raise ValueError('Unsupported catalog version')
    groups = data['groups']
    ids = [g['id'] for g in groups]
    if len(set(ids)) != len(ids) or any(not re.fullmatch(r'[a-z0-9-]+', i) for i in ids):
        raise ValueError('Duplicate or invalid group ID')
    for group in groups:
        if group['suite'] not in PROFILES - {'full'} or not group['command']:
            raise ValueError('Invalid group')
        for binary in group.get('binaries', []):
            if (not isinstance(binary, dict) or set(binary) != {'env', 'default'}
                    or not isinstance(binary['env'], str)
                    or not re.fullmatch(r'TALOCART_(?:TEST_.*|MODULE_TEST_BINARY)', binary['env'])
                    or not isinstance(binary['default'], str) or not binary['default']):
                raise ValueError('Invalid binary prerequisite')
        if group.get('parent'):
            parent = next((g for g in groups if g['id'] == group['parent']), None)
            if not parent or parent.get('parent') or parent['suite'] != group['suite']:
                raise ValueError('Invalid targeted group parent')
    return groups


def select(groups, suites, only):
    if set(suites) - PROFILES or set(only) - {g['id'] for g in groups}:
        raise UsageError('Unknown suite or group; use --list')
    candidates = [g for g in groups if not suites or 'full' in suites or g['suite'] in suites]
    if only:
        if set(only) - {g['id'] for g in candidates}:
            raise UsageError('Selected group is outside the requested suite; use --list')
        chosen = [g for g in candidates if g['id'] in only]
        if any(g.get('parent') in only for g in chosen):
            raise UsageError('Choose aggregate or targeted groups, not both; inspect --dry-run')
        return chosen
    # Targeted alternatives never duplicate work in an aggregate suite or CI.
    return [g for g in candidates if not g.get('parent')] if suites else []


def child_environment(source, group):
    paths = {'TALOCART_BASE_TEST_BINARY', 'TALOCART_MODULE_TEST_BINARY', 'TALOCART_FIXTURE_DIR'}
    # Ambient *_ONLY and *_TEST_CASE flags must never silently narrow a catalog group.
    env = {k: v for k, v in source.items() if not k.startswith('TALOCART_') or
           k.startswith(('TALOCART_TEST_', 'TALOCART_BROWSER_')) or k in paths}
    env.pop('TALOCART_TEST_CATALOG', None)
    env.update(group.get('env', {}))
    env.setdefault('TALOCART_TEST_BINARY', 'target/debug/talocart')
    return env


def host_binary(group, env):
    """Match each script's host selection for both preflight and evidence reuse."""
    if group['id'] == 'runtime-recovery':
        return (env.get('TALOCART_BASE_TEST_BINARY') or env.get('TALOCART_TEST_BINARY')
                or 'target/debug/talocart')
    if group['suite'] == 'browser':
        return env.get('TALOCART_TEST_BINARY', 'target/debug/talocart')
    if group['suite'] == 'packages':
        return env.get('TALOCART_BASE_TEST_BINARY', 'target/debug/talocart')
    return None


def declared_binaries(group, env):
    return [env.get(item['env']) or item['default'] for item in group.get('binaries', [])]


def preflight(root, groups, source):
    for group in groups:
        if not shutil.which(group['command'][0]):
            raise UsageError('Missing command prerequisite; inspect --dry-run')
        env = child_environment(source, group)
        if any(not os.access(root / binary, os.X_OK) for binary in declared_binaries(group, env)):
            raise UsageError('Missing declared binary prerequisite; inspect --dry-run and build first')
    database = any(g['suite'] in {'browser', 'postgres', 'packages'} for g in groups)
    if not database:
        return
    env = child_environment(source, {})
    url = urlsplit(env.get('TALOCART_TEST_DATABASE_URL', ''))
    if url.scheme not in {'postgres', 'postgresql'} or url.hostname not in {'localhost', '127.0.0.1', '::1'} or not url.path.endswith('_test'):
        raise UsageError('Dedicated loopback _test database required; use scripts/test-env.py')
    for group in groups:
        env = child_environment(source, group)
        binary = host_binary(group, env)
        if binary and not os.access(root / binary, os.X_OK):
            raise UsageError('Missing test binary; build once before running browser/package groups')
        if group['id'] == 'package-updates':
            fixtures = root / env.get('TALOCART_FIXTURE_DIR', 'target/debug')
            if any(not os.access(fixtures / name, os.X_OK) for name in ['fixture-v1', 'fixture-v2', 'fixture-v3-fixed', 'fixture-v3-failing']):
                raise UsageError('Build talocart-package-fixture --bins first')
    # Check the entire selection before paying for any Rust builds or long test groups.
    browser = any(g['suite'] in {'browser', 'packages', 'postgres'}
                  and g.get('requires_browser', True) for g in groups)
    probe = """(async()=>{const {Client}=require('pg');
      const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL,connectionTimeoutMillis:5000});
      try{await db.connect();await db.query('SELECT 1');}finally{await db.end();}
    """
    if browser:
        probe += """const {chromium}=require('playwright');
          const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
          await browser.close();"""
    probe += "})().catch(()=>{process.exitCode=1;});"
    try:
        result = subprocess.run(['node', '-e', probe], cwd=root, env=env, capture_output=True, timeout=20)
    except (OSError, subprocess.TimeoutExpired):
        raise UsageError('Database/browser preflight failed; check npm dependencies and test services') from None
    if result.returncode:
        raise UsageError('Database/browser preflight failed; check npm dependencies, test services and browser channel')


def js_inputs(root, entry):
    pending = [entry]; found = set()
    while pending:
        name = pending.pop()
        if name in found:
            continue
        found.add(name)
        path = root / name
        if not path.is_file():
            continue
        for relative in re.findall(r"require\(['\"](\.[^'\"]+)['\"]\)", path.read_text()):
            dependency = path.parent / relative
            if not dependency.suffix:
                dependency = dependency.with_suffix('.js')
            pending.append(os.path.relpath(dependency.resolve(), root.resolve()))
    return found


def hash_files(root, files):
    digest = hashlib.sha256()
    for name in sorted(files):
        path = Path(name) if Path(name).is_absolute() else root / name
        digest.update(name.encode()); digest.update(b'\0')
        if not path.is_file():
            digest.update(b'missing\0')
            continue
        with path.open('rb') as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(block)
        digest.update(b'\0')
    return digest.hexdigest()


def source_files(root):
    # An exported profile has no .git, even when nested inside another checkout.
    # Never use that ancestor's inventory or include generated/runtime contents.
    if (root / '.git').exists():
        return subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'], cwd=root).decode().split('\0')
    excluded = {'.git', '.local', 'dist', 'target', 'node_modules', 'storage', '__pycache__'}
    names = []
    for directory, folders, files in os.walk(root, followlinks=False):
        parent = Path(directory)
        folders[:] = [name for name in folders if name not in excluded and not (parent/name).is_symlink()]
        names.extend(str((parent/name).relative_to(root)) for name in files if not (parent/name).is_symlink())
    return names


def inputs(root, group, env):
    names = source_files(root)
    production = {'apps', 'crates', 'modules', 'assets', 'themes', 'migrations', 'seeders', 'distribution'}
    files = {n for n in names if n and (Path(n).parts[0] in production and not n.endswith('.md') or
             n in {'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'package.json', 'package-lock.json'} or n.startswith('.cargo/'))}
    files.update({'scripts/test-run.py', 'scripts/test-suite.json', 'scripts/test-env.py', 'dev/test.compose.yml'})
    files.update(declared_binaries(group, env))
    suite = group['suite']
    if suite in {'browser', 'packages'}:
        for part in group['command']:
            if part.endswith('.cjs'):
                files.update(js_inputs(root, part))
        files.add('node_modules/.package-lock.json')
        files.add(host_binary(group, env))
        if group['id'] == 'runtime-recovery' and env.get('TALOCART_TEST_PREVIOUS_BINARY'):
            files.add(env['TALOCART_TEST_PREVIOUS_BINARY'])
        if group['id'] == 'smtp-recovery':
            files.update({'scripts/release/operations.py', 'scripts/release/pg-container.py'})
        if group['id'] == 'package-updates':
            files.update(str(p) for p in (root / env.get('TALOCART_FIXTURE_DIR', 'target/debug')).glob('fixture-*') if p.is_file() and not p.suffix)
    elif suite == 'tooling':
        # Rust tests can invoke Node helpers, Python packaging tests read source distributions.
        files.update(n for n in names if n and (n.startswith(('scripts/', 'tests/', 'docs/')) or n.endswith('.md')))
    else:
        # PostgreSQL tests launch browser helpers; code changes conservatively
        # invalidate these groups, documentation-only edits do not.
        files.update(n for n in names if n.startswith(('scripts/', 'tests/')) and not n.endswith('.md'))
    if not (root / 'Cargo.toml').exists() and (root.parent / 'integration/Cargo.toml').is_file():
        files.update({'../Cargo.toml', '../Cargo.lock', '../.cargo/config.toml'})
        for folder in ('modules', 'integration', 'tooling'):
            files.update('../' + folder + '/' + name for name in source_files(root.parent / folder)
                         if name and (suite == 'tooling' or not name.endswith('.md')))
    return files


def runtime_identity(root):
    versions = {}
    for command in [('cargo', '--version'), ('rustc', '--version'), ('node', '--version')]:
        try:
            result = subprocess.run(command, cwd=root, capture_output=True, text=True, timeout=15)
            versions[command[0]] = result.stdout.strip() if result.returncode == 0 else 'unavailable'
        except (OSError, subprocess.TimeoutExpired):
            versions[command[0]] = 'unavailable'
    versions['python'] = sys.version
    browser_paths = [Path('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome')]
    for name in ['google-chrome', 'chromium', 'chromium-browser', 'microsoft-edge']:
        if shutil.which(name):
            browser_paths.append(Path(shutil.which(name)).resolve())
    try:
        result = subprocess.run(['node', '-e', "console.log(require('playwright').chromium.executablePath())"], cwd=root, capture_output=True, text=True, timeout=15)
        if result.returncode == 0:
            browser_paths.append(Path(result.stdout.strip()))
    except (OSError, subprocess.TimeoutExpired):
        pass
    versions['browsers'] = [(str(p), p.stat().st_size, p.stat().st_mtime_ns) for p in browser_paths if p.is_file()]
    return versions


def fingerprint(root, group, env, runtime):
    relevant = {k: v for k, v in env.items() if k.startswith(('TALOCART_', 'RELEASE_', 'RUST', 'CARGO', 'NODE_', 'LD_', 'DYLD_')) or k in {'PATH', 'CI', 'TZ', 'LANG', 'PLAYWRIGHT_BROWSERS_PATH'}}
    # Persist only the digest, never database credentials or environment values.
    payload = [group, hash_files(root, inputs(root, group, env)), relevant, runtime]
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()


def reusable(entry, digest, output, now=None):
    now = time.time() if now is None else now
    return bool(entry and entry.get('status') == 'passed' and entry.get('fingerprint') == digest and
                0 <= now - entry.get('finished', 0) <= 86400 and (output / entry.get('log', '')).is_file())


def save_state(output, state):
    temporary = output / 'state.tmp'
    with open(temporary, 'w', opener=lambda p, flags: os.open(p, flags, 0o600)) as stream:
        json.dump(state, stream, indent=2)
        stream.write('\n')
    temporary.replace(output / 'state.json')


def terminate(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def execute(group, root, output, env, state, snapshot, resume, timeout):
    key = group['id']; digest = snapshot()
    if resume and reusable(state.get(key), digest, output):
        print(f'REUSE {key}: matching green evidence (<24h)', flush=True)
        return 0
    log = f'{key}-{uuid.uuid4().hex[:12]}.log'
    state[key] = {'status': 'running', 'fingerprint': digest, 'log': log, 'started': time.time()}
    save_state(output, state)
    print(f'RUN   {key} -> {output / log}', flush=True)
    started = time.monotonic(); status = 'failed'; code = 1
    with open(output / log, 'w', opener=lambda p, flags: os.open(p, flags, 0o600)) as stream:
        try:
            process = subprocess.Popen(group['command'], cwd=root, env=env, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                code = process.wait(timeout=timeout)
                status = 'passed' if code == 0 else 'failed'
            except subprocess.TimeoutExpired:
                terminate(process); code = 124; status = 'timeout'
            except KeyboardInterrupt:
                terminate(process); code = 130; status = 'interrupted'
        except OSError:
            stream.write('Unable to start command; check installed prerequisites.\n')
    if snapshot() != digest:
        status = 'stale'; code = code or 1
    state[key].update(status=status, exit_code=code, seconds=round(time.monotonic()-started, 3), finished=time.time())
    save_state(output, state)
    print(f'{status.upper():5} {key}: {state[key]["seconds"]:.1f}s', flush=True)
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--suite', action='append', choices=sorted(PROFILES), default=[])
    parser.add_argument('--only', action='append', default=[], help='Exact group ID; repeatable')
    parser.add_argument('--list', action='store_true')
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--resume', action='store_true', help='Reuse matching green evidence from the last 24 hours')
    parser.add_argument('--keep-going', action='store_true')
    parser.add_argument('--timeout', type=float, default=3600, help='Seconds per group')
    args = parser.parse_args()
    groups = catalog(ROOT)
    if args.list:
        for group in groups:
            detail = f' (targeted; covered by {group["parent"]})' if group.get('parent') else ''
            print(f'{group["id"]:32} {group["suite"]}{detail}')
        return 0
    chosen = select(groups, args.suite, args.only)
    if not chosen or args.timeout <= 0:
        parser.error('Select --suite or --only and a positive timeout; use --list')
    if args.dry_run:
        for group in chosen:
            print(group['id'], json.dumps(group['command']), json.dumps(group.get('env', {})), 'binaries=' + json.dumps(declared_binaries(group, child_environment(os.environ, group))))
        return 0
    output = ROOT / '.local/test-runs'; output.mkdir(parents=True, exist_ok=True, mode=0o700)
    with open(output / 'runner.lock', 'w') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise UsageError('Another test runner owns this checkout; do not overlap database/browser groups') from None
        state_path = output / 'state.json'
        state = json.loads(state_path.read_text()) if state_path.exists() else {}
        preflight(ROOT, chosen, os.environ)
        runtime = runtime_identity(ROOT); failed = []; outcomes = []; started = time.time()
        for group in chosen:
            env = child_environment(os.environ, group)
            if group['suite'] in {'browser', 'postgres', 'packages'} and not env.get('TALOCART_TEST_DATABASE_URL'):
                raise UsageError('Missing dedicated test database; run through scripts/test-env.py')
            before = state.get(group['id'])
            code = execute(group, ROOT, output, env, state,
                           lambda: fingerprint(ROOT, group, env, runtime), args.resume, args.timeout)
            entry = state[group['id']]
            outcomes.append({'id': group['id'], 'status': 'reused' if entry is before else entry['status'],
                             'seconds': 0 if entry is before else entry['seconds'], 'log': entry['log']})
            if code:
                failed.append(group['id'])
                if code == 130 or not args.keep_going:
                    break
        report = {'requested': [g['id'] for g in chosen], 'outcomes': outcomes,
                  'not_run': [g['id'] for g in chosen[len(outcomes):]],
                  'started': started, 'seconds': round(time.time()-started, 3)}
        report_path = output / f'run-{uuid.uuid4().hex[:12]}.json'
        with open(report_path, 'w', opener=lambda p, flags: os.open(p, flags, 0o600)) as stream:
            json.dump(report, stream, indent=2)
        print(f'Finished: {len(outcomes)}/{len(chosen)} groups; failures: {", ".join(failed) or "none"}; report: {report_path}', flush=True)
        return 1 if failed else 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except UsageError as error:
        sys.exit(str(error))
    except (OSError, ValueError, KeyError) as error:
        # No environment values or provider responses are included in this diagnostic.
        sys.exit(f'Test runner: {type(error).__name__}. Check selection, catalog and local prerequisites (--list / --dry-run).')
