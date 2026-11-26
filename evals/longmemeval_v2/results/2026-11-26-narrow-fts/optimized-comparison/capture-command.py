from pathlib import Path
import hashlib,json,os,platform,shutil,subprocess,tempfile,time
repo=Path.cwd()
root=repo/'evals/longmemeval_v2/results/2026-11-26-narrow-fts'
reg=json.loads((root/'registration.json').read_text())
out=root/'optimized-comparison'
out.mkdir(exist_ok=False)
source=repo/'crates/memory-mcp/src/knowledge/queries.rs'
current=source.read_bytes()
baseline=subprocess.check_output(['git','show','HEAD:crates/memory-mcp/src/knowledge/queries.rs'])
for name,data in [('baseline-queries.rs',baseline),('candidate-queries.rs',current)]: (out/name).write_bytes(data)
amend={**reg,'diagnostic_amendment':'Pre-registered optimized application comparison after both debug candidates exceeded 45 seconds. Same inputs, limits and source-span acceptance. Both variants use CARGO_PROFILE_TEST_OPT_LEVEL=3 with existing test profile otherwise unchanged; no cross-profile speedup claim. Fixed order baseline then candidate; one capture per case per variant, all retained. No model calls.','profile':{'CARGO_PROFILE_TEST_OPT_LEVEL':'3'},'machine':platform.machine(),'system':platform.system(),'variants':{name:hashlib.sha256(data).hexdigest() for name,data in [('baseline',baseline),('candidate',current)]}}
(out/'registration.json').write_text(json.dumps(amend,indent=2)+'\n')
snapshot=Path(Path('/tmp/fts-snapshot-path.txt').read_text())
def digest(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def hashes(): return {str(f.relative_to(snapshot)):digest(f) for f in sorted(snapshot.rglob('*')) if f.is_file()}
assert hashes()==reg['snapshot_file_sha256']
try:
 for variant,data in [('baseline',baseline),('candidate',current)]:
  folder=out/variant; folder.mkdir()
  source.write_bytes(data)
  try:
   env=os.environ.copy(); env['CARGO_PROFILE_TEST_OPT_LEVEL']='3'
   cmd=['cargo','test','--locked','-p','memory_mcp','--features','eval-support','--test','full_history_evidence','--no-run','--message-format=json']
   start=time.monotonic()
   with (folder/'build.jsonl').open('w') as log, (folder/'build.log').open('w') as err:
    result=subprocess.run(cmd,env=env,stdout=log,stderr=err,timeout=600)
   assert result.returncode==0,(variant,result.returncode)
   messages=[json.loads(line) for line in (folder/'build.jsonl').read_text().splitlines()]
   exe=next(Path(m['executable']) for m in messages if m.get('reason')=='compiler-artifact' and m.get('executable') and m['target']['name']=='full_history_evidence')
   pinned=Path(tempfile.mkdtemp(prefix='fts-optimized-bin-'))/'full_history_evidence'; shutil.copy2(exe,pinned)
   (folder/'capture-identity.json').write_text(json.dumps({'command':cmd,'build_seconds':time.monotonic()-start,'binary_sha256':digest(pinned),'query_source_sha256':digest(source),'test_source_sha256':digest(repo/'crates/memory-mcp/tests/full_history_evidence.rs'),'registration_sha256':digest(out/'registration.json')},indent=2)+'\n')
  finally: source.write_bytes(current)
  print('BUILD_DONE',variant,flush=True)
  for case,test in [('0c6b0c60','full_history_dropdown_retains_reviewed_source'),('f61a096f','full_history_toolbar_retains_reviewed_source')]:
   assert hashes()==reg['snapshot_file_sha256']
   with tempfile.TemporaryDirectory(prefix='fts-full-history-') as tmp:
    copy_start=time.monotonic(); clone=Path(tmp)/'store'; shutil.copytree(snapshot,clone); copy_seconds=time.monotonic()-copy_start
    env=os.environ.copy();env['MEMORY_FULL_HISTORY_INPUT']=str(out/'registration.json');env['MEMORY_FULL_HISTORY_STORE']=str(clone)
    cmd=[str(pinned),test,'--ignored','--nocapture']
    with (folder/(case+'.log')).open('w') as log:
     start=time.monotonic(); child=subprocess.Popen(cmd,env=env,stdout=log,stderr=subprocess.STDOUT)
     try: code=child.wait(timeout=45); timedout=False
     except subprocess.TimeoutExpired: child.kill();code=child.wait(timeout=10);timedout=True
    result={'case_id':case,'command':cmd,'pid':child.pid,'copy_seconds':copy_seconds,'elapsed_seconds':time.monotonic()-start,'external_timeout':timedout,'exit_code':code}
    (folder/(case+'-process.json')).write_text(json.dumps(result,indent=2)+'\n'); print(variant,json.dumps(result),flush=True)
   assert hashes()==reg['snapshot_file_sha256']
  assert digest(pinned)==json.loads((folder/'capture-identity.json').read_text())['binary_sha256']
  shutil.rmtree(pinned.parent)
 (out/'snapshot-integrity.json').write_text(json.dumps({'before_equals_registration':True,'after_equals_registration':True,'file_sha256':hashes()},indent=2)+'\n')
finally: source.write_bytes(current)
