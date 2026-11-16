#!/usr/bin/python3 -I
"""Administrative, one-time installation of the unified updater. Preview by default.

Run from a separately reviewed root-owned source directory, never an application
release. Requires a running compatible base already exposing the root control
socket. Does not deploy application code, migrate SQL or rewrite module state.
"""
import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import pwd
import re
import secrets
import stat
import sys
import tempfile
import tomllib

sys.dont_write_bytecode = True

INSTALL = Path('/usr/local/libexec/talocart')
HTTP = Path('/etc/nginx/conf.d/talocart-update-http.conf')
SERVER = Path('/etc/talocart/nginx-update-server.conf')
SERVICE = Path('/etc/systemd/system/talocart-update.service')
TIMER = Path('/etc/systemd/system/talocart-update.timer')
REQUEST_PATH = Path('/etc/systemd/system/talocart-update.path')

SERVICE_TEXT = '''[Unit]
Description=Talocart approved distribution check and requested update
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
User=root
Group=root
ExecStart=/usr/bin/python3 -I /usr/local/libexec/talocart/update_launcher.py
TimeoutStartSec=6h
UMask=0077
RuntimeDirectory=talocart-update
RuntimeDirectoryMode=0700
RuntimeDirectoryPreserve=yes
'''
TIMER_TEXT = '''[Unit]
Description=Check Talocart update availability and pending requests

[Timer]
OnBootSec=1min
OnUnitInactiveSec=1min
RandomizedDelaySec=10
Unit=talocart-update.service

[Install]
WantedBy=timers.target
'''
PATH_TEXT = '''[Unit]
Description=Process owner-confirmed Talocart update and manual check requests

[Path]
PathExists=/var/lib/talocart-update/requests/install.json
PathExists=/var/lib/talocart-update/requests/check.json
Unit=talocart-update.service

[Install]
WantedBy=multi-user.target
'''


def require(value, message):
    if not value: raise ValueError(message)


def trusted(path):
    for item in [path, *path.parents]:
        info = item.lstat()
        require(not stat.S_ISLNK(info.st_mode) and info.st_uid == 0 and not info.st_mode & 0o022,
                'Administrative input must be root-owned without writable ancestors')


def sibling(name):
    path = Path(__file__).with_name(name + '.py')
    trusted(path)
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def bounded(path, limit, *, owner=None):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= limit
                and (owner is None or info.st_uid == owner), 'Invalid administrative input')
        raw = stream.read(limit + 1)
    require(len(raw) <= limit, 'Administrative input exceeds limit')
    return raw


def parent_trust(path):
    require(path.is_absolute() and '..' not in path.parts, 'Absolute managed paths required')
    existing = path
    while not existing.exists() and not existing.is_symlink(): existing = existing.parent
    trusted(existing)


def public_key(path, launcher):
    # DER has exactly one Ed25519 SPKI encoding; never compare PEM whitespace.
    raw = launcher.command(['openssl','pkey','-pubin','-in',path,'-pubout','-outform','DER'])
    require(len(raw) == 44 and raw[:12].hex() == '302a300506032b6570032100', 'Ed25519 public key required')
    return raw


def proxy_headers(site):
    """Validate direct proxy blocks in the supported self-contained virtual host.

    Preserve quoted punctuation/comments; do not count a server-level header as
    proof for a location with its own proxy headers. Nginx still validates syntax.
    """
    tokens=re.findall(r"\s+|\#[^\n]*|\"(?:\\.|[^\"\\])*\"|'(?:\\.|[^'\\])*'|[{};]|[^\s{};#]+",site)
    blocks=[[]]; statement=[]; proxies=0
    for token in tokens:
        if token.isspace() or token.startswith('#'): continue
        if token=='{':
            require(statement, 'Unsupported proxy configuration block')
            blocks.append([]); statement=[]
        elif token==';':
            require(statement, 'Unsupported empty proxy directive')
            blocks[-1].append(statement); statement=[]
        elif token=='}':
            require(not statement and len(blocks)>1, 'Unbalanced proxy configuration')
            block=blocks.pop()
            if any(row[0]=='proxy_pass' for row in block):
                proxies+=1
                values=[row[2:] for row in block if len(row)>1 and row[0]=='proxy_set_header' and row[1].lower()=='x-talocart-update-probe']
                require(values and all(value==[''] for value in values) and not any(row[0]=='include' for row in block),
                        'Clear the private probe header directly in every self-contained proxy location')
        else:
            statement.append(token[1:-1] if token.startswith(('\"',"'")) else token)
    require(len(blocks)==1 and not statement and proxies>0,'Unsupported or missing proxy locations')


