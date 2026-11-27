from pathlib import Path
from datetime import datetime,timezone
import hashlib,importlib.metadata,json,os,platform,subprocess,sys,time,urllib.request
root=Path('/Users/solovey/Documents/dev/memory_mcp');out=root/'evals/longmemeval_v2/results/2026-11-27-source-range-qualification';manifest=root/'evals/results/local-inference-startup-2026-11-27/model-bundle.json'
def sha(p):
 with Path(p).open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
bundle=json.loads(manifest.read_text());model=Path(bundle['model'])
for row in bundle['files']:
 file=model/row['path']
 if file.stat().st_size != row['bytes'] or sha(file)!=row['sha256']:raise RuntimeError('model bundle differs '+row['path'])
pid=int(Path('/tmp/memory-quality-owned-mlx-reader.pid').read_text());os.kill(pid,0)
assert platform.system()=='Darwin' and platform.machine()=='arm64'
end=time.monotonic()+60
while True:
 try:
  with urllib.request.urlopen('http://127.0.0.1:18767/v1/models',timeout=2) as response: health=json.load(response)
  break
 except OSError:
  if time.monotonic()>=end:raise
  time.sleep(1)
identity=dict(schema='source-range-local-execution/v1',frozen_at_utc=datetime.now(timezone.utc).isoformat(),architecture=platform.machine(),owned_server_pid=pid,server_script_sha256=sha('/tmp/memory-quality-owned-mlx-reader.py'),model_bundle_sha256=sha(manifest),all_model_files_rehashed=len(bundle['files']),python=sys.version,packages={p:importlib.metadata.version(p)for p in ('mlx','mlx-lm','openai','httpx')},registration_sha256=sha(out/'registration.json'),acceptance_sha256=sha(out/'acceptance.json'),source_review_sha256=sha(out/'source-review.json'),source_reference_sha256=sha(root/'evals/longmemeval_v2/source_references.py'),answer_matcher_sha256=sha(root/'evals/longmemeval_v2/exact_answer_quality.py'),model_list_health_response=health,concurrency='Local release HTTP tests active; no rustc observed before launch. An unrelated Docker build and system services also active. Resource isolation/cold cache not established; latency descriptive only, not a speedup comparison.',prior_reader_prompts_unchanged=213)
with (out/'execution-identity.json').open('x') as f:json.dump(identity,f,indent=2);f.write('\n')
print('verified model identity; starting sole80-call capture',flush=True)
result=subprocess.run([sys.executable,'-m','evals.longmemeval_v2.reader_protocol','run','--output',str(out)],cwd=root,timeout=1860)
raise SystemExit(result.returncode)
