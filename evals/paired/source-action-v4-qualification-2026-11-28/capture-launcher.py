"""Sole native local capture; no model dependency is installed by this script."""
import asyncio
from datetime import datetime,timezone
import hashlib
import importlib.metadata
import json
from pathlib import Path
import platform
import sys
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from evals.paired.skill_workflow import capture,verify_plan

OUT = Path(__file__).resolve().parent


def sha(path):
    result = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda:stream.read(8*1024*1024),b''):
            result.update(block)
    return result.hexdigest()


def save(name,data):
    with (OUT/name).open('x') as target:
        json.dump(data,target,indent=2)
        target.write('\n')


async def main():
    acceptance = json.loads((OUT/'acceptance.json').read_text())
    pid_path = Path('/tmp/memory-quality-v4-owned-mlx.pid')
    runtime_started = pid_path.stat().st_mtime
    remaining = acceptance['global_budget_seconds']-(time.time()-runtime_started)
    deadline = time.monotonic()+remaining
    if remaining <= 0:
        raise TimeoutError('global deadline exhausted before capture')
    plan_path = OUT/'registered/frozen-plan.json'
    assert sha(plan_path)==acceptance['frozen_plan_sha256']
    review = json.loads((OUT/'pre-inference-source-review.json').read_text())
    assert review['accepted'] is True
    assert review['frozen_plan_sha256']==acceptance['frozen_plan_sha256']
    assert review['acceptance_sha256']==sha(OUT/'acceptance.json')
    plan = json.loads(plan_path.read_text())
    verify_plan(plan)
    manifest = ROOT/'evals/results/local-inference-startup-2026-11-27/model-bundle.json'
    bundle = json.loads(manifest.read_text())
    for record in bundle['files']:
        path = Path(bundle['model'])/record['path']
        assert path.stat().st_size==record['bytes'] and sha(path)==record['sha256']
    health = json.load(urllib.request.urlopen(plan['endpoint']+'/models',timeout=5))
    assert health['data'][0]['id']==plan['model']==bundle['model']
    assert platform.system()=='Darwin' and platform.machine()=='arm64'
    save('execution-identity.json',{'schema':'source-action-v4-local-execution/v1',
        'at_utc':datetime.now(timezone.utc).isoformat(),'acceptance_sha256':sha(OUT/'acceptance.json'),
        'model_bundle_sha256':sha(manifest),'all_model_files_rehashed':len(bundle['files']),
        'python':sys.version,'architecture':platform.machine(),'owned_server_pid':int(pid_path.read_text()),
        'owned_runtime_started_at_utc':datetime.fromtimestamp(runtime_started,timezone.utc).isoformat(),
        'global_budget_seconds':acceptance['global_budget_seconds'],
        'packages':{p:importlib.metadata.version(p) for p in ('mlx','mlx-lm','openai','httpx')},
        'pre_inference_source_review_sha256':sha(OUT/'pre-inference-source-review.json'),
        'scope':'native offline Apple Silicon MLX, loopback only; no LM Studio service or remote inference',
        'latency_scope':'descriptive; no isolated or causal latency claim'})
    warmup = urllib.request.Request(plan['endpoint']+'/chat/completions',data=json.dumps({
        'model':plan['model'],'messages':[{'role':'user','content':'Reply ready.'}],
        'max_tokens':8,'temperature':0}).encode(),headers={'Content-Type':'application/json'})
    started = time.monotonic()
    body = json.load(urllib.request.urlopen(warmup,timeout=min(120,deadline-time.monotonic())))
    save('runtime-preflight.json',{'scope':'startup only, no task/gold score',
        'elapsed_seconds':time.monotonic()-started,'response':body})
    started = time.monotonic()
    try:
        await asyncio.wait_for(capture(OUT/'registered'),timeout=deadline-time.monotonic()-30)
    except Exception as error:
        save('capture-failed.json',{'error_type':type(error).__name__,'error':str(error),
            'scheduled':12,'elapsed_seconds':time.monotonic()-started,
            'missing_trials':'remain failed/unobserved in fixed12 denominator; no retry'})
        raise
    save('capture-completed.json',{'scheduled':12,'elapsed_seconds':time.monotonic()-started,
        'runtime_elapsed_seconds':time.time()-runtime_started,'at_utc':datetime.now(timezone.utc).isoformat()})


if __name__=='__main__':
    asyncio.run(main())
