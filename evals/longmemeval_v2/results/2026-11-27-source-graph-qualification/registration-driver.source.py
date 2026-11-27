from pathlib import Path
from datetime import datetime,timezone
import hashlib,json,shutil
from evals.longmemeval_v2.reader_protocol import freeze,protocol_messages,read_rows,project_sources
ROOT=Path('/Users/solovey/Documents/dev/memory_mcp');BASE=ROOT/'evals/longmemeval_v2/results';OLD=BASE/'2026-11-27-source-ledger-qualification';OUT=BASE/'2026-11-27-source-graph-qualification';FIX=ROOT/'evals/longmemeval_v2/fixtures/source-graph-independent-v1'
def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
prior=[]
for directory in sorted(BASE.iterdir()):
 if not (directory/'registration.json').exists() or not (directory/'prompts.json').exists():continue
 plan=json.loads((directory/'registration.json').read_text())
 if plan.get('schema')!='memory-mcp-reader-protocol/v1':continue
 rows=read_rows(Path(plan['rows_path']));prompts=json.loads((directory/'prompts.json').read_text());count=0
 for row in rows:
  for variant in plan['variants']:
   assert protocol_messages(row,variant)==prompts[row['question_id']][variant],(directory.name,row['question_id'],variant)
   count+=1
 prior.append(dict(directory=directory.name,prompts=count,sha256=sha(directory/'prompts.json')))
print('prior prompts preserved:',sum(p['prompts'] for p in prior))
FIX.mkdir(exist_ok=False)
controls=[
 ('graph-equal-links','In the Bay menu, which link follows Dock?',"[1] menu 'Bay'\n  [2] link 'Dock'\n  [3] link 'Lagoon'\n  [4] link 'Dock'\n  [5] link 'Lagoon'\n[6] heading 'End'\n",'Lagoon','single_value','Both Dock occurrences have the same next link value; different source lines are not a conflict.'),
 ('graph-conflicting-links','In the Bay menu, which link follows Dock?',"[1] menu 'Bay'\n  [2] link 'Dock'\n  [3] link 'Lagoon'\n  [4] link 'Dock'\n  [5] link 'Delta'\n[6] heading 'End'\n",'UNKNOWN','unknown','Two selected-role Dock occurrences have different successors.'),
 ('graph-opaque-repeat','In the Bay toolbar, which button follows Dock?',"[1] toolbar 'Bay'\n  [2] button 'Dock'\n  [3] button 'Lagoon'\n  [4] button 'Dock'\n  [5] group 'Nested'\n    [6] button 'Delta'\n  [7] button 'Lagoon'\n[8] heading 'End'\n",'UNKNOWN','unknown','One selected anchor has an opaque group after it; do not skip the group to invent a peer successor.'),
 ('graph-scope-count','How many buttons are in the Bay toolbar?',"[1] toolbar 'Bay'\n  [2] button 'Dock'\n  [3] button 'Lagoon'\n  [4] button 'Dock'\n  [5] button 'Lagoon'\n[6] toolbar 'Ridge'\n  [7] button 'Peak'\n  [8] button 'Slope'\n[9] heading 'End'\n",'4','single_value','Four distinct button nodes in Bay, including repeated names; Ridge belongs to another scope.'),
 ('graph-role-selection','In the Bay toolbar, which button immediately follows the button Dock?',"[1] toolbar 'Bay'\n  [2] button 'Dock'\n  [3] button 'Lagoon'\n  [4] link 'Dock'\n  [5] link 'Delta'\n[6] heading 'End'\n",'Lagoon','single_value','Requested anchor and successor are buttons; same-named link is outside selected anchor role.'),
 ('graph-virtualized-last','In the Bay toolbar, which button follows Lagoon?',"[1] toolbar 'Bay', virtualized=true\n  [2] button 'Dock'\n  [3] button 'Lagoon'\n[4] heading 'End'\n",'UNKNOWN','unknown','Virtualized membership cannot establish that the last rendered button has no successor.'),
 ('graph-escaped-name','In the Bay toolbar, which button follows Quai?',"[1] toolbar 'Bay'\n  [2] button 'Quai'\n  [3] button 'Crème'\n[4] heading 'End'\n",'Crème','single_value','Literal source successor Crème is reversibly JSON escaped in the raw stored input.'),
 ('graph-single-closed-end','In the Bay toolbar, which button follows Dock?',"[1] toolbar 'Bay'\n  [2] button 'Dock'\n[3] heading 'End'\n",'None','false_premise','Single action in a closed complete toolbar has no following button.')]
