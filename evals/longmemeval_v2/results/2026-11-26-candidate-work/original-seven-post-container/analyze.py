from pathlib import Path
import hashlib,json,statistics,subprocess,sys,time
repo=Path.cwd();root=repo/'evals/longmemeval_v2/results/2026-11-26-candidate-work/original-seven-post-container'
baseline=repo/'evals/longmemeval_v2/results/2026-11-26-current-master-repeat/run-01'
def read(p):return json.loads(p.read_text())
def rows(p):return [json.loads(s) for s in p.read_text().splitlines() if s]
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
assert all(read(root/'final-integrity.json').values())
registration=read(root/'analysis-registration.json');changes={}
for p,h in registration['input_sha256'].items():
 actual=digest(repo/p)
 if actual!=h:
  assert p in ['evals/longmemeval_v2/reviewed_span_coverage.py','evals/longmemeval_v2/long_history_diagnosis.py']
  changes[p]={'registered_before':h,'corrected_after':actual}
assert set(changes)=={'evals/longmemeval_v2/reviewed_span_coverage.py','evals/longmemeval_v2/long_history_diagnosis.py'}
(root/'metric-correction.json').write_text(json.dumps({'schema':'source-presence-measurement-correction/v1','source_changes':changes,'cause':'Legacy partial newline/tab decoder does not recognize the complete JSON-serialized source containing protected literal backslashes or escaped quotes.','version':'reviewed-span-coverage/v2; exact complete source forms within one item; not decoded readability or answer quality','original_v1_result_retained':True,'new_accepted_answers':False,'new_model_calls':False,'negative_controls':['truncation','split between items','changed literal escape','changed quoted label','extra recursive escaping'],'red_log':'verification/selection/serialized-span-cli-red.log','green_log':'verification/selection/serialized-span-cli-boundaries.log','same_corrected_metric_applied_to_baseline_and_candidate':True},indent=2)+'\n')
native={r['question_id']:r for r in rows(root/'scores/per_question.jsonl')};readers=rows(root/'scores/reader_outputs.jsonl');prompts=rows(root/'scores/prompt_rows.jsonl')
assert len(native)==len(readers)==len(prompts)==7
assert {r['question_id'] for r in readers}==set(native)
for r in readers:
 assert r['status']=='captured' and r['response_raw']==native[r['question_id']]['response_raw']
 r['response_parsed_boxed']=native[r['question_id']]['response_parsed_boxed']
(root/'diagnostic-readers.jsonl').write_text(''.join(json.dumps(r,ensure_ascii=False)+'\n' for r in readers))
python=repo/'target/c4-native/env/bin/python';common=['--cohort',repo/'evals/longmemeval_v2/small-web-text-cohort.json','--trajectories',repo/'target/c4-native/cohort/trajectories.json']
commands=[('current-span-v2',[python,'-m','evals.longmemeval_v2.reviewed_span_coverage',*common,'--per-question',root/'scores/prompt_rows.jsonl','--output',root/'source-presence-v2.json']),('baseline-span-v2',[python,'-m','evals.longmemeval_v2.reviewed_span_coverage',*common,'--per-question',baseline/'prompt_rows.jsonl','--output',root/'baseline-source-presence-v2.json']),('strict-phrase-set',[python,'-m','evals.longmemeval_v2.exact_answer_quality','--per-question',root/'scores/per_question.jsonl','--output',root/'strict-phrase-set.json']),('independent-answer-diagnosis',[python,'-m','evals.longmemeval_v2.long_history_diagnosis',*common,'--truth',repo/'evals/longmemeval_v2/results/2026-11-26-long-history-diagnosis/reviewed-truth.json','--prompts',root/'scores/prompt_rows.jsonl','--readers',root/'diagnostic-readers.jsonl','--output',root/'independent-diagnosis.json'])]
log=[]
for name,command in commands:
 start=time.monotonic()
 with (root/(name+'.log')).open('w') as output:r=subprocess.run([str(x) for x in command],stdout=output,stderr=subprocess.STDOUT,timeout=60)
 log.append({'name':name,'command':[str(x) for x in command],'exit_code':r.returncode,'elapsed_seconds':time.monotonic()-start})
 (root/'offline-commands.json').write_text(json.dumps(log,indent=2)+'\n');assert r.returncode==0
current=read(root/'source-presence-v2.json');previous=read(root/'baseline-source-presence-v2.json');diag=read(root/'independent-diagnosis.json');cover=read(root/'scores/score-coverage.json')
def latency(ps):
 values=[p['memory_query_duration_seconds'] for p in ps];return {'count':len(values),'mean':statistics.mean(values),'median':statistics.median(values),'max':max(values),'includes':'private store copy, startup and CLI read; concurrent three-worker original profile'}
summary={'schema':'original-seven-final-analysis/v1','native_scorer':cover,'strict_independent_answers':{'correct':diag['correct'],'selected':diag['selected'],'complete_evidence_reader_failures':diag['reader_failures_with_complete_evidence']},'strict_phrase_set':read(root/'strict-phrase-set.json'),'source_presence_v2':{'baseline':previous['reviewed_span_coverage'],'candidate':current['reviewed_span_coverage'],'v1_candidate_retained':read(root/'reviewed-span-coverage.json')['reviewed_span_coverage']},'latency_seconds':{'baseline':latency(rows(baseline/'prompt_rows.jsonl')),'candidate':latency(prompts),'interpretation':'Single exposed fixed-cohort capture, uncontrolled host; descriptive difference, not causal latency estimate or stable-tail guarantee.'},'at_completion_token_cap':sum(r['usage']['completion_tokens']==256 for r in native.values()),'without_boxed_answer':sum('\\boxed{' not in r['response_raw'] for r in native.values()),'baseline_answer_boundary':registration['baseline_boundary'],'cases':{}}
for r in prompts:
 id=r['question_id'];n=native[id];summary['cases'][id]={'native_score':n['score'],'strict_independent_verdict':diag['cases'][id]['reader_verdict'],'complete_span_before':previous['cases'][id]['present'],'complete_span_current':current['cases'][id]['present'],'representations':current['cases'][id]['matching_representations'],'query_seconds':r['memory_query_duration_seconds'],'context_tokens':r['memory_context_token_count'],'context_truncated':r['memory_context_was_truncated'],'completion_tokens':n['usage']['completion_tokens'],'has_boxed_answer':'\\boxed{' in n['response_raw'],'parsed_answer':n['response_parsed_boxed']}
summary['artifact_sha256']={str(p.relative_to(repo)):digest(p) for p in [root/'registration.json',root/'analysis-registration.json',root/'final-integrity.json',root/'scores/prompt_rows.jsonl',root/'scores/reader_outputs.jsonl',root/'scores/per_question.jsonl',root/'metric-correction.json']}
(root/'final-analysis.json').write_text(json.dumps(summary,indent=2,ensure_ascii=False)+'\n')
print(json.dumps({k:summary[k] for k in ['strict_independent_answers','source_presence_v2','latency_seconds','at_completion_token_cap','without_boxed_answer']},indent=2))
