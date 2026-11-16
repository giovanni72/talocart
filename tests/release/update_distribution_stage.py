"""Real owner request, signed HTTPS distribution, native updater and coherent backup.

GitHub transport is a loopback TLS fixture in a container with no external network.
Talocart, PostgreSQL, native modules, systemd, the launcher and Nginx are real.
"""
import hashlib
import http.server as http_server
import json
import os
from pathlib import Path
import re
import shutil
import ssl
import struct
import threading
import time
from urllib.parse import urlencode

import distribution
import update_coordinator as coordinator
import update_engine as engine
import update_launcher as launcher
import updates_feed as feed


def native_descriptor(payload):
    assert payload[:9] in (b'TALOPKG1\n', b'TALOPKG2\n')
    offset = 13 if payload.startswith(b'TALOPKG2\n') else 77
    return json.loads(payload[offset:offset + struct.unpack('>I', payload[9:13])[0]])


def exercise(command,http,config,account,work,keys,artifact,selected,metadata,owner_password):
    state=launcher.STATE
    initial_metadata=metadata
    metadata=json.loads(Path('/test/base/release.json').read_text())
    base_changed=initial_metadata['version']!=metadata['version']
    module_changed=not Path('/test/base-only').exists()
    def app(*argv):
        return command('runuser','-u','talocart','--',selected/'bin/talocart',*argv).decode().strip()
    def sql(query):
        return command('runuser','-u','postgres','--','psql','-d','talocart_integration_test','-Atc',query).decode().strip()
    def csrf_from(html): return re.search(r'name="csrf" value="([^"]+)"',html)[1]
    status,html=http('GET','/admin/login'); assert status==200
    status,_=http('POST','/admin/login',dict(csrf=csrf_from(html),email='owner@example.test',password=owner_password))
    assert status==303
    packages=Path('/var/lib/talocart/fixture-packages');packages.mkdir(mode=0o700);os.chown(packages,account.pw_uid,account.pw_gid)
    publisher='' # Target packages require no publisher key.
    # A previous host can only consume its historical encoding. Keep this test-only
    # setup separate from the target builder and the new installation policy.
    legacy_initial='--key ' in app('package-build','--help')
    legacy_key=packages/'legacy-initial.pk8'
    legacy_publisher=app('package-keygen','--output',legacy_key) if legacy_initial else None
    for version in ('v1','v2','v3-fixed','v3-failing'):
        argv=['package-build','--executable',Path('/test/fixtures')/('fixture-'+version),'--output',packages/(version+'.talopkg')]
        if version=='v1' and legacy_initial:
            app(*argv,'--key',legacy_key)
        else:
            command('runuser','-u','talocart','--',Path('/test/base/bin/talocart'),*argv)
    status,html=http('GET','/admin/modules'); assert status==200
    csrf=csrf_from(html)
    if legacy_initial:
        assert http('POST','/admin/modules/publishers',dict(csrf=csrf,public_key=legacy_publisher))[0]==303
    else:
        assert 'package-public-key' not in html
    # A multipart upload goes through the real owner/CSRF/package preview path.
    boundary='talocart-integration-upload'
    body=(('--'+boundary+'\r\nContent-Disposition: form-data; name="csrf"\r\n\r\n'+csrf+'\r\n'
        '--'+boundary+'\r\nContent-Disposition: form-data; name="package"; filename="fixture.talopkg"\r\nContent-Type: application/octet-stream\r\n\r\n').encode()
        +(packages/'v1.talopkg').read_bytes()+('\r\n--'+boundary+'--\r\n').encode())
    status,html=http('POST','/admin/modules/preview',body,content_type='multipart/form-data; boundary='+boundary)
    assert status==200 and 'data-preview="install"' in html, 'Native preview status: '+str(status)
    preview=re.search(r'name="preview" value="([^"]+)"',html)[1]
    assert http('POST','/admin/modules/install',dict(csrf=csrf,preview=preview))[0]==303
    assert http('POST','/admin/modules/toggle',dict(csrf=csrf,id='fixture-module',enabled='true'))[0]==303
    status,html=http('GET','/admin/extensions/fixture-module'); assert status==200 and '1.0.0' in html
    unchanged=None
    if Path('/test/unchanged.talopkg').exists():
        signed=Path('/test/unchanged.talopkg').read_bytes()
        unchanged=native_descriptor(signed)
        assert unchanged['id']!='fixture-module'
        if legacy_initial:
            assert signed.startswith(b'TALOPKG1\n'), 'Earlier hosts require a compatible legacy archive'
            assert http('POST','/admin/modules/publishers',dict(csrf=csrf,public_key=unchanged['publisher']))[0]==303
        body=(('--'+boundary+'\r\nContent-Disposition: form-data; name="csrf"\r\n\r\n'+csrf+'\r\n'
            '--'+boundary+'\r\nContent-Disposition: form-data; name="package"; filename="earlier.talopkg"\r\nContent-Type: application/octet-stream\r\n\r\n').encode()
            +signed+('\r\n--'+boundary+'--\r\n').encode())
        status,html=http('POST','/admin/modules/preview',body,content_type='multipart/form-data; boundary='+boundary)
        assert status==200 and 'data-preview="install"' in html, 'Earlier module preview failed: '+str(status)
        assert http('POST','/admin/modules/install',dict(csrf=csrf,preview=re.search(r'name="preview" value="([^"]+)"',html)[1]))[0]==303
        if not Path('/test/unchanged-disabled').exists():
            assert http('POST','/admin/modules/toggle',dict(csrf=csrf,id=unchanged['id'],enabled='true'))[0]==303
            assert http('GET',unchanged['menu'])[0]==200
    sql("INSERT INTO fixture_module_jobs(status) VALUES ('pending')")
    original=coordinator.control(state,account.pw_uid,'inventory',configuration_sha256=hashlib.sha256(Path(config['config']).read_bytes()).hexdigest())
    original_pid=command('systemctl','show','-p','MainPID','--value',config['service'])
    before_media=(Path('/var/lib/talocart/media/synthetic-original.bin').read_bytes())
    before_config=Path(config['config']).read_bytes()
    data=work/'feed';data.mkdir(mode=0o700)
    base=data/('talocart-'+metadata['version']+'-x86_64-unknown-linux-gnu.tar.gz');shutil.copyfile('/test/base.tar.gz',base)
    for name in (artifact['asset'],artifact['signature_asset']):shutil.copyfile(work/'engine'/name,data/name)
    signed=(packages/'v2.talopkg').read_bytes()
    native_manifest=native_descriptor(signed)
    offered_version=native_manifest['version']
    target_version=offered_version if module_changed else original['modules'][0]['version']
    pak=data/('fixture-module-'+target_version+'-linux.talopkg');shutil.copyfile(packages/'v2.talopkg',pak)
    def record(path,**extra):return dict(asset=path.name,size=path.stat().st_size,sha256=feed.digest(path),**extra)
    contracts=metadata['build']
    manifest=dict(format=2,channel='stable',repository=config['repository'],release=dict(id='v11.13.0',sequence=1,commit=metadata['source_commit']),
        minimum=dict(launcher='1.0.0',engine='1.0.0'),platforms=[dict(target='x86_64-unknown-linux-gnu',
        base=record(base,version=metadata['version'],commit=metadata['source_commit'],contracts={k:contracts[k] for k in ('sdk_version','protocol','database')}),
        modules=[record(pak,id='fixture-module',version=offered_version,publisher=publisher)] if module_changed else [],updater=artifact)])
    distribution.validate(manifest,config['repository'])
    raw=coordinator.canonical(manifest);(data/'distribution.json').write_bytes(raw);(data/'distribution.sig').write_bytes(feed.sign(raw,keys['app'][0]))
    assets=[dict(id=i+1,name=path.name,size=path.stat().st_size) for i,path in enumerate(sorted(data.iterdir()))]
    by_id={str(item['id']):data/item['name'] for item in assets}
    api_cert,api_key=work/'api.pem',work/'api.key'
    command('openssl','req','-newkey','rsa:2048','-nodes','-subj','/CN=api.github.com','-keyout',api_key,'-out',work/'api.csr')
    (work/'api.extensions').write_text('subjectAltName=DNS:api.github.com\nextendedKeyUsage=serverAuth\n')
    command('openssl','x509','-req','-in',work/'api.csr','-CA',work/'ca.pem','-CAkey',work/'ca.key','-CAcreateserial','-days','1','-extfile',work/'api.extensions','-out',api_cert)
    api_key.chmod(0o600)
    shutil.copyfile(work/'ca.pem','/usr/local/share/ca-certificates/talocart-integration.crt');command('update-ca-certificates')
    with Path('/etc/hosts').open('a') as stream:stream.write('\n127.0.0.1 api.github.com\n')
    class Handler(http_server.BaseHTTPRequestHandler):
        def do_GET(self):
            prefix='/repos/'+config['repository']
            if self.path==prefix+'/releases/latest':
                body=json.dumps(dict(draft=False,prerelease=False,tag_name=manifest['release']['id'],assets=assets)).encode()
            elif self.path.startswith(prefix+'/releases/assets/') and self.path.rsplit('/',1)[1] in by_id:
                body=by_id[self.path.rsplit('/',1)[1]].read_bytes()
            else:self.send_error(404);return
            self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        def log_message(self,*_):pass
    server=http_server.ThreadingHTTPServer(('127.0.0.1',443),Handler)
    context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(api_cert,api_key);server.socket=context.wrap_socket(server.socket,server_side=True)
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    try:
        try:
            command('systemctl','start','talocart-update.service')
        except AssertionError:
            # Synthetic diagnostic rerun is read-only; never bypass Execution.
            import fcntl
            import update_agent as agent
            from types import SimpleNamespace
            with (state/'agent.lock').open('r+b') as lock:
                fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
                coordinator.Preparation(config,feed,distribution,SimpleNamespace(connection=agent.connection,download=agent.download),engine).run()
            raise
        available=json.loads((state/'status.json').read_text());assert available['phase']=='available'
        assert available['plan']['base_action']==('update' if base_changed else 'keep') and available['plan']['modules'][0]['action']==('update' if module_changed else 'keep')
        assert available['candidate'] is not None
        command('systemctl','start','talocart-update.path')
        def request():
            before=json.loads((state/'status.json').read_text())['checked_at']
            status,html=http('GET','/admin/updates');assert status==200 and 'fixture-module' in html
            candidate=json.loads((state/'status.json').read_text())['candidate']
            status,_=http('POST','/admin/updates',dict(csrf=csrf_from(html),confirm='yes',**candidate));assert status==303
            deadline=time.monotonic()+150
            while time.monotonic()<deadline:
                value=json.loads((state/'status.json').read_text())
                if value['phase'] in ('failed','succeeded') and value['checked_at']!=before and not (state/'requests/install.json').exists() and not (state/'preparing-request.json').exists():
                    active=command('systemctl','show','-p','ActiveState','--value','talocart-update.service').strip()
                    if active in (b'inactive',b'failed'):return json.loads((state/'status.json').read_text())
                time.sleep(0.1)
            raise AssertionError('Owner-confirmed attempt did not finish')
        if Path('/test/interrupt.json').exists():
            from update_interruption_stage import interrupt_attempt
            sql("UPDATE fixture_module_jobs SET status='done'")
            interrupt_attempt(command,http,config,csrf_from)
            return
        refused=request();assert refused['phase']=='failed' and refused['notice']=='control_busy' and not refused['recovery_required']
        assert command('systemctl','show','-p','MainPID','--value',config['service'])==original_pid
        assert not (state/'maintenance').exists() and not (state/'writer-fence').exists()
        assert not list(Path(config['backup_root']).glob('*/data/backup.json')), 'Busy refusal must precede backup'
        assert sql('SELECT status FROM fixture_module_jobs')=='pending'
        status,html=http('GET','/admin/extensions/fixture-module');assert status==200 and '1.0.0' in html
        # Simulated job completion, not cancellation by the updater.
        sql("UPDATE fixture_module_jobs SET status='done'")
        installed=request();assert installed['phase']=='succeeded' and not installed['recovery_required']
        assert installed['backup_id'] and not launcher.incomplete()
        snapshot=coordinator.control(state,account.pw_uid,'inventory',configuration_sha256=hashlib.sha256(before_config).hexdigest())
        assert snapshot['modules'][0]['version']==target_version and snapshot['modules'][0]['enabled']
        if not module_changed:assert snapshot['modules']==original['modules'], 'Base-only update changed native executable or activation'
        if unchanged:
            previous=next(m for m in original['modules'] if m['id']==unchanged['id'])
            kept=next(m for m in snapshot['modules'] if m['id']==unchanged['id'])
            assert kept==previous and kept['sha256']==unchanged['sha256'] and kept['version']==unchanged['version']
            assert kept['enabled']==(not Path('/test/unchanged-disabled').exists())

        assert sql('SELECT status FROM fixture_module_jobs')=='done'
        assert sql('SELECT count(*) FROM _fixture_module_migrations')==('2' if module_changed else '1')
        assert json.loads((Path(config['current'])/'release.json').read_text())['version']==metadata['version']
        assert Path(config['config']).read_bytes()==before_config
        assert Path('/var/lib/talocart/media/synthetic-original.bin').read_bytes()==before_media
        status,html=http('GET','/admin/extensions/fixture-module');assert status==200 and target_version in html
        coordinator.Probe(config,engine).smoke()
        backup=Path(config['backup_root'])/installed['backup_id']/'data'
        assert json.loads((backup/'backup.json').read_text())['format']==1
        assert backup.stat().st_uid==0
        print(json.dumps(dict(integration_stage='base-and-module' if base_changed and module_changed else 'base-only' if base_changed else 'module-only',passed=True,owner_https_request=True,
            approved_signed_https_feed=True,real_native_module=True,real_busy_refusal_and_safe_retry=True,
            coherent_backup_id=installed['backup_id'],base_kept=not base_changed,initial_base=initial_metadata['version'],target_base=metadata['version'],module_kept=not module_changed,enabled_preserved=True,
            data_and_media_preserved=True,isolated_then_normal_activation=True,earlier_module_version=unchanged['version'] if unchanged else None,earlier_module_enabled=not Path('/test/unchanged-disabled').exists() if unchanged else None)))
        def publish_next():
            distribution.validate(manifest,config['repository'])
            raw=coordinator.canonical(manifest);(data/'distribution.json').write_bytes(raw);(data/'distribution.sig').write_bytes(feed.sign(raw,keys['app'][0]))
            assets[:]=[dict(id=i+1,name=path.name,size=path.stat().st_size) for i,path in enumerate(sorted(data.iterdir()))]
            by_id.clear();by_id.update({str(item['id']):data/item['name'] for item in assets})
            # Model passage of the hourly check interval; never remove recovery state.
            cached=json.loads((state/'status.json').read_text());cached['checked_epoch']=0
            coordinator.atomic(state/'status.json',cached,engine.trusted)
            command('systemctl','start','talocart-update.service')
            assert json.loads((state/'status.json').read_text())['phase']=='available'
        if not Path('/test/final-engine').exists():
            import updater_bundle
            # A correctly root-signed archive can still fail its runtime self-check.
            # It must not remain an ordinary retryable proposal for the same bytes.
            bad_source=work/'bad-engine-source'
            shutil.copytree('/test/root-source',bad_source)
            agent=bad_source/'update_agent.py';text=agent.read_text()
            marker="'engine_version': coordinator.ENGINE_VERSION"
            assert text.count(marker)==1
            # The builder binds the coordinator version to bundle metadata, so
            # explicitly corrupt the runtime report to exercise rejection.
            agent.write_text(text.replace(marker,"'engine_version': '0.0.0'"))
            bad_engine=updater_bundle.build(bad_source,'1.1.0',keys['root'][0],work/'bad-engine')
            for name in (bad_engine['asset'],bad_engine['signature_asset']):shutil.copyfile(work/'bad-engine'/name,data/name)
            manifest['release'].update(id='v11.14.0',sequence=2)
            manifest['minimum']['engine']='1.1.0';manifest['platforms'][0].update(updater=bad_engine,modules=[])
            before_bad_pid=command('systemctl','show','-p','MainPID','--value',config['service'])
            publish_next()
            status,html=http('GET','/admin/updates');assert status==200
            bad_candidate=json.loads((state/'status.json').read_text())['candidate']
            assert http('POST','/admin/updates',dict(csrf=csrf_from(html),confirm='yes',**bad_candidate))[0]==303
            deadline=time.monotonic()+60
            while time.monotonic()<deadline:
                if (state/'engine-handoff-failed.json').exists() and command('systemctl','show','-p','ActiveState','--value','talocart-update.service').strip()==b'failed':break
                time.sleep(0.1)
            else:raise AssertionError('Invalid self-check did not fail before engine activation')
            assert launcher.selected().name=='1.0.0'
            command('systemctl','start','talocart-update.service')
            blocked=json.loads((state/'status.json').read_text())
            assert blocked['phase']=='blocked' and blocked['plan']['blocked'], 'Failed engine must stay blocked until different approved bytes arrive'
            assert blocked['candidate'] is None and not launcher.incomplete()
            assert command('systemctl','show','-p','MainPID','--value',config['service'])==before_bad_pid
            print(json.dumps(dict(integration_stage='bad-engine-self-check',passed=True,previous_engine_retained=True,
                shop_unchanged=True,same_bytes_blocked=True,new_confirmation_required=True)))
            source=work/'engine-source-1.1';source.mkdir(mode=0o700)
            for name in launcher.FILES | {'update_plan.py','update_state.py','update_host.py','update_probe.py','maintenance.html'}:shutil.copyfile(Path('/test/root-source')/name,source/name)
            module=source/'update_coordinator.py';text=module.read_text();assert "ENGINE_VERSION = '1.0.0'" in text
            module.write_text(text.replace("ENGINE_VERSION = '1.0.0'","ENGINE_VERSION = '1.1.0'"))
            upgraded=updater_bundle.build(source,'1.1.0',keys['root'][0],work/'engine-1.1')
            for name in (upgraded['asset'],upgraded['signature_asset']):shutil.copyfile(work/'engine-1.1'/name,data/name)
            manifest['release'].update(id='v11.14.1',sequence=3)
            manifest['minimum']['engine']='1.1.0';manifest['platforms'][0].update(updater=upgraded,modules=[])
            engine_pid=command('systemctl','show','-p','MainPID','--value',config['service'])
            before_backups=set(Path(config['backup_root']).glob('*/data/backup.json'))
            publish_next();engine_result=request();assert engine_result['phase']=='succeeded'
            assert launcher.selected().name=='1.1.0' and (launcher.BUNDLES/'1.0.0').exists()
            assert command('systemctl','show','-p','MainPID','--value',config['service'])==engine_pid
            assert set(Path(config['backup_root']).glob('*/data/backup.json'))==before_backups
            assert coordinator.control(state,account.pw_uid,'inventory',configuration_sha256=hashlib.sha256(before_config).hexdigest())==snapshot
            assert json.loads((state/'deployed.json').read_text())['updater']==dict(version='1.1.0',sha256=upgraded['sha256'])
            assert Path(config['config']).read_bytes()==before_config and not launcher.incomplete()
            print(json.dumps(dict(integration_stage='engine-only',passed=True,owner_https_request=True,
                signed_root_handoff=True,new_process_acknowledged=True,previous_engine_retained=True,
                no_application_restart=True,no_backup_or_sql=True,native_inventory_unchanged=True)))
        if Path('/test/faults').exists():
            bad=packages/'v3-failing.talopkg';signed=bad.read_bytes()
            target=native_descriptor(signed)
            failing=data/('fixture-module-'+target['version']+'-failing.talopkg');shutil.copyfile(bad,failing)
            manifest['release'].update(id='v11.15.0',sequence=4)
            manifest['platforms'][0]['modules']=[record(failing,id=target['id'],version=target['version'],publisher=target['publisher'])]
            publish_next();failure=request()
            assert failure['phase']=='failed' and failure['recovery_required']
            assert (state/'maintenance').exists() and (state/'writer-fence').exists()
            assert command('systemctl','show','-p','MainPID','--value',config['service']).strip()==b'0'
            assert sql('SELECT count(*) FROM _fixture_module_migrations')=='3', 'Earlier migration must remain: no down migration'
            attempt=json.loads((state/'processing.json').read_text())['attempt']
            journal=json.loads((state/'attempts'/(attempt+'.json')).read_text())
            verified=journal['results']['backed_up']['id']
            assert verified==attempt and journal['pending']=='applying'
            assert failure['backup_id']==verified, 'Independent launcher must preserve the verified backup identity'
            print(json.dumps(dict(integration_stage='uncertain-sql',passed=True,writer_fence=True,
                backup_id=verified,no_automatic_rollback=True,application_stopped=True)))
            # Administrative recovery drill on NEW database and file paths.
            # The uncertain original remains fenced; restore never overwrites it.
            command('runuser','-u','postgres','--','createdb','-O','talocart_fixture','talocart_restored_test')
            restored=Path('/var/lib/talocart-restored');restored.mkdir(mode=0o700);os.chown(restored,account.pw_uid,account.pw_gid)
            restore_config=Path('/etc/talocart/restored.toml')
            restore_config.write_bytes(before_config.replace(b'talocart_integration_test',b'talocart_restored_test')
                .replace(b'/var/lib/talocart/',b'/var/lib/talocart-restored/').replace(b'127.0.0.1:3007',b'127.0.0.1:3008'))
            restore_config.chmod(0o640);os.chown(restore_config,0,account.pw_gid)
            backup=Path(config['backup_root'])/verified/'data'
            command('runuser','-u','talocart','--','/usr/bin/python3','-I',selected/'scripts/release/operations.py',
                'restore','--backup',backup,'--config',restore_config,timeout=300)
            command('runuser','-u','talocart','--',backup/'talocart','--config',restore_config,'doctor')
            native=json.loads(command('runuser','-u','talocart','--',backup/'talocart','--config',restore_config,'package-update-plan','--check-database'))
            assert native['inventory_sha256']==snapshot['inventory_sha256']
            assert native['modules'][0]['from']==target_version and native['modules'][0]['enabled'] and native['modules'][0]['action']=='keep'
            assert (restored/'media/synthetic-original.bin').read_bytes()==before_media
            restored_rows=command('runuser','-u','postgres','--','psql','-d','talocart_restored_test','-Atc',
                "SELECT status FROM fixture_module_jobs; SELECT count(*) FROM _fixture_module_migrations").decode().splitlines()
            assert restored_rows==['done','2' if module_changed else '1']
            import subprocess
            import urllib.request
            log=work/'restore-verification.log'
            with log.open('wb') as stream:
                process=subprocess.Popen([str(backup/'talocart'),'--config',str(restore_config),'serve-verify'],
                    user=account.pw_uid,group=account.pw_gid,cwd=restored,env={'PATH':'/usr/sbin:/usr/bin:/sbin:/bin','LANG':'C.UTF-8'},
                    stdout=stream,stderr=stream)
                try:
                    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
                    for _ in range(100):
                        assert process.poll() is None, 'Restored verification host exited'
                        try:
                            with opener.open('http://127.0.0.1:3008/health/ready',timeout=1) as response:
                                if response.read()==b'verification-ready\n':break
                        except OSError:time.sleep(0.1)
                    else:raise AssertionError('Restored verification readiness timed out')
                    with opener.open('http://127.0.0.1:3008/catalog',timeout=5) as response:assert response.status==200
                finally:
                    process.terminate()
                    try:assert process.wait(timeout=20)==0
                    except subprocess.TimeoutExpired:process.kill();process.wait();raise
            assert (state/'writer-fence').exists() and (state/'maintenance').exists()
            assert coordinator.Probe(config,engine)._request('/health/ready',private=False)[0]==503
            assert sql('SELECT count(*) FROM _fixture_module_migrations')=='3'
            print(json.dumps(dict(integration_stage='restore-new-targets',passed=True,new_database=True,new_media_and_packages=True,
                native_inventory_and_enabled_preserved=True,doctor=True,readonly_serving=True,
                backup_id=verified,uncertain_original_remains_fenced=True)))
    finally:
        command('systemctl','stop','talocart-update.timer','talocart-update.path')
        server.shutdown();server.server_close();thread.join(timeout=5)
