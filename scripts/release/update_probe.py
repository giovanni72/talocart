"""Managed maintenance gate and bounded HTTPS probes through the real proxy."""
import hashlib
import os
from pathlib import Path
import re
import stat
import tempfile

from update_plan import require
from update_host import root_bytes, sync_folder

# The release bundler embeds the canonical page into the signed Python member.
# The signed engine has a fixed file allowlist; the page is part of that payload.
EMBEDDED_MAINTENANCE_PAGE = None
MAINTENANCE_PAGE = (EMBEDDED_MAINTENANCE_PAGE if EMBEDDED_MAINTENANCE_PAGE is not None
                    else ((Path(__file__).resolve().parent / 'maintenance.html')
                          if (Path(__file__).resolve().parent / 'maintenance.html').is_file()
                          else (Path(__file__).resolve().parents[2] / 'assets/maintenance.html')).read_text())
MAINTENANCE_STYLE = MAINTENANCE_PAGE.split('<style>', 1)[1].split('</style>', 1)[0]
MAINTENANCE_SCRIPT = MAINTENANCE_PAGE.split('<script>', 1)[1].split('</script>', 1)[0]


def maintenance_page():
    """Return the inline 503 body and a CSP admitting only its own style and script."""
    import base64
    body = MAINTENANCE_PAGE
    require(not set(body) & set("$'\\") and len(body.encode()) <= 3800, 'Maintenance page unsafe for an Nginx token')
    pin = lambda text: "'sha256-" + base64.b64encode(hashlib.sha256(text.encode()).digest()).decode() + "'"
    policy = ("default-src 'none'; style-src " + pin(MAINTENANCE_STYLE) + "; script-src " + pin(MAINTENANCE_SCRIPT)
              + "; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
    return body, policy


def nginx_probe_configuration(token):
    """Root-installed snippets: original TCP peer AND secret, never forwarded IP."""
    require(isinstance(token, str) and re.fullmatch(r'[0-9a-f]{64}', token), 'Invalid proxy probe token')
    http = '''# Generated root-private updater probe; include in nginx http context.
geo $realip_remote_addr $talocart_update_local_peer {
    default 0;
    127.0.0.1 1;
    ::1 1;
}
map "$talocart_update_local_peer:$http_x_talocart_update_probe" $talocart_update_bypass {
    default 0;
    "~^1:TOKEN$" 1;
}
map $talocart_update_bypass $talocart_update_probe_allowed {
    default "";
    1 "1";
}
'''.replace('TOKEN', token)
    server = '''# Include in the managed HTTPS server, before location selection.
open_file_cache off;
error_page 503 =503 @talocart_update_maintenance;
set $talocart_update_blocked 0;
if (-f /var/lib/talocart-update/maintenance) { set $talocart_update_blocked 1; }
if ($talocart_update_bypass = 1) { set $talocart_update_blocked 0; }
if ($talocart_update_blocked = 1) { return 503; }
add_header X-Talocart-Update-Probe-Allowed $talocart_update_probe_allowed always;
location @talocart_update_maintenance {
    default_type text/html;
    add_header Cache-Control "no-store" always;
    add_header Retry-After "10" always;
    add_header X-Content-Type-Options "nosniff" always;
    add_header Content-Security-Policy "POLICY" always;
    return 503 'PAGE';
}
'''
    page, policy = maintenance_page()
    server = server.replace('POLICY', policy).replace('PAGE', page)
    return http, server


def refresh_maintenance_gate(token, engine, *, path=Path('/etc/talocart/nginx-update-server.conf')):
    """Refresh only the root-managed page, retaining the live probe and fence rules.

    Runs before downtime under the updater lock; an invalid configuration is
    restored on disk while the previously loaded Nginx configuration stays live.
    """
    previous = root_bytes(path, engine, 16384)
    prefix = b'# Include in the managed HTTPS server, before location selection.\n'
    require(previous.startswith(prefix), 'Refusing to replace an unmanaged Nginx gate')
    replacement = nginx_probe_configuration(token)[1].encode()
    mode = stat.S_IMODE(path.stat().st_mode)
    backup = path.with_name('.' + path.name + '.previous')
    def write(destination, raw):
        fd, name = tempfile.mkstemp(prefix='.maintenance-', dir=path.parent)
        temporary = Path(name)
        try:
            with os.fdopen(fd, 'wb') as stream:
                os.fchmod(stream.fileno(), mode)
                stream.write(raw); stream.flush(); os.fsync(stream.fileno())
            os.replace(temporary, destination)
            sync_folder(path.parent)
        finally:
            temporary.unlink(missing_ok=True)
    # Recover an interrupted publication before attempting a new one. No marker,
    # probe token or service state is inferred from this backup.
    if backup.exists() or backup.is_symlink():
        previous = root_bytes(backup, engine, 16384)
        require(previous.startswith(prefix), 'Invalid interrupted Nginx gate backup')
        write(path, previous)
        engine.run(['nginx','-t'])
        engine.run(['systemctl','reload','nginx.service'])
        backup.unlink(); sync_folder(path.parent)
    if previous == replacement:
        return False
    write(backup, previous)
    write(path, replacement)
    try:
        engine.run(['nginx','-t'])
        engine.run(['systemctl','reload','nginx.service'])
    except BaseException:
        write(path, previous)
        backup.unlink(missing_ok=True); sync_folder(path.parent)
        raise
    backup.unlink(); sync_folder(path.parent)

    return True


class Probe:
    """HTTPS through the real virtual host, connected directly over loopback.

    Certificate chain, public SNI and Host are checked. No DNS/proxy/redirect
    fallback; only fixed read probes, with an overall network deadline.
    """
    def __init__(self, config, engine):
        import ssl
        from urllib.parse import urlsplit
        self.origin = urlsplit(config['public_origin'])
        require(self.origin.scheme == 'https' and self.origin.hostname and not self.origin.username
                and not self.origin.password and self.origin.path in ('', '/')
                and not self.origin.query and not self.origin.fragment, 'Invalid HTTPS probe origin')
        path = Path(config['probe_token_file'])
        engine.trusted(path)
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, 'rb') as stream:
            info = os.fstat(stream.fileno())
            require(stat.S_ISREG(info.st_mode) and info.st_mode & 0o077 == 0 and info.st_size <= 65,
                    'HTTPS probe token must be root-private')
            raw = stream.read(66)
        self.token = raw.decode().strip()
        require(re.fullmatch(r'[0-9a-f]{64}', self.token), 'Invalid HTTPS probe token')
        ca = config.get('probe_ca_file')
        if ca:
            engine.trusted(Path(ca))
        self.context = ssl.create_default_context(cafile=ca)
        self.timeout = 30

    def refresh_gate(self, engine):
        refresh_maintenance_gate(self.token, engine)

    def _request(self, path, *, private=True, source=None, extra_headers=None):
        import http.client
        import signal
        import socket
        import threading
        require(path in ('/health/ready', '/', '/catalog', '/cart', '/cart?preview=1', '/assets/ui.css', '/admin/login', '/maintenance/status'),
                'Invalid HTTP probe path')
        require(threading.current_thread() is threading.main_thread()
                and signal.getitimer(signal.ITIMER_REAL) == (0.0, 0.0), 'HTTPS deadline requires the coordinator thread')
        class LocalHTTPS(http.client.HTTPSConnection):
            def connect(connection):
                raw = socket.create_connection(('127.0.0.1', connection.port), connection.timeout, connection.source_address)
                try:
                    connection.sock = connection._context.wrap_socket(raw, server_hostname=connection.host)
                except BaseException:
                    raw.close()
                    raise
        connection = LocalHTTPS(self.origin.hostname, self.origin.port or 443, context=self.context,
                                timeout=10, source_address=source)
        headers = {'Host': self.origin.netloc, 'User-Agent': 'talocart-updater', 'Connection': 'close'}
        if private:
            headers['X-Talocart-Update-Probe'] = self.token
        headers.update(extra_headers or {})
        def expired(*_):
            raise TimeoutError('HTTPS probe deadline exceeded')
        previous = signal.signal(signal.SIGALRM, expired)
        signal.setitimer(signal.ITIMER_REAL, self.timeout)
        try:
            # Older hosts ignore this query and already require all enabled modules.
            target = path + '?all_modules=true' if path == '/health/ready' else path
            connection.request('GET', target, headers=headers)
            response = connection.getresponse()
            body = response.read(65536)
            return response.status, body, dict(response.getheaders())
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, previous)
            connection.close()

    def check(self, path, verification=False, *, headers=None):
        status, body, raw_headers = self._request(path, extra_headers=headers)
        received = {k.lower(): v for k, v in raw_headers.items()}
        require(status == 200 and body and received.get('x-talocart-update-probe-allowed') == '1', 'Private HTTPS probe failed')
        if path == '/health/ready':
            require(body == (b'verification-ready\n' if verification else b'ready\n')
                    and (received.get('x-talocart-verification') == '1') == verification,
                    'HTTPS serving mode mismatch')
        return body

    def closed(self):
        status, _, headers = self._request('/health/ready', private=False)
        require(status == 503 and not any(k.lower() == 'x-talocart-update-probe-allowed' and v == '1'
                                        for k, v in headers.items()), 'Public maintenance gate is open')

    def smoke(self, verification=False):
        self.check('/health/ready', verification)
        manual = False
        if not verification:
            status, body, headers = self._request('/maintenance/status')
            headers = {k.lower(): v for k, v in headers.items()}
            manual = status == 503 and headers.get('x-talocart-maintenance') == 'manual'
            # Older bases have no manual-maintenance endpoint.
            require(manual or status in (204, 404), 'Invalid storefront maintenance status')
        for path in ('/', '/catalog', '/cart?preview=1' if verification else '/cart'):
            status, body, headers = self._request(path)
            headers = {k.lower(): v for k, v in headers.items()}
            closed = status == 503 and headers.get('x-talocart-maintenance') == 'manual'
            require(headers.get('x-talocart-update-probe-allowed') == '1', 'Private HTTPS probe failed')
            if verification:
                require(headers.get('x-talocart-verification') == '1', 'HTTPS serving mode mismatch')
            if manual or (verification and closed):
                require(closed and body == MAINTENANCE_PAGE.encode(), 'Manual maintenance was not preserved')
            else:
                require(status == 200 and body, 'Private HTTPS probe failed')
        self.check('/assets/ui.css', verification)
        if not verification:
            for locale in ('en', 'it'):
                body = self.check('/admin/login', headers={'Cookie': 'talocart_admin_language=' + locale})
                require(('lang="' + locale + '"').encode() in body, 'Administrative locale probe failed')
