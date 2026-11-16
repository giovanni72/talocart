"""Actual administrative bootstrap on disposable Ubuntu/systemd, synthetic ELF host.

The host implements only health/control for infrastructure checks. This does not
qualify application/database upgrades, which use the final Talocart artifacts.
"""
import hashlib
import json
import os
from pathlib import Path
import pwd
import subprocess
import sys
import time

sys.path.insert(0,'/test/scripts/release')
import bootstrap_updater as bootstrap
import update_launcher as launcher
import updater_bundle


def command(*args, ok=True):
    result=subprocess.run(list(map(str,args)),capture_output=True,timeout=120)
    if ok and result.returncode:
        print(result.stderr.decode(),file=sys.stderr)
        raise AssertionError('Synthetic bootstrap command failed')
    return result


def main():
    assert sys.argv[1:]==['--disposable-container'] and Path('/.dockerenv').exists() and os.geteuid()==0
    assert Path('/proc/1/comm').read_text().strip()=='systemd'
    assert not launcher.STATE.exists(), 'Bootstrap fixture needs a separate fresh container'
    os.umask(0o022)
    stage=Path('/root/bootstrap-source'); stage.mkdir(mode=0o700)
    for name in launcher.FILES|{'update_launcher.py','bootstrap_updater.py'}:
        (stage/name).write_bytes(updater_bundle.coordinator_source(Path('/test/scripts/release')) if name == 'update_coordinator.py' else (Path('/test/scripts/release')/name).read_bytes())
    keys={}
    for role in ('root','app'):
        private,public=stage/(role+'-private.pem'),stage/(role+'-public.pem')
        command('openssl','genpkey','-algorithm','ED25519','-out',private); private.chmod(0o600)
        public.write_bytes(command('openssl','pkey','-in',private,'-pubout').stdout)
        keys[role]=(private,public)
    artifact=updater_bundle.build(stage,'1.0.0',keys['root'][0],stage/'dist')
    ca,cakey,cert,key=[stage/name for name in ('ca.pem','ca.key','server.pem','server.key')]
    command('openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1','-subj','/CN=Bootstrap synthetic CA','-keyout',cakey,'-out',ca)
    command('openssl','req','-newkey','rsa:2048','-nodes','-subj','/CN=bootstrap-updater.test','-keyout',key,'-out',stage/'server.csr')
    (stage/'extensions').write_text('subjectAltName=DNS:bootstrap-updater.test\nextendedKeyUsage=serverAuth\n')
    command('openssl','x509','-req','-in',stage/'server.csr','-CA',ca,'-CAkey',cakey,'-CAcreateserial','-days','1','-extfile',stage/'extensions','-out',cert)
    for path in (cakey,key): path.chmod(0o600)
    account=pwd.getpwnam('nobody')
    state=launcher.STATE; state.mkdir(mode=0o755)
    requests=state/'requests'; requests.mkdir(mode=0o700); os.chown(requests,account.pw_uid,account.pw_gid)
    etc=Path('/etc/talocart'); etc.mkdir(mode=0o750); os.chown(etc,0,account.pw_gid)
    app=etc/'talocart.toml'
    package_root=Path('/var/lib/talocart-bootstrap/packages'); package_root.mkdir(parents=True); os.chown(package_root,account.pw_uid,account.pw_gid)
    registry=package_root/'registry.json'; registry.write_text('{"publishers":[],"installed":[]}'); registry.chmod(0o600); os.chown(registry,account.pw_uid,account.pw_gid)
    app.write_text('[packages]\nroot="'+str(package_root)+'"\n[server]\nlisten="127.0.0.1:3007"\npublic_origin="https://bootstrap-updater.test:8443"\nsecure_cookies=true\n[updates]\nroot="'+str(state)+'"\n')
    app.chmod(0o640); os.chown(app,0,account.pw_gid)
    app_hash=hashlib.sha256(app.read_bytes()).hexdigest()
    releases=Path('/opt/talocart/releases'); (releases/'synthetic/bin').mkdir(parents=True)
    current=releases.parent/'current'; current.symlink_to(releases/'synthetic')
    build=dict(version='1.0.0',commit='a'*40,source_dirty=False,tagged=True)
    source=r'''
#define _GNU_SOURCE
#include <sys/socket.h>
#include <sys/un.h>
#include <netinet/in.h>
#include <poll.h>
#include <unistd.h>
#include <sys/stat.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
int main(int argc,char **argv) {
 if(argc==2 && !strcmp(argv[1],"build-info")){puts(BUILD_INFO);return 0;}
 if(argc!=4 || strcmp(argv[1],"--config") || strcmp(argv[3],"serve"))return 2;
 int tcp=socket(AF_INET,SOCK_STREAM,0),un=socket(AF_UNIX,SOCK_STREAM,0),one=1;
 setsockopt(tcp,SOL_SOCKET,SO_REUSEADDR,&one,sizeof(one));
 struct sockaddr_in addr={.sin_family=AF_INET,.sin_port=htons(3007),.sin_addr.s_addr=htonl(INADDR_LOOPBACK)};
 struct sockaddr_un local={.sun_family=AF_UNIX};strcpy(local.sun_path,"/var/lib/talocart-update/requests/control.sock");
 unlink(local.sun_path);
 if(bind(tcp,(void*)&addr,sizeof(addr)) || bind(un,(void*)&local,sizeof(local)))return 3;
 chmod(local.sun_path,0600);listen(tcp,8);listen(un,8);
 struct pollfd fds[2]={{.fd=tcp,.events=POLLIN},{.fd=un,.events=POLLIN}};
 for(;;){if(poll(fds,2,-1)<0)continue;
 for(int i=0;i<2;i++){if(!(fds[i].revents&POLLIN))continue;int fd=accept(fds[i].fd,NULL,NULL);if(fd<0)continue;
 char input[4096]={0};read(fd,input,sizeof(input)-1);
 const char *reply=i?"{\"ok\":true,\"snapshot\":{\"inventory_sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"modules\":[],\"suspended\":false},\"error\":null,\"configuration_sha256\":\"CONFIG_HASH\"}\n":"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nready\n";
 write(fd,reply,strlen(reply));close(fd);}}
}
'''.replace('CONFIG_HASH',app_hash).replace('BUILD_INFO',json.dumps(json.dumps(build)))
    (stage/'host.c').write_text(source)
    command('gcc','-O2','-o',releases/'synthetic/bin/talocart',stage/'host.c')
    ops=releases/'synthetic/scripts/release/operations.py'; ops.parent.mkdir(parents=True); ops.write_text('# Synthetic file; not executed by infrastructure bootstrap\n')
    files={p.relative_to(releases/'synthetic').as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in (releases/'synthetic').rglob('*') if p.is_file()}
    (releases/'synthetic/release.json').write_text(json.dumps(dict(format=1,profile='base',source_dirty=False,version=build['version'],source_commit=build['commit'],build=build,files=files)))
    unit=Path('/etc/systemd/system/talocart-bootstrap-app.service')
    unit.write_text('[Service]\nType=simple\nUser=nobody\nGroup='+account.pw_name.replace('nobody','nogroup')+'\nExecStart='+str(current/'bin/talocart')+' --config '+str(app)+' serve\n')
    # Existing legacy scheduling is present, idle and never executes application code.
    Path('/etc/systemd/system/talocart-update.service').write_text('[Service]\nType=oneshot\nExecStart=/bin/true\n')
    Path('/etc/systemd/system/talocart-update.timer').write_text(bootstrap.TIMER_TEXT)
    command('systemctl','daemon-reload'); command('systemctl','start',unit.name)
    for _ in range(50):
        if (requests/'control.sock').exists():break
        time.sleep(0.1)
    else: raise AssertionError('Synthetic host did not start')
    initial_pid=command('systemctl','show','-p','MainPID','--value',unit.name).stdout.strip()
    app_public=etc/'update-public.pem'; app_public.write_bytes(keys['app'][1].read_bytes())
    nginx_site=Path('/etc/nginx/conf.d/bootstrap-site.conf')
    nginx_site.write_text('server { listen 127.0.0.1:8443 ssl; server_name bootstrap-updater.test;\nssl_certificate '+str(cert)+';\nssl_certificate_key '+str(key)+';\ninclude /etc/talocart/nginx-update-server.conf;\nlocation / { proxy_set_header X-Talocart-Update-Probe ""; proxy_pass http://127.0.0.1:3007; }\n}\n')
    # The staged HTTPS include is loaded after generated snippets exist. Start Nginx
    # on the distro default configuration first, then provision this staged site.
    nginx_site.rename(stage/'site.pending')
    command('systemctl','start','nginx.service')
    (stage/'site.pending').rename(nginx_site)
    backup=Path('/var/backups/talocart');backup.mkdir();os.chown(backup,account.pw_uid,account.pw_gid)
    config=dict(feed_format=2,environment='staging',service_user='nobody',service=unit.name,
                release_root=str(releases),current=str(current),config=str(app),backup_root=str(backup),incoming=str(state/'incoming'),
                public_origin='https://bootstrap-updater.test:8443',ready_url='http://127.0.0.1:3007/health/ready',
                writer_units=[],writers_fenced=True,repository='synthetic/fixture',public_key=str(app_public),token_file='',
                updater_public_key=str(etc/'root-code-public.pem'),probe_token_file=str(etc/'probe-token'),probe_ca_file=str(ca))
    staged_config=stage/'update.toml'; staged_config.write_text(''.join(k+'='+json.dumps(v)+'\n' for k,v in config.items()));staged_config.chmod(0o600)
    argv=['/usr/bin/python3','-I',str(stage/'bootstrap_updater.py'),'--config-source',str(staged_config),
          '--engine',str(stage/'dist'/artifact['asset']),'--signature',str(stage/'dist'/artifact['signature_asset']),
          '--engine-version','1.0.0','--root-public-key',str(keys['root'][1]),'--nginx-site',str(nginx_site)]
    preview=json.loads(command(*argv).stdout)
    assert not preview['application_deploy'] and not launcher.CURRENT.exists() and not launcher.CONFIG.exists()
    assert not bootstrap.HTTP.exists() and app_hash==hashlib.sha256(app.read_bytes()).hexdigest()
    bad_signature=stage/'bad.sig'; bad_signature.write_bytes(bytes(64))
    rejected=argv.copy(); rejected[rejected.index('--signature')+1]=str(bad_signature)
    assert command(*rejected,'--apply',ok=False).returncode==1
    assert not launcher.CURRENT.exists() and not launcher.CONFIG.exists() and not bootstrap.HTTP.exists()
    root_der=command('openssl','pkey','-pubin','-in',keys['root'][1],'-outform','DER').stdout
    registry.write_text(json.dumps(dict(publishers=[root_der[-32:].hex()],installed=[])))
    assert command(*argv,'--apply',ok=False).returncode==1, 'A module publisher key authorized root code'
    assert not launcher.CURRENT.exists() and not launcher.CONFIG.exists()
    registry.write_text('{"publishers":[],"installed":[]}')
    original_site=nginx_site.read_text()
    nginx_site.write_text(original_site.replace('proxy_set_header X-Talocart-Update-Probe "";',''))
    assert command(*argv,'--apply',ok=False).returncode==1, 'Missing upstream token stripping passed bootstrap'
    assert not launcher.CURRENT.exists() and not launcher.CONFIG.exists()
    nginx_site.write_text(original_site)
    installed=command(*argv,'--apply',ok=False)
    if installed.returncode:
        print(installed.stderr.decode(),file=sys.stderr)
        # Only synthetic container data, never host credentials.
        for log in state.glob('failure.log'): print(log.read_text()[-8192:],file=sys.stderr)
        print(command('journalctl','-u','nginx.service','-n','15','--no-pager',ok=False).stdout.decode(),file=sys.stderr)
        raise AssertionError('Bootstrap installation failed')
    result=json.loads(installed.stdout)
    assert result['installed'] and launcher.selected().name=='1.0.0'
    assert command('systemctl','is-enabled','talocart-update.timer').stdout.strip()==b'enabled'
    assert command('systemctl','is-active','talocart-update.path').stdout.strip()==b'active'
    assert command('systemctl','show','-p','MainPID','--value',unit.name).stdout.strip()==initial_pid
    assert app_hash==hashlib.sha256(app.read_bytes()).hexdigest() and not launcher.incomplete()
    assert stat_mode(backup)==0o750 and backup.stat().st_uid==0
    assert stat_mode(Path(config['probe_token_file']))==0o600 and stat_mode(bootstrap.HTTP)==0o600
    assert json.loads((state/'bootstrap.json').read_text())['engine_sha256']==artifact['sha256']
    # Explicit rerun is idempotent and does not rotate a root key or restart the host.
    assert json.loads(command(*argv,'--apply').stdout)['installed']
    assert command('systemctl','show','-p','MainPID','--value',unit.name).stdout.strip()==initial_pid
    command('systemctl','stop','talocart-update.timer','talocart-update.path')
    print(json.dumps(dict(bootstrap_smoke='passed',ubuntu=Path('/etc/os-release').read_text().splitlines()[0],
          actual_root_cli=True,signed_engine=True,preview_preserves_files=True,no_application_restart=True,
          real_nginx_tls_gate=True,private_permissions=True,scheduling_enabled=True,idempotent_rerun=True)))


def stat_mode(path): return path.stat().st_mode&0o777


if __name__=='__main__': main()
