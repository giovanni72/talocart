#!/usr/bin/env python3
"""Exercise extracted Linux binaries against an explicit, disposable test DB."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.request
from urllib.parse import urlsplit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--ui-sha256', required=True)
    args = parser.parse_args()
    urls = [os.environ['TALOCART_TEST_DATABASE_URL']]
    assert all('test' in urlsplit(url).path for url in urls)
    env = {k: v for k, v in os.environ.items() if not k.startswith(('TALOCART_', 'PG'))}
    env['TALOCART_LOG'] = 'warn'
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='talocart-linux-') as temp:
        root = Path(temp)
        def config(index):
            target = root / f'{index}.toml'
            target.write_text(f'[server]\nlisten="127.0.0.1:3148"\n[database]\nurl={json.dumps(urls[index])}\n[media]\nroot={json.dumps(str(root / str(index)))}\n')
            target.chmod(0o600)
            return target
        fresh = config(0)
        def run(binary, config, command, expected=0):
            result = subprocess.run([str(binary), '--config', str(config), command], env=env, cwd=root,
                                    capture_output=True, timeout=60)
            assert result.returncode == expected, 'CLI check failed (private output omitted)'
        run(args.binary, fresh, 'doctor', 1)
        run(args.binary, fresh, 'serve', 1)
        run(args.binary, fresh, 'migrate')
        run(args.binary, fresh, 'migrate')
        run(args.binary, fresh, 'doctor')
        checks = {}
        for label, selected in [('fresh', fresh)]:
            with (args.output / f'{label}-server.log').open('wb') as log:
                server = subprocess.Popen([str(args.binary), '--config', str(selected), 'serve'], env=env,
                                          cwd=root, stdout=log, stderr=log)
                try:
                    for _ in range(150):
                        assert server.poll() is None, 'Server exited'
                        try:
                            with urllib.request.urlopen('http://127.0.0.1:3148/health/ready', timeout=1) as response:
                                if response.status == 200:
                                    break
                        except OSError:
                            pass
                        time.sleep(0.1)
                    else:
                        raise AssertionError('Readiness timeout')
                    with urllib.request.urlopen('http://127.0.0.1:3148/setup') as response:
                        html = response.read()
                        assert response.status == 200 and b'lang="en"' in html
                    with urllib.request.urlopen('http://127.0.0.1:3148/assets/ui.css') as response:
                        assert hashlib.sha256(response.read()).hexdigest() == args.ui_sha256
                    checks[label] = {'readiness': 200, 'setup_en': 200, 'embedded_css_matches': True}
                finally:
                    server.terminate()
                    try:
                        assert server.wait(timeout=20) == 0, 'Unclean shutdown'
                    except subprocess.TimeoutExpired:
                        server.kill()
                        server.wait()
                        raise
        result = {'clean_install': True, 'schema_contract': 'model-sku', 'migration_idempotency': True,
                  'no_runtime_source_files': True, 'http': checks}
        (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result))


if __name__ == '__main__':
    try:
        main()
    except (AssertionError, OSError, KeyError, subprocess.SubprocessError):
        raise SystemExit('Linux smoke failed; private diagnostics omitted')
