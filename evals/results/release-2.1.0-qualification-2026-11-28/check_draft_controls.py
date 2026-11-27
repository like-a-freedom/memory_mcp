import json,os,subprocess,tempfile
from pathlib import Path
import yaml
step=yaml.safe_load(Path('.github/workflows/release.yml').read_text())['jobs']['publish']['steps'][0]
cases=[('reviewed draft',{'isDraft':True,'body':'Reviewed'},False,0,True),('public needs explicit recovery',{'isDraft':False,'body':'Reviewed'},False,0,False),('public explicit recovery',{'isDraft':False,'body':'Reviewed'},True,0,True),('empty notes',{'isDraft':True,'body':''},False,0,False),('lookup failed',{},False,1,False),('malformed state',None,False,0,False)]
rows=[]
with tempfile.TemporaryDirectory(prefix='memory-release-draft-controls-')as temporary:
 root=Path(temporary);binary=root/'bin';binary.mkdir();gh=binary/'gh';gh.write_text('#!/usr/bin/env python3\nimport os,sys\nprint(os.environ["STATE"])\nsys.exit(int(os.environ["LOOKUP_EXIT"]))\n');gh.chmod(0o755)
 for index,(name,state,allowed,code,want)in enumerate(cases):
  work=root/str(index);work.mkdir();result=subprocess.run(['bash','-c',step['run']],env={**os.environ,'PATH':str(binary)+os.pathsep+os.environ['PATH'],'STATE':json.dumps(state)if state is not None else 'malformed','LOOKUP_EXIT':str(code),'RUNNER_TEMP':str(work),'GITHUB_OUTPUT':str(work/'output'),'TAG':'v2.1.0','ALLOW_RECOVERY':str(allowed).lower()},text=True,capture_output=True,timeout=5);rows.append({'case':name,'expected':want,'actual':result.returncode==0,'matched':(result.returncode==0)==want})
print(json.dumps(rows,indent=2));raise SystemExit(0 if all(r['matched']for r in rows)else 1)
