"""Real signed administrative/engine archives; no execution or service operations."""
import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'scripts/release'))
import updater_bundle as bundle
import updates_feed as feed


class Bundles(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name); self.key=self.root/'root.key'; self.public=self.root/'root.pub'
        feed.command(['openssl','genpkey','-algorithm','ED25519','-out',self.key]); self.key.chmod(0o600)
        self.public.write_bytes(feed.command(['openssl','pkey','-in',self.key,'-pubout']))

    def test_engine_verify_binds_signature_version_exact_members_and_reproducible_bytes(self):
        first=bundle.build(ROOT/'scripts/release','1.0.0',self.key,self.root/'first')
        second=bundle.build(ROOT/'scripts/release','1.0.0',self.key,self.root/'second')
        archive=self.root/'first'/first['asset']; sig=self.root/'first'/first['signature_asset']
        self.assertEqual(archive.read_bytes(),(self.root/'second'/second['asset']).read_bytes())
        self.assertEqual(bundle.verify(archive,sig,self.public,'1.0.0')['version'],'1.0.0')
        with self.assertRaises(ValueError): bundle.verify(archive,sig,self.public,'1.0.1')
        sig.write_bytes(bytes(64))
        with self.assertRaises(ValueError): bundle.verify(archive,sig,self.public,'1.0.0')

    def test_engine_self_check_identity_and_page_are_embedded_without_external_files(self):
        record=bundle.build(ROOT/'scripts/release','1.0.9',self.key,self.root/'versioned')
        with tarfile.open(self.root/'versioned'/record['asset']) as stream:
            raw=stream.extractfile('update_coordinator.py').read()
        namespace={'__file__':str(self.root/'absent-source'/'update_coordinator.py')}
        exec(compile(raw,namespace['__file__'],'exec'),namespace)
        self.assertEqual(namespace['ENGINE_VERSION'],'1.0.9')
        self.assertEqual(namespace['MAINTENANCE_PAGE'],(ROOT/'assets/maintenance.html').read_text())
        self.assertEqual(namespace['maintenance_page']()[1],(ROOT/'assets/maintenance.csp').read_text())

    def test_packaged_host_and_probe_run_without_repository_or_helper_modules(self):
        record = bundle.build(ROOT/'scripts/release', '1.0.0', self.key, self.root/'isolated-engine')
        runtime = self.root/'runtime'
        runtime.mkdir()
        with tarfile.open(self.root/'isolated-engine'/record['asset']) as stream:
            (runtime/'coordinator.py').write_bytes(stream.extractfile('update_coordinator.py').read())
        artifact = runtime/'synthetic.json'
        artifact.write_bytes(b'{}')
        artifact.chmod(0o600)
        code = """
import runpy, sys
from pathlib import Path
from types import SimpleNamespace
ns = runpy.run_path(sys.argv[1])
engine = SimpleNamespace(trusted=lambda _: None)
assert ns['root_bytes'](Path(sys.argv[2]), engine, 2) == b'{}'
try:
    ns['root_bytes'](Path(sys.argv[2]), engine, 1)
except ValueError:
    pass
else:
    raise AssertionError('Oversized private host input accepted')
assert ns['Operations'].__name__ == 'Operations'
assert ns['Probe'].__name__ == 'Probe'
http, server = ns['nginx_probe_configuration']('a'*64)
assert ns['MAINTENANCE_PAGE'] in server and 'realip_remote_addr' in http
assert ns['maintenance_page']()[0] == ns['MAINTENANCE_PAGE']
"""
        subprocess.run([sys.executable, '-I', '-c', code, str(runtime/'coordinator.py'), str(artifact)],
                       cwd=runtime, check=True, capture_output=True, timeout=30)

    def test_valid_root_signature_cannot_hide_changed_member_or_oversized_manifest(self):
        record=bundle.build(ROOT/'scripts/release','1.0.0',self.key,self.root/'normal')
        with tarfile.open(self.root/'normal'/record['asset']) as stream:
            original={m.name:stream.extractfile(m).read() for m in stream}
        for kind in ('checksum','manifest-limit'):
            files=original.copy()
            if kind=='checksum': files['update_engine.py']+=b'\n# changed member'
            else: files['bundle.json']+=b' '*8192
            result=bundle.write_archive(files,'invalid.tar.gz','1.0.0',self.key,self.root/kind)
            with self.subTest(kind=kind),self.assertRaises(ValueError):
                bundle.verify(self.root/kind/result['asset'],self.root/kind/result['signature_asset'],self.public,'1.0.0')

    def test_signed_administrative_kit_has_provenance_and_is_not_an_ordinary_engine(self):
        record=bundle.build_bootstrap(ROOT,'1.0.0','a'*40,'v11.12.0',self.key,self.root/'kit')
        archive=self.root/'kit'/record['asset']; sig=self.root/'kit'/record['signature_asset']
        self.assertEqual(record['asset'],'talocart-bootstrap-11.12.0.tar.gz')
        feed.verify_signature(archive.read_bytes(),sig.read_bytes(),self.public,bundle.launcher.MAX_BUNDLE)
        with tarfile.open(archive) as stream: files={m.name:stream.extractfile(m).read() for m in stream}
        metadata=json.loads(files.pop('bootstrap.json'))
        self.assertEqual(metadata['source_commit'],'a'*40)
        self.assertEqual(metadata['source_tag'],'v11.12.0')
        self.assertEqual(metadata['launcher_version'],'1.0.0')
        self.assertEqual(metadata['files'],{name:hashlib.sha256(raw).hexdigest() for name,raw in files.items()})
        self.assertEqual(set(files),{'bootstrap_updater.py','update_launcher.py','update_engine.py','update_coordinator.py','update-unified.toml','README.md'})
        self.assertIn(b'ENGINE_VERSION',files['README.md'])
        with self.assertRaises(ValueError): bundle.verify(archive,sig,self.public,'1.0.0')

    def test_even_signed_links_traversal_and_duplicate_members_are_rejected(self):
        for kind in ('link','path','duplicate'):
            archive=self.root/(kind+'.tar.gz')
            with tarfile.open(archive,'w:gz') as stream:
                item=tarfile.TarInfo('../outside' if kind=='path' else 'update_agent.py')
                if kind=='link': item.type=tarfile.SYMTYPE; item.linkname='/outside'
                else: item.size=1
                stream.addfile(item,None if kind=='link' else io.BytesIO(b'x'))
                if kind=='duplicate': stream.addfile(item,io.BytesIO(b'x'))
            sig=archive.with_suffix('.sig'); sig.write_bytes(feed.sign(archive.read_bytes(),self.key))
            with self.subTest(kind=kind),self.assertRaises(ValueError): bundle.verify(archive,sig,self.public,'1.0.0')
            self.assertFalse((self.root.parent/'outside').exists())