def gateway_ready(probe):
    """Nginx reload acknowledgement precedes activation of a new TLS listener.

    Wait for the listener and the freshly loaded probe header. Old workers may
    still serve the previous configuration after reload. TLS, mode and other
    configuration errors remain immediate; all retries have a fixed deadline.
    """
    import time
    deadline=time.monotonic()+5
    while True:
        try:
            probe.check('/health/ready')
            return
        except (ConnectionRefusedError, ValueError) as error:
            if not isinstance(error, ConnectionRefusedError) and (type(error) is not ValueError or str(error) != 'Private HTTPS probe failed'):
                raise
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.05)


class Bootstrap:
    def __init__(self, args, config, raw, launcher, engine, coordinator):
        self.args, self.config, self.raw = args, config, raw
        self.launcher, self.engine, self.c = launcher, engine, coordinator
        self.state = launcher.STATE
        self.account = pwd.getpwnam(config['service_user'])
        require(self.account.pw_uid > 0 and self.account.pw_gid > 0, 'Unprivileged application account required')
        self.app = Path(config['config'])
        self.key = Path(config['updater_public_key'])
        self.token = Path(config['probe_token_file'])

    def preflight(self):
        l, e, c, config = self.launcher, self.engine, self.c, self.config
        e.check_config(config)
        require(config.get('feed_format') == 2 and not config.get('bootstrap_updates', False), 'Use a complete format-2 configuration for bootstrap')
        require(self.args.config_source.resolve() != l.CONFIG.resolve(), 'Stage the new configuration separately from the live configuration')
        require(not l.incomplete(), 'Resolve incomplete application operations before bootstrap')
        require(re.fullmatch(l.SEMVER, self.args.engine_version), 'Invalid initial engine version')
        destinations = [self.app,l.CONFIG,self.key,self.token,Path(config['public_key'])]
        if config.get('token_file'): destinations.append(Path(config['token_file']))
        require(len(destinations) == len(set(destinations)) and self.key.parent == l.CONFIG.parent
                and self.token.parent == l.CONFIG.parent, 'Use distinct managed configuration, key and token paths')
        if self.token.exists() or self.token.is_symlink():
            trusted(self.token)
            require(self.token.stat().st_mode & 0o077 == 0 and re.fullmatch(r'[0-9a-f]{64}', l.read(self.token,65).decode().strip()),
                    'Existing probe token must be private and valid')
        for path in (self.args.engine, self.args.signature, self.args.root_public_key, self.args.nginx_site,
                     self.args.config_source, Path(config['public_key'])): trusted(path)
        root = public_key(self.args.root_public_key, l)
        require(root != public_key(Path(config['public_key']), l), 'Root code and application signing keys must differ')
        if self.key.exists() or self.key.is_symlink():
            trusted(self.key)
            require(public_key(self.key, l) == root, 'Root-key rotation requires separate administrative maintenance')
        archive = l.read(self.args.engine, l.MAX_BUNDLE)
        require(len(l.read(self.args.signature,64)) == 64, 'Invalid root signature size')
        l.command(['openssl','pkeyutl','-verify','-rawin','-pubin','-inkey',self.args.root_public_key,
                   '-in',self.args.engine,'-sigfile',self.args.signature])
        self.archive_sha256 = hashlib.sha256(archive).hexdigest()
        self.root_key = root
        e.runtime_configuration(config)
        app_raw = c.root_bytes(self.app, e, 1024**2, app_config=True)
        app = tomllib.loads(app_raw.decode())
        require(app.get('updates',{}).get('root') == str(self.state), 'Install a compatible running base and update channel before bootstrap')
        package_root = Path(app.get('packages',{}).get('root','storage/packages'))
        if not package_root.is_absolute(): package_root = self.app.parent / package_root
        registry = package_root / 'registry.json'
        if registry.exists() or registry.is_symlink():
            data = json.loads(bounded(registry,16*1024**2,owner=self.account.pw_uid))
            require(isinstance(data,dict) and isinstance(data.get('publishers'),list)
                    and root[-32:].hex() not in data['publishers'], 'Root code key must differ from trusted module publishers')
        self.app_sha256 = hashlib.sha256(app_raw).hexdigest()
        c.check_units(config,e,guards=False)
        current=Path(config['current'])
        selected=current.resolve(strict=True)
        require(current.is_symlink() and selected.parent == Path(config['release_root']), 'Invalid managed current release')
        installed=c.check_tree(selected,e)
        require(json.loads(c.Operations(config,e).application(selected/'bin/talocart','build-info')) == installed['build'],
                'Current executable does not match the installed release identity')
        e.running_binary(config, selected / 'bin/talocart')
        snapshot = c.control(self.state,self.account.pw_uid,'inventory',configuration_sha256=self.app_sha256)
        require(not snapshot['suspended'], 'Resume or recover the host before bootstrap')
        self.engine.health(config)
        require(e.run(['systemctl','show','-p','ActiveState','--value','talocart-update.service']) not in ('active','activating','deactivating'),
                'Wait for the existing updater to finish before bootstrap')
        site=l.read(self.args.nginx_site,1024**2).decode()
        require('include /etc/talocart/nginx-update-server.conf;' in site,
                'Prepare the HTTPS site include before bootstrap; remove the legacy maintenance return')
        proxy_headers(site)
        for name in ('release_root','incoming'):
            path=Path(config[name])
            parent_trust(path)
        backup=Path(config['backup_root'])
        parent_trust(backup.parent)
        if backup.exists() or backup.is_symlink():
            info=backup.lstat()
            require(stat.S_ISDIR(info.st_mode) and info.st_uid in (0,self.account.pw_uid), 'Invalid existing backup root')
        for path in (INSTALL,l.RUNTIME,HTTP.parent,SERVER.parent,SERVICE.parent,self.key,self.token): parent_trust(path.parent if path in (self.key,self.token) else path)
        if l.CURRENT.exists() or l.CURRENT.is_symlink():
            current=l.selected()
            require(current.name == self.args.engine_version and l.engine_receipt(current.name) == self.archive_sha256,
                    'Bootstrap cannot replace an already selected engine with different code')
        self.files = {INSTALL/'update_launcher.py':(l.read(Path(l.__file__),l.MAX_BUNDLE),0o644),
                      l.CONFIG:(self.raw,0o600),self.key:(l.read(self.args.root_public_key,8192),0o644),
                      SERVICE:(SERVICE_TEXT.encode(),0o644),TIMER:(TIMER_TEXT.encode(),0o644),REQUEST_PATH:(PATH_TEXT.encode(),0o644)}
        for unit in [config['service'],*config['writer_units']]:
            path=SERVICE.parent/(unit+'.d')/'90-talocart-update.conf'
            parent_trust(path.parent)
            self.files[path]=(c.unit_guard(unit==config['service']).encode(),0o644)
        for path in self.files:
            if path.exists() or path.is_symlink():
                trusted(path); require(path.is_file(), 'Managed output is not a regular file')
        return dict(format=1,operation='bootstrap-unified-updater',engine_version=self.args.engine_version,
                    engine_sha256=self.archive_sha256,root_key_sha256=hashlib.sha256(root).hexdigest(),
                    files=[str(path) for path in [*self.files,self.token,HTTP,SERVER]],
                    directories=[dict(path=str(path),mode=oct(mode),uid=0,gid=gid) for path,mode,gid in self.layout()],
                    application_deploy=False,activation='enable timer and request path after TLS/gate checks')

    def layout(self):
        return [(self.state,0o755,0),(INSTALL,0o755,0),(self.launcher.BUNDLES,0o755,0),(self.launcher.RUNTIME,0o700,0),
                (Path(self.config['release_root']),0o755,0),(Path(self.config['incoming']),0o700,0),
                (Path(self.config['backup_root']),0o750,self.account.pw_gid)]

    def directory(self,path,mode=0o755,gid=0):
        if not path.exists() and not path.is_symlink():
            parent_trust(path.parent)
            if not path.parent.exists(): self.directory(path.parent)
            path.mkdir(mode=mode)
        require(stat.S_ISDIR(path.lstat().st_mode), 'Managed directory must not be a link')
        os.chown(path,0,gid); path.chmod(mode)
        trusted(path)

    def write(self,path,raw,mode=0o644):
        self.directory(path.parent) if not path.parent.exists() else trusted(path.parent)
        if path.exists() or path.is_symlink(): trusted(path)
        old = None
        if path.exists():
            info=path.stat()
            require(stat.S_ISREG(info.st_mode), 'Managed file is not regular')
            old=dict(mode=stat.S_IMODE(info.st_mode),uid=info.st_uid,gid=info.st_gid,
                     file=str(len(self.saved)))
            with (self.history/old['file']).open('xb') as snapshot:
                os.fchmod(snapshot.fileno(),0o600)
                snapshot.write(bounded(path,16*1024**2)); snapshot.flush(); os.fsync(snapshot.fileno())
        self.saved.append(dict(path=str(path),previous=old))
        self.c.atomic(self.history/'changes.json',dict(format=1,changes=self.saved),trusted)
        fd, temporary=tempfile.mkstemp(prefix='.bootstrap-',dir=path.parent)
        try:
            with os.fdopen(fd,'wb') as stream:
                os.fchmod(stream.fileno(),mode); stream.write(raw); stream.flush(); os.fsync(stream.fileno())
            os.replace(temporary,path); self.c.sync_folder(path.parent)
        finally: Path(temporary).unlink(missing_ok=True)

    def apply(self):
        l,e,c,config=self.launcher,self.engine,self.c,self.config
        self.preflight()
        self.saved=[]
        self.directory(self.state,0o755)
        self.directory(self.state/'bootstrap-attempts',0o700)
        self.history=self.state/'bootstrap-attempts'/secrets.token_hex(16)
        self.directory(self.history,0o700)
        # Stop scheduling, never interrupt an application deployment. Both locks are
        # held by main before this preflight and until all checks are finished.
        if e.run(['systemctl','show','-p','LoadState','--value','talocart-update.timer']) != 'not-found':
            e.run(['systemctl','stop','talocart-update.timer'])
        if e.run(['systemctl','show','-p','LoadState','--value','talocart-update.path']) == 'loaded':
            e.run(['systemctl','stop','talocart-update.path'])
        for path,mode,gid in self.layout(): self.directory(path,mode,gid)
        requests=self.state/'requests'
        require(requests.is_dir() and not requests.is_symlink() and requests.stat().st_uid==self.account.pw_uid
                and requests.stat().st_mode&0o777==0o700,'Running host must own the private requests directory')
        for path,(raw,mode) in self.files.items(): self.write(path,raw,mode)
        if self.token.exists() or self.token.is_symlink():
            trusted(self.token)
            token=l.read(self.token,65).decode().strip()
            require(self.token.stat().st_mode&0o077==0,'Existing probe token must be root-private')
        else:
            token=secrets.token_hex(32); self.write(self.token,(token+'\n').encode(),0o600)
        maps,gate=c.nginx_probe_configuration(token)
        self.write(HTTP,maps.encode(),0o600); self.write(SERVER,gate.encode(),0o600)
        if not l.CURRENT.exists() and not l.CURRENT.is_symlink():
            l.install_bundle(self.args.engine,self.args.signature,self.key,self.args.engine_version)
        e.run(['systemctl','daemon-reload'])
        e.run(['nginx','-t'])
        e.run(['systemctl','reload','nginx.service'])
        probe=c.Probe(config,e)
        gateway_ready(probe)
        # No application/SQL changes. A few seconds of public 503 qualify the gate.
        # Keep the ordinary application restartable under this live admin lock.
        c.atomic(self.history/'checks.json',dict(format=1,phase='gateway',pending=True),trusted)
        c.atomic(l.RUNTIME/'application-start',dict(attempt='bootstrap'),trusted)
        try:
            l.marker('maintenance'); probe.closed(); probe.check('/health/ready')
        finally:
            (self.state/'maintenance').unlink(missing_ok=True)
            c.sync_folder(self.state)
            (l.RUNTIME/'application-start').unlink(missing_ok=True)
            c.sync_folder(l.RUNTIME)
        status,body,_=probe._request('/health/ready',private=False)
        require(status==200 and body==b'ready\n','Public gateway did not reopen')
        require(hashlib.sha256(c.root_bytes(self.app,e,1024**2,app_config=True)).hexdigest()==self.app_sha256,'Application configuration changed during bootstrap')
        c.atomic(self.state/'bootstrap.json',dict(format=1,launcher_version=l.VERSION,engine_version=self.args.engine_version,
                 engine_sha256=self.archive_sha256),trusted)
        c.check_units(config,e)
        c.atomic(self.history/'completed.json',dict(format=1,completed=True),trusted)
        e.run(['systemctl','enable','--now','talocart-update.timer','talocart-update.path'])
        return dict(format=1,installed=True,engine_version=self.args.engine_version,bootstrap_id=self.history.name)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config-source',type=Path,required=True)
    parser.add_argument('--engine',type=Path,required=True)
    parser.add_argument('--signature',type=Path,required=True)
    parser.add_argument('--engine-version',required=True)
    parser.add_argument('--root-public-key',type=Path,required=True)
    parser.add_argument('--nginx-site',type=Path,required=True)
    parser.add_argument('--apply',action='store_true',help='Install infrastructure, briefly test 503, enable scheduling; no application deploy')
    args=parser.parse_args()
    try:
        require(sys.platform=='linux' and os.geteuid()==0,'Administrative Linux root execution required')
        os.umask(0o077)
        trusted(Path(__file__).resolve())
        l,e,c=[sibling(name) for name in ('update_launcher','update_engine','update_coordinator')]
        raw=l.read(args.config_source,65536)
        config=tomllib.loads(raw.decode())
        e.STATE=l.STATE
        # The live application's channel must already exist; no silent provisioning
        # over an unsupported pre-control-socket executable.
        trusted(l.STATE)
        locks=[]
        try:
            for name in ('launcher.lock','agent.lock'):
                fd=os.open(l.STATE/name,os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW|os.O_NONBLOCK,0o600)
                info=os.fstat(fd)
                if not (stat.S_ISREG(info.st_mode) and info.st_uid==0 and not info.st_mode&0o022):
                    os.close(fd); raise ValueError('Invalid administrative lock')
                stream=os.fdopen(fd,'r+b'); locks.append(stream)
                fcntl.flock(stream,fcntl.LOCK_EX|fcntl.LOCK_NB)
            job=Bootstrap(args,config,raw,l,e,c)
            print(json.dumps(job.apply() if args.apply else job.preflight()))
        finally:
            for stream in reversed(locks): stream.close()
    except Exception as error:
        try:
            import traceback
            private = Path('/var/lib/talocart-update/bootstrap-failure.log')
            trusted(private.parent)
            fd = os.open(private, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
            with os.fdopen(fd, 'w') as log:
                os.fchmod(log.fileno(), 0o600)
                log.write(traceback.format_exc())
        except Exception:
            pass
        # Our direct ValueError messages contain fixed operator guidance, not raw
        # command/network diagnostics, configuration contents or decoded input.
        reason=str(error) if type(error) is ValueError else type(error).__name__
        parser.exit(1,'Updater bootstrap failed: '+reason+'. Scheduling remains disabled after installation starts. Inspect root-private bootstrap-attempts. No application deploy or SQL migration.\n')


if __name__=='__main__': main()
