"""Local real crypto/archive checks with a fake GitHub boundary; no actual upload.

Synthetic module bytes cover publication byte integrity only. Internal native
signatures/compatibility are tested by the Rust host and Linux qualification.
"""
import copy
import json
import os
import subprocess
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SCRIPTS=Path(__file__).resolve().parents[2]/'scripts/release'
sys.path.insert(0,str(SCRIPTS))
import distribution
import publish
import updater_bundle
import updates_feed as feed


class Publication(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.keys={}
        for role in ('app','root'):
            private,public=self.root/(role+'.key'),self.root/(role+'.pub')
            feed.command(['openssl','genpkey','-algorithm','ED25519','-out',private]); private.chmod(0o600)
            public.write_bytes(feed.command(['openssl','pkey','-in',private,'-pubout']))
            self.keys[role]=(private,public)
        self.contracts=dict(sdk_version='4.0.0',protocol=3,database=dict(version=3,migrations=1,sha256='b'*64))
        base=self.base('15.0.1','a'*40)
        record=updater_bundle.build(SCRIPTS,'1.0.0',self.keys['root'][0],self.root/'engine')
        for name in (record['asset'],record['signature_asset']): shutil.copyfile(self.root/'engine'/name,self.root/name)
        module=self.root/'koala-import-15.0.0-linux.talopkg'; module.write_bytes(b'synthetic unchanged native package')
        self.value=dict(format=2,channel='stable',repository='owner/repo',release=dict(id='v15.0.1',sequence=8,commit='a'*40),
                        minimum=dict(launcher='1.0.0',engine='1.0.0'),platforms=[dict(target=feed.TARGETS[0],base=base,
                        modules=[dict(asset=module.name,size=module.stat().st_size,sha256=feed.digest(module),id='koala-import',version='15.0.0',publisher='c'*64)],updater=record)])
        self.spec=self.root/'spec.json'
        self.draft={}; self.calls=[]; self.edits=0
        self.original=feed.command
        self.metadata=dict(isDraft=True,isPrerelease=False,tagName='v15.0.1')
        def command(argv):
            if argv[0]!='gh': return self.original(argv)
            self.calls.append(list(argv))
            operation=argv[2]
            if operation=='create':
                if self.draft: raise ValueError('Draft already exists')
                self.assertIn('--draft',argv)
                for path in argv:
                    if isinstance(path,Path): self.draft[path.name]=path.read_bytes()
                return b''
            if operation=='view':
                return json.dumps(self.metadata|dict(assets=[dict(name=n,size=len(raw)) for n,raw in self.draft.items()])).encode()
            if operation=='download':
                folder=Path(argv[argv.index('--dir')+1])
                for index,arg in enumerate(argv):
                    if arg=='--pattern':
                        name=argv[index+1]; (folder/name).write_bytes(self.draft[name])
                return b''
            if operation=='edit':
                self.assertIn('--latest',argv); self.assertIn('--draft=false',argv)
                self.edits+=1; self.metadata['isDraft']=False
                return b''
            raise AssertionError('Unexpected remote operation')
        self.patches=[patch.object(feed,'command',side_effect=command),patch.object(publish.version,'check',return_value='15.0.1'),
                      patch.object(publish.version,'git',side_effect=lambda root,*args:'main' if args[0]=='branch' else 'a'*40),
                      patch.object(publish.version,'fingerprint',return_value='f'*40),patch.object(publish,'remote_tag')]
        for context in self.patches: context.start(); self.addCleanup(context.stop)

    def base(self,version,commit):
        target=feed.TARGETS[0]
        tree=self.root/('base-'+version); (tree/'bin').mkdir(parents=True); (tree/'scripts/release').mkdir(parents=True)
        (tree/'bin/talocart').write_bytes(b'synthetic base executable')
        (tree/'scripts/release/operations.py').write_bytes(b'# synthetic backup code')
        build=dict(version=version,commit=commit,source_dirty=False,tagged=True,source_fingerprint='f'*40,**self.contracts)
        metadata=dict(format=1,profile='base',version=version,source_commit=commit,source_dirty=False,target=target,build=build,
                      files={p.relative_to(tree).as_posix():feed.digest(p) for p in tree.rglob('*') if p.is_file()})
        (tree/'release.json').write_text(json.dumps(metadata))
        archive=self.root/f'talocart-{version}-{target}.tar.gz'
        with tarfile.open(archive,'w:gz') as stream: stream.add(tree,arcname=archive.name.removesuffix('.tar.gz'))
        return dict(asset=archive.name,size=archive.stat().st_size,sha256=feed.digest(archive),version=version,commit=commit,contracts=copy.deepcopy(self.contracts))

    def upload(self,verified=True):
        self.spec.write_text(json.dumps(self.value))
        publish.candidate_distribution('owner/repo',self.spec,self.root,self.keys['app'][0],self.keys['root'][1],verified)

    def approve(self):
        publish.approve_distribution('owner/repo','v15.0.1',self.keys['app'][1],self.keys['root'][1])

    def test_draft_creation_and_approval_verify_same_bytes(self):
        self.upload()
        value=distribution.verify(self.draft['distribution.json'],self.draft['distribution.sig'],self.keys['app'][1],'owner/repo')
        self.assertEqual(value,self.value)
        self.assertNotIn('update.json',self.draft)
        self.assertNotIn('update.sig',self.draft)
        before=copy.deepcopy(self.draft)
        self.approve()
        self.assertEqual(self.edits,1)
        self.assertEqual(before,self.draft)
        self.assertEqual([call[2] for call in self.calls].count('create'),1)
        with self.assertRaises(ValueError): self.approve()
        self.assertEqual(self.edits,1)

    def test_module_only_distribution_reuses_compatible_base(self):
        self.value['platforms'][0]['base']=self.base('15.0.0','d'*40)
        self.upload()
        self.assertNotIn('update.json',self.draft)
        self.approve(); self.assertEqual(self.edits,1)
        self.assertTrue(any(call.args[1:] == ('v15.0.0','d'*40) for call in publish.remote_tag.call_args_list))

    def test_unverified_wrong_tag_contract_or_corrupt_components_never_upload(self):
        with self.assertRaises(ValueError): self.upload(verified=False)
        for edit in [lambda v:v['release'].update(commit='c'*40),lambda v:v['platforms'][0]['base']['contracts'].update(protocol=4)]:
            before=copy.deepcopy(self.value); edit(self.value)
            with self.assertRaises(ValueError): self.upload()
            self.value=before; self.assertFalse(self.draft)
        path=self.root/self.value['platforms'][0]['modules'][0]['asset']; path.write_bytes(b'changed')
        with self.assertRaises(ValueError): self.upload()
        self.assertFalse(self.draft)

    def test_application_signing_key_cannot_authorize_root_bundle(self):
        updater=self.value['platforms'][0]['updater']
        (self.root/updater['signature_asset']).write_bytes(feed.sign((self.root/updater['asset']).read_bytes(),self.keys['app'][0]))
        with self.assertRaises(ValueError): self.upload()
        self.assertFalse(self.draft)

    def test_draft_tampering_is_rejected_before_promotion(self):
        self.upload()
        original=copy.deepcopy(self.draft)
        for name in [*publish.artifact_records(self.value),'distribution.sig']:
            self.draft=copy.deepcopy(original)
            self.draft[name]=b'x'*len(self.draft[name])
            with self.subTest(asset=name),self.assertRaises((ValueError,tarfile.TarError)): self.approve()
            self.assertEqual(self.edits,0)
        self.draft=copy.deepcopy(original)
        self.draft.update({'update.json':b'{}','update.sig':bytes(64)})
        with self.assertRaises(ValueError): self.approve()
        self.assertEqual(self.edits,0)

    def test_approval_cli_uses_real_children_crypto_and_download_patterns(self):
        self.upload()
        assets=self.root/'remote-assets'; assets.mkdir()
        for name,raw in self.draft.items(): (assets/name).write_bytes(raw)
        tools=self.root/'tools'; tools.mkdir(); trace=self.root/'trace.jsonl'
        gh=tools/'gh'
        gh.write_text('#!'+sys.executable+'\nimport json,os,sys,pathlib,shutil\n'
            'a=sys.argv[1:]; root=pathlib.Path(os.environ["ASSETS"])\n'
            'with open(os.environ["TRACE"],"a") as f: f.write(json.dumps(a)+"\\n")\n'
            'if a[0]=="api": print(json.dumps(dict(object=dict(type="tag",sha="c"*40) if "/git/ref/" in a[1] else dict(type="commit",sha="a"*40))))\n'
            'elif a[1]=="view": print(json.dumps(dict(isDraft=True,isPrerelease=False,tagName="v15.0.1",assets=[dict(name=p.name,size=p.stat().st_size) for p in root.iterdir()])))\n'
            'elif a[1]=="download":\n'
            ' for i,item in enumerate(a):\n'
            '  if item=="--pattern": shutil.copyfile(root/a[i+1],pathlib.Path(a[a.index("--dir")+1])/a[i+1])\n')
        gh.chmod(0o755)
        env=os.environ|dict(PATH=str(tools)+os.pathsep+os.environ['PATH'],ASSETS=str(assets),TRACE=str(trace))
        argv=[sys.executable,str(SCRIPTS/'publish.py'),'approve-unified','--repo','owner/repo','--tag','v15.0.1',
              '--public-key',str(self.keys['app'][1]),'--root-public-key',str(self.keys['root'][1])]
        result=subprocess.run(argv,env=env,capture_output=True,timeout=30)
        self.assertEqual(result.returncode,0,result.stderr)
        commands=[json.loads(line) for line in trace.read_text().splitlines()]
        self.assertEqual(commands[-1][:3],['release','edit','v15.0.1'])
        self.assertTrue(all('--pattern' in row for row in commands if row[:2]==['release','download']))
        (assets/self.value['platforms'][0]['updater']['signature_asset']).write_bytes(bytes(64))
        trace.write_text('')
        result=subprocess.run(argv,env=env,capture_output=True,timeout=30)
        self.assertNotEqual(result.returncode,0)
        self.assertNotIn('"edit"',trace.read_text())

    def test_duplicate_draft_cannot_be_clobbered_by_new_candidate(self):
        self.upload(); before=copy.deepcopy(self.draft)
        with self.assertRaises(ValueError): self.upload()
        self.assertEqual(before,self.draft)


if __name__=='__main__': unittest.main()
