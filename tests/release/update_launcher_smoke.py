"""Real root ownership, signed bundle activation and engine-independent recovery.

Only in the disposable systemd fixture, after update_systemd_smoke.py. No host mounts.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

SCRIPTS = Path('/test/scripts/release')
sys.path.insert(0, str(SCRIPTS))
import update_launcher as launcher
import updater_bundle


def main():
    assert sys.argv[1:] == ['--disposable-container']
    assert Path('/.dockerenv').exists() and os.geteuid() == 0
    assert Path('/proc/1/comm').read_text().strip() == 'systemd'
    os.umask(0o022)
    root = Path('/usr/local/libexec/talocart')
    (root / 'update_launcher.py').write_bytes(Path(launcher.__file__).read_bytes())
    launcher.BUNDLES.mkdir()
    keys = Path('/test/keys')
    keys.mkdir(mode=0o700)
    private, public = keys / 'root-private.pem', keys / 'root-public.pem'
    launcher.command(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', private])
    private.chmod(0o600)
    public.write_bytes(launcher.command(['openssl', 'pkey', '-in', private, '-pubout']))
    record = updater_bundle.build(SCRIPTS, '1.0.0', private, Path('/test/dist'))
    launcher.install_bundle(Path('/test/dist') / record['asset'],
                            Path('/test/dist') / record['signature_asset'], public, '1.0.0')
    assert launcher.selected() == launcher.BUNDLES / '1.0.0'
    unsafe = Path('/tmp/unsafe-engine.py')
    unsafe.write_text('# root-owned but unsafe ancestor')
    try:
        launcher.trusted(unsafe)
    except ValueError:
        pass
    else:
        raise AssertionError('World-writable ancestor accepted')
    config = dict(service='talocart-update-fixture-app.service',
                  writer_units=['talocart-update-fixture-writer.service'])
    # Exercise an actual old-engine exit and launch a fresh signed engine process.
    incoming = Path('/test/updater-incoming')
    incoming.mkdir(mode=0o700)
    old_pid, new_pid = Path('/test/old-engine-pid'), Path('/test/new-engine-pid')
    def bundle(version, normal, state_format=1):
        source = Path('/test') / ('source-' + version)
        source.mkdir()
        for name in launcher.FILES | {'update_plan.py','update_state.py','update_host.py','update_probe.py'}:
            (source / name).write_bytes((SCRIPTS / name).read_bytes())
        (source / 'maintenance.html').write_bytes(Path('/test/assets/maintenance.html').read_bytes())
        (source / 'update_agent.py').write_text(
            'import json,os,sys\nfrom pathlib import Path\n'
            'if sys.argv[1:] == ["--self-check"]:\n'
            ' print(json.dumps(dict(format=1,state_format=' + str(state_format) + ',engine_version=' + repr(version) + ')))\n'
            'else:\n' + ''.join(' ' + line + '\n' for line in normal.splitlines()))
        record = updater_bundle.build(source, version, private, Path('/test') / ('bundle-' + version))
        for name in (record['asset'], record['signature_asset']):
            (incoming / name).write_bytes((Path('/test') / ('bundle-' + version) / name).read_bytes())
        return record
    final = bundle('1.0.2', 'Path(' + repr(str(new_pid)) + ').write_text(str(os.getpid()))')
    handoff = dict(format=1, updater=final, plan_sha256='a'*64, minimum_launcher='1.0.0')
    path = launcher.STATE / 'engine-handoff.json'
    first = bundle('1.0.1',
        'Path(' + repr(str(old_pid)) + ').write_text(str(os.getpid()))\n'
        'p=Path(' + repr(str(path)) + ')\n'
        'p.write_text(' + repr(json.dumps(handoff)) + ')\n'
        'p.chmod(0o600)\nraise SystemExit(75)')
    launcher.install_bundle(incoming / first['asset'], incoming / first['signature_asset'], public, '1.0.1')
    config_file = launcher.CONFIG
    config_file.parent.mkdir(mode=0o750, exist_ok=True)
    configured = config | dict(incoming=str(incoming), updater_public_key=str(public))
    config_file.write_text(''.join(name + '=' + json.dumps(value) + '\n' for name, value in configured.items()))
    config_file.chmod(0o600)
    result = subprocess.run(['/usr/bin/python3', '-I', str(root / 'update_launcher.py')], capture_output=True, timeout=120)
    assert result.returncode == 0, result.stderr
    assert old_pid.read_text() != new_pid.read_text() and launcher.selected().name == '1.0.2'
    assert not path.exists() and (launcher.BUNDLES / '1.0.1').is_dir()
    # A signed but incompatible self-check must not pin a failed pending request.
    bad = bundle('1.0.3', 'raise AssertionError("must not run")', state_format=2)
    failed_handoff = dict(format=1, updater=bad, plan_sha256='b'*64, minimum_launcher='1.0.0')
    launcher.atomic_json(path, failed_handoff)
    result = subprocess.run(['/usr/bin/python3', '-I', str(root / 'update_launcher.py')], capture_output=True, timeout=120)
    assert result.returncode == 1 and launcher.selected().name == '1.0.2'
    assert not path.exists()
    failure = launcher.STATE / 'engine-handoff-failed.json'
    assert json.loads(failure.read_text()) == failed_handoff
    assert failure.stat().st_mode & 0o077 == 0
    # The retained old engine can run on the next invocation to read corrected feeds.
    result = subprocess.run(['/usr/bin/python3', '-I', str(root / 'update_launcher.py')], capture_output=True, timeout=120)
    assert result.returncode == 0, result.stderr
    failure.unlink()
    (Path('/etc/systemd/system') / (config['service'] + '.d') / '90-talocart-update.conf').write_text(
        '[Service]\nExecCondition=+/usr/bin/python3 -I '
        '/usr/local/libexec/talocart/update_launcher.py --application-start-allowed\n')
    (Path('/etc/systemd/system') / (config['writer_units'][0] + '.d') / '90-talocart-update.conf').write_text(
        '[Service]\nExecCondition=+/usr/bin/python3 -I '
        '/usr/local/libexec/talocart/update_launcher.py --writer-start-allowed\n')
    launcher.command(['systemctl', 'daemon-reload'])
    def active(unit):
        return launcher.command(['systemctl', 'show', '-p', 'ActiveState', '--value', unit]).strip() == b'active'
    units = [config['service'], *config['writer_units']]
    launcher.command(['systemctl', 'start', *units])
    assert all(active(u) for u in units)
    # A durable claimed request blocks restart even before maintenance marker exists.
    launcher.atomic_json(launcher.STATE / 'processing.json', dict(attempt='synthetic'))
    launcher.command(['systemctl', 'stop', config['service']])
    launcher.command(['systemctl', 'start', config['service']])
    assert not active(config['service'])
    launcher.recovery(config)
    permit = launcher.RUNTIME / 'application-start'
    permit.touch(mode=0o600)
    lock = launcher.STATE / 'agent.lock'
    lock.touch(mode=0o600)
    child = subprocess.Popen([sys.executable, '-c',
        'import fcntl,sys; f=open(sys.argv[1]); fcntl.flock(f,fcntl.LOCK_EX); '
        'print("locked",flush=True); sys.stdin.read()', str(lock)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        assert child.stdout.readline().strip() == 'locked'
        launcher.command(['systemctl', 'start', *units])
        assert active(config['service']) and not active(config['writer_units'][0])
        (launcher.RUNTIME / 'writers-start').touch(mode=0o600)
        launcher.command(['systemctl', 'start', config['writer_units'][0]])
        assert active(config['writer_units'][0])
        child.kill()
        child.wait(timeout=5)
        launcher.command(['systemctl', 'stop', config['service']])
        launcher.command(['systemctl', 'start', config['service']])
        assert not active(config['service'])
        launcher.command(['systemctl', 'stop', config['writer_units'][0]])
        launcher.command(['systemctl', 'start', config['writer_units'][0]])
        assert not active(config['writer_units'][0])
    finally:
        if child.poll() is None:
            child.kill()
        child.communicate(timeout=5)
    # Recovery must work with a corrupt selected engine and without importing it.
    (launcher.CURRENT / 'update_engine.py').write_text('raise RuntimeError("broken")\n')
    config_file = launcher.CONFIG
    config_file.parent.mkdir(mode=0o750, exist_ok=True)
    config_file.write_text('feed_format=2\nservice_user="root"\nservice="' + config['service'] + '"\nwriter_units=["' + config['writer_units'][0] + '"]\n')
    config_file.chmod(0o600)
    # Execute the actually installed, root-owned entry point (docker-copied test
    # sources are not the trusted installation and may retain the desktop UID).
    result = subprocess.run(['/usr/bin/python3', '-I', str(root / 'update_launcher.py')],
                            capture_output=True, timeout=120)
    assert result.returncode == 1 and not result.stderr, result.stderr
    assert not permit.exists() and not (launcher.RUNTIME / 'writers-start').exists() and not any(active(u) for u in units)
    assert (launcher.STATE / 'maintenance').exists() and (launcher.STATE / 'writer-fence').exists()
    public = json.loads((launcher.STATE / 'status.json').read_text())
    assert public['format'] == 2 and public['recovery_required'] and public['phase'] == 'failed'
    assert (launcher.STATE / 'status.json').stat().st_mode & 0o777 == 0o640
    print(json.dumps(dict(launcher_smoke='passed', real_root_signature=True,
                         immutable_bundle=True, unsafe_ancestor_rejected=True,
                         claimed_request_blocks_start=True, dead_agent_blocks_restart=True,
                         recovery_without_engine=True, signed_handoff_fresh_process=True, failed_handoff_allows_old_engine_retry=True, writer_permit_requires_live_agent=True)))


if __name__ == '__main__':
    main()
