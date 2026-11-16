"""Unprivileged updater CLI cgroups and independent cleanup, disposable systemd only."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

sys.path.insert(0, '/test/scripts/release')
import update_coordinator as coordinator
import update_engine as engine
import update_launcher as launcher


def main():
    assert sys.argv[1:] == ['--disposable-container']
    assert Path('/.dockerenv').exists() and os.geteuid() == 0
    os.umask(0o022)
    for name in ('processing.json', 'maintenance', 'writer-fence', 'operation.json'):
        (launcher.STATE / name).unlink(missing_ok=True)
    config = dict(service_user='nobody', config='/test/not-used.toml',
                  service='talocart-update-fixture-app.service',
                  writer_units=['talocart-update-fixture-writer.service'])
    units = [config['service'], *config['writer_units']]
    engine.run(['systemctl', 'start', *units])
    def active(unit):
        return engine.run(['systemctl', 'show', '-p', 'ActiveState', '--value', unit]) == 'active'
    assert all(active(u) for u in units)
    ops = coordinator.Operations(config, engine)
    executable = Path('/usr/local/bin/talocart-operation-fixture')
    detached = Path('/tmp/talocart-unexpected-detached-child')
    executable.write_text(
        '#!/usr/bin/python3\nimport json,os,subprocess,sys\n'
        'if sys.argv[-1]=="doctor":\n print("synthetic failure",file=sys.stderr)\n raise SystemExit(2)\n'
        'subprocess.Popen(["/usr/bin/python3","-c",'
        + repr('import time;from pathlib import Path;time.sleep(2);Path(' + repr(str(detached)) + ').write_text("orphan")') + '])\n'
        'print(json.dumps(dict(uid=os.getuid(),clean_environment=not any(k.startswith("TALOCART_") for k in os.environ))))\n')
    executable.chmod(0o755)
    try:
        result = json.loads(ops.application(executable, 'build-info'))
    except Exception:
        # Only this disposable fixture's synthetic commands/output, never host data.
        print((engine.STATE / 'failure.log').read_text()[-8192:], file=sys.stderr)
        raise
    assert result['uid'] == ops.uid != 0 and result['clean_environment']
    time.sleep(2.2)
    assert not detached.exists(), 'Successful CLI left a detached child'
    payload = b'x'*16384
    result = ops._run(['/usr/bin/python3', '-c', 'import sys;print(len(sys.stdin.buffer.read()))'], 10, False, payload)
    assert result == '16384' and not (launcher.STATE / 'operation.json').exists()
    for argv, timeout in [([str(executable), '--config', config['config'], 'doctor'], 10),
                          (['/bin/sleep', 'infinity'], 1),
                          (['/usr/bin/python3', '-c', 'import sys;sys.stdout.buffer.write(b"x"*1100000)'], 10)]:
        try:
            ops._run(argv, timeout, False)
        except ValueError:
            pass
        else:
            raise AssertionError('Failed/timed out/oversized command accepted')
        assert not (launcher.STATE / 'operation.json').exists()
        assert all(active(u) for u in units), 'Safe preparation failure stopped the shop'
    try:
        ops.application(executable, 'package-update-migrate')
    except ValueError:
        pass
    else:
        raise AssertionError('Mutation admitted without attempt/fence')
    launcher.atomic_json(launcher.STATE / 'processing.json', dict(attempt='synthetic'))
    launcher.marker('maintenance')
    launcher.marker('writer-fence')
    try:
        ops.application(executable, 'package-update-verify')
    except ValueError:
        pass
    else:
        raise AssertionError('Offline CLI admitted while writers were active')
    engine.run(['systemctl', 'stop', *units])
    permit = launcher.RUNTIME / 'application-start'
    permit.touch(mode=0o600)
    try:
        ops.application(executable, 'package-update-verify')
    except ValueError:
        pass
    else:
        raise AssertionError('Offline CLI admitted with an activation permit')
    permit.unlink()
    result = json.loads(ops.application(executable, 'package-update-verify'))
    assert result['uid'] == ops.uid and not (launcher.STATE / 'operation.json').exists()
    for name in ('processing.json', 'maintenance', 'writer-fence'):
        (launcher.STATE / name).unlink()
    engine.run(['systemctl', 'start', *units])
    # An interrupted root parent leaves a durable, bounded unit identity. The
    # launcher stops that cgroup without importing the selected (corrupt) engine.
    controller = Path('/test/operation-controller.py')
    controller.write_text(
        'import sys\nsys.path.insert(0,"/test/scripts/release")\n'
        'import update_coordinator as c,update_engine as e\n'
        'c.Operations(' + repr(config) + ',e)._run(["/bin/sleep","infinity"],3900,False)\n')
    with Path('/test/operation-controller.log').open('wb') as log:
        process = subprocess.Popen(['/usr/bin/python3', str(controller)], stdout=log, stderr=log)
        try:
            for _ in range(100):
                path = launcher.STATE / 'operation.json'
                if path.exists():
                    value = json.loads(path.read_text())
                    if active(value['unit']):
                        break
                time.sleep(0.1)
            else:
                raise AssertionError('Synthetic operation did not start')
            process.kill()
            process.wait(timeout=10)
            assert not launcher.incomplete(), 'Read-only CLI alone must not fence a healthy shop'
            launcher.reap_operation()
            assert not path.exists() and not active(value['unit'])
            assert all(active(u) for u in units)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
            launcher.reap_operation()
    # Backup sealing uses real root ownership and openat/no-follow, not mocks.
    backups=Path('/test/seal-backups'); backups.mkdir()
    output=backups/'verified'; (output/'nested').mkdir(parents=True)
    (output/'nested/data').write_text('synthetic preserved data')
    (output/'executable').write_text('synthetic executable'); (output/'executable').chmod(0o700)
    for path in [output,*output.rglob('*')]: os.chown(path,ops.uid,ops.gid)
    coordinator.seal_backup(output,ops.gid,engine)
    assert all(p.stat().st_uid==0 for p in [output,*output.rglob('*')])
    assert (output/'nested/data').stat().st_mode&0o777==0o640
    assert (output/'executable').stat().st_mode&0o777==0o750
    outside=backups/'outside'; outside.write_text('must not change'); outside.chmod(0o600); os.chown(outside,ops.uid,ops.gid)
    for kind in ('symlink','hardlink','fifo'):
        unsafe=backups/kind; unsafe.mkdir(); os.chown(unsafe,ops.uid,ops.gid)
        path=unsafe/'escape'
        if kind=='symlink': path.symlink_to(outside)
        elif kind=='hardlink': os.link(outside,path)
        else: os.mkfifo(path)
        try: coordinator.seal_backup(unsafe,ops.gid,engine)
        except (OSError,ValueError): pass
        else: raise AssertionError('Unsafe backup member accepted: '+kind)
        assert outside.stat().st_uid==ops.uid and outside.stat().st_mode&0o777==0o600
        path.unlink()
    print(json.dumps(dict(operation_smoke='passed', service_uid=True, clean_environment=True,
                          no_detached_children=True, bounded_stdio=True, timeout_cleanup=True,
                          safe_failures_leave_shop_active=True, root_parent_sigkill_cleanup=True, offline_requires_stopped_fenced_writers=True, backup_sealing_root_owned_no_links=True)))


if __name__ == '__main__':
    main()