# This uses only an isolated temporary Git/Cargo project, never the application repo.
import subprocess
import shutil
import test_version


class AdministrativeCli(test_version.GitFixture):
    def test_cli_requires_clean_annotated_main_and_preserves_private_key_outside_repo(self):
        scripts=self.root/'scripts/release'; scripts.mkdir(parents=True)
        for name in ('updater_bundle.py','update_launcher.py','update_coordinator.py','update_plan.py','update_state.py','update_host.py','update_probe.py','update_engine.py','bootstrap_updater.py','updates_feed.py','version.py'):
            shutil.copyfile(ROOT/'scripts/release'/name,scripts/name)
        (self.root/'assets').mkdir()
        shutil.copyfile(ROOT/'assets/maintenance.html',self.root/'assets/maintenance.html')
        (self.root/'distribution').mkdir()
        for name in ('update-unified.toml','bootstrap-README.md'):
            shutil.copyfile(ROOT/'distribution'/name,self.root/'distribution'/name)
        self.git('add','scripts','distribution','assets'); self.commit()
        self.git('tag','-a','v1.2.3','-m','Synthetic administrative fixture')
        with tempfile.TemporaryDirectory() as temporary:
            stage=Path(temporary); key=stage/'private'; public=stage/'public'
            feed.command(['openssl','genpkey','-algorithm','ED25519','-out',key]); key.chmod(0o600)
            public.write_bytes(feed.command(['openssl','pkey','-in',key,'-pubout']))
            argv=[sys.executable,str(scripts/'updater_bundle.py'),'--bootstrap','--version','1.0.0','--key',str(key),'--output',str(stage/'kit')]
            refused=subprocess.run(argv,capture_output=True,timeout=30)
            self.assertNotEqual(refused.returncode,0); self.assertFalse((stage/'kit').exists())
            result=subprocess.run(argv+['--verified'],capture_output=True,timeout=30)
            self.assertEqual(result.returncode,0,result.stderr)
            record=json.loads(result.stdout)
            self.assertEqual(record['asset'],'talocart-bootstrap-1.2.3.tar.gz')
            feed.verify_signature((stage/'kit'/record['asset']).read_bytes(),(stage/'kit'/record['signature_asset']).read_bytes(),public,bundle.launcher.MAX_BUNDLE)
            self.assertEqual(self.git('status','--porcelain'),'')
            (self.root/'app/src/main.rs').write_text('fn main() { println!("changed"); }\n')
            argv[argv.index('--output')+1]=str(stage/'dirty-kit')
            dirty=subprocess.run(argv+['--verified'],capture_output=True,timeout=30)
            self.assertNotEqual(dirty.returncode,0); self.assertFalse((stage/'dirty-kit').exists())


if __name__=='__main__': unittest.main()
