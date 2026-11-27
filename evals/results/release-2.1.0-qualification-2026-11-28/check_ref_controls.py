import json,os,subprocess
from pathlib import Path
import yaml
workflow=yaml.safe_load(Path('.github/workflows/release.yml').read_text())
step=workflow['jobs']['ref']['steps'][0]
cases=[('unknown ref type','other','master','v2.1.0','123',False),('prerelease build tag','tag','v2.1.0-rc.1+build.2','v2.1.0-rc.1+build.2','',True),('tag exact','tag','v2.1.0','v2.1.0','',True),('tag mismatch','tag','v2.0.0','v2.1.0','123',False),('master qualified recovery','branch','master','v2.1.0','123',True),('master unqualified','branch','master','v2.1.0','',False),('foreign branch','branch','feature','v2.1.0','123',False),('tag invalid','branch','master','main','123',False),('candidate malformed','branch','master','v2.1.0','../123',False)]
rows=[]
for name,kind,ref,tag,candidate,want in cases:
 result=subprocess.run(['bash','-c',step['run']],env={**os.environ,'REF_TYPE':kind,'REF_NAME':ref,'TAG':tag,'CANDIDATE_RUN_ID':candidate},capture_output=True,text=True,timeout=5)
 rows.append({'case':name,'expected':want,'actual':result.returncode==0,'matched':(result.returncode==0)==want})
print(json.dumps(rows,indent=2));raise SystemExit(0 if all(r['matched']for r in rows)else 1)
