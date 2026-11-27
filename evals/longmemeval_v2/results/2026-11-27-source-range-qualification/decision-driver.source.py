from pathlib import Path
from datetime import datetime,timezone
import hashlib,json,math,statistics
from evals.longmemeval_v2.reader_protocol import read_rows,summarize
root=Path('/Users/solovey/Documents/dev/memory_mcp');out=root/'evals/longmemeval_v2/results/2026-11-27-source-range-qualification'
def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def save(name,data):
 with (out/name).open('x')as f:json.dump(data,f,indent=2);f.write('\n')
plan=json.loads((out/'registration.json').read_text());accept=json.loads((out/'acceptance.json').read_text());assert sha(out/'registration.json')==accept['registration_sha256']
rows=read_rows(Path(plan['rows_path']));truth=json.loads(Path(plan['truth_path']).read_text())['cases'];outputs=json.loads((out/'outputs.json').read_text());report=json.loads((out/'report.json').read_text())
replayed={v:summarize(rows,truth,outputs[v],v)for v in plan['variants']};assert replayed==report['variants']
base=replayed['source-graph-final-first'];new=replayed['source-range-final-first']
regressions=[k for k in plan['cohorts']['previous24']if base['cases'][k]['value_correct']and not new['cases'][k]['value_correct']]
attempts=[json.loads(line)for line in(out/'inference-attempts.jsonl').read_text().splitlines()]
events=[json.loads(line)for line in(out/'inference-events.jsonl').read_text().splitlines()]
assert len({a['request_id']for a in attempts})==len(attempts)
assert all(a['attempt']==1 and a['max_retries']==0 for a in attempts)
assert all(sum(e['request_id']==a['request_id']and e['event']=='attempt_started'for e in events)==1 for a in attempts)
schedule={r['question_id']+':'+v for r in rows for v in plan['variants']}
assert len(schedule)==80 and {a['question_id']for a in attempts}<=schedule
cohorts={}
for name,ids in plan['cohorts'].items():
 selected=[r for r in rows if r['question_id']in ids]
 cohorts[name]={v:{k:value for k,value in summarize(selected,{k:truth[k]for k in ids},{k:value for k,value in outputs[v].items()if k in ids},v).items()if k!='cases'}for v in plan['variants']}
for name,ids in plan['cohorts'].items():
 cohorts[name]['source-graph-final-first']['literal_quote_joint'] = sum(base['cases'][key]['completed_correct'] and base['cases'][key]['format_valid'] and base['cases'][key]['quote_exact'] is True for key in ids)
latency={}
for variant in plan['variants']:
 observed=[a for a in attempts if a['question_id'].endswith(':'+variant)]
 seconds=sorted(a['elapsed_seconds']for a in observed)
 latency[variant]={'observed_calls':len(observed),'median_seconds':statistics.median(seconds)if seconds else None,'p95_seconds':seconds[max(0,math.ceil(.95*len(seconds))-1)]if seconds else None,'max_seconds':max(seconds,default=None),'completion_tokens':sum((a.get('usage')or{}).get('completion_tokens',0)for a in observed),'faults':sum(a['outcome']!='success'for a in observed)}
passed=(new['completed_correct']==40 and new['source_bound']==40 and new['proof_covered']==40 and new['errors']==0 and not regressions and not any(c['truncated']for c in new['cases'].values()))
verdict={'schema':'source-range-qualification-decision/v1','created_at_utc':datetime.now(timezone.utc).isoformat(),'decision':'PASS'if passed else'FAIL','scheduled':80,'recorded_outputs':sum(len(v)for v in outputs.values()),'dispatched_attempts':len(attempts),'shared24_value_regressions':regressions,'cohorts':cohorts,'metric_semantics':{'source-graph-final-first.completed_correct':'legacy correct value and normal completion, without mandatory literal quotation','source-graph-final-first.literal_quote_joint':'legacy value, valid boxed format, actual single-item literal QUOTE and normal completion','source-range-final-first.completed_correct':'new strict value, JSON, source identity, reviewed support coverage and normal completion; not the old literal-QUOTE contract'},'latency_descriptive_only':latency,'report_sha256':sha(out/'report.json'),'acceptance_sha256':sha(out/'acceptance.json'),'registration_sha256':sha(out/'registration.json'),'frozen_report_replay_equal':True,'not_claimed':['independent memory benefit','isolated latency gain','equal output contracts for source-range and literal QUOTE','autonomous workflow qualification']}
save('candidate-decision-v1.json',verdict)
save('full-history-activation.json',{'schema':'source-range-conditional-full-history/v1','activated':passed,'reason':'strict qualification passed; full-history capture pending'if passed else'strict all40 qualification failed; seven-case inference not activated','conditional_cases':7,'actual_calls_so_far':0})
print(json.dumps({'decision':verdict['decision'],'candidate_strict':new['completed_correct'],'candidate_value':new['value_correct'],'source_bound':new['source_bound'],'proof_covered':new['proof_covered'],'cohorts':cohorts,'latency':latency},indent=2))
