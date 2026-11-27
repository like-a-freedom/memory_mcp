from collections import Counter
from pathlib import Path
from datetime import datetime,timezone
import hashlib,json,statistics
r=Path('/Users/solovey/Documents/dev/memory_mcp/evals/paired/skill-completion-qualification-2026-11-27')
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
acceptance=json.loads((r/'acceptance.json').read_text());cohorts={};allrows=[];allattempts=[];integrity={}
for registered in acceptance['plans']:
 name=registered['directory'];folder=r/name;plan=folder/'frozen-plan.json';assert digest(plan)==registered['sha256']
 p=json.loads(plan.read_text());review=json.loads((folder/'semantic-review-v1.json').read_text());rows=review['rows'];assert len(rows)==registered['scheduled'];assert review['frozen_plan_sha256']==digest(plan)
 ids={f"{c['skill']}-{c['id']}-{arm}" for c in p['cases'] for arm in p['arms']};assert ids=={x['identity']for x in rows};assert len(ids)==len(rows)
 trials={}
 for x in rows:
  path=folder/'capture'/x['identity']/'trial.json';assert digest(path)==x['trial_sha256'];trials[x['identity']]=json.loads(path.read_text())
  assert x['registered_operation_pass']==trials[x['identity']]['grade']['operation_and_exact_arguments']
  assert x['terminal']==trials[x['identity']]['terminal']
  allrows.append({**x,'elapsed_seconds':trials[x['identity']]['elapsed_seconds']})
 attempts=[json.loads(line)for line in(folder/'capture/inference-attempts.jsonl').read_text().splitlines()]
 events=[json.loads(line)for line in(folder/'capture/inference-events.jsonl').read_text().splitlines()]
 starts=[e['request_id']for e in events if e['event']=='attempt_started'];ends=[e['request_id']for e in events if e['event']=='attempt_finished'];assert Counter(starts)==Counter(ends)==Counter(a['request_id']for a in attempts);assert len(starts)==len(set(starts));assert all(a['attempt']==1 for a in attempts)
 integrity[name]={'scheduled_and_recorded':len(rows),'request_starts':len(starts),'request_terminals':len(ends),'abandoned_requests':0,'duplicate_request_ids':0,'plan_sha256':digest(plan),'review_sha256':digest(folder/'semantic-review-v1.json')}
 cohorts[name]={'scheduled':len(rows),'terminal':dict(Counter(x['terminal']for x in rows)),'strict_full_pass':sum(x['full_pass']for x in rows),'operation_pass':sum(x['registered_operation_pass']for x in rows),'semantic_answer_pass':sum(x['semantic_clauses']is True for x in rows),'arms':{arm:{'scheduled':sum(x['arm']==arm for x in rows),'strict_full_pass':sum(x['arm']==arm and x['full_pass']for x in rows),'answered':sum(x['arm']==arm and x['terminal']=='answered'for x in rows)}for arm in p['arms']},'runtime_seconds':{'median':statistics.median(t['elapsed_seconds']for t in trials.values()),'max':max(t['elapsed_seconds']for t in trials.values())},'inference_attempt_outcomes':dict(Counter(a['outcome']for a in attempts))}
 allattempts+=attempts
current=[x for x in allrows if x['arm']=='current'];assert len(current)==acceptance['current_scheduled']==22
out={'schema':'memory-skill-completion-decision/v1','at_utc':datetime.now(timezone.utc).isoformat(),'decision':'FAIL','criterion':acceptance['require'],'acceptance_sha256':digest(r/'acceptance.json'),'scheduled':len(allrows),'terminal':dict(Counter(x['terminal']for x in allrows)),'strict_full_pass':sum(x['full_pass']for x in allrows),'operation_pass':sum(x['registered_operation_pass']for x in allrows),'current':{'scheduled':len(current),'strict_full_pass':sum(x['full_pass']for x in current)},'previous':{'scheduled':len(allrows)-len(current),'strict_full_pass':sum(x['full_pass']for x in allrows if x['arm']=='previous')},'cohorts':cohorts,'request_outcomes':dict(Counter(a['outcome']for a in allattempts)),'invalid_model_alias_attempt':{'excluded_from_valid_registration':True,'retained_directory':'invalid-model-alias-attempt','http_404_requests':9,'scope':'Infrastructure correction only; all valid44 outcomes retained without retries.'},'source_identity':{'captured_runner_sha256':digest(r/'runner.source.py'),'captured_mcp_binding_sha256':digest(r/'mcp-binding.source.py'),'note':'Captured sources archived before post-capture deterministic recovery fix. Old scores are not reinterpreted with changed runtime; any future inference must freeze new source identity.'},'limits':['Development qualification, not unseen confirmation or causal instruction effect.','Primary non-blinded source-based semantic review, not independent adjudication.','Historical32 and fresh12 have different cases; never pool as a population estimate.','Only native Apple Silicon local offline MLX; no LM Studio service or remote inference.','Expected controlled reembed interruption is a tested handled outcome, not an inference timeout.','No full-history reader activation, no production default change, no C2 pilot or power calculation.'],'capture_seconds':json.loads((r/'capture-completed.json').read_text())['elapsed_seconds']}
assert out['strict_full_pass']==20 and out['current']['strict_full_pass']==10
for name,data in [('candidate-decision-v1.json',out),('capture-integrity-v1.json',{'schema':'bounded-workflow-integrity/v1','cohorts':integrity,'old_scores_unchanged':True})]:
 with(r/name).open('x')as f:json.dump(data,f,ensure_ascii=False,indent=2);f.write('\n')
print(json.dumps(out,ensure_ascii=False,indent=2))
