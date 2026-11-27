from pathlib import Path
import json,hashlib
ROOT=Path('/Users/solovey/Documents/dev/memory_mcp');OUT=ROOT/'evals/longmemeval_v2/results/2026-11-27-source-graph-qualification'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
r=json.loads((OUT/'report.json').read_text());plan=json.loads((OUT/'registration.json').read_text());accept=json.loads((OUT/'acceptance.json').read_text())
assert sha(OUT/'registration.json')==accept['registration_sha256']
attempts=[json.loads(line) for line in (OUT/'inference-attempts.jsonl').read_text().splitlines() if line]
events=[json.loads(line) for line in (OUT/'inference-events.jsonl').read_text().splitlines() if line]
started=[event for event in events if event.get('event')=='attempt_started']
rows=[]
for variant,data in r['variants'].items():
 cases=data['cases'];full=[key for key,value in cases.items() if value['completed_correct'] and value['format_valid'] and value['quote_exact'] is True and not value['error']]
 cohorts={}
 for cohort,ids in plan['cohorts'].items():
  cs=[cases[key] for key in ids];cohorts[cohort]=dict(selected=len(cs),completed_value=sum(c['completed_correct'] for c in cs),completed_format_literal_quote=sum(key in full for key in ids),value=sum(c['value_correct'] for c in cs),format=sum(c['format_valid'] for c in cs),literal_quote=sum(c['quote_exact'] is True for c in cs),truncated=sum(c['truncated'] for c in cs),errors=sum(bool(c['error']) for c in cs))
 rows.append(dict(variant=variant,selected=len(cases),completed_format_literal_quote=len(full),failed_cases={key:value for key,value in cases.items() if key not in full},cohorts=cohorts,unchanged_report_counts={k:v for k,v in data.items() if k!='cases'}))
candidate=next(row for row in rows if row['variant']==accept['candidate']);integrity_ok=(len(started)==accept['scheduled'] and len(attempts)==accept['scheduled'] and len({event['request_id'] for event in started})==len(started))
passed=candidate['completed_format_literal_quote']==accept['all_cases_required'] and integrity_ok
result=dict(schema='source-graph-qualification-decision/v1',decision='PASS' if passed else 'FAIL',qualified_for_frozen24case_criterion=passed,scheduled_calls=accept['scheduled'],scheduled_case_families=accept['case_families'],actual_started_requests=len(started),finished_attempts=len(attempts),response_bodies=sum(data['selected']-data['unobserved'] for data in r['variants'].values()),request_ids_unique=len({event['request_id'] for event in started})==len(started),variants=rows,source_binding_and_entailment='Literal quote presence only; this gate does not establish full source binding, quote entailment, autonomous-agent qualification or memory-treatment benefit.',input_sha256={p.name:sha(p) for p in [OUT/'registration.json',OUT/'acceptance.json',OUT/'prompts.json',OUT/'outputs.json',OUT/'report.json',OUT/'rows.jsonl',OUT/'truth.json',OUT/'inference-attempts.jsonl',OUT/'inference-events.jsonl']})
with (OUT/'candidate-decision-v1.json').open('x') as f:json.dump(result,f,indent=2);f.write('\n')
print(json.dumps({k:v for k,v in result.items() if k not in ('variants','input_sha256')},indent=2))
for row in rows:print(row['variant'],row['completed_format_literal_quote'],row['cohorts'])
