from pathlib import Path
import subprocess
p=Path('crates/memory-mcp/src/memory/lifecycle_workers/decay.rs')
new=p.read_bytes()
try:
 old=subprocess.check_output(['git','show','HEAD:'+str(p)])
 p.write_bytes(old)
 with open('/tmp/memory-quality-decay-owner-red.log','wb') as log:
  code=subprocess.run(['cargo','test','-p','memory_mcp','--test','fact_access_upgrade','decay_heat_read_failure_leaves_cold_fact_valid','--locked'],stdout=log,stderr=subprocess.STDOUT,timeout=180).returncode
 if code!=101: raise RuntimeError('Expected regression scenario to reject original decay path: '+str(code))
finally:
 p.write_bytes(new)
