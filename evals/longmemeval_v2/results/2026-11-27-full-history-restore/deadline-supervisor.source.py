"""Bound only the owned source restoration; never targets other processes."""
import json,os,signal,subprocess,time
from pathlib import Path
PID=91879
OUT=Path('/tmp/memory-quality-full-100-2026-11-27/deadline-receipt.json')
def own_process():
 r=subprocess.run(['ps','-p',str(PID),'-o','etime=','-o','command='],capture_output=True,text=True,check=False)
 if r.returncode or ' -m evals.longmemeval_v2.build_preflight' not in r.stdout:return None
 elapsed=r.stdout.strip().split()[0];parts=elapsed.split(':');seconds=sum(int(p)*60**i for i,p in enumerate(reversed(parts)))
 return seconds
start=own_process()
if start is None:raise SystemExit('Owned source build already exited')
remaining=5400-start
end=time.monotonic()+remaining
outcome='exited_before_continuation_ceiling'
while own_process() is not None:
 if time.monotonic()>=end:
  # Find only direct owned CLI children after verifying parent identity again.
  rows=subprocess.check_output(['ps','-Ao','pid=,ppid=,command='],text=True)
  for row in rows.splitlines():
   parts=row.strip().split(None,2)
   if len(parts)==3 and int(parts[1])==PID and '/memory-quality-product-2026-11-27 ' in parts[2]:os.kill(int(parts[0]),signal.SIGTERM)
  if own_process() is not None:os.kill(PID,signal.SIGTERM)
  outcome='continuation_deadline_terminated'
  break
 time.sleep(min(10,max(0.01,end-time.monotonic())))
OUT.write_text(json.dumps({'owned_build_pid':PID,'whole_build_ceiling_seconds':5400,'outcome':outcome,'original_gate_seconds':1800,'original_gate_assessed_separately':True},indent=2)+'\n')
