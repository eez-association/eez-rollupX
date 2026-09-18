"""Tiny-workspace archived handoff oracle; does not simulate physical ENOSPC."""
import argparse, json, os, pathlib, subprocess
parser=argparse.ArgumentParser()
for name in ('native-test','runtime-bin','image','report'): parser.add_argument('--'+name,required=True)
a=parser.parse_args()
assert a.image=='fixture-quota:not-run'
assert os.environ['EEZ_TEST_PREBUILT']=='1'
assert pathlib.Path(a.runtime_bin)==pathlib.Path(os.environ['EEZ_TEST_RUNTIME_BIN_DIR'])
env=dict(os.environ,CARGO_MANIFEST_DIR=str(pathlib.Path.cwd()/'crates/eez-node-e2e'))
r=subprocess.run([a.native_test,'dynamic_enospc_bidirectional_cross_chain_smoke','--ignored','--exact'],env=env,text=True,stdout=subprocess.PIPE,check=True)
assert '1 passed; 0 failed' in r.stdout
pathlib.Path('quota-relocation.executed').write_text('passed')
pathlib.Path(a.report).parent.mkdir(parents=True,exist_ok=True)
pathlib.Path(a.report).write_text(json.dumps({'scope':'archived handoff only','passed':True}))
