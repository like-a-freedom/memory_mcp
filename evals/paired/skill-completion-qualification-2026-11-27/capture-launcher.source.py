from pathlib import Path
from datetime import datetime,timezone
import asyncio,hashlib,json,platform,sys,urllib.request,importlib.metadata,time
from evals.paired.skill_workflow import capture
root=Path('/Users/solovey/Documents/dev/memory_mcp');out=root/'evals/paired/skill-completion-qualification-2026-11-27'
def sha(p):
 h=hashlib.sha256()
 with Path(p).open('rb')as f:
  for b in iter(lambda:f.read(8*1024*1024),b''):h.update(b)
 return h.hexdigest()
manifest=root/'evals/results/local-inference-startup-2026-11-27/model-bundle.json';bundle=json.loads(manifest.read_text())
for record in bundle['files']:
 p=Path(bundle['model'])/record['path'];assert p.stat().st_size==record['bytes'];assert sha(p)==record['sha256']
health=json.load(urllib.request.urlopen('http://127.0.0.1:18767/v1/models',timeout=5));assert health['data'][0]['id']==bundle['model']
assert platform.machine()=='arm64'
identity={'schema':'bounded-workflow-local-execution/v1','frozen_at_utc':datetime.now(timezone.utc).isoformat(),'architecture':platform.machine(),'owned_server_pid':int(Path('/tmp/memory-quality-owned-mlx-reader.pid').read_text()),'server_script_sha256':sha('/tmp/memory-quality-owned-mlx-reader.py'),'model_bundle_sha256':sha(manifest),'all_model_files_rehashed':len(bundle['files']),'python':sys.version,'packages':{p:importlib.metadata.version(p)for p in ['mlx','mlx-lm','openai','httpx']},'acceptance_sha256':sha(out/'acceptance.json'),'model_health':health,'order':['fresh-native','legacy-regression'],'runtime_scope':'native Apple Silicon local MLX; no LM Studio service, no remote inference','latency_scope':'descriptive; system services and short xtask checks can overlap, no causal performance comparison','binary_production_source_commit':'96998c43ca7265e40bfcfdf8904ab40ead0bb058'}
with(out/'execution-identity.json').open('x')as f:json.dump(identity,f,indent=2);f.write('\n')
warmup_request=urllib.request.Request('http://127.0.0.1:18767/v1/chat/completions',data=json.dumps({'model':bundle['model'],'messages':[{'role':'user','content':'Reply ready.'}],'max_tokens':8,'temperature':0}).encode(),headers={'Content-Type':'application/json'})
warmup_started=time.monotonic()
warmup=json.load(urllib.request.urlopen(warmup_request,timeout=120))
with(out/'runtime-preflight.json').open('x')as f:json.dump({'scope':'startup warmup only, no task/gold or benchmark score','elapsed_seconds':time.monotonic()-warmup_started,'response':warmup},f,indent=2);f.write('\n')
started=time.monotonic()
async def main():
 for name in identity['order']:
  print('START',name,flush=True);await capture(out/name);print('END',name,flush=True)
asyncio.run(main())
with(out/'capture-completed.json').open('x')as f:json.dump({'elapsed_seconds':time.monotonic()-started,'at_utc':datetime.now(timezone.utc).isoformat(),'scheduled':44},f,indent=2);f.write('\n')
