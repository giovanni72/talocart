"""Explicit smoke for a disposable, network-isolated systemd Docker container.

Run only with --disposable-container; never on a shop host. Uses dedicated
fixture units and real systemd, Linux locks, permissions and SIGKILL.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'scripts/release'))
import update_engine as engine
import update_coordinator as coordinator
import update_launcher as launcher


def main():
    assert sys.argv[1:] == ['--disposable-container']
    assert Path('/.dockerenv').exists() and os.geteuid() == 0
    assert Path('/proc/1/comm').read_text().strip() == 'systemd'
    assert not engine.STATE.exists(), 'Requires a fresh disposable container'
    os.umask(0o022)  # Match the installed updater service.
    engine.STATE.mkdir(mode=0o755)
    destination = Path('/usr/local/libexec/talocart')
    destination.mkdir(parents=True)
    (destination / 'update_launcher.py').write_bytes(Path(launcher.__file__).read_bytes())
    config = dict(service='talocart-update-fixture-app.service',
                  writer_units=['talocart-update-fixture-writer.service'])
    unit = ('[Unit]\nDescription=Disposable updater regression fixture\n'
            '[Service]\nUser=nobody\nGroup=nogroup\nExecStart=/bin/sleep infinity\n'
            'NoNewPrivileges=true\nProtectSystem=strict\nProtectHome=true\nPrivateTmp=true\n')
    for name in [config['service'], *config['writer_units']]:
        (Path('/etc/systemd/system') / name).write_text(unit)
    for name in [config['service'], *config['writer_units']]:
        dropin = Path('/etc/systemd/system') / (name + '.d')
        dropin.mkdir()
        (dropin / '90-talocart-update.conf').write_text(coordinator.unit_guard(name == config['service']))
    engine.RUNTIME.mkdir(mode=0o700, exist_ok=True)
    engine.run(['systemctl', 'daemon-reload'])
    units = [config['service'], *config['writer_units']]
    def active(name):
        return engine.run(['systemctl', 'show', '-p', 'ActiveState', '--value', name]) == 'active'
    # Ordinary start remains possible outside maintenance.
    engine.run(['systemctl', 'start', *units])
    assert all(active(u) for u in units)
    engine.recover(config)
    engine.run(['systemctl', 'start', *units])
    assert not any(active(u) for u in units)
    # A stale permit without a live updater must not authorize start.
    permit = engine.RUNTIME / 'application-start'
    permit.touch(mode=0o600)
    (engine.STATE / 'agent.lock').touch(mode=0o644)
    engine.run(['systemctl', 'start', config['service']])
    assert not active(config['service'])
    child = subprocess.Popen([sys.executable, '-c',
        'import fcntl,sys; f=open(sys.argv[1]); fcntl.flock(f,fcntl.LOCK_EX); '
        'print("locked",flush=True); sys.stdin.read()', str(engine.STATE / 'agent.lock')],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        assert child.stdout.readline().strip() == 'locked'
        engine.run(['systemctl', 'start', *units])
        assert active(config['service']) and not active(config['writer_units'][0])
        # Boot semantics: absence of /run permit blocks even with a lock held.
        engine.run(['systemctl', 'stop', config['service']])
        permit.unlink()
        engine.run(['systemctl', 'start', config['service']])
        assert not active(config['service'])
        permit.touch(mode=0o600)
        engine.run(['systemctl', 'start', config['service']])
        assert active(config['service'])
        child.kill(); child.wait(timeout=5)
        engine.run(['systemctl', 'stop', config['service']])
        engine.run(['systemctl', 'start', config['service']])
        assert not active(config['service']), 'A dead updater cannot authorize restart'
        engine.recover(config)
        assert not permit.exists() and not any(active(u) for u in units)
        assert (engine.STATE / 'maintenance').exists() and (engine.STATE / 'writer-fence').exists()
        # Explicit recovery completion restores normal operation.
        (engine.STATE / 'maintenance').unlink()
        (engine.STATE / 'writer-fence').unlink()
        engine.run(['systemctl', 'start', *units])
        assert all(active(u) for u in units)
        print(json.dumps(dict(systemd_smoke='passed', live_activation=True,
                             external_writers_fenced=True, sigkill_blocks_restart=True,
                             missing_boot_permit_blocks_start=True, recovery_stops_writers=True)))
    finally:
        if child.poll() is None:
            child.kill()
        child.communicate(timeout=5)
        engine.run(['systemctl', 'stop', *units])


if __name__ == '__main__':
    main()
