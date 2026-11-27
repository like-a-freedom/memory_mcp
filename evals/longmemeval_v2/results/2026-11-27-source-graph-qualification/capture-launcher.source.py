from pathlib import Path
from datetime import datetime,timezone
import hashlib,importlib.metadata,json,os,platform,subprocess,sys
ROOT=Path('/Users/solovey/Documents/dev/memory_mcp');OUT=ROOT/'evals/longmemeval_v2/results/2026-11-27-source-graph-qualification';manifest=ROOT/'evals/results/local-inference-startup-2026-11-27/model-bundle.json'
def sha(p):
 with Path(p).open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
bundle=json.loads(manifest.read_text());model=Path(bundle['model'])
for row in bundle['files']:
 file=model/row['path']
 if file.stat().st_size!=row['bytes'] or sha(file)!=row['sha256']:raise RuntimeError('model bundle differs: '+row['path'])
pid=int(Path('/tmp/memory-quality-owned-mlx-reader.pid').read_text());os.kill(pid,0)
identity=dict(schema='source-graph-local-execution/v1',frozen_at_utc=datetime.now(timezone.utc).isoformat(),architecture=platform.machine(),owned_server_pid=pid,server_script_sha256=sha('/tmp/memory-quality-mlx-source-graph-start.py'),server_start_log='/tmp/memory-quality-mlx-source-graph-start.log',server_startup_seconds=None,model_state='Owned server restarted before capture after absent previous PID; no prior requests in this runtime; OS/weight caches unmeasured. Not proof of cold state or a latency comparison; all weights rehashed.',model_bundle_sha256=sha(manifest),all_model_files_rehashed=len(bundle['files']),python=sys.version,packages={p:importlib.metadata.version(p) for p in ('mlx','mlx-lm','openai','httpx')},reader_source_sha256=sha(ROOT/'evals/longmemeval_v2/reader_protocol.py'),registration_sha256=sha(OUT/'registration.json'),acceptance_sha256=sha(OUT/'acceptance.json'),source_review_sha256=sha(OUT/'source-review.json'),prior_prompts_unchanged=165,source_only_v7_gate_sha256=sha(ROOT/'evals/longmemeval_v2/results/2026-11-27-full-history-positional-v7/candidate-decision-v1.json'),final_readiness_checks_sha256=sha(ROOT/'evals/results/fact-access-isolation-implementation-2026-11-27/positional-final-readiness-checks.json'),concurrent_owned_cargo_or_native_evals=False)
if platform.machine()!='arm64':raise RuntimeError('native Apple Silicon only for this actual capture')
with (OUT/'execution-identity.json').open('x') as f:json.dump(identity,f,indent=2);f.write('\n')
print('verified model bundle and capture identity',flush=True)
result=subprocess.run([sys.executable,'-m','evals.longmemeval_v2.reader_protocol','run','--output',str(OUT)],cwd=ROOT,timeout=1860)
raise SystemExit(result.returncode)
