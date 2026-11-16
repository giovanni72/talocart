"""Real SIGKILL/reboot at durable boundaries; hooks exist only in signed test copies."""
import json
import os
from pathlib import Path
import subprocess
import sys
sys.path.insert(0,"/test/root-source")
import time

import update_launcher as launcher


def instrument(source):
    # The production engine has no pause switch or user-controlled test hooks.
    # This module appends instrumentation only to the disposable signed copy.
    path=source/'update_coordinator.py'
    path.write_text(path.read_text()+r'''

# Disposable integration instrumentation; never ship this source copy.
def _integration_gate(boundary, attempt):
    import time
    setting=json.loads(Path('/test/interrupt.json').read_text())
    if setting['boundary']!=boundary:return
    atomic(Path('/var/lib/talocart-update/integration-paused.json'),
        dict(boundary=boundary,attempt=attempt,pid=os.getpid()),lambda _:None)
    while True:time.sleep(0.05)
_integration_atomic=atomic
def _integration_atomic_hook(path,value,trusted):
    result=_integration_atomic(path,value,trusted)
    if path.name=='processing.json' and isinstance(value,dict) and 'attempt' in value:
        _integration_gate('processing',value['attempt'])
    if path.name=='deployed.json' and isinstance(value,dict) and value.get('backup_id'):
        _integration_gate('deployed',value['backup_id'])
    return result
atomic=_integration_atomic_hook
_integration_begin=Journal.begin
@classmethod
def _integration_begin_hook(cls,*args,**kwargs):
    value=_integration_begin(*args,**kwargs)
    _integration_gate('begin',value.value['id'])
    return value
Journal.begin=_integration_begin_hook
_integration_intent=Journal.intent
def _integration_intent_hook(self,phase):
    value=_integration_intent(self,phase)
    _integration_gate('intent:'+phase,self.value['id'])
    return value
Journal.intent=_integration_intent_hook
_integration_complete=Journal.complete
def _integration_complete_hook(self,phase,result=None):
    value=_integration_complete(self,phase,result)
    _integration_gate('complete:'+phase,self.value['id'])
    return value
Journal.complete=_integration_complete_hook
_integration_publish=publish_status
def _integration_publish_hook(*args,**kwargs):
    value=_integration_publish(*args,**kwargs)
    if args[2]=='succeeded':
        context=json.loads(Path('/var/lib/talocart-update/processing.json').read_text())
        _integration_gate('published',context['attempt'])
    return value
publish_status=_integration_publish_hook
_integration_revoke=Execution.revoke
def _integration_revoke_hook(self):
    if hasattr(self,'journal') and self.journal.value['phase']=='succeeded' and not (self.state/'processing.json').exists():
        _integration_gate('ack',self.journal.value['id'])
    return _integration_revoke(self)
Execution.revoke=_integration_revoke_hook
''')


def interrupt_attempt(command,http,config,csrf_from):
    setting=json.loads(Path('/test/interrupt.json').read_text())
    state=launcher.STATE
    status,html=http('GET','/admin/updates');assert status==200
    candidate=json.loads((state/'status.json').read_text())['candidate']
    assert http('POST','/admin/updates',dict(csrf=csrf_from(html),confirm='yes',**candidate))[0]==303
    paused=state/'integration-paused.json'
    deadline=time.monotonic()+150
    while not paused.exists():
        assert time.monotonic()<deadline, 'Engine did not reach the requested durable boundary'
        time.sleep(0.05)
    context=json.loads(paused.read_text())|dict(config=config,mode=setting['mode'])
    assert context['boundary']==setting['boundary']
    (Path('/test/recovery-context.json')).write_text(json.dumps(context));Path('/test/recovery-context.json').chmod(0o600)
    if setting['mode']=='reboot':
        Path('/test/reboot-ready').write_text('ready')
        return
    command('systemctl','kill','--kill-whom=all','--signal=SIGKILL','talocart-update.service')
    for _ in range(100):
        active=command('systemctl','show','-p','ActiveState','--value','talocart-update.service').strip()
        if active in (b'failed',b'inactive'):break
        time.sleep(0.05)
    else:raise AssertionError('Killed updater unit did not stop')
    verify(command)


