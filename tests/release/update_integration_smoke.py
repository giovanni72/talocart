"""Real Talocart, PostgreSQL 17 and Ubuntu/systemd; disposable container only.

The first stage qualifies fresh installation, the authenticated live control
channel and administrative bootstrap. It does not claim an application upgrade.
All keys, accounts and data are synthetic and destroyed with the container.
"""
import hashlib
import json
import os
from pathlib import Path
import pwd
import secrets
import shutil
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, '/test/root-source')
import bootstrap_updater as bootstrap
import update_coordinator as coordinator
import update_engine as engine
import update_launcher as launcher
import updater_bundle


def command(*args, input=None, timeout=120):
    result = subprocess.run(list(map(str, args)), input=input, capture_output=True, timeout=timeout)
    if result.returncode:
        # The container contains synthetic data only; logs stay in the local report.
        print(result.stderr.decode(errors='replace'), file=sys.stderr)
        raise AssertionError('Isolated integration command failed: ' + str(args[0]))
    return result.stdout


def wait_ready():
    for _ in range(150):
        try:
            with urllib.request.urlopen('http://127.0.0.1:3007/health/ready', timeout=1) as response:
                if response.status == 200 and response.read() == b'ready\n':
                    return
        except OSError:
            time.sleep(0.1)
    raise AssertionError('Real Talocart readiness timed out')


