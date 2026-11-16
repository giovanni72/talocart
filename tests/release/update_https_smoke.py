"""Real TLS/Nginx maintenance and client limits, synthetic loopback-only HTTP."""
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import ssl
import socket
import subprocess
import sys
import threading
import time

sys.path.insert(0, '/test/scripts/release')
import update_coordinator as coordinator
import update_engine as engine
import update_launcher as launcher


def command(*args):
    result = subprocess.run(list(map(str, args)), capture_output=True, timeout=30)
    if result.returncode:
        print(result.stderr.decode(), file=sys.stderr)  # Synthetic container only.
    result.check_returncode()
    return result.stdout


def authentication_limits(folder, ca, cert, key):
    """Exercise the shipped proxy template with actual TCP peers, never forwarded IPs."""
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            assert self.headers.get('X-Forwarded-For') is None, 'Template forwarded an untrusted client address'
            body = b'synthetic auth upstream'
            self.send_response(200)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        do_POST = do_GET
        def log_message(self, *_): pass
    upstream = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=upstream.serve_forever, daemon=True)
    thread.start()
    raw = Path('/test/distribution/nginx.conf').read_text()
    configured = raw
    for old, new in (
        ('/etc/letsencrypt/live/shop.example.com/fullchain.pem', str(cert)),
        ('/etc/letsencrypt/live/shop.example.com/privkey.pem', str(key)),
        ('shop.example.com', 'talocart-updater.test'),
        ('listen 80;', 'listen 127.0.0.1:8088;'),
        ('listen 443 ssl;', 'listen 127.0.0.1:8444 ssl;'),
        ('http://127.0.0.1:3000', 'http://127.0.0.1:' + str(upstream.server_port)),
        ('    deny all;\n', ''),  # The synthetic operator has completed setup.
    ):
        assert old in configured, 'Shipped proxy fixture replacement no longer matches: ' + old
        configured = configured.replace(old, new)
    assert 'real_ip_header' not in configured and 'set_real_ip_from' not in configured
    nginx = folder/'authentication-nginx.conf'
    nginx.write_text('pid /run/talocart-auth-fixture.pid;\nerror_log /test/https/auth-error.log;\n'
                     'events {}\nhttp { access_log off;\n' + configured + '\n}\n')
    command('nginx', '-t', '-c', nginx)
    process = subprocess.Popen(['nginx', '-c', str(nginx), '-g', 'daemon off;'],
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    context = ssl.create_default_context(cafile=ca)
    class LocalHTTPS(http.client.HTTPSConnection):
        def connect(connection):
            connection.sock = context.wrap_socket(socket.create_connection(
                ('127.0.0.1', 8444), 5, connection.source_address), server_hostname=connection.host)
    def request(path, *, source='127.0.0.1', headers=None, method='GET'):
        connection = LocalHTTPS('talocart-updater.test', 8444, timeout=5,
                                context=context, source_address=(source, 0))
        try:
            connection.request(method, path, headers={'Connection':'close', **(headers or {})})
            response = connection.getresponse()
            response.read()
            return response.status
        finally:
            connection.close()
    try:
        for _ in range(50):
            try:
                assert request('/health/ready') == 200
                break
            except OSError:
                time.sleep(.1)
        else:
            raise AssertionError('Shipped authentication proxy did not start')
        # Non-authentication traffic must not consume the client's auth burst.
        for index in range(40):
            assert request('/catalog' if index % 2 else '/health/ready') == 200
        assert request('/account/login', headers={'X-Forwarded-For':'198.51.100.10'}) == 200
        accepted = 1
        for _ in range(60):
            status = request('/en/account/login', method='POST')
            if status == 429:
                break
            assert status == 200
            accepted += 1
        else:
            raise AssertionError('One TCP client never exhausted the authentication burst')
        for path in ('/account/login', '/it/account/login', '/en/account/register',
                     '/it/account/reset', '/account/confirm', '/en/account/confirmation',
                     '/en/account/login?next=%2Fcatalog', '/it/account/%6cogin'):
            assert request(path) == 429, 'Shared/normalized auth route bypassed the client limit: ' + path
        for address in ('198.51.100.11', '203.0.113.20', '127.0.0.2'):
            assert request('/account/login', headers={'X-Forwarded-For':address}) == 429, 'Spoofed XFF bypassed the TCP peer budget'
        assert request('/it/account/login', source='127.0.0.2', headers={'X-Forwarded-For':'127.0.0.1'}) == 200, 'Independent TCP clients share an authentication budget'
        for path in ('/catalog', '/health/ready', '/assets/ui.css', '/account/login-extra'):
            assert request(path) == 200, 'Authentication exhaustion blocked an unrelated route'
        print(json.dumps(dict(auth_proxy_smoke='passed', template='distribution/nginx.conf',
            template_sha256=hashlib.sha256(raw.encode()).hexdigest(), accepted_before_rejection=accepted,
            client_burst_429=True, independent_tcp_clients=True, spoofed_xff_rejected=True,
            non_auth_excluded=True, shared_en_it_budget=True, normalized_uri_and_query=True)))
    finally:
        process.terminate()
        process.communicate(timeout=10)
        upstream.shutdown()
        upstream.server_close()
        thread.join(timeout=5)


def main():
    assert sys.argv[1:] == ['--disposable-container']
    assert Path('/.dockerenv').exists() and os.geteuid() == 0
    os.umask(0o022)
    launcher.STATE.mkdir(mode=0o755, parents=True, exist_ok=True)
    folder = Path('/test/https')
    folder.mkdir(mode=0o700)
    ca, cakey, cert, key = [folder / name for name in ('ca.pem', 'ca.key', 'server.pem', 'server.key')]
    command('openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
            '-subj', '/CN=Talocart synthetic CA', '-keyout', cakey, '-out', ca)
    command('openssl', 'req', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=talocart-updater.test',
            '-keyout', key, '-out', folder / 'server.csr')
    (folder / 'extensions').write_text('subjectAltName=DNS:talocart-updater.test\nextendedKeyUsage=serverAuth\n')
    command('openssl', 'x509', '-req', '-in', folder / 'server.csr', '-CA', ca, '-CAkey', cakey,
            '-CAcreateserial', '-days', '1', '-extfile', folder / 'extensions', '-out', cert)
    for path in (key, cakey): path.chmod(0o600)
    mode = 'normal'
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            assert self.headers.get('X-Talocart-Update-Probe') is None, 'Proxy leaked the root-private probe token upstream'
            if mode in ('manual', 'verify-manual') and (self.path == '/maintenance/status' or self.path in ('/', '/catalog', '/cart', '/cart?preview=1')):
                body = coordinator.MAINTENANCE_PAGE.encode()
                self.send_response(503)
                self.send_header('X-Talocart-Maintenance', 'manual')
                if mode == 'verify-manual': self.send_header('X-Talocart-Verification', '1')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers(); self.wfile.write(body)
                return
            if self.path == '/maintenance/status':
                self.send_response(204)
                self.end_headers()
                return
            if self.path == '/health/ready?all_modules=true' and mode == 'optional-down':
                self.send_response(503); self.end_headers(); self.wfile.write(b'not ready\n'); return
            if self.path in ('/health/ready', '/health/ready?all_modules=true'):
                body = b'verification-ready\n' if mode in ('verify', 'verify-manual') else b'ready\n'
            elif self.path == '/admin/login':
                locale = 'it' if 'talocart_admin_language=it' in self.headers.get('Cookie', '') else 'en'
                body = ('<html lang="' + locale + '">synthetic login</html>').encode()
            else:
                body = b'synthetic public content'
            self.send_response(200)
            if mode in ('verify', 'verify-manual'): self.send_header('X-Talocart-Verification', '1')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *_): pass
    http_server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=http_server.serve_forever, daemon=True)
    thread.start()
    token = folder / 'probe-token'
    token.write_text('a'*64)
    token.chmod(0o600)
    maps, gate = coordinator.nginx_probe_configuration(token.read_text())
    (folder / 'maps.conf').write_text(maps)
    (folder / 'gate.conf').write_text(gate)
    nginx = folder / 'nginx.conf'
    nginx.write_text('pid /run/talocart-probe.pid;\nerror_log /test/https/nginx-error.log;\nevents {}\nhttp {\n'
        'access_log off;\nopen_file_cache max=100 inactive=60s;\nopen_file_cache_valid 60s;\nopen_file_cache_errors on;\ninclude /test/https/maps.conf;\nserver {\nlisten 127.0.0.1:8443 ssl;\n'
        'server_name talocart-updater.test;\nssl_certificate /test/https/server.pem;\nssl_certificate_key /test/https/server.key;\n'
        'set_real_ip_from 127.0.0.0/8;\nreal_ip_header X-Forwarded-For;\ninclude /test/https/gate.conf;\n'
        'location / { proxy_set_header X-Talocart-Update-Probe ""; proxy_set_header Host $http_host; proxy_pass http://127.0.0.1:' + str(http_server.server_port) + '; }\n}}\n')
    launcher.STATE.chmod(0o755)  # Nginx can stat the public maintenance marker, not private state files.
    maintenance = launcher.STATE / 'maintenance'
    maintenance.unlink(missing_ok=True)
    command('nginx', '-t', '-c', nginx)
    process = subprocess.Popen(['nginx', '-c', str(nginx), '-g', 'daemon off;'], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        config = dict(public_origin='https://talocart-updater.test:8443', probe_token_file=str(token), probe_ca_file=str(ca))
        probe = coordinator.Probe(config, engine)
        for _ in range(50):
            try:
                probe.check('/health/ready')
                break
            except OSError:
                time.sleep(0.1)
        else:
            raise AssertionError('Synthetic HTTPS gateway not ready')
        mode = 'optional-down'
        try:
            probe.check('/health/ready')
            raise AssertionError('Updater accepted an unavailable optional module')
        except ValueError:
            pass
        mode = 'normal'
        class RefreshEngine:
            trusted = staticmethod(engine.trusted)
            @staticmethod
            def run(argv):
                if argv == ['nginx','-t']: return command('nginx','-t','-c',nginx)
                if argv == ['systemctl','reload','nginx.service']: return command('nginx','-s','reload','-c',nginx)
                raise AssertionError('Unexpected refresh operation')
        (folder / 'gate.conf').write_text(gate.replace(coordinator.MAINTENANCE_PAGE, '<html>old page</html>'))
        assert coordinator.refresh_maintenance_gate(token.read_text(), RefreshEngine(), path=folder/'gate.conf')
        assert not coordinator.refresh_maintenance_gate(token.read_text(), RefreshEngine(), path=folder/'gate.conf')
        time.sleep(.2)
        probe.smoke()
        launcher.marker('maintenance')
        probe.closed()
        public_status, public_body, public_headers = probe._request('/health/ready', private=False)
        public_headers = {k.lower():v for k,v in public_headers.items()}
        assert public_status == 503 and b'lang="it"' in public_body and b'lang="en"' in public_body
        assert b'Ricarica / Reload' in public_body and public_headers['retry-after'] == '10'
        assert public_headers['cache-control'] == 'no-store' and 'text/html' in public_headers['content-type']
        assert public_headers['content-security-policy'] == coordinator.maintenance_page()[1]
        probe.smoke()
        status, _, _ = probe._request('/health/ready', source=('127.0.0.2', 0),
                                      extra_headers={'X-Forwarded-For': '127.0.0.1'})
        assert status == 503, 'A public peer spoofing a forwarded loopback address bypassed maintenance'
        mode = 'verify'
        probe.smoke(verification=True)
        try:
            probe.smoke()
        except ValueError:
            pass
        else:
            raise AssertionError('Verification host accepted as ordinary readiness')
        for changed in [config | {'public_origin': 'https://wrong-host.test:8443'},
                        {k: v for k, v in config.items() if k != 'probe_ca_file'}]:
            try:
                coordinator.Probe(changed, engine).check('/health/ready', True)
            except ssl.SSLError:
                pass
            else:
                raise AssertionError('TLS certificate/hostname verification was skipped')
        token.write_text('b'*64)
        try:
            coordinator.Probe(config, engine).check('/health/ready', True)
        except ValueError:
            pass
        else:
            raise AssertionError('Wrong secret bypassed maintenance')
        token.write_text('a'*64)
        mode = 'verify-manual'
        probe.smoke(verification=True)
        mode = 'manual'
        probe.smoke()
        mode = 'normal'
        maintenance.unlink()
        assert probe._request('/health/ready', private=False)[0] == 200
        print(json.dumps(dict(https_smoke='passed', tls_chain_and_sni=True, real_gateway=True,
                             public_503_during_probe=True, forwarded_header_cannot_bypass=True,
                             secret_and_local_peer_required=True, readiness_modes_distinct=True)))
        authentication_limits(folder, ca, cert, key)
    finally:
        process.terminate()
        process.communicate(timeout=10)
        http_server.shutdown()
        http_server.server_close()
        thread.join(timeout=5)
        maintenance.unlink(missing_ok=True)


if __name__ == '__main__':
    main()
