"""Private durable plan storage and write-ahead attempt journal; no service control."""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import tempfile

from update_plan import canonical, digest, require

PHASES = ('prepared', 'updater_ready', 'quiescing', 'fenced', 'backed_up',
          'applying', 'verifying', 'resuming', 'succeeded')


def atomic(path, value, trusted):
    trusted(path.parent)
    fd, name = tempfile.mkstemp(prefix='.' + path.name + '-', dir=path.parent)
    temporary = Path(name)
    try:
        with os.fdopen(fd, 'wb') as stream:
            os.fchmod(stream.fileno(), 0o600)
            stream.write(canonical(value))
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        folder = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(folder)
        finally:
            os.close(folder)
    finally:
        temporary.unlink(missing_ok=True)



def private_json(path, trusted, limit=262144):
    trusted(path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_mode & 0o077 == 0 and info.st_size <= limit,
                'Invalid private update state')
        raw = stream.read(limit + 1)
    require(len(raw) <= limit, 'Private update state exceeds limit')
    return json.loads(raw)


class PlanStore:
    """Root-only proposals and replay watermark. Never stores credentials or executes effects."""
    def __init__(self, state, trusted):
        self.state, self.trusted = state, trusted
        trusted(state)

    def configuration_fingerprint(self, material):
        # Public plan identifiers must not become an oracle on low-entropy DB secrets.
        import hmac
        import secrets
        path = self.state / 'plan-secret'
        if not path.exists() and not path.is_symlink():
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            with os.fdopen(fd, 'wb') as stream:
                stream.write(secrets.token_bytes(32))
                stream.flush()
                os.fsync(stream.fileno())
            directory = os.open(self.state, os.O_RDONLY | os.O_DIRECTORY)
            try: os.fsync(directory)
            finally: os.close(directory)
        self.trusted(path)
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, 'rb') as stream:
            info = os.fstat(stream.fileno())
            require(stat.S_ISREG(info.st_mode) and info.st_mode & 0o077 == 0 and info.st_size == 32,
                    'Invalid private plan secret')
            secret = stream.read(33)
        require(len(secret) == 32, 'Invalid private plan secret')
        return hmac.new(secret, b'talocart-configuration-v1\0' + canonical(material), hashlib.sha256).hexdigest()

    def accept_manifest(self, manifest, manifest_digest, distribution):
        path = self.state / 'distribution-sequence.json'
        previous = private_json(path, self.trusted, 1024) if path.exists() or path.is_symlink() else dict(sequence=0, digest='0'*64)
        distribution.check_sequence(manifest, manifest_digest, previous)
        atomic(path, dict(sequence=manifest['release']['sequence'], digest=manifest_digest), self.trusted)

    def save(self, plan):
        identity = digest(plan)
        folder = self.state / 'plans'
        folder.mkdir(mode=0o700, exist_ok=True)
        self.trusted(folder)
        require(folder.stat().st_mode & 0o077 == 0, 'Plan directory must be private')
        path = folder / (identity + '.json')
        if path.exists() or path.is_symlink():
            require(private_json(path, self.trusted) == plan, 'Immutable plan was changed')
        else:
            atomic(path, plan, self.trusted)
        return identity

    def load(self, identity):
        require(isinstance(identity, str) and re.fullmatch(r'[0-9a-f]{64}', identity), 'Invalid plan identity')
        plan = private_json(self.state / 'plans' / (identity + '.json'), self.trusted)
        require(digest(plan) == identity, 'Immutable plan digest mismatch')
        return plan


class Journal:
    """Write-ahead phase record. Call intent BEFORE effects, complete AFTER success.

    An interruption never implies rollback. A repeated operation is not authorized
    by this class: recovery must reconcile actual database/services/files first.
    """
    def __init__(self, path, trusted, value):
        self.path, self.trusted, self.value = path, trusted, value

    @classmethod
    def begin(cls, path, trusted, plan_digest, attempt_id, snapshot):
        require(not path.exists() and not path.is_symlink(), 'Attempt already exists')
        require(re.fullmatch(r'[0-9a-f]{64}', plan_digest) and re.fullmatch(r'[0-9a-f]{32}', attempt_id),
                'Invalid attempt identity')
        value = dict(format=1, id=attempt_id, plan_sha256=plan_digest,
                     phase='prepared', pending=None, snapshot=snapshot, results={})
        atomic(path, value, trusted)
        return cls(path, trusted, value)

    def intent(self, phase):
        require(phase in PHASES and PHASES.index(phase) == PHASES.index(self.value['phase']) + 1
                and self.value['pending'] is None, 'Invalid attempt phase transition')
        value = self.value | {'pending': phase}
        atomic(self.path, value, self.trusted)
        self.value = value

    def abort(self):
        require(self.value['phase'] in ('prepared', 'updater_ready')
                and self.value['pending'] in (None, 'quiescing'), 'Cannot abort after fencing/application')
        value = self.value | {'phase': 'aborted', 'pending': None}
        atomic(self.path, value, self.trusted)
        self.value = value

    def complete(self, phase, result=None):
        require(self.value['pending'] == phase, 'Phase was not journaled before execution')
        results = self.value['results'] | {phase: result}
        value = self.value | {'phase': phase, 'pending': None, 'results': results}
        atomic(self.path, value, self.trusted)
        self.value = value
