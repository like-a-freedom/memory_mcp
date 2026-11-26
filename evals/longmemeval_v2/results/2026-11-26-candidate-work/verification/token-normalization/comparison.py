from pathlib import Path
import subprocess,tempfile,json,hashlib,shutil
repo=Path.cwd();root=repo/'evals/longmemeval_v2/results/2026-11-26-candidate-work/verification/token-normalization';root.mkdir(exist_ok=True)
files=['crates/memory-mcp/src/shared/search.rs','crates/memory-mcp/src/memory/retrieval/lexical.rs','crates/memory-mcp/src/memory/retrieval/budget.rs'];saved={f:(repo/f).read_bytes() for f in files};sha=lambda x:hashlib.sha256(x).hexdigest()
patch=(repo/'evals/longmemeval_v2/results/2026-11-26-candidate-work/bounded-access-waves/source.patch').read_bytes()
command=['cargo','test','--locked','-p','memory_mcp','--test','lexical_term_set_resource','public_composite_source_tokens','--','--nocapture']
try:
 with tempfile.TemporaryDirectory() as tmp:
  for f in files:
   p=Path(tmp)/f;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(subprocess.check_output(['git','show','HEAD:'+f]))
  subprocess.run(['git','apply',*[f'--include={f}' for f in files]],input=patch,cwd=tmp,check=True)
  baseline={f:(Path(tmp)/f).read_bytes() for f in files}
  reg={'schema':'corrected-allocation-comparison/v1','measurement':'Count allocations during public caller future polls, excluding each wrapped storage future poll; background runtime tasks excluded. Same permanent2000-ID source8facts, budget5, literal IDs and2048byte excerpts; threshold1000000 unchanged. Baseline is bounded-access-waves normalization; all other current sources unchanged.','command':command,'test_sha256':sha((repo/'crates/memory-mcp/tests/lexical_term_set_resource.rs').read_bytes()),'candidate_sha256':{f:sha(s) for f,s in saved.items()},'baseline_sha256':{f:sha(s) for f,s in baseline.items()}}
  (root/'resource-registration.json').write_text(json.dumps(reg,indent=2)+'\n')
  for f,s in baseline.items():(repo/f).write_bytes(s)
  with (root/'corrected-baseline.log').open('w') as log:r=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,timeout=600)
  print('corrected_baseline_exit',r.returncode,flush=True)
finally:
 for f,s in saved.items():(repo/f).write_bytes(s)
 assert all((repo/f).read_bytes()==s for f,s in saved.items())
 (root/'candidate-restored.json').write_text(json.dumps({'all_three_candidate_sources_restored':True})+'\n')
cmd=['cargo','test','--locked','-p','memory_mcp','--test','lexical_term_set_resource','--','--nocapture']
with (root/'resource-green.log').open('w') as log:r=subprocess.run(cmd,stdout=log,stderr=subprocess.STDOUT,timeout=600)
print('candidate_resource_exit',r.returncode,flush=True)
if r.returncode==0:subprocess.run(['python3','/tmp/candidate-narrow-controls.py'],check=True)
