"""The installed engine runs with Python -I; no checkout/PYTHONPATH imports."""
from pathlib import Path
import subprocess
import sys
import unittest

SCRIPTS=Path(__file__).resolve().parents[2]/'scripts/release'


class AgentIsolation(unittest.TestCase):
    def test_verified_siblings_resolve_dependencies_in_isolated_python(self):
        source='''
import importlib.util,sys,types
from pathlib import Path
folder=Path(sys.argv[1])
agent=types.ModuleType('isolated_agent');agent.__file__=str(folder/'update_agent.py')
# Execute the entry point as the interpreter does, without creating an import cache for it.
exec(compile((folder/'update_agent.py').read_bytes(),agent.__file__,'exec'),agent.__dict__)
# Files in this unit fixture belong to the normal test user. Ownership/ancestry
# is independently exercised by the actual root/systemd integration fixture.
agent.trusted=lambda _:None
assert str(folder) not in sys.path
feed=agent.sibling('updates_feed')
distribution=agent.sibling('distribution')
assert distribution.feed is feed
assert sys.modules['updates_feed'] is feed
assert str(folder) not in sys.path
agent.self_check()
assert not (folder/'__pycache__').exists()
'''
        # Worktree caches may preexist from the test collector; copy only sources.
        import tempfile,shutil
        sys.path.insert(0, str(SCRIPTS))
        import updater_bundle
        with tempfile.TemporaryDirectory() as temporary:
            folder=Path(temporary)
            for name in ('update_agent','update_engine','update_coordinator','distribution','updates_feed'):
                if name == 'update_coordinator':
                    (folder/(name+'.py')).write_bytes(updater_bundle.coordinator_source(SCRIPTS))
                else:
                    shutil.copyfile(SCRIPTS/(name+'.py'),folder/(name+'.py'))
            result=subprocess.run([sys.executable,'-I','-c',source,str(folder)],capture_output=True,text=True,timeout=30)
            self.assertEqual(result.returncode,0,result.stderr)


if __name__=='__main__':unittest.main()
