#!/usr/bin/env python3
"""Qualify real Talocart on disposable Ubuntu/systemd/PostgreSQL; never deploys."""
import argparse
import json
from pathlib import Path
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]


def command(*args, timeout=300):
    result = subprocess.run(['docker', *map(str,args)], capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        print(result.stdout, end=''); print(result.stderr, end='')
        raise RuntimeError('Disposable integration command failed')
    return result.stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--first-install-failure', choices=('database','tls','kill','reboot','disk-full'), help='First-install fail-closed drill')
    parser.add_argument('--first-install', action='store_true', help='One signed first-installation package, including updater')
    parser.add_argument('--release', type=Path, required=True, help='Extracted clean/tagged Linux base; test data only')
    parser.add_argument('--interrupt-at',choices=['processing','begin','deployed','published','ack']+[kind+':'+phase for kind in ('intent','complete') for phase in ('updater_ready','quiescing','fenced','backed_up','applying','verifying','resuming','succeeded')])
    parser.add_argument('--interrupt-mode',choices=['kill','reboot'],default='kill')
    parser.add_argument('--final-engine',type=Path,help='Final signed engine directory and public key; no private signing keys copied')
    parser.add_argument('--unchanged-module',type=Path,help='Optional earlier real native package to keep unchanged')
    parser.add_argument('--unchanged-disabled',action='store_true',help='Keep that earlier module disabled')
    parser.add_argument('--initial-release',type=Path,help='Optional previous base with the compatible live channel')
    parser.add_argument('--base-only',action='store_true',help='Keep the installed native module while updating the base')
    parser.add_argument('--faults', action='store_true', help='Also exercise uncertain SQL and restore on fresh targets')
    parser.add_argument('--fixtures', type=Path, help='Optional native-module stage: directory of compatible Linux fixture executables')
    parser.add_argument('--archive', type=Path, help='Exact archive matching the extracted base')
    parser.add_argument('--image', default='talocart-updater-integration:ubuntu24-pg17')
    args = parser.parse_args()
    if args.final_engine and args.interrupt_at:
        parser.error('Interruption instrumentation requires disposable signing keys, never final keys')
    if args.first_install and (not args.archive or args.fixtures or args.interrupt_at or args.initial_release or args.final_engine or args.faults):
        parser.error('First installation requires only --release and --archive')
    if args.first_install_failure and not args.first_install:
        parser.error('Failure drill requires --first-install')
    base = args.release.resolve(strict=True)
    metadata = json.loads((base/'release.json').read_text())
    assert metadata['target'] == 'x86_64-unknown-linux-gnu', 'This fixture qualifies x86_64'
    assert metadata['build']['tagged'] and not metadata['source_dirty'] and metadata['profile'] == 'base'
    name = 'talocart-integration-'+uuid.uuid4().hex[:12]
    try:
        command('run','--detach','--name',name,'--platform','linux/amd64','--network','none',
            '--privileged','--cgroupns','private','--tmpfs','/run','--tmpfs','/run/lock',
            '--entrypoint','/usr/bin/systemd',args.image,'--system','--unit=multi-user.target')
        for _ in range(45):
            probe = subprocess.run(['docker','exec',name,'systemctl','is-system-running'],capture_output=True,text=True,timeout=5)
            if probe.stdout.strip() in ('running','degraded'): break
            time.sleep(1)
        else: raise RuntimeError('Isolated systemd did not start')
        command('exec',name,'mkdir','-p','/test/root-source')
        command('cp',base,f'{name}:/test/base')
        if args.final_engine:command('cp',args.final_engine.resolve(strict=True),f'{name}:/test/final-engine')
        if args.initial_release:command('cp',args.initial_release.resolve(strict=True),f'{name}:/test/initial')
        if args.base_only:
            assert args.initial_release and args.fixtures
            command('exec',name,'touch','/test/base-only')
        for path in ('update_engine.py','update_launcher.py','update_agent.py','updates_feed.py',
                     'distribution.py','update_coordinator.py','update_plan.py','update_state.py','update_host.py','update_probe.py','updater_bundle.py','bootstrap_updater.py','install_server.py', 'install_guard.py'):
            command('cp',ROOT/'scripts/release'/path,f'{name}:/test/root-source/{path}')
        command('cp',ROOT/'assets/maintenance.html',f'{name}:/test/root-source/maintenance.html')
        if args.first_install:
            command('cp',ROOT/'tests/release/first_install_smoke.py',f'{name}:/test/check.py')
            command('cp',ROOT/'distribution',f'{name}:/test/distribution')
            command('cp',args.archive.resolve(strict=True),f'{name}:/test/base.tar.gz')
            if args.first_install_failure:
                command('exec',name,'touch','/test/first-install-'+args.first_install_failure)
        else:
            command('cp',ROOT/'tests/release/update_integration_smoke.py',f'{name}:/test/check.py')
        if args.fixtures:
            assert args.archive and args.archive.is_file()
            command('cp',args.fixtures.resolve(),f'{name}:/test/fixtures')
            command('cp',args.archive.resolve(),f'{name}:/test/base.tar.gz')
            command('cp',ROOT/'tests/release/update_distribution_stage.py',f'{name}:/test/update_distribution_stage.py')
        # Docker copies may preserve host UID/mode. Administrative inputs must
        # actually be installed as root, just like a verified kit on Ubuntu.
        if args.unchanged_module:
            command('cp',args.unchanged_module.resolve(strict=True),f'{name}:/test/unchanged.talopkg')
        if args.unchanged_disabled:
            assert args.unchanged_module
            command('exec',name,'touch','/test/unchanged-disabled')
        if args.interrupt_at:
            assert args.fixtures and not args.faults
            setting=json.dumps(dict(boundary=args.interrupt_at,mode=args.interrupt_mode))
            command('exec',name,'python3','-c',"from pathlib import Path; import sys; Path('/test/interrupt.json').write_text(sys.argv[1])",setting)
            command('cp',ROOT/'tests/release/update_interruption_stage.py',f'{name}:/test/update_interruption_stage.py')
        if args.faults:
            assert args.fixtures
            command('exec',name,'touch','/test/faults')
        command('exec',name,'chown','-R','0:0','/test')
        command('exec',name,'chmod','-R','go-w','/test')
        print(command('exec',name,'python3','/test/check.py','--disposable-container',timeout=600))
        if args.first_install_failure == 'reboot':
            command('exec',name,'test','-f','/test/reboot-ready')
            command('restart','--time','0',name)
            time.sleep(3)
            code = "import subprocess,json; from pathlib import Path; r=subprocess.run(['systemctl','start','talocart.service'],capture_output=True); assert r.returncode!=0; assert subprocess.run(['systemctl','is-active','talocart.service'],capture_output=True).stdout.strip()!=b'active'; assert json.loads(Path('/var/lib/talocart-update/installation.json').read_text())['phase']=='installing'; print('First-install reboot: application stopped and restart rejected')"
            print(command('exec',name,'python3','-c',code,timeout=60))
        if args.interrupt_at and args.interrupt_mode=='reboot':
            command('exec',name,'test','-f','/test/reboot-ready')
            command('restart','--time','0',name)
            for _ in range(45):
                probe=subprocess.run(['docker','exec',name,'systemctl','is-system-running'],capture_output=True,text=True,timeout=5)
                if probe.stdout.strip() in ('running','degraded'):break
                time.sleep(1)
            else:raise RuntimeError('Rebooted systemd did not start')
            command('exec',name,'systemctl','start','postgresql@17-main.service')
            print(command('exec',name,'python3','/test/update_interruption_stage.py',timeout=150))
    except BaseException:
        # Only synthetic container data. No host credentials or runtime mounts.
        diagnostics=subprocess.run(['docker','exec',name,'journalctl','-u','talocart-integration.service','-n','30','--no-pager'],capture_output=True,text=True,timeout=15)
        print(diagnostics.stdout)
        for unit in ('talocart.service','talocart-update.service','nginx.service'):
            log=subprocess.run(['docker','exec',name,'journalctl','-u',unit,'-n','20','--no-pager'],capture_output=True,text=True,timeout=15)
            print(log.stdout)
        diagnostic=subprocess.run(['docker','exec',name,'python3','-c',"from pathlib import Path; p=Path('/var/lib/talocart-update/failure.log'); print(p.read_text()[-16384:] if p.exists() else 'No application failure log')"],capture_output=True,text=True,timeout=15)
        print(diagnostic.stdout)
        raise
    finally:
        subprocess.run(['docker','rm','-f',name],capture_output=True,timeout=30,check=False)


if __name__ == '__main__': main()
