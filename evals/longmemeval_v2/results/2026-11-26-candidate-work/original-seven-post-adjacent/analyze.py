from pathlib import Path
import hashlib,json,statistics,subprocess,sys,time
repo=Path.cwd();root=Path(__file__).resolve().parent
baseline=root.parent/'original-seven-post-container'
def read(p):return json.loads(p.read_text())
def rows(p):return [json.loads(s) for s in p.read_text().splitlines() if s]
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
assert all(read(root/'final-integrity.json').values())
registration=read(root/'analysis-registration.json')
assert all(digest(repo/p)==h for p,h in registration['input_sha256'].items())
native={r['question_id']:r for r in rows(root/'scores/per_question.jsonl')};readers=rows(root/'scores/reader_outputs.jsonl');prompts=rows(root/'scores/prompt_rows.jsonl')
assert len(native)==len(readers)==len(prompts)==7
assert {r['question_id'] for r in readers}==set(native)
for r in readers:
 assert r['status']=='captured' and r['response_raw']==native[r['question_id']]['response_raw']
 r['response_parsed_boxed']=native[r['question_id']]['response_parsed_boxed']
(root/'diagnostic-readers.jsonl').write_text(''.join(json.dumps(r,ensure_ascii=False)+'\n' for r in readers))
python=repo/'target/c4-native/env/bin/python';common=['--cohort',repo/'evals/longmemeval_v2/small-web-text-cohort.json','--trajectories',repo/'target/c4-native/cohort/trajectories.json']
commands=[('current-span-v2',[python,'-m','evals.longmemeval_v2.reviewed_span_coverage',*common,'--per-question',root/'scores/prompt_rows.jsonl','--output',root/'source-presence-v2.json']),('baseline-span-v2',[python,'-m','evals.longmemeval_v2.reviewed_span_coverage',*common,'--per-question',baseline/'scores/prompt_rows.jsonl','--output',root/'baseline-source-presence-v2.json']),('strict-phrase-set',[python,'-m','evals.longmemeval_v2.exact_answer_quality','--per-question',root/'scores/per_question.jsonl','--output',root/'strict-phrase-set.json']),('independent-answer-diagnosis',[python,'-m','evals.longmemeval_v2.long_history_diagnosis',*common,'--truth',repo/'evals/longmemeval_v2/results/2026-11-26-long-history-diagnosis/reviewed-truth.json','--prompts',root/'scores/prompt_rows.jsonl','--readers',root/'diagnostic-readers.jsonl','--output',root/'independent-diagnosis.json'])]
log=[]
for name,command in commands:
 start=time.monotonic()
 with (root/(name+'.log')).open('w') as output:r=subprocess.run([str(x) for x in command],stdout=output,stderr=subprocess.STDOUT,timeout=60)
 log.append({'name':name,'command':[str(x) for x in command],'exit_code':r.returncode,'elapsed_seconds':time.monotonic()-start})
 (root/'offline-commands.json').write_text(json.dumps(log,indent=2)+'\n');assert r.returncode==0
current=read(root/'source-presence-v2.json');previous=read(root/'baseline-source-presence-v2.json');diag=read(root/'independent-diagnosis.json');cover=read(root/'scores/score-coverage.json')
def latency(ps):
 values=[p['memory_query_duration_seconds'] for p in ps];return {'count':len(values),'mean':statistics.mean(values),'median':statistics.median(values),'max':max(values),'includes':'private store copy, startup and CLI read; concurrent three-worker original profile'}
summary={'schema':'original-seven-final-analysis/v1','native_scorer':cover,'strict_independent_answers':{'correct':diag['correct'],'selected':diag['selected'],'complete_evidence_reader_failures':diag['reader_failures_with_complete_evidence']},'strict_phrase_set':read(root/'strict-phrase-set.json'),'source_presence_v2':{'baseline':previous['reviewed_span_coverage'],'candidate':current['reviewed_span_coverage'],'metric_contract':current['comparison_contract']},'latency_seconds':{'baseline':latency(rows(baseline/'scores/prompt_rows.jsonl')),'candidate':latency(prompts),'interpretation':'Single exposed fixed-cohort capture, uncontrolled host; descriptive difference, not causal latency estimate or stable-tail guarantee.'},'at_completion_token_cap':sum(r['usage']['completion_tokens']==256 for r in native.values()),'without_boxed_answer':sum('\\boxed{' not in r['response_raw'] for r in native.values()),'baseline_answer_boundary':registration['baseline_boundary'],'cases':{}}
for r in prompts:
 id=r['question_id'];n=native[id];summary['cases'][id]={'native_score':n['score'],'strict_independent_verdict':diag['cases'][id]['reader_verdict'],'complete_span_before':previous['cases'][id]['present'],'complete_span_current':current['cases'][id]['present'],'representations':current['cases'][id]['matching_representations'],'query_seconds':r['memory_query_duration_seconds'],'context_tokens':r['memory_context_token_count'],'context_truncated':r['memory_context_was_truncated'],'completion_tokens':n['usage']['completion_tokens'],'has_boxed_answer':'\\boxed{' in n['response_raw'],'parsed_answer':n['response_parsed_boxed']}
alt=read(root.parent/'adjacent-groups-final/registration.json')['additional_exposed_directory_control']
directory_prompt=next(p for p in prompts if p['question_id']=='6d975a08')
summary['additional_exposed_directory_control']={'complete_block_in_one_actual_reader_item':any(alt['complete_serialized_block'] in item.get('value','') for item in directory_prompt['memory_context']),'source_binding_in_reader_item':'unobserved: original adapter sends text only; source identity must be verified in full-store native receipt','original_reviewed_span_metric_unchanged':True}
summary['artifact_sha256']={str(p.relative_to(repo)):digest(p) for p in [root/'registration.json',root/'analysis-registration.json',root/'final-integrity.json',root/'scores/prompt_rows.jsonl',root/'scores/reader_outputs.jsonl',root/'scores/per_question.jsonl']}
(root/'final-analysis.json').write_text(json.dumps(summary,indent=2,ensure_ascii=False)+'\n')
print(json.dumps({k:summary[k] for k in ['strict_independent_answers','source_presence_v2','latency_seconds','at_completion_token_cap','without_boxed_answer']},indent=2))
