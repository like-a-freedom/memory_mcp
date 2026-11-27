"""One frozen source-selection candidate replay; reuse tested V5 capture primitives."""
from pathlib import Path
import hashlib,importlib.util,json,os,platform,time
ROOT=Path('/Users/solovey/Documents/dev/memory_mcp')
BASE=ROOT/'evals/longmemeval_v2/results/2026-11-27-full-history-access-isolation'
OUT=ROOT/'evals/longmemeval_v2/results/2026-11-27-full-history-positional-v7'
RAW=Path('/tmp/memory-quality-full-100-2026-11-27/observations-positional-v7')
HELPER=BASE/'capture_driver.source.py'
spec=importlib.util.spec_from_file_location('frozen_v5_driver',HELPER);h=importlib.util.module_from_spec(spec);spec.loader.exec_module(h)
BIN=Path('/tmp/memory-quality-history-v7-2026-11-27')
def run():
 registration=h.load(OUT/'registration.json')
 for path,digest in registration['frozen_sha256'].items():
  if h.sha(Path(path))!=digest:raise RuntimeError('frozen file differs: '+path)
 if platform.machine()!='arm64' or h.hashes(h.SOURCE)!=registration['source_snapshot_file_sha256'] or h.hashes(h.UPGRADED)!=registration['candidate_snapshot_file_sha256']:
  raise RuntimeError('architecture or immutable initial store changed')
 reads=h.load(h.READ_INPUT)
 if tuple(reads['cases'])!=h.CASES or registration['limits']!=h.LIMITS:raise RuntimeError('registered limits changed')
 RAW.mkdir(exist_ok=False); (OUT/'candidate').mkdir()
 plans=[dict(ordinal=i,case_id=case,arm='candidate',command=h.argv(BIN,h.READ_TEST)) for i,case in enumerate(h.CASES)]
 h.save(OUT/'schedule.json',{'scheduled':7,'attempts':plans})
 rows=[]; reader=[]
 with (OUT/'attempts.jsonl').open('x') as journal:
  for plan in plans:
   case=plan['case_id']; registered=reads['cases'][case];raw=RAW/(case+'.log'); result=dict(plan,raw_log=str(raw))
   h.record(journal,'start',**result)
   try:
    meta,events=h.invoke_clone(BIN,h.UPGRADED,h.READ_TEST,h.READ_INPUT,raw,h.LIMITS['child_seconds'],case)
    result.update(meta);result.update(h.classify_read(meta,events,case,registered['required_serialized_span']),events=h.compact_events(events,registered['required_serialized_span']))
   except Exception as error:
    result.update(runtime_completed=False,delivered_items=None,retrieval_fault='capture_fault',capture_fault=str(error))
    if raw.is_file():result.update(raw_sha256=h.sha(raw),raw_bytes=raw.stat().st_size)
   h.save(OUT/'candidate'/f'{case}.json',result)
   summary={k:v for k,v in result.items() if k not in ('delivered_items','events')}; rows.append(summary); h.record(journal,'terminal',**summary)
   items=result['delivered_items'] or []
   reader.append(dict(question_id=case,question_text=registered['query'],prompt_messages=[],retrieval_fault=result['retrieval_fault'],memory_context=[{'type':'text','value':item['content']} for item in items]))
   print(case,result.get('exit_code'),result.get('assembly_elapsed_ms'),result.get('elapsed_seconds'),'items',len(items),flush=True)
 with (OUT/'reader-rows.jsonl').open('x') as f:
  for row in reader:f.write(json.dumps(row,ensure_ascii=True,allow_nan=False)+'\n')
  f.flush();os.fsync(f.fileno())
 source_ok=h.hashes(h.SOURCE)==registration['source_snapshot_file_sha256'];candidate_ok=h.hashes(h.UPGRADED)==registration['candidate_snapshot_file_sha256']
 h.save(OUT/'reads-summary.json',dict(scheduled=7,rows=rows,source_snapshot_unchanged=source_ok,candidate_initial_snapshot_unchanged=candidate_ok))
 if not source_ok or not candidate_ok:raise RuntimeError('initial snapshot changed')
if __name__=='__main__':run()
