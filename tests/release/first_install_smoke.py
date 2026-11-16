"""New-server package installs the real tagged base AND updater; disposable host."""
import hashlib,json,os,pwd,secrets,shutil,subprocess,sys,tarfile,time,signal
from pathlib import Path
sys.dont_write_bytecode=True
sys.path.insert(0,'/test/root-source')
import updater_bundle as bundles
import updates_feed as feed


def command(*args,input=None,ok=True):
    r=subprocess.run(list(map(str,args)),input=input,capture_output=True,timeout=240)
    if ok and r.returncode:
        print(r.stdout.decode(),r.stderr.decode(),file=sys.stderr)
        for p in Path('/var/lib/talocart-update').glob('*failure.log'):
            print(p.read_text(),file=sys.stderr)
        raise AssertionError('First-install fixture command failed')
    return r


def main():
    assert sys.argv[1:]==['--disposable-container'] and Path('/.dockerenv').exists() and os.geteuid()==0
    os.umask(0o022)
    w=Path('/root/first-install');w.mkdir(mode=0o700)
    command('useradd','--system','--user-group','--home-dir','/var/lib/talocart','talocart')
    account=pwd.getpwnam('talocart')
    etc=Path('/etc/talocart');etc.mkdir(mode=0o750);os.chown(etc,0,account.pw_gid)
    command('systemctl','start','postgresql@17-main.service')
    password=secrets.token_hex(32)
    command('runuser','-u','postgres','--','psql','-v','ON_ERROR_STOP=1',input=("CREATE ROLE install_fixture LOGIN PASSWORD '"+password+"';").encode())
    command('runuser','-u','postgres','--','createdb','-O','install_fixture','talocart_install_test')
    app=etc/'talocart.toml';app.write_text('[server]\nlisten="127.0.0.1:3007"\npublic_origin="https://first-install.test:8443"\nsecure_cookies=true\n[database]\nurl="postgres://install_fixture:'+password+'@127.0.0.1:5432/talocart_install_test"\n[media]\nroot="/var/lib/talocart/media"\n[packages]\nroot="/var/lib/talocart/packages"\n[logging]\nroot="/var/lib/talocart/logs"\n')
    app.chmod(0o600);os.chown(app,account.pw_uid,account.pw_gid)
    keys={}
    for name in ('app','root'):
        key=w/(name+'.key');public=w/(name+'.pub');command('openssl','genpkey','-algorithm','ED25519','-out',key);key.chmod(0o600)
        public.write_bytes(feed.command(['openssl','pkey','-in',key,'-pubout']));public.chmod(0o644);keys[name]=(key,public)
    shutil.copyfile(keys['app'][1],etc/'update-public.pem')
    cert=etc/'fixture.crt';key=etc/'fixture.key'
    command('openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1','-subj','/CN=first-install.test',
            '-addext','subjectAltName=DNS:first-install.test','-keyout',key,'-out',cert)
    key.chmod(0o600);cert.chmod(0o644)
    command('systemctl','start','nginx')
    site=Path('/etc/nginx/sites-available/first-install')
    site.write_text('server { listen 127.0.0.1:8443 ssl; server_name first-install.test;\nssl_certificate '+str(cert)+'; ssl_certificate_key '+str(key)+';\ninclude /etc/talocart/nginx-update-server.conf;\nlocation / { proxy_set_header Host first-install.test:8443; proxy_set_header X-Forwarded-Proto https; proxy_set_header X-Talocart-Update-Probe ""; proxy_pass http://127.0.0.1:3007; }\n}\n')
    (Path('/etc/nginx/sites-enabled')/site.name).symlink_to(site)
    c=dict(feed_format=2,environment='staging',service_user='talocart',service='talocart.service',release_root='/opt/talocart/releases',current='/opt/talocart/current',config=str(app),backup_root='/var/backups/talocart',incoming='/var/lib/talocart-update/incoming',public_origin='https://first-install.test:8443',ready_url='http://127.0.0.1:3007/health/ready',writer_units=[],writers_fenced=True,repository='synthetic/fixture',public_key=str(etc/'update-public.pem'),token_file='',updater_public_key='/etc/talocart/updater-root-public.pem',probe_token_file='/etc/talocart/update-probe-token',probe_ca_file=str(cert))
    config=w/'update.toml';config.write_text(''.join(k+'='+json.dumps(v)+'\n' for k,v in c.items()));config.chmod(0o600)
    artifacts=w/'artifacts';artifacts.mkdir(mode=0o700)
    meta=json.loads(Path('/test/base/release.json').read_text());version=meta['version'];target=meta['target']
    base=artifacts/('talocart-'+version+'-'+target+'.tar.gz');shutil.copyfile('/test/base.tar.gz',base)
    engine=bundles.build(Path('/test/root-source'),'1.0.0',keys['root'][0],w/'engine')
    for p in (w/'engine').iterdir():shutil.copyfile(p,artifacts/p.name)
    entry=dict(asset=base.name,size=base.stat().st_size,sha256=feed.digest(base),version=version,commit=meta['source_commit'],contracts={k:meta['build'][k] for k in ('sdk_version','protocol','database')})
    manifest=dict(format=2,channel='stable',repository=c['repository'],release=dict(id='v'+version,sequence=1,commit=meta['source_commit']),minimum=dict(launcher='1.0.0',engine='1.0.0'),platforms=[dict(target=target,base=entry,modules=[],updater=engine)])
    raw=(json.dumps(manifest,sort_keys=True,separators=(',',':'))+'\n').encode();(artifacts/'distribution.json').write_bytes(raw);(artifacts/'distribution.sig').write_bytes(feed.sign(raw,keys['app'][0]))
    repo=w/'repository';(repo/'scripts').mkdir(parents=True);shutil.copytree('/test/root-source',repo/'scripts/release');shutil.copytree('/test/distribution',repo/'distribution')
    interrupted = any(Path('/test/first-install-'+mode).exists() for mode in ('kill','reboot','disk-full'))
    if interrupted:
        # Fault injection is confined to the synthetic root-signed kit.
        source = repo/'scripts/release/install_server.py'
        raw = source.read_text()
        if Path('/test/first-install-disk-full').exists():
            injection = "e.run(['mount', '-t', 'tmpfs', '-o', 'size=4k', 'tmpfs', str(l.STATE)]); (l.STATE/'full').write_bytes(bytes(4096)); raise OSError('synthetic full state filesystem')"
        else:
            injection = "__import__('signal'); os.kill(os.getpid(), __import__('signal').SIGSTOP)"
        source.write_text(raw.replace('                e.health(c)', '                '+injection+'\n                e.health(c)'))
    result=bundles.build_installation(repo,meta['source_commit'],'v'+version,target,artifacts,keys['app'][1],keys['root'][1],keys['root'][0],w/'package')
    archive=w/'package'/result['asset'];feed.verify_signature(archive.read_bytes(),(w/'package'/result['signature_asset']).read_bytes(),keys['root'][1],512*1024**2)
    unpack=w/'unpacked';unpack.mkdir(mode=0o700)
    with tarfile.open(archive) as t:t.extractall(unpack,filter='data')
    argv=['/usr/bin/python3','-I',str(unpack/'install_server.py'),'--config-source',str(config),'--root-public-key',str(keys['root'][1]),'--nginx-site',str(site)]
    before=app.read_bytes();preview=command(*argv)
    assert json.loads(preview.stdout)['updater_required'] and not Path('/opt/talocart/current').exists() and app.read_bytes()==before
    # Signature failure must precede any database, unit or app-config mutation.
    signature=unpack/'distribution.sig';saved=signature.read_bytes();signature.write_bytes(bytes(64))
    assert command(*argv,'--apply',ok=False).returncode!=0 and not Path('/opt/talocart/current').exists() and app.read_bytes()==before
    signature.write_bytes(saved)
    if Path('/test/first-install-database').exists():
        command('runuser','-u','postgres','--','psql','-d','talocart_install_test','-c','CREATE TABLE preserved(value integer); INSERT INTO preserved VALUES (7)')
        assert command(*argv,'--apply',ok=False).returncode!=0
        assert command('runuser','-u','postgres','--','psql','-At','-d','talocart_install_test','-c','SELECT value FROM preserved').stdout.strip()==b'7'
        assert not Path('/opt/talocart/current').exists()
        assert Path('/var/lib/talocart-update/writer-fence').exists()
        print(json.dumps(dict(nonempty_database_rejected=True,data_preserved=True,failed_install_fenced=True)))
        return
    if Path('/test/first-install-tls').exists():
        site.write_text(site.read_text().replace('ssl_certificate '+str(cert), 'ssl_certificate /missing-install-certificate'))
        assert command(*argv,'--apply',ok=False).returncode!=0
        assert command('systemctl','is-active','talocart.service',ok=False).stdout.strip()!=b'active'
        assert Path('/var/lib/talocart-update/writer-fence').exists() and Path('/var/lib/talocart-update/maintenance').exists()
        print(json.dumps(dict(tls_failure=True,application_stopped=True,failed_install_fenced=True)))
        return
    if interrupted:
        if Path('/test/first-install-disk-full').exists():
            assert command(*argv,'--apply',ok=False).returncode != 0
            command('umount', '/var/lib/talocart-update')
        else:
            process = subprocess.Popen([*argv, '--apply'])
            deadline = time.monotonic()+120
            while time.monotonic() < deadline:
                assert process.poll() is None, 'Installer exited before injection'
                if '\nState:\tT' in Path('/proc/'+str(process.pid)+'/status').read_text(): break
                time.sleep(.1)
            else: raise AssertionError('Installer never reached running-app boundary')
            assert command('systemctl','is-active','talocart.service').stdout.strip()==b'active'
            # A boot must not bypass the pending-installation dependency.
            command('systemctl','enable','talocart.service')
            if Path('/test/first-install-reboot').exists():
                Path('/test/reboot-ready').touch()
                print(json.dumps(dict(first_install_reboot_ready=True)))
                return
            process.kill();process.wait(timeout=10)
        deadline=time.monotonic()+20
        while time.monotonic()<deadline:
            if command('systemctl','is-active','talocart.service',ok=False).stdout.strip()!=b'active':break
            time.sleep(.1)
        else:raise AssertionError('Interrupted install left application running')
        command('systemctl','reset-failed','talocart-installation.service','talocart.service',ok=False)
        assert command('systemctl','start','talocart.service',ok=False).returncode!=0
        assert json.loads(Path('/var/lib/talocart-update/installation.json').read_text())['phase']=='installing'
        print(json.dumps(dict(first_install_interruption=True,application_stopped=True,restart_rejected=True)))
        return
    installed=json.loads(command(*argv,'--apply').stdout);assert installed['installed'] and installed['updater_installed']
    assert all(command('systemctl','is-active',u).stdout.strip()==b'active' for u in ('talocart.service','talocart-update.timer','talocart-update.path'))
    assert b'[updates]' in app.read_bytes() and app.stat().st_uid==account.pw_uid and app.stat().st_mode&0o077==0
    assert command(*argv,'--apply',ok=False).returncode!=0, 'Second install overwrote an active server'
    command('systemctl','stop','talocart.service','talocart-installation.service')
    command('systemctl','start','talocart.service')
    import update_engine as e
    e.health(c)
    actual=json.loads(command('/opt/talocart/current/bin/talocart','build-info').stdout);assert actual==meta['build']
    import update_coordinator as co,update_engine as e
    probe=co.Probe(c,e);probe.check('/health/ready')
    import http.client,socket,ssl,re
    from urllib.parse import urlencode
    class LocalHTTPS(http.client.HTTPSConnection):
        def connect(connection):
            raw=socket.create_connection(('127.0.0.1',connection.port),10)
            connection.sock=connection._context.wrap_socket(raw,server_hostname=connection.host)
    cookies={}
    def http(method,path,form=None):
        connection=LocalHTTPS('first-install.test',8443,context=ssl.create_default_context(cafile=cert),timeout=15)
        headers={'Cookie':'; '.join(k+'='+v for k,v in cookies.items()),'Origin':c['public_origin']}
        if form is not None:headers['Content-Type']='application/x-www-form-urlencoded'
        connection.request(method,path,None if form is None else urlencode(form),headers)
        r=connection.getresponse();body=r.read().decode()
        for k,v in r.getheaders():
            if k.lower()=='set-cookie':
                name,value=v.split(';',1)[0].split('=',1);cookies[name]=value
        status=r.status;connection.close();return status,body
    status,html=http('GET','/setup');assert status==200
    csrf=re.search(r'name="csrf" value="([^"]+)"',html)[1]
    status,_=http('POST','/setup',dict(csrf=csrf,email='owner@example.test',password=secrets.token_hex(24),name='First install',display_name='Owner',primary_locale='en',locale='en'))
    assert status==303
    probe.smoke()
    assert not any((Path('/var/lib/talocart-update')/n).exists() for n in ('maintenance','writer-fence','processing.json'))
    print(json.dumps(dict(first_install=True,base=version,updater='1.0.0',signature_rejection=True,repeat_rejected=True,preview_readonly=True,https=True,scheduling=True)))


if __name__=='__main__':main()
