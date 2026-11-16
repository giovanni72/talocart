"""Local signing, approval and pull-agent contracts; no GitHub or server mutations."""
import hashlib
import json
import os
from pathlib import Path
import pwd
import subprocess
import sys
import tempfile
import tarfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[2] / 'scripts/release'
sys.path.insert(0, str(SCRIPTS))
import updates_feed as feed
import update_agent as agent
import publish


class SignedUpdates(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.keys = tempfile.TemporaryDirectory()
        cls.private, cls.public = Path(cls.keys.name) / 'private.pem', Path(cls.keys.name) / 'public.pem'
        feed.command(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', cls.private])
        cls.private.chmod(0o600)
        cls.public.write_bytes(feed.command(['openssl', 'pkey', '-in', cls.private, '-pubout']))

    @classmethod
    def tearDownClass(cls):
        cls.keys.cleanup()

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.package = b'synthetic package'
        self.item = dict(target=feed.TARGETS[0], asset=f'talocart-2.0.0-{feed.TARGETS[0]}.tar.gz',
                         sha256=hashlib.sha256(self.package).hexdigest(), size=len(self.package))
        self.value = dict(format=1, channel='stable', version='2.0.0', commit='a'*40, packages=[self.item])
        self.raw = json.dumps(self.value).encode()
        self.signature = feed.sign(self.raw, self.private)
        self.release = dict(draft=False, prerelease=False, tag_name='v2.0.0')
        data = {'update.json': self.raw, 'update.sig': self.signature, self.item['asset']: self.package}
        class GitHub:
            def latest(inner): return self.release
            def asset(inner, release, name, limit, output=None):
                if output is not None: Path(output).write_bytes(data[name])
                else: return data[name]
        self.github = GitHub()



    def test_redirect_does_not_forward_repository_token(self):
        from urllib.request import Request
        request = Request('https://api.github.com/assets/1', headers={'Authorization':'Bearer synthetic-private'})
        redirected=feed.Redirect().redirect_request(request,None,302,'',{},'https://release-assets.githubusercontent.com/file')
        self.assertFalse(redirected.has_header('Authorization'))
        with self.assertRaises(ValueError):
            feed.Redirect().redirect_request(request,None,302,'',{},'http://example.invalid/file')


    def config(self):
        (self.root / 'requests').mkdir()
        (self.root / 'incoming').mkdir()
        (self.root / 'current').mkdir()
        (self.root / 'current/release.json').write_text('{"version":"1.0.0"}')
        return dict(service_user=pwd.getpwuid(os.getuid()).pw_name, current=str(self.root/'current'),
                    public_key=str(self.public), repository='owner/repo', incoming=str(self.root/'incoming'),
                    environment='staging',public_origin='https://example.invalid')






    def test_download_revalidates_cache_replaces_corruption_and_cleans_partial(self):
        archive = self.root / self.item['asset']
        partial = archive.with_name(archive.name + '.partial')
        with patch.object(agent, 'trusted'):
            archive.write_bytes(b'X' * len(self.package))
            partial.write_bytes(b'left by interrupted download')
            agent.download(self.github, self.release, self.item, archive, feed)
            self.assertEqual(archive.read_bytes(), self.package)
            self.assertFalse(partial.exists())
            archive.unlink()
            def interrupted(release, name, limit, output):
                Path(output).write_bytes(b'partial')
                raise OSError('Synthetic connection failure')
            with patch.object(self.github, 'asset', side_effect=interrupted):
                with self.assertRaises(OSError):
                    agent.download(self.github, self.release, self.item, archive, feed)
            self.assertFalse(partial.exists())
            agent.download(self.github, self.release, self.item, archive, feed)
            self.assertEqual(archive.read_bytes(), self.package)
            # Never follow or delete a symlink, even when its bytes match.
            archive.unlink()
            target = self.root / 'preserved'
            target.write_bytes(self.package)
            archive.symlink_to(target)
            with self.assertRaises(ValueError):
                agent.download(self.github, self.release, self.item, archive, feed)
            self.assertEqual(target.read_bytes(), self.package)




if __name__=='__main__': unittest.main()
