"""Server slices must preserve coverage, overlap the wait, and propagate failure."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('server_tests', HERE/'run-server-tests.py')
server = importlib.util.module_from_spec(spec)
spec.loader.exec_module(server)
NAMES = ['acct_read::lake', 'ordinary::unit', 'pg_audit::audit', 'pg_labels::labels',
         'pg_migrate::migrate', 'pg_zero::future', 'pg_0::future', server.ROTATION]


class ServerTests(unittest.TestCase):
    def test_partition_is_exhaustive_disjoint_and_future_names_do_not_disappear(self):
        early, late = server.partition(NAMES)
        self.assertEqual(early, NAMES[:4])
        self.assertEqual(late, NAMES[4:7])
        self.assertEqual(set(early + late + [server.ROTATION]), set(NAMES))
        for names in [[], NAMES[:-1], NAMES + [NAMES[0]]]:
            with self.subTest(names=names), self.assertRaises(ValueError):
                server.partition(names)

    def gate(self, suite, mode='ok'):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            (root/'names.json').write_text(json.dumps(NAMES))
            (root/'cargo').write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
root=Path(__file__).parent
if sys.argv[1]=='metadata':
 print(json.dumps({'packages':[{'id':'server-id','name':'er-server','manifest_path':str(root/'Cargo.toml')}]}))
else:
 assert '--no-run' in sys.argv
 if os.environ['MODE']=='compile-fails': sys.exit(1)
 with (root/'builds').open('a') as f: f.write('build\\n')
 if os.environ['MODE']!='missing-artifact':
  print(json.dumps({'reason':'compiler-artifact','package_id':'server-id',
   'target':{'name':'main','kind':['test']},'executable':str(root/'server-test')}))
''')
            (root/'server-test').write_text('''#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path
root=Path(__file__).parent; names=json.loads((root/'names.json').read_text())
if '--list' in sys.argv:
 for name in names: print(name+': test')
 sys.exit(0)
assert '--exact' in sys.argv
selected=[arg for arg in sys.argv[1:] if not arg.startswith('--')]
assert selected and all(n in names for n in selected)
role='rotation' if len(selected)==1 and 'pg_satoken::' in selected[0] else 'ordinary'
(root/(role+'.json')).write_text(json.dumps(selected))
# A serial implementation fails: both groups must start before either waits.
if os.environ['SUITE']=='pg-al':
 deadline=time.monotonic()+3
 while len(list(root.glob('*.json')))<3:
  if time.monotonic()>deadline: sys.exit(3)
  time.sleep(.01)
if os.environ['MODE']=='test-fails' and role=='ordinary': sys.exit(1)
''')
            for name in ['cargo','server-test']: (root/name).chmod(0o755)
            env={**os.environ,'PATH':str(root)+os.pathsep+os.environ['PATH'],
                 'ER_TEST_PG_DSN':'host=unused dbname=unused','DEDUPE_E2E_DUCKLAKE':'1',
                 'SUITE':suite,'MODE':mode}
            if mode=='no-services': env.pop('ER_TEST_PG_DSN')
            p=subprocess.run(['python3',str(HERE/'run-server-tests.py'),suite],env=env,
                             capture_output=True,text=True,timeout=30)
            groups={name:json.loads((root/(name+'.json')).read_text()) for name in ['rotation','ordinary'] if (root/(name+'.json')).exists()}
            builds=(root/'builds').read_text().count('build') if (root/'builds').exists() else 0
            return p,groups,builds

    def test_early_slice_runs_rotation_concurrently_after_exactly_one_build(self):
        p,groups,builds=self.gate('pg-al')
        self.assertEqual(p.returncode,0,p.stderr)
        self.assertEqual(groups,{'rotation':[server.ROTATION],'ordinary':NAMES[:4]})
        self.assertEqual(builds,1)

    def test_late_slice_is_the_complement_without_repeating_rotation(self):
        p,groups,builds=self.gate('pg-mz')
        self.assertEqual(p.returncode,0,p.stderr)
        self.assertEqual(groups,{'ordinary':NAMES[4:7]})
        self.assertEqual(builds,1)

    def test_missing_services_failed_build_missing_artifact_or_failed_group_is_red(self):
        for mode in ['no-services','compile-fails','missing-artifact','test-fails']:
            with self.subTest(mode=mode):
                p,groups,_=self.gate('pg-al',mode)
                self.assertNotEqual(p.returncode,0)
                if mode!='test-fails': self.assertFalse(groups)
