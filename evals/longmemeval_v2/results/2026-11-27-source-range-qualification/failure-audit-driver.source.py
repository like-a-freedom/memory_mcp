from pathlib import Path
import json
from evals.longmemeval_v2.reader_protocol import read_rows,project_sources
from evals.longmemeval_v2.source_references import source_catalog
out=Path('evals/longmemeval_v2/results/2026-11-27-source-range-qualification')
rows=read_rows(out/'rows.jsonl');truth=json.loads((out/'truth.json').read_text())['cases'];scores=json.loads((out/'report.json').read_text())['variants']['source-range-final-first']['cases'];audit=[]
for row in rows:
 key=row['question_id'];score=scores[key];missing=[]
 catalog=source_catalog(project_sources(row['memory_context']))
 for alternative in truth[key]['evidence_alternatives']:
  absent=[]
  for required in alternative:
   source=next(s for s in catalog.values()if s['source_item_index']==required['source_item_index'])
   for number,line in enumerate(source['lines'],1):
    first=max(line['raw_range'][0],required['raw_range'][0]);last=min(line['raw_range'][1],required['raw_range'][1])
    if first>=last:continue
    cursor=first
    for a,b in sorted(ref['raw_range']for ref in score['resolved_evidence']if ref['source_item_index']==required['source_item_index']and ref['raw_sha256']==required['raw_sha256']):
     if a>cursor:break
     cursor=max(cursor,b)
    if cursor<last:absent.append({'source_item_index':required['source_item_index'],'line':number,'text':line['quote'].rstrip(),'missing_raw_range':[cursor,last]})
  missing.append(absent)
 item={'question_id':key,'question':row['question_text'],'accepted_answers':truth[key]['accepted_answers'],'answer':score['answer'],'value_correct':score['value_correct'],'format_valid':score['format_valid'],'source_bound':score['source_bound'],'proof_covered':score['proof_covered'],'strict_joint':score['completed_correct'],'missing_lines_by_alternative':missing,'resolved_evidence':score['resolved_evidence']}
 audit.append(item)
 if not score['completed_correct']:
  print(key,'value',score['value_correct'],'binding',score['source_bound'],'proof',score['proof_covered'],'missing',[(m['line'],m['text'])for m in missing[0]])
with (out/'source-failure-audit-v1.json').open('x')as f:json.dump({'schema':'source-range-failure-audit/v1','policy':'Mechanical missing spans against the unchanged preregistered oracle, not a revised gold or general entailment judge. All matching occurrences require scope closure; no support labels changed after outputs.','cases':audit},f,indent=2);f.write('\n')
