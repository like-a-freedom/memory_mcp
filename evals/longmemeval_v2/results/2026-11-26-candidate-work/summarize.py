from pathlib import Path
import json
root=Path('evals/longmemeval_v2/results/2026-11-26-candidate-work');decoder=json.JSONDecoder();rows=[]
for directory in sorted(root.iterdir()):
 if not directory.is_dir() or not (directory/'registration.json').exists():continue
 for case in json.loads((directory/'registration.json').read_text()).get('cases', {}):
  path=directory/(case+'.log')
  if not path.exists():continue
  row={'variant':directory.name,'case_id':case,'inventory':[],'work':[],'phases':[]}
  for line in path.read_text().splitlines():
   if 'FULL_HISTORY_START ' in line:
    row['registered_source_episode']=decoder.raw_decode(line.split('FULL_HISTORY_START ',1)[1])[0]['source_episode']
   for tag,key in [('FULL_HISTORY_CANDIDATES','inventory'),('FULL_HISTORY_WORK','work'),('FULL_HISTORY_PHASE','phases')]:
    if tag+' ' in line:
     try:row[key].append(decoder.raw_decode(line.split(tag+' ',1)[1])[0])
     except json.JSONDecodeError:pass
   for tag in ['FULL_HISTORY_RECEIPT','FULL_HISTORY_PARTIAL_RECEIPT']:
    if tag+' ' not in line:continue
    obj=decoder.raw_decode(line.split(tag+' ',1)[1])[0]
    row['read_elapsed_ms']=obj.get('elapsed_ms');row['read_completed']=tag=='FULL_HISTORY_RECEIPT'
    receipt=obj.get('receipt',{});stages=receipt.get('stages',obj.get('stages',[]));reg=json.loads((directory/'registration.json').read_text());span=reg['cases'][case]['required_serialized_span'];row['evidence_stages']=[{'stage':stage.get('stage'),'elapsed_ms':stage.get('elapsed_ms'),'items':len(stage.get('items',[])),'complete_reviewed_span':any(item.get('source_episode')==row.get('registered_source_episode') and span in item.get('content','') for item in stage.get('items',[]))} for stage in stages]
    # Exclude returned/raw source text from this compact metadata index.
    items=receipt.get('items',[])
    if items:
     reg=json.loads((directory/'registration.json').read_text());span=reg['cases'][case]['required_serialized_span'];row['complete_reviewed_span']=any(x.get('source_episode')==row.get('registered_source_episode') and span in x.get('content','') for x in items)
  process=directory/(case+'-process.json')
  if process.exists():row['process']=json.loads(process.read_text());row['strict_pass']=(row['process']['exit_code']==0 and not row['process']['external_timeout'] and row['process']['elapsed_seconds']<=json.loads((directory/'registration.json').read_text())['limits']['child_seconds'] and row.get('read_completed',False) and row.get('complete_reviewed_span',False) and row.get('read_elapsed_ms',float('inf'))<=1000*json.loads((directory/'registration.json').read_text())['limits']['read_seconds'])
  row['excluded_from_final_acceptance']=(directory/'invalid-source-drift.json').exists()
  if row['excluded_from_final_acceptance']:row['strict_pass']=False
  rows.append(row)
(root/'candidate-summary.json').write_text(json.dumps(rows,indent=2)+'\n')
for row in rows:
 if row['variant']=='borrowed-dictionaries-complete-groups':print(json.dumps({k:row[k] for k in ['variant','case_id','read_elapsed_ms','read_completed','complete_reviewed_span','strict_pass','evidence_stages'] if k in row},ensure_ascii=False))
