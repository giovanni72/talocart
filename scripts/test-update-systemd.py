#!/usr/bin/env python3
"""Run updater guards in a disposable systemd container; no shop/network mounts."""
import argparse
from pathlib import Path
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]


def command(*args):
    return subprocess.run(['docker', *map(str, args)], check=True, text=True, capture_output=True, timeout=300).stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', help='Reuse a local image with Python 3.11+, OpenSSL, D-Bus, Nginx and systemd')
    parser.add_argument('--only', choices=['https','bootstrap'], help='Run only the independent HTTPS fixture')
    args = parser.parse_args()
    image = args.image or 'talocart-updater-systemd-test:local'
    if not args.image:
        command('build', '-f', ROOT / 'tests/release/Dockerfile.systemd', '-t', image, ROOT / 'tests/release')
    name = 'talocart-updater-test-' + uuid.uuid4().hex[:12]
    try:
        command('run', '--detach', '--name', name, '--network', 'none', '--privileged',
                '--cgroupns', 'private', '--tmpfs', '/run', '--tmpfs', '/run/lock',
                '--entrypoint', '/usr/bin/systemd', image, '--system', '--unit=multi-user.target')
        for _ in range(30):
            probe = subprocess.run(['docker', 'exec', name, 'systemctl', 'is-system-running'],
                                   capture_output=True, text=True, timeout=5, check=False)
            if probe.stdout.strip() in ('running', 'degraded'):
                break
            time.sleep(1)
        else:
            raise RuntimeError('Isolated systemd did not become ready')
        command('exec', name, 'mkdir', '-p', '/test/scripts/release', '/test/tests/release')
        command('exec', name, 'chmod', '755', '/test', '/test/scripts', '/test/scripts/release',
                '/test/tests', '/test/tests/release')
        for path in ('scripts/release/update_engine.py', 'scripts/release/update_launcher.py',
                     'scripts/release/update_agent.py', 'scripts/release/updates_feed.py',
                     'scripts/release/distribution.py', 'scripts/release/update_coordinator.py', 'scripts/release/update_plan.py', 'scripts/release/update_state.py', 'scripts/release/update_host.py', 'scripts/release/update_probe.py',
                     'scripts/release/updater_bundle.py', 'tests/release/update_systemd_smoke.py',
                     'tests/release/update_launcher_smoke.py', 'tests/release/update_operations_smoke.py', 'tests/release/update_https_smoke.py', 'scripts/release/bootstrap_updater.py', 'tests/release/update_bootstrap_smoke.py'):
            command('cp', ROOT / path, f'{name}:/test/{path}')
        command('exec', name, 'mkdir', '-p', '/test/assets', '/test/distribution')
        command('cp', ROOT/'assets/maintenance.html', f'{name}:/test/assets/maintenance.html')
        command('cp', ROOT/'distribution/nginx.conf', f'{name}:/test/distribution/nginx.conf')
        if not args.only:
            print(command('exec', name, 'python3', '/test/tests/release/update_systemd_smoke.py', '--disposable-container'))
            print(command('exec', name, 'python3', '/test/tests/release/update_launcher_smoke.py', '--disposable-container'))
            print(command('exec', name, 'python3', '/test/tests/release/update_operations_smoke.py', '--disposable-container'))
        if args.only == 'bootstrap':
            print(command('exec', name, 'python3', '/test/tests/release/update_bootstrap_smoke.py', '--disposable-container'))
        else:
            print(command('exec', name, 'python3', '/test/tests/release/update_https_smoke.py', '--disposable-container'))
    except subprocess.CalledProcessError as error:
        # This container contains only synthetic fixtures, no credentials.
        print(error.stderr)
        raise
    finally:
        subprocess.run(['docker', 'rm', '-f', name], capture_output=True, timeout=30, check=False)
    if not args.only:
        subprocess.run([sys.executable,str(Path(__file__).resolve()),'--image',image,'--only','bootstrap'],check=True)


if __name__ == '__main__':
    main()
