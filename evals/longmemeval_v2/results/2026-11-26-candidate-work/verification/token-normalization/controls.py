from pathlib import Path
import subprocess,time,json
root=Path('evals/longmemeval_v2/results/2026-11-26-candidate-work/verification/token-normalization');root.mkdir(exist_ok=True)
commands=[('retrieval',['cargo','test','--locked','-p','memory_mcp','--lib','memory::retrieval']),('shared',['cargo','test','--locked','-p','memory_mcp','--lib','shared::']),('public',['cargo','test','--locked','-p','memory_mcp','--features','eval-support','--test','episode_candidate_hydration','--test','equivalent_alias_reads','--test','fact_access_payload','--test','fact_access_concurrency','--test','graph_read_concurrency','--test','lexical_candidate_hydration','--test','retrieval_partial_evidence','--test','long_history_stage_evidence','--test','retrieval_query_projection','--test','retrieval_query_batching','--test','query_scaling_regressions'])]
results=[]
for name,cmd in commands:
 t=time.monotonic()
 with (root/(name+'.log')).open('w') as log:r=subprocess.run(cmd,stdout=log,stderr=subprocess.STDOUT,timeout=600)
 row={'name':name,'command':cmd,'exit_code':r.returncode,'seconds':time.monotonic()-t};results.append(row);(root/'commands.json').write_text(json.dumps(results,indent=2)+'\n');print(json.dumps(row),flush=True)
 if r.returncode:break
