"""Signed release metadata and bounded GitHub Releases transport (stdlib/OpenSSL 3)."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import urllib.request
from urllib.parse import urlsplit

TARGETS = ('x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu')
VERSION = r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def command(argv):
    result = subprocess.run(list(map(str, argv)), capture_output=True, timeout=120, check=False)
    require(result.returncode == 0, 'Release command failed; sensitive diagnostics omitted')
    return result.stdout


def sign(data, key):
    with tempfile.TemporaryDirectory(prefix='talocart-sign-') as temporary:
        source = Path(temporary) / 'manifest'
        source.write_bytes(data)
        signature = command(['openssl', 'pkeyutl', '-sign', '-rawin', '-inkey', key, '-in', source])
        require(len(signature) == 64, 'An Ed25519 signing key is required')
        return signature


def newer_or_equal(actual, required):
    require(isinstance(actual, str) and re.fullmatch(VERSION, actual)
            and isinstance(required, str) and re.fullmatch(VERSION, required), 'Invalid stable component version')
    return tuple(map(int, actual.split('.'))) >= tuple(map(int, required.split('.')))


def verify_signature(data, signature, key, limit=16384):
    require(len(data) <= limit and len(signature) == 64, 'Invalid signed metadata size')
    with tempfile.TemporaryDirectory(prefix='talocart-verify-') as temporary:
        source, sig = Path(temporary) / 'manifest', Path(temporary) / 'signature'
        source.write_bytes(data)
        sig.write_bytes(signature)
        command(['openssl', 'pkeyutl', '-verify', '-rawin', '-pubin', '-inkey', key,
                 '-in', source, '-sigfile', sig])


class Redirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        require(urlsplit(newurl).scheme == 'https', 'Insecure release redirect')
        redirected = super().redirect_request(request, fp, code, msg, headers, newurl)
        if redirected is not None:
            # Signed URLs from GitHub asset storage do not need the repository token.
            redirected.remove_header('Authorization')
        return redirected


class GitHub:
    def __init__(self, repository, token=''):
        require(re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository), 'Invalid repository')
        self.base = 'https://api.github.com/repos/' + repository
        self.token = token
        self.opener = urllib.request.build_opener(Redirect(), urllib.request.ProxyHandler({}))

    def open(self, path, asset=False):
        headers = {'User-Agent': 'Talocart-Updater/1', 'Accept': 'application/octet-stream' if asset else 'application/vnd.github+json'}
        if self.token:
            headers['Authorization'] = 'Bearer ' + self.token
        return self.opener.open(urllib.request.Request(self.base + path, headers=headers), timeout=60)

    def latest(self):
        with self.open('/releases/latest') as response:
            data = response.read(1024 * 1024 + 1)
        require(len(data) <= 1024 * 1024, 'Release API response exceeds limit')
        value = json.loads(data)
        require(value.get('draft') is False and value.get('prerelease') is False, 'Release is not approved')
        return value

    def asset(self, release, name, limit, output=None):
        items = [asset for asset in release['assets'] if asset.get('name') == name]
        require(len(items) == 1 and isinstance(items[0].get('id'), int), 'Missing or ambiguous release asset')
        require(0 < items[0]['size'] <= limit, 'Release asset exceeds limit')
        with self.open('/releases/assets/' + str(items[0]['id']), asset=True) as response:
            if output is None:
                data = response.read(limit + 1)
                require(len(data) == items[0]['size'] and len(data) <= limit, 'Asset length mismatch')
                return data
            total = 0
            with Path(output).open('xb') as stream:
                for block in iter(lambda: response.read(1024 * 1024), b''):
                    total += len(block)
                    require(total <= limit, 'Asset exceeds signed length')
                    stream.write(block)
            require(total == limit == items[0]['size'], 'Package length mismatch')
