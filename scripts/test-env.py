#!/usr/bin/env python3
"""Run tests with dedicated loopback PostgreSQL/Mailpit and private local credentials."""
import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / '.local' / 'test-services.json'
COMPOSE = ['docker', 'compose', '--project-name', 'talocart-tests', '-f', str(ROOT / 'dev/test.compose.yml')]


def password():
    CONFIG.parent.mkdir(exist_ok=True)
    try:
        with open(CONFIG, 'x', opener=lambda path, flags: os.open(path, flags, 0o600)) as stream:
            json.dump({'password': secrets.token_hex(32)}, stream)
    except FileExistsError:
        pass
    value = json.loads(CONFIG.read_text())['password']
    if not isinstance(value, str) or len(value) != 64 or any(c not in '0123456789abcdef' for c in value):
        raise ValueError('Invalid private test-services configuration')
    return value


def environment(secret, source=None):
    source = os.environ if source is None else source
    # Application overrides must not redirect a synthetic server to application data.
    test_paths = {'TALOCART_BASE_TEST_BINARY', 'TALOCART_MODULE_TEST_BINARY', 'TALOCART_FIXTURE_DIR'}
    env = {k: v for k, v in source.items() if not k.startswith('TALOCART_') or k.startswith(('TALOCART_TEST_', 'TALOCART_BROWSER_')) or k in test_paths}
    env.update({
        'TALOCART_TEST_SERVICE_PASSWORD': secret,
        'TALOCART_TEST_DATABASE_URL': f'postgres://talocart_test:{secret}@127.0.0.1:55432/talocart_test',
        'TALOCART_TEST_MAILPIT_URL': 'http://127.0.0.1:18025',
        'TALOCART_TEST_SMTP_PORT': '11025',
    })
    if 'TALOCART_BROWSER_CHANNEL' not in env and Path('/Applications/Google Chrome.app').exists():
        env['TALOCART_BROWSER_CHANNEL'] = 'chrome'
    return env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['up', 'run', 'status', 'stop'])
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if (args.action == 'run') != bool(command):
        parser.error('Use run -- COMMAND [ARGS]; other actions take no command')
    secret = password()
    env = environment(secret)
    action = {'up': ['up', '-d', '--wait'], 'run': ['up', '-d', '--wait'], 'status': ['ps'], 'stop': ['stop']}[args.action]
    result = subprocess.run(COMPOSE + action, cwd=ROOT, env=env, capture_output=True, text=True, check=False)
    # Compose errors must not expose interpolated credentials.
    for output, stream in [(result.stdout, sys.stdout), (result.stderr, sys.stderr)]:
        stream.write(output.replace(secret, '[redacted]'))
    if result.returncode or args.action != 'run':
        return result.returncode
    env.pop('TALOCART_TEST_SERVICE_PASSWORD')
    return subprocess.run(command, cwd=ROOT, env=env, check=False).returncode


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError):
        sys.exit('Test environment unavailable; check Docker and .local/test-services.json (private details omitted).')
