#!/usr/bin/env python3
"""Qualify 42 durable-boundary failures on disposable Ubuntu hosts."""
import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
PHASES = ('updater_ready', 'quiescing', 'fenced', 'backed_up', 'applying', 'verifying', 'resuming', 'succeeded')
BOUNDARIES = ['processing', 'begin', *[kind+':'+phase for phase in PHASES for kind in ('intent', 'complete')], 'deployed', 'published', 'ack']


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('release', 'archive', 'fixtures', 'initial-release', 'output'):
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--image', default='talocart-updater-integration:ubuntu24-pg17')
    parser.add_argument('--max-workers', type=int, choices=(1, 2), default=2)
    args = parser.parse_args()
    output = args.output.resolve(); output.mkdir(parents=True, exist_ok=True)
    sources = [ROOT/'scripts/test-update-integration.py', Path(__file__).resolve()]
    sources += list((ROOT/'scripts/release').glob('*.py'))
    sources += list((ROOT/'tests/release').glob('update_*_stage.py'))
    sources += [ROOT/'tests/release/update_integration_smoke.py']
    inputs = {str(p.relative_to(ROOT)): digest(p) for p in sources}
    artifacts = [args.archive.resolve(strict=True), args.initial_release/'bin/talocart', args.release/'bin/talocart']
    artifacts += sorted(args.fixtures.glob('fixture-*'))
    report = dict(inputs=inputs, artifacts={str(p): digest(p) for p in artifacts if p.is_file()},
                  started=time.time(), max_workers=args.max_workers, cases=[])
    common = [sys.executable, str(ROOT/'scripts/test-update-integration.py')]
    for name in ('release', 'archive', 'fixtures', 'initial_release'):
        common += ['--'+name.replace('_', '-'), str(getattr(args, name).resolve(strict=True))]
    common += ['--image', args.image]

    def run(mode, boundary):
        log = output/('interrupt-'+mode+'-'+boundary.replace(':', '-')+'.jsonl')
        start = time.monotonic()
        with log.open('w') as stream:
            log.chmod(0o600)
            # The inner runner bounds Docker commands and owns container cleanup.
            # Do not kill it externally before its finally block can run.
            result = subprocess.run(common+['--interrupt-at', boundary, '--interrupt-mode', mode],
                                    stdout=stream, stderr=subprocess.STDOUT)
            code = result.returncode
        return dict(mode=mode, boundary=boundary, exit_code=code,
                    seconds=round(time.monotonic()-start, 3), log=log.name, log_sha256=digest(log))

    with ThreadPoolExecutor(max_workers=args.max_workers) as pool:
        futures = [pool.submit(run, mode, boundary) for boundary in BOUNDARIES for mode in ('kill', 'reboot')]
        for future in as_completed(futures):
            value = future.result(); report['cases'].append(value)
            print(value['mode'], value['boundary'], 'PASSED' if value['exit_code']==0 else 'FAILED', flush=True)
            report['seconds'] = round(time.time()-report['started'], 3)
            path = output/'interruption-matrix.json'
            path.write_text(json.dumps(report, indent=2)+'\n'); path.chmod(0o600)
    report['inputs_unchanged'] = all(digest(ROOT/p)==value for p, value in inputs.items())
    path.write_text(json.dumps(report, indent=2)+'\n')
    return int(len(report['cases'])!=42 or not report['inputs_unchanged'] or any(c['exit_code'] for c in report['cases']))


if __name__ == '__main__':
    raise SystemExit(main())
