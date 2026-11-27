"""Bounded owned native runtime and sole registered workflow capture."""
from datetime import datetime,timezone
import hashlib,json,os,signal,socket,subprocess,time,urllib.request
from pathlib import Path
root=Path('/Users/solovey/Documents/dev/memory_mcp')
out=root/'evals/paired/source-action-v4-qualification-2026-11-28'
python='/private/tmp/memory-mcp-lme-postb5-venv/bin/python'
acceptance=json.loads((out/'acceptance.json').read_text())
review=json.loads((out/'pre-inference-source-review.json').read_text())
assert review['accepted'] is True
assert review['acceptance_sha256']==hashlib.sha256((out/'acceptance.json').read_bytes()).hexdigest()
try:
 with socket.create_connection(('127.0.0.1',18767),timeout=1): pass
except ConnectionRefusedError: pass
else: raise SystemExit('Port18767 already occupied; not touching another process')
with (out/'runtime.log').open('x') as log:
 env={**os.environ,'HF_HUB_OFFLINE':'1','TRANSFORMERS_OFFLINE':'1','PYTHONDONTWRITEBYTECODE':'1'}
 runtime=subprocess.Popen([python,'-B','/tmp/memory-quality-v4-owned-mlx.py'],cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT)
 started=time.monotonic()
 try:
  for _ in range(90):
   if runtime.poll() is not None: raise RuntimeError('Owned runtime exited before health')
   try:
    with urllib.request.urlopen('http://127.0.0.1:18767/v1/models',timeout=1) as health:
     json.load(health)
    break
   except (OSError,ValueError): time.sleep(1)
  else: raise TimeoutError('Owned runtime startup exceeded90s')
  remaining=acceptance['global_budget_seconds']-(time.monotonic()-started)-15
  result=subprocess.run([python,'-B',str(out/'capture-launcher.py')],cwd=root,env=env,timeout=remaining)
  if result.returncode: raise RuntimeError('Capture launcher failed with exit '+str(result.returncode))
 finally:
  if runtime.poll() is None:
   runtime.terminate()
   try: runtime.wait(timeout=10)
   except subprocess.TimeoutExpired:
    runtime.kill();runtime.wait(timeout=5)
  (out/'owned-runtime-cleanup.json').write_text(json.dumps({'pid':runtime.pid,'returncode':runtime.returncode,'elapsed_seconds':time.monotonic()-started,'at_utc':datetime.now(timezone.utc).isoformat(),'scope':'only the child process created by this coordinator was stopped; no LM Studio service used'},indent=2)+'\n')
