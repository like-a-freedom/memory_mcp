"""Verify delivered source, scoring and legacy prompt parity without inference."""
import json
from pathlib import Path
import os
import sys
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT))
os.chdir(ROOT)
from evals.longmemeval_v2.reader_protocol import protocol_messages, classify
from evals.longmemeval_v2.source_references import ITEM_VARIANT
root=Path('evals/longmemeval_v2/results/2026-11-27-source-item-qualification')
result={'cohorts':{},'legacy_prompts_equal':0,'old_output_classifications_equal':0,'inference_calls':0}
for cohort in ['shared40','fresh16']:
 p=root/cohort
 rows=[json.loads(s) for s in (p/'rows.jsonl').read_text().splitlines()]
 prompts=json.loads((p/'prompts.json').read_text()); old_bytes=new_bytes=0
 truths=json.loads((p/'truth.json').read_text())['cases']; outputs=json.loads((p/'outputs.json').read_text()); report=json.loads((p/'report.json').read_text())['variants']
 for row in rows:
  q=row['question_id']; old=prompts[q]['source-item-final-first']; new=protocol_messages(row,ITEM_VARIANT)
  assert old[-1]==new[-1],q+' source bytes changed'
  old_bytes+=sum(len(m['content'].encode()) for m in old); new_bytes+=sum(len(m['content'].encode()) for m in new)
  for variant,messages in prompts[q].items():
   if variant!='source-item-final-first':
    assert protocol_messages(row,variant)==messages,q+' legacy prompt changed'
    result['legacy_prompts_equal']+=1
  current=classify(row,truths[q],outputs['source-item-final-first'][q],ITEM_VARIANT)
  assert current==report['source-item-final-first']['cases'][q],q+' grading changed'
  result['old_output_classifications_equal']+=1
 result['cohorts'][cohort]={'cases':len(rows),'old_content_utf8_bytes':old_bytes,'new_content_utf8_bytes':new_bytes,'reduction_fraction':1-new_bytes/old_bytes}
p=Path('evals/longmemeval_v2/results/2026-11-27-source-range-qualification')
prompts=json.loads((p/'prompts.json').read_text())
for row in (json.loads(s) for s in (p/'rows.jsonl').read_text().splitlines()):
 for variant,messages in prompts[row['question_id']].items():
  assert protocol_messages(row,variant)==messages
  result['legacy_prompts_equal']+=1
expected=json.loads(Path(__file__).with_name('verification.json').read_text())['comparison']
assert result==expected, 'frozen contract comparison changed'
print(json.dumps(result,indent=2))