newrows=[];newtruth={};review=[]
for key,question,source,answer,kind,reason in controls:
 raw=json.dumps(source,ensure_ascii=True)
 row=dict(question_id=key,question_text=question,prompt_messages=[],memory_context=[dict(type='text',value=raw)])
 newrows.append(row);newtruth[key]=dict(accepted_answers=[answer],answer_kind=kind)
 projection,=project_sources(row['memory_context'],source_graph=True)
 assert projection['text']==source
 review.append(dict(question_id=key,source_sha256=sha_bytes(source.encode()) if False else hashlib.sha256(source.encode()).hexdigest(),answer=answer,rationale=reason,source_lines=source.splitlines(),graph=projection['graph']))
(FIX/'rows.jsonl').write_text(''.join(json.dumps(r,ensure_ascii=True)+'\n' for r in newrows))
(FIX/'truth.json').write_text(json.dumps(dict(cases=newtruth),indent=2,ensure_ascii=True)+'\n')
(FIX/'README.md').write_text('# Source-graph development controls\n\nEight author-created, source-reviewed controls. They are not a hidden benchmark or independent memory-benefit confirmation. The graph receives no questions, labels or accepted answers. See the frozen qualification source review for literal sources and worked rationales.\n')
rows=read_rows(OLD/'rows.jsonl')+newrows;truth=json.loads((OLD/'truth.json').read_text())['cases']|newtruth
# Inputs outside the output directory allow freeze to claim its directory exclusively.
inputs=ROOT/'evals/longmemeval_v2/fixtures/source-graph-independent-v1/qualification'
inputs.mkdir()
(inputs/'rows.jsonl').write_text(''.join(json.dumps(r,ensure_ascii=True)+'\n' for r in rows))
(inputs/'truth.json').write_text(json.dumps(dict(cases=truth),indent=2,ensure_ascii=True)+'\n')
freeze(OUT,inputs/'rows.jsonl',inputs/'truth.json',str(Path('/Users/solovey/.cache/lm-studio/models/lmstudio-community/Qwen3.8-27B-MLX-4bit')),'http://127.0.0.1:18767/v1',variants=['source-ledger-final-first','source-graph-final-first'])
shutil.copy2(inputs/'rows.jsonl',OUT/'rows.jsonl');shutil.copy2(inputs/'truth.json',OUT/'truth.json')
plan=json.loads((OUT/'registration.json').read_text());plan['cohorts']={'previously_exposed_sixteen':[r['question_id'] for r in rows[:16]],'new_source_reviewed_eight':[r['question_id'] for r in newrows]}
(OUT/'registration.json').write_text(json.dumps(plan,indent=2)+'\n')
accept=dict(schema='reader-qualification-acceptance/v1',frozen_at_utc=datetime.now(timezone.utc).isoformat(),before_inference_outputs=True,registration_sha256=sha(OUT/'registration.json'),runner_sha256=sha(ROOT/'evals/longmemeval_v2/reader_protocol.py'),scheduled=48,case_families=24,candidate='source-graph-final-first',all_cases_required=24,per_case_required=['completed correct frozen value','one valid boxed answer','one literal delivered-source quote','no error or length truncation'],stop_rule='One new capture, retries0; preserve all outputs/failures, no repair/relabel/budget changes. On FAIL stop protocol iteration and do not activate actual full-history reader comparison.',not_claimed=['held-out benefit confirmation','literal quote as full entailment/source-binding proof','autonomous workflow qualification'])
(OUT/'acceptance.json').write_text(json.dumps(accept,indent=2)+'\n')
(OUT/'source-review.json').write_text(json.dumps(dict(schema='reader-source-graph-source-review/v1',before_outputs=True,author_review_limitation='Author-created development controls, authored and reviewed by implementer; no blinded independent qualification claim.',original_sixteen_truth_sha256=sha(OLD/'truth.json'),original_sixteen_rows_sha256=sha(OLD/'rows.jsonl'),original_sixteen_preserved=all(rows[i]==r for i,r in enumerate(read_rows(OLD/'rows.jsonl'))),new_controls=review,prior_frozen_prompts=prior),indent=2,ensure_ascii=True)+'\n')
shutil.copy2(ROOT/'evals/longmemeval_v2/reader_protocol.py',OUT/'runner.source.py')
for name in ['red','repeat-red','completeness-red','green','focused']:
 src=Path('/tmp/memory-quality-source-graph-'+name+'.log')
 if src.exists():shutil.copy2(src,OUT/('deterministic-'+name+'.log'))
(OUT/'README.md').write_text('# Source-graph qualification, 27 November 2026\n\nFrozen before outputs: 24 cases (16 exposed + 8 new author-created/source-reviewed development controls), 48 calls; source-ledger versus source-graph. No label or budget changes: 256 completion tokens, 60 seconds/request, retries0, 1800 seconds total. Acceptance requires all 24 candidate cases completed correct, format-valid and literal-source-quoted, with zero faults/truncations. On FAIL stop protocol iteration; full-history reader stays unactivated. No held-out or autonomous-agent benefit claim.\n')
print('frozen',len(rows),'cases',sha(OUT/'registration.json'))