def verify(command):
    context=json.loads(Path('/test/recovery-context.json').read_text())
    config=context['config'];state=launcher.STATE
    journal_path=state/'attempts'/(context['attempt']+'.json')
    journal=json.loads(journal_path.read_text()) if journal_path.exists() else dict(results={})
    if context['boundary']=='processing':assert not journal_path.exists()
    if context['boundary']=='ack':
        assert not launcher.incomplete()
        assert launcher.application_start_allowed() and launcher.writer_start_allowed()
        assert json.loads((state/'status.json').read_text())['phase']=='succeeded'
        command('systemctl','restart',config['service'])
        assert command('systemctl','show','-p','MainPID','--value',config['service']).strip()!=b'0'
        print(json.dumps(dict(integration_stage='interruption',boundary='ack',mode=context['mode'],passed=True,acknowledged_success_preserved=True)))
        return
    if context['mode']=='reboot':
        assert not (launcher.RUNTIME/'application-start').exists() and not (launcher.RUNTIME/'writers-start').exists()
        assert command('systemctl','is-enabled',config['service']).strip()==b'enabled'
        assert command('systemctl','show','-p','MainPID','--value',config['service']).strip()==b'0', 'Boot must block writer before exec'
    assert launcher.incomplete(), 'Durable journal must have an independent recovery intent before the engine can die'
    assert not launcher.application_start_allowed() and not launcher.writer_start_allowed(), 'Dead lock or reboot must revoke permits'
    # Corrupt the selected engine as well: recovery must precede any import/hash check.
    broken=launcher.CURRENT.resolve()/'update_agent.py'
    broken.write_text('raise RuntimeError("synthetic broken engine; must never import during recovery")\n')
    command('systemctl','restart',config['service'])
    assert command('systemctl','show','-p','MainPID','--value',config['service']).strip()==b'0'
    attempts=set((state/'attempts').glob('*.json'))
    backups=set(Path(config['backup_root']).glob('*/data/backup.json'))
    for _ in range(2):
        result=subprocess.run(['/usr/bin/python3','-I','/usr/local/libexec/talocart/update_launcher.py'],capture_output=True,timeout=45)
        assert result.returncode==1
        status=json.loads((state/'status.json').read_text());assert status['phase']=='failed' and status['recovery_required']
        assert (state/'maintenance').exists() and (state/'writer-fence').exists()
        assert command('systemctl','show','-p','MainPID','--value',config['service']).strip()==b'0'
        assert set((state/'attempts').glob('*.json'))==attempts and set(Path(config['backup_root']).glob('*/data/backup.json'))==backups
    verified=journal['results'].get('backed_up')
    assert status['backup_id']==(verified['id'] if verified else None)
    assert (Path('/var/lib/talocart/media/synthetic-original.bin')).read_bytes()==b'original synthetic media\x00'
    rows=command('runuser','-u','postgres','--','psql','-d','talocart_integration_test','-Atc',
        'SELECT status FROM fixture_module_jobs; SELECT count(*) FROM _fixture_module_migrations').decode().splitlines()
    # Public HTTPS stays closed even with the selected engine deliberately broken.
    command('systemctl','start','nginx.service')
    from update_coordinator import Probe
    assert Probe(config,__import__('update_engine'))._request('/health/ready',private=False)[0]==503
    expected='2' if 'applying' in journal['results'] else '1'
    assert rows==['done',expected], 'Recovery may not roll back SQL or discard jobs'
    print(json.dumps(dict(integration_stage='interruption',boundary=context['boundary'],mode=context['mode'],passed=True,
        independent_recovery_with_broken_engine=True,writer_guard=True,backup_id=status['backup_id'],
        jobs_and_media_preserved=True,no_duplicate_attempt_or_backup=True)))


if __name__=='__main__':
    assert os.geteuid()==0 and Path('/.dockerenv').exists()
    def command(*args):return subprocess.check_output(list(map(str,args)),timeout=120)
    verify(command)
