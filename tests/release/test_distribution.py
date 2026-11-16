"""Signed distribution v2: no network, production keys, service changes or deploys."""
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[2] / 'scripts/release'
sys.path.insert(0, str(SCRIPTS))
import distribution as dist
import updates_feed as feed


class Distribution(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.keys = tempfile.TemporaryDirectory()
        cls.private = Path(cls.keys.name) / 'private.pem'
        cls.public = Path(cls.keys.name) / 'public.pem'
        feed.command(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', cls.private])
        cls.private.chmod(0o600)
        cls.public.write_bytes(feed.command(['openssl', 'pkey', '-in', cls.private, '-pubout']))

    @classmethod
    def tearDownClass(cls):
        cls.keys.cleanup()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        target = feed.TARGETS[0]
        def artifact(name, **extra):
            content = name.encode()
            (self.root / name).write_bytes(content)
            return dict(asset=name, size=len(content), sha256=hashlib.sha256(content).hexdigest(), **extra)
        base = artifact(f'talocart-11.11.0-{target}.tar.gz', version='11.11.0', commit='a'*40,
                        contracts=dict(sdk_version='2.0.0', protocol=2,
                                       database=dict(version=1, migrations=10, sha256='b'*64)))
        module = artifact('koala-import-11.12.0-linux.talopkg', id='koala-import', version='11.12.0', publisher='c'*64)
        updater = artifact('updater-1.0.0.tar.gz', version='1.0.0', signature_asset='updater-1.0.0.tar.gz.sig')
        (self.root / updater['signature_asset']).write_bytes(b's'*64)
        self.value = dict(format=2, channel='stable', repository='owner/repo',
                          release=dict(id='v11.12.0', sequence=7, commit='d'*40),
                          minimum=dict(launcher='1.0.0', engine='1.0.0'),
                          platforms=[dict(target=target, base=base, modules=[module], updater=updater)])

    def verify(self, value=None):
        raw = json.dumps(self.value if value is None else value).encode()
        return dist.verify(raw, feed.sign(raw, self.private), self.public, 'owner/repo')

    def test_distribution_identity_is_independent_of_base_and_signed_bytes_are_bound(self):
        self.assertEqual(self.verify(), self.value)
        raw = json.dumps(self.value).encode()
        signature = feed.sign(raw, self.private)
        with self.assertRaises(ValueError):
            dist.verify(raw.replace(b'11.12.0', b'11.13.0'), signature, self.public, 'owner/repo')
        with self.assertRaises(ValueError):
            dist.verify(raw, signature, self.public, 'other/repo')

    def test_module_artifacts_need_no_publisher_key_but_remain_bound_to_distribution(self):
        module = self.value['platforms'][0]['modules'][0]
        for publisher in ['', None]:
            if publisher is None:
                module.pop('publisher', None)
            else:
                module['publisher'] = publisher
            with self.subTest(publisher=publisher):
                self.assertEqual(self.verify(), self.value)
                dist.check_artifacts(self.value, self.root)
        raw = json.dumps(self.value).encode()
        signature = feed.sign(raw, self.private)
        module['sha256'] = 'f' * 64
        with self.assertRaises(ValueError):
            dist.verify(json.dumps(self.value).encode(), signature, self.public, 'owner/repo')
        with self.assertRaises(ValueError):
            dist.check_artifacts(self.value, self.root)

    def test_duplicate_unknown_fields_bad_types_and_paths_are_rejected(self):
        edits = [
            lambda v: v.update(format=True),
            lambda v: v.update(extra='ignored'),
            lambda v: v['release'].update(sequence=True),
            lambda v: v['release'].update(sequence=0),
            lambda v: v['minimum'].update(launcher='01.0.0'),
            lambda v: v['platforms'][0]['base'].update(asset='../escape'),
            lambda v: v['platforms'][0]['base'].update(size=True),
            lambda v: v['platforms'][0]['base']['contracts']['database'].update(migrations=0),
            lambda v: v['platforms'].append(copy.deepcopy(v['platforms'][0])),
            lambda v: v['platforms'][0]['modules'].append(copy.deepcopy(v['platforms'][0]['modules'][0])),
            lambda v: v['platforms'][0].update(modules=[None]),
            lambda v: v['platforms'][0].update(modules=[42]),
            lambda v: v['platforms'][0]['modules'][0].update(id='base'),
            lambda v: v['platforms'][0]['modules'][0].update(publisher='not-a-key'),
            lambda v: v['platforms'][0]['updater'].update(signature_asset='other.sig'),
            lambda v: v['minimum'].update(engine='2.0.0'),
            lambda v: v['platforms'][0]['modules'][0].update(asset=v['platforms'][0]['updater']['asset']),
        ]
        for edit in edits:
            value = copy.deepcopy(self.value)
            edit(value)
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.verify(value)
        raw = json.dumps(self.value).replace('"format": 2', '"format": 1, "format": 2').encode()
        with self.assertRaises(ValueError):
            dist.verify(raw, feed.sign(raw, self.private), self.public, 'owner/repo')
        with self.assertRaises(ValueError):
            dist.decode(b' ' * (dist.LIMIT+1))

    def test_architectures_can_share_only_identical_universal_engine_records(self):
        other=copy.deepcopy(self.value['platforms'][0]); other['target']=feed.TARGETS[1]
        other['base']['asset']=f"talocart-11.11.0-{feed.TARGETS[1]}.tar.gz"
        other['modules'][0]['asset']='koala-arm64.talopkg'
        self.value['platforms'].append(other)
        self.assertEqual(self.verify(),self.value)
        other['updater']['sha256']='f'*64
        with self.assertRaises(ValueError): self.verify()

    def test_sequence_rejects_replay_and_equivocation_but_allows_exact_retry(self):
        digest = 'a'*64
        dist.check_sequence(self.value, digest, dict(sequence=6, digest='b'*64))
        dist.check_sequence(self.value, digest, dict(sequence=7, digest=digest))
        for sequence, previous in [(8, digest), (7, 'b'*64), (True, digest)]:
            with self.assertRaises(ValueError):
                dist.check_sequence(self.value, digest, dict(sequence=sequence, digest=previous))

    def test_artifact_integrity_checks_all_components_and_privileged_signature_presence(self):
        dist.check_artifacts(self.value, self.root)
        for item in dist.artifacts(self.value):
            path = self.root / item['asset']
            original = path.read_bytes()
            path.write_bytes(b'corrupt')
            with self.assertRaises(ValueError):
                dist.check_artifacts(self.value, self.root)
            path.write_bytes(original)
        sig = self.root / self.value['platforms'][0]['updater']['signature_asset']
        sig.write_bytes(b'short')
        with self.assertRaises(ValueError):
            dist.check_artifacts(self.value, self.root)

    def test_approved_distribution_is_bound_to_stable_tag(self):
        raw = json.dumps(self.value).encode()
        data = {'distribution.json': raw, 'distribution.sig': feed.sign(raw, self.private)}
        release = dict(draft=False, prerelease=False, tag_name='v11.12.0')
        class GitHub:
            def latest(self): return release
            def asset(self, selected, name, limit): return data[name]
        _, value, digest = dist.approved(GitHub(), self.public, 'owner/repo')
        self.assertEqual(value, self.value)
        self.assertEqual(digest, hashlib.sha256(raw).hexdigest())
        for change in [dict(draft=True), dict(prerelease=True), dict(tag_name='v11.13.0')]:
            old = release.copy()
            release.update(change)
            with self.assertRaises(ValueError):
                dist.approved(GitHub(), self.public, 'owner/repo')
            release.clear()
            release.update(old)

    def test_real_local_sign_and_verify_cli_never_overwrites_candidate(self):
        spec = self.root / 'spec.json'
        spec.write_text(json.dumps(self.value))
        output = self.root / 'signed'
        argv = [sys.executable, str(SCRIPTS / 'distribution.py'), 'sign', '--repo', 'owner/repo',
                '--manifest', str(spec), '--key', str(self.private), '--artifacts', str(self.root),
                '--output', str(output)]
        result = subprocess.run(argv, capture_output=True, timeout=20)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())
        result = subprocess.run(argv + ['--verified'], capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        before = (output / 'distribution.json').read_bytes()
        verify = [sys.executable, str(SCRIPTS / 'distribution.py'), 'verify', '--repo', 'owner/repo',
                  '--manifest', str(output / 'distribution.json'), '--signature', str(output / 'distribution.sig'),
                  '--public-key', str(self.public)]
        result = subprocess.run(verify, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = subprocess.run(argv + ['--verified'], capture_output=True, timeout=20)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((output / 'distribution.json').read_bytes(), before)


if __name__ == '__main__':
    unittest.main()
