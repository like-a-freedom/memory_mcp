from pathlib import Path
from datetime import datetime,timezone
import json,hashlib,re,subprocess
from evals.longmemeval_v2.reader_protocol import read_rows,protocol_messages,summarize
ROOT=Path('/Users/solovey/Documents/dev/memory_mcp');BASE=ROOT/'evals/longmemeval_v2/results';OUT=BASE/'2026-11-27-source-graph-qualification'
def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
checks={};count=0
for directory in sorted(BASE.iterdir()):
 if not (directory/'registration.json').exists() or not (directory/'prompts.json').exists():continue
 plan=json.loads((directory/'registration.json').read_text())
 if plan.get('schema')!='memory-mcp-reader-protocol/v1':continue
 rows=read_rows(Path(plan['rows_path']));prompts=json.loads((directory/'prompts.json').read_text())
 for row in rows:
  for variant in plan['variants']:
   assert protocol_messages(row,variant)==prompts[row['question_id']][variant],(directory.name,row['question_id'],variant)
   count+=1
checks['all_frozen_prompts_identical']=count
plan=json.loads((OUT/'registration.json').read_text());truth=json.loads(Path(plan['truth_path']).read_text())['cases'];rows=read_rows(Path(plan['rows_path']));outputs=json.loads((OUT/'outputs.json').read_text());report=json.loads((OUT/'report.json').read_text())
assert sha(Path(plan['rows_path']))==plan['rows_sha256'] and sha(Path(plan['truth_path']))==plan['truth_sha256']
for variant,actual in report['variants'].items():assert summarize(rows,truth,outputs[variant],variant)==actual
checks['report_replayed_exactly_from_raw_outputs']=True
accept=json.loads((OUT/'acceptance.json').read_text());decision=json.loads((OUT/'candidate-decision-v1.json').read_text())
assert sha(OUT/'registration.json')==accept['registration_sha256']
for name,value in decision['input_sha256'].items():assert sha(OUT/name)==value,name
checks['candidate_decision_input_hashes_verified']=len(decision['input_sha256'])
review=json.loads((OUT/'final-semantic-review-v1.json').read_text())
for name,value in review['registration_hashes'].items():assert sha(OUT/name)==value,name
for name,value in review['source_hashes'].items():
 if name.endswith('.py'):assert sha(ROOT/name)==value,name
checks['reviewed_code_unchanged']=True
checks['reviewed_documents_status_addenda']='Spec and plan have actual-result/checklist updates after review; frozen registration/code unchanged.'
p=ROOT/'evals/results/fact-access-isolation-implementation-2026-11-27';physical=json.loads((p/'final-readonly-review-v3.json').read_text())['source_sha256']|json.loads((p/'fact-access-index-readiness-tdd.json').read_text())['changed_source_sha256']
n=0
for name,value in physical.items():
 if name.startswith('crates/'):
  assert sha(ROOT/name)==value,name;n+=1
checks['reviewed_current_rust_files_unchanged']=n
for receipt in [ROOT/'evals/results/memory-quality-verification-2026-11-27/reader-source-graph-final-python.json',p/'positional-final-readiness-checks.json']:
 d=json.loads(receipt.read_text());items=d['checks'] if isinstance(d,dict) else d
 for item in items:
  assert item['exit_code']==0
  assert sha(Path(item['log']))==item['sha256'],item['log']
checks['verification_log_hashes_match']=True
paths=[ROOT/'docs/evals/MEMORY_QUALITY_FOLLOWUP_2026-11-27.md',ROOT/'docs/superpowers/plans/2026-11-26-post-release-memory-quality.md',ROOT/'docs/superpowers/plans/2026-11-27-reader-source-graph.md',ROOT/'docs/superpowers/specs/2026-11-27-independent-memory-benefit-decision.md']
links=0
for path in paths:
 for target in re.findall(r'\]\(([^\s)]+)\)',path.read_text()):
  if '://' in target or target.startswith('#'):continue
  target=target.split('#',1)[0]
  if not target:continue
  assert (path.parent/target).exists(),(path,target);links+=1
checks['local_document_links_resolve']=links
assert json.loads((OUT/'runtime-cleanup.json').read_text())['exit_observed']
assert not json.loads((OUT/'full-history-activation.json').read_text())['admitted']
assert json.loads((ROOT/'evals/paired/benefit-decision-2026-11-27/decision-v3.json').read_text())['status']=='NO_GO'
checks['negative_gates_and_cleanup_retained']=True
result=subprocess.run(['git','diff','--check'],cwd=ROOT,capture_output=True,text=True);assert result.returncode==0,result.stdout+result.stderr
checks['git_diff_check']=True
(OUT/'final-integrity.json').write_text(json.dumps(dict(schema='memory-quality-final-integrity/v1',at_utc=datetime.now(timezone.utc).isoformat(),checks=checks,qualification='Reader FAIL retained; C2 NO_GO. These are integrity/verification checks, not additional production scenario counts.'),indent=2)+'\n')
print(json.dumps(checks,indent=2))