def main():
    assert sys.argv[1:] == ['--disposable-container']
    assert Path('/.dockerenv').exists() and os.geteuid() == 0
    assert Path('/proc/1/comm').read_text().strip() == 'systemd'
    assert 'Ubuntu 24.04' in Path('/etc/os-release').read_text()
    assert not launcher.STATE.exists(), 'Each integration run requires a fresh container'
    os.umask(0o022)
    work = Path('/root/integration'); work.mkdir(mode=0o700)
    command('useradd', '--system', '--user-group', '--home-dir', '/var/lib/talocart', 'talocart')
    account = pwd.getpwnam('talocart')
    data = Path('/var/lib/talocart'); data.mkdir(mode=0o700)
    os.chown(data, account.pw_uid, account.pw_gid)
    etc = Path('/etc/talocart'); etc.mkdir(mode=0o750); os.chown(etc, 0, account.pw_gid)
    state = launcher.STATE; state.mkdir(mode=0o755)
    requests = state/'requests'; requests.mkdir(mode=0o700); os.chown(requests, account.pw_uid, account.pw_gid)
    releases = Path('/opt/talocart/releases'); releases.mkdir(parents=True)
    initial=Path('/test/initial') if Path('/test/initial').exists() else Path('/test/base')
    metadata = json.loads((initial/'release.json').read_text())
    selected = releases/metadata['version']; shutil.copytree(initial, selected)
    for path in [selected, *selected.rglob('*')]:
        path.chmod(0o755 if path.is_dir() or path == selected/'bin/talocart' else 0o644)
    current = releases.parent/'current'; current.symlink_to(selected)
    assert coordinator.check_tree(selected, engine) == metadata
    password = secrets.token_hex(32)
    command('systemctl', 'start', 'postgresql@17-main.service')
    version = command('runuser', '-u', 'postgres', '--', 'psql', '-Atc', 'SHOW server_version').decode().strip()
    assert version.startswith('17.')
    command('runuser', '-u', 'postgres', '--', 'psql', '-v', 'ON_ERROR_STOP=1',
            input=("CREATE ROLE talocart_fixture LOGIN PASSWORD '"+password+"';\n").encode())
    command('runuser', '-u', 'postgres', '--', 'createdb', '-O', 'talocart_fixture', 'talocart_integration_test')
    app = etc/'talocart.toml'
    app.write_text('[server]\nlisten="127.0.0.1:3007"\npublic_origin="https://integration-updater.test:8443"\nsecure_cookies=true\n'
        '[database]\nurl="postgres://talocart_fixture:'+password+'@127.0.0.1:5432/talocart_integration_test"\n'
        '[media]\nroot="/var/lib/talocart/media"\n[packages]\nroot="/var/lib/talocart/packages"\n'
        '[logging]\nroot="/var/lib/talocart/logs"\n[updates]\nroot="'+str(state)+'"\n')
    app.chmod(0o640); os.chown(app, 0, account.pw_gid)
    original_config = app.read_bytes(); configuration = hashlib.sha256(original_config).hexdigest()
    binary = current/'bin/talocart'
    def application(operation):
        return command('runuser', '-u', 'talocart', '--', binary, '--config', app, operation)
    application('migrate'); application('doctor')
    for path in ('media', 'packages', 'logs'):
        folder = data/path; folder.mkdir(mode=0o700, exist_ok=True); os.chown(folder, account.pw_uid, account.pw_gid)
    media = data/'media/synthetic-original.bin'; media.write_bytes(b'original synthetic media\x00')
    os.chown(media, account.pw_uid, account.pw_gid); media.chmod(0o600)
    unit = Path('/etc/systemd/system/talocart-integration.service')
    unit.write_text('[Unit]\nAfter=postgresql@17-main.service\nRequires=postgresql@17-main.service\n[Service]\nType=simple\nUser=talocart\nGroup=talocart\nWorkingDirectory=/var/lib/talocart\n'
        'ExecStart='+str(binary)+' --config '+str(app)+' serve\nTimeoutStopSec=90\nUMask=0077\n[Install]\nWantedBy=multi-user.target\n')
    Path('/etc/systemd/system/talocart-update.service').write_text('[Service]\nType=oneshot\nExecStart=/bin/true\n')
    Path('/etc/systemd/system/talocart-update.timer').write_text(bootstrap.TIMER_TEXT)
    command('systemctl', 'daemon-reload'); command('systemctl', 'enable','--now',unit.name); wait_ready()
    assert command('systemctl','is-enabled',unit.name).strip()==b'enabled'
    initial_pid = command('systemctl', 'show', '-p', 'MainPID', '--value', unit.name)
    config = dict(feed_format=2, environment='staging', service_user='talocart', service=unit.name,
        release_root=str(releases), current=str(current), config=str(app),
        backup_root='/var/backups/talocart-integration', incoming=str(state/'incoming'),
        public_origin='https://integration-updater.test:8443', ready_url='http://127.0.0.1:3007/health/ready',
        writer_units=[], writers_fenced=True, repository='synthetic/integration', token_file='',
        public_key=str(etc/'update-public.pem'), updater_public_key=str(etc/'root-code-public.pem'),
        probe_token_file=str(etc/'probe-token'), probe_ca_file=str(work/'ca.pem'))
    engine.running_binary(config, selected/'bin/talocart')
    snapshot = coordinator.control(state, account.pw_uid, 'inventory', configuration_sha256=configuration)
    assert not snapshot['suspended'] and snapshot['modules'] == []
    for operation, inventory, code in [('quiesce', 'f'*64, 'conflict'), ('inventory', None, 'configuration')]:
        try:
            coordinator.control(state, account.pw_uid, operation, inventory,
                configuration_sha256='e'*64 if code == 'configuration' else configuration)
        except coordinator.ControlRefusal as refusal:
            assert refusal.code == code
        else:
            raise AssertionError('Live control failed to refuse changed identity')
        assert coordinator.control(state, account.pw_uid, 'resume', configuration_sha256=configuration) == snapshot
    assert not (state/'maintenance').exists()
    quiesced = coordinator.control(state, account.pw_uid, 'quiesce', snapshot['inventory_sha256'], configuration_sha256=configuration)
    assert quiesced['suspended'] and quiesced['modules'] == snapshot['modules']
    resumed = coordinator.control(state, account.pw_uid, 'resume', configuration_sha256=configuration)
    assert resumed == snapshot
    # Changing disk bytes cannot silently change the configuration loaded at boot.
    app.write_bytes(original_config+b'\n# synthetic drift\n')
    try:
        coordinator.control(state, account.pw_uid, 'inventory', configuration_sha256=hashlib.sha256(app.read_bytes()).hexdigest())
    except coordinator.ControlRefusal as refusal:
        assert refusal.code == 'configuration'
    else:
        raise AssertionError('Live startup configuration was not bound')
    finally:
        app.write_bytes(original_config)
    assert coordinator.control(state, account.pw_uid, 'inventory', configuration_sha256=configuration) == snapshot
    # Distinct ephemeral application/root-code keys; neither is copied to the host workspace.
    keys = {}
    final_engine=Path('/test/final-engine')
    for role in (('app',) if final_engine.exists() else ('root','app')):
        private, public = work/(role+'-private.pem'), work/(role+'-public.pem')
        command('openssl', 'genpkey', '-algorithm', 'ED25519', '-out', private); private.chmod(0o600)
        public.write_bytes(command('openssl', 'pkey', '-in', private, '-pubout')); keys[role]=(private, public)
    Path(config['public_key']).write_bytes(keys['app'][1].read_bytes())
    if Path('/test/interrupt.json').exists():
        from update_interruption_stage import instrument
        instrument(Path('/test/root-source'))
    if final_engine.exists():
        # Only production PUBLIC material crosses into a disposable host.
        keys['root']=(None,final_engine/'updater-root-public.pem')
        archive=final_engine/'talocart-updater-1.0.0.tar.gz';signature=final_engine/(archive.name+'.sig')
        verified=updater_bundle.verify(archive,signature,keys['root'][1],'1.0.0')
        assert verified['files']=={name:hashlib.sha256(updater_bundle.coordinator_source(Path('/test/root-source')) if name == 'update_coordinator.py' else (Path('/test/root-source')/name).read_bytes()).hexdigest() for name in launcher.FILES}
        (work/'engine').mkdir(mode=0o700)
        for path in (archive,signature):shutil.copyfile(path,work/'engine'/path.name)
        artifact=dict(version='1.0.0',asset=archive.name,signature_asset=signature.name,size=archive.stat().st_size,sha256=hashlib.sha256(archive.read_bytes()).hexdigest())
    else:
        artifact = updater_bundle.build(Path('/test/root-source'), '1.0.0', keys['root'][0], work/'engine')
    ca, cakey, cert, key = [work/name for name in ('ca.pem', 'ca.key', 'server.pem', 'server.key')]
    command('openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
        '-subj', '/CN=Integration synthetic CA', '-keyout', cakey, '-out', ca)
    command('openssl', 'req', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=integration-updater.test',
        '-keyout', key, '-out', work/'server.csr')
    (work/'extensions').write_text('subjectAltName=DNS:integration-updater.test\nextendedKeyUsage=serverAuth\n')
    command('openssl', 'x509', '-req', '-in', work/'server.csr', '-CA', ca, '-CAkey', cakey,
        '-CAcreateserial', '-days', '1', '-extfile', work/'extensions', '-out', cert)
    for path in (key, cakey): path.chmod(0o600)
    nginx_site = Path('/etc/nginx/conf.d/integration-site.conf')
    content = 'server { listen 127.0.0.1:8443 ssl; server_name integration-updater.test;\nssl_certificate '+str(cert)+';\nssl_certificate_key '+str(key)+';\ninclude /etc/talocart/nginx-update-server.conf;\nlocation / { proxy_set_header X-Talocart-Update-Probe ""; proxy_set_header Host $http_host; proxy_pass http://127.0.0.1:3007; }\n}\n'
    # Exercise the packaged proxy limits/headers rather than Nginx's 1 MiB default.
    template=(Path('/test/base')/'distribution/nginx.conf').read_text()
    content=template[template.index('server {',template.index('server {')+1):]
    content=content.replace('listen 443 ssl;','listen 127.0.0.1:8443 ssl;').replace('server_name shop.example.com;','server_name integration-updater.test;')
    content=content.replace('/etc/letsencrypt/live/shop.example.com/fullchain.pem',str(cert)).replace('/etc/letsencrypt/live/shop.example.com/privkey.pem',str(key))
    content=content.replace('deny all;','include /etc/talocart/nginx-update-server.conf;').replace('http://127.0.0.1:3000','http://127.0.0.1:3007').replace('proxy_set_header Host shop.example.com;','proxy_set_header Host $http_host;')
    command('systemctl', 'start', 'nginx.service'); nginx_site.write_text(content)
    staged = work/'update.toml'; staged.write_text(''.join(k+'='+json.dumps(v)+'\n' for k,v in config.items())); staged.chmod(0o600)
    # Administrative kits embed coordinator dependencies for isolated Python.
    # Exercise those assembled bytes, as the real bootstrap distribution does.
    bootstrap_stage = work/'bootstrap'; bootstrap_stage.mkdir(mode=0o700)
    for name in ('bootstrap_updater.py', 'update_launcher.py', 'update_engine.py', 'update_coordinator.py'):
        source = Path('/test/root-source')
        (bootstrap_stage/name).write_bytes(updater_bundle.coordinator_source(source)
            if name == 'update_coordinator.py' else (source/name).read_bytes())
    argv=['/usr/bin/python3', '-I', bootstrap_stage/'bootstrap_updater.py', '--config-source', staged,
        '--engine', work/'engine'/artifact['asset'], '--signature', work/'engine'/artifact['signature_asset'],
        '--engine-version', '1.0.0', '--root-public-key', keys['root'][1], '--nginx-site', nginx_site]
    preview = json.loads(command(*argv)); assert not preview['application_deploy']
    result = json.loads(command(*argv, '--apply')); assert result['installed']
    command('systemctl', 'stop', 'talocart-update.timer', 'talocart-update.path')
    assert command('systemctl', 'show', '-p', 'MainPID', '--value', unit.name) == initial_pid
    assert app.read_bytes() == original_config and media.read_bytes() == b'original synthetic media\x00'
    # Install the synthetic store through its real HTTPS/CSRF flow before
    # probing catalog/cart/login; an uninstalled host deliberately redirects.
    import http.client
    import re
    import socket
    import ssl
    from http.cookies import SimpleCookie
    from urllib.parse import urlencode
    class LocalHTTPS(http.client.HTTPSConnection):
        def connect(connection):
            raw = socket.create_connection(('127.0.0.1', connection.port), 10)
            connection.sock = connection._context.wrap_socket(raw, server_hostname=connection.host)
    cookies = {}
    def http(method, path, form=None, *, content_type=None):
        connection = LocalHTTPS('integration-updater.test',8443,context=ssl.create_default_context(cafile=ca),timeout=15)
        headers = {'Cookie': '; '.join(k+'='+v for k,v in cookies.items()), 'Origin': config['public_origin']}
        if form is not None: headers['Content-Type']=content_type or 'application/x-www-form-urlencoded'
        body=form if isinstance(form,bytes) else urlencode(form) if form is not None else None
        connection.request(method,path,body,headers)
        try:
            response=connection.getresponse(); body=response.read().decode()
            for name,value in response.getheaders():
                if name.lower()=='set-cookie':
                    parsed=SimpleCookie(); parsed.load(value)
                    cookies.update({k:v.value for k,v in parsed.items()})
            return response.status,body
        finally: connection.close()
    status,html=http('GET','/setup'); assert status==200
    csrf=re.search(r'name="csrf" value="([^"]+)"',html)[1]
    owner_password=secrets.token_hex(24)
    status,_=http('POST','/setup',dict(csrf=csrf,email='owner@example.test',password=owner_password,
        name='Isolated integration store',display_name='Owner',primary_locale='en',locale='en'))
    assert status==303, 'Synthetic store setup must succeed through HTTPS'
    coordinator.Probe(config, engine).smoke()
    assert not launcher.incomplete()
    if Path('/test/fixtures').exists():
        from update_distribution_stage import exercise
        exercise(command,http,config,account,work,keys,artifact,selected,metadata,owner_password)
    print(json.dumps(dict(integration_stage='bootstrap', passed=True, application_version=metadata['version'],
        application_sha256=engine.sha256(selected/'bin/talocart'), postgres=version,
        real_control_peer=True, live_conflict=True, startup_configuration_bound=True,
        quiesce_resume_preserves_inventory=True, bootstrap_without_restart=True, real_tls_gate=True,
        limitation='This record covers the initial bootstrap; other stages have independent records.')))


if __name__ == '__main__':
    main()
