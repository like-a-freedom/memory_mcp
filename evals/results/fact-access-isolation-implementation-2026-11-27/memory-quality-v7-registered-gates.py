from pathlib import Path
import os,json,hashlib,subprocess,time
ROOT=Path('/Users/solovey/Documents/dev/memory_mcp'); os.chdir(ROOT)
OUT=ROOT/'evals/results/native-v7-registered-2026-11-27'
BIN=Path('/tmp/memory-quality-eval-v7-2026-11-27');PRODUCT=Path('/tmp/memory-quality-product-v7-2026-11-27')
def sha(p):
 with Path(p).open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def save(p,v):
 with p.open('x') as f:json.dump(v,f,indent=2);f.write('\n')
old=json.loads((ROOT/'evals/results/native-quality-2026-11-27/registration.json').read_text())
commands=[('temporal',[str(BIN),'run','--profile','evals/profiles/memory_decisions_current.json','--artifact',str(OUT/'run.json')],180,{'MEMORY_DECISIONS_SERVER_BIN':str(PRODUCT),'MEMORY_DECISIONS_PRODUCT_REVISION':'496368549d856957e5fa317c7adea462d925288b+quoted-controls-sha:'+sha(ROOT/'crates/memory-mcp/src/memory/retrieval/query_mode.rs'),'MEMORY_DECISIONS_ARTIFACT_ROOT':str(OUT/'current')}),('comparison',[str(BIN),'compare-current-memory','--baseline','evals/paired/current-contract-baseline-2026-11-26/receipts/index.json','--candidate',str(OUT/'current/index.json'),'--corpus','evals/corpora/memory-decisions-v10','--output',str(OUT/'comparison.json')],30,{}),('retrieval',[str(BIN),'run','--profile','evals/profiles/retrieval_quality.json','--artifact',str(OUT/'retrieval.json')],600,old['model_env'])]
if not OUT.exists():
 OUT.mkdir();save(OUT/'registration.json',{'schema':'native-v7-regression/v1','registered_before_outputs':True,'no_reruns':True,'runner_sha256':sha(BIN),'product_sha256':sha(PRODUCT),'source_admission_sha256':sha(ROOT/'crates/memory-mcp/src/memory/retrieval/query_mode.rs'),'formation_unchanged':'anno|identity-admission=v3; source-only r29 not relabelled or regenerated','expected_temporal':8,'expected_retrieval':37,'required_invalid':0,'floors':'unchanged original six 1.0','commands':[{'name':n,'argv':c,'deadline_seconds':s,'env':e} for n,c,s,e in commands]})
if __name__=='__main__' and len(__import__('sys').argv)>1:
 for name,command,seconds,env in commands:
  start=time.monotonic();fault=None
  with (OUT/f'{name}.log').open('xb') as f:
   try:r=subprocess.run(command,env={**os.environ,**env},stdout=f,stderr=subprocess.STDOUT,timeout=seconds);code=r.returncode
   except subprocess.TimeoutExpired:code=None;fault='external_timeout'
  receipt={'command':command,'exit_code':code,'fault':fault,'elapsed_seconds':time.monotonic()-start,'log_sha256':sha(OUT/f'{name}.log')};save(OUT/f'{name}-execution.json',receipt);print(name,receipt,flush=True)
  if code!=0:raise SystemExit(1)
