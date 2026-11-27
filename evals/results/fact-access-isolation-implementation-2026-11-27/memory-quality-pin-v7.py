from pathlib import Path
import subprocess,os,json,hashlib,time,platform
ROOT=Path('/Users/solovey/Documents/dev/memory_mcp');OUT=ROOT/'evals/results/fact-access-isolation-implementation-2026-11-27'
commands=[('product',['cargo','build','-p','memory_mcp','--features','eval-support','--bin','memory_mcp','--locked','--message-format=json'],{},'memory_mcp'),('eval',['cargo','build','-p','eval-harness','--bin','memory-eval','--locked','--message-format=json'],{},'memory-eval'),('history',['cargo','test','-p','memory_mcp','--features','eval-support','--test','full_history_evidence','--no-run','--locked','--message-format=json'],{'CARGO_PROFILE_TEST_OPT_LEVEL':'3'},'full_history_evidence')]
rows=[]
for name,cmd,env,target in commands:
 start=time.monotonic();log=Path('/tmp')/f'memory-quality-pin-v7-{name}.log'
 with log.open('xb') as f:r=subprocess.run(cmd,cwd=ROOT,env={**os.environ,**env},stdout=f,stderr=subprocess.STDOUT,timeout=1200)
 if r.returncode:raise SystemExit(f'{name} failed {r.returncode}; {log}')
 binaries=[]
 for line in log.read_text().splitlines():
  try:m=json.loads(line)
  except ValueError:continue
  if m.get('reason')=='compiler-artifact' and m.get('target',{}).get('name')==target and m.get('executable'):binaries.append(Path(m['executable']))
 if len(binaries)!=1:raise RuntimeError((name,binaries))
 dest=Path('/tmp')/f'memory-quality-{name}-v7-2026-11-27'
 import shutil
 if dest.exists():raise RuntimeError('pin already exists')
 shutil.copy2(binaries[0],dest)
 with dest.open('rb') as f:digest=hashlib.file_digest(f,'sha256').hexdigest()
 rows.append({'role':name,'command':cmd,'env':env,'path':str(dest),'sha256':digest,'elapsed_seconds':time.monotonic()-start,'log_sha256':hashlib.sha256(log.read_bytes()).hexdigest()})
 print(name,digest,round(rows[-1]['elapsed_seconds'],3),flush=True)
with (OUT/'binary-pins-v7.json').open('x') as f:json.dump({'architecture':platform.machine(),'binaries':rows},f,indent=2);f.write('\n')
