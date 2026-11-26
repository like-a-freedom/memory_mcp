from pathlib import Path
import hashlib,json,os,shutil,subprocess,tempfile,time
repo=Path.cwd();out=repo/'evals/longmemeval_v2/results/2026-11-26-candidate-work/topical-tabs-preflight'
reg=json.loads((out/'registration.json').read_text());snapshot=Path(Path('/tmp/fts-snapshot-path.txt').read_text())
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def hashes():return {str(f.relative_to(snapshot)):digest(f) for f in sorted(snapshot.rglob('*')) if f.is_file()}
assert hashes()==reg['snapshot_file_sha256']
assert all(digest(repo/p)==h for p,h in reg['candidate_source_sha256'].items())
checks=json.loads((out.parent/'verification/source-refresh-current/commands.json').read_text())
assert len(checks)==7 and all(row['exit_code']==0 for row in checks)
with (out/'attempt-start.json').open('x') as log: json.dump({'started_unix_seconds':time.time(),'no_reruns':True},log)
env=os.environ.copy();env['CARGO_PROFILE_TEST_OPT_LEVEL']='3'
cmd=['cargo','test','--locked','-p','memory_mcp','--features','eval-support','--test','full_history_evidence','--no-run','--message-format=json']
with (out/'build.jsonl').open('w') as log,(out/'build.log').open('w') as err:result=subprocess.run(cmd,env=env,stdout=log,stderr=err,timeout=600)
assert result.returncode==0
messages=[json.loads(l) for l in (out/'build.jsonl').read_text().splitlines()]
exe=next(Path(m['executable']) for m in messages if m.get('reason')=='compiler-artifact' and m.get('executable') and m['target']['name']=='full_history_evidence')
with tempfile.TemporaryDirectory(prefix='fts-metric-cache-') as pinned_dir:
 pinned=Path(pinned_dir)/'full_history_evidence';shutil.copy2(exe,pinned)
 product=next(Path(m['executable']) for m in messages if m.get('reason')=='compiler-artifact' and m.get('executable') and m['target']['name']=='memory_mcp')
 pinned_product=repo/'target/candidate-work-accepted/memory_mcp';pinned_product.parent.mkdir(exist_ok=True);shutil.copy2(product,pinned_product)
 identity={'driver_sha256':digest(out/'capture.py'),'product_binary_sha256':digest(pinned_product),'product_binary_path':str(pinned_product),'command':cmd,'binary_sha256':digest(pinned),'registration_sha256':digest(out/'registration.json')}
 (out/'capture-identity.json').write_text(json.dumps(identity,indent=2)+'\n')
 for case,test in [('6d975a08','full_history_directory_retains_reviewed_source')]:
  with tempfile.TemporaryDirectory(prefix='fts-cache-full-history-') as tmp:
   start=time.monotonic();clone=Path(tmp)/'store';shutil.copytree(snapshot,clone);copy_seconds=time.monotonic()-start
   env['MEMORY_FULL_HISTORY_INPUT']=str(out/'registration.json');env['MEMORY_FULL_HISTORY_STORE']=str(clone)
   cmd=[str(pinned),test,'--ignored','--nocapture']
   with (out/(case+'.log')).open('w') as log:
    start=time.monotonic();child=subprocess.Popen(cmd,env=env,stdout=log,stderr=subprocess.STDOUT)
    try:code=child.wait(timeout=max(0.01,45-(time.monotonic()-start)));timedout=False
    except subprocess.TimeoutExpired:child.kill();code=child.wait(timeout=10);timedout=True
   result={'case_id':case,'command':cmd,'pid':child.pid,'copy_seconds':copy_seconds,'elapsed_seconds':time.monotonic()-start,'external_timeout':timedout,'process_deadline_exceeded':time.monotonic()-start>reg['limits']['child_seconds'],'exit_code':code}
   (out/(case+'-process.json')).write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result),flush=True)
  assert hashes()==reg['snapshot_file_sha256']
 assert digest(pinned)==identity['binary_sha256']
assert all(digest(repo/p)==h for p,h in reg['candidate_source_sha256'].items())
(out/'snapshot-integrity.json').write_text(json.dumps({'before_equals_registration':True,'after_equals_registration':True,'file_sha256':hashes()},indent=2)+'\n')
