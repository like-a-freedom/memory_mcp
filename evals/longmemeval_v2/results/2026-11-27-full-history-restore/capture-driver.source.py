"""Registered operations driver; raw stage bodies remain outside the repository."""
from pathlib import Path
import hashlib,json,os,shutil,subprocess,sys,tempfile,time
from evals.longmemeval_v2.snapshot_manifest import verify_snapshot
ROOT=Path('/Users/solovey/Documents/dev/memory_mcp')
OUT=ROOT/'evals/longmemeval_v2/results/2026-11-27-full-history-restore'
RAW=Path('/tmp/memory-quality-full-100-2026-11-27/observations')
SNAPSHOT=Path('/tmp/memory-quality-full-100-2026-11-27/build/memory_state/store')
BINARY=Path('/tmp/memory-quality-history-registered-2026-11-27')
def sha(path):
 with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def save(path,obj):
 with path.open('x') as f:json.dump(obj,f,ensure_ascii=False,indent=2);f.write('\n')
def hashes():return {str(p.relative_to(SNAPSHOT)):sha(p) for p in sorted(SNAPSHOT.rglob('*')) if p.is_file()}
def invoke(test,registration,raw,seconds,case=None):
 with tempfile.TemporaryDirectory(prefix='memory-quality-restored-read-') as tmp:
  start=time.monotonic();store=Path(tmp)/'store';shutil.copytree(SNAPSHOT,store);copy_seconds=time.monotonic()-start
  env={k:os.environ[k] for k in ('PATH','TMPDIR','SYSTEMROOT','WINDIR','TEMP','TMP') if k in os.environ}
  env.update(MEMORY_FULL_HISTORY_STORE=str(store),MEMORY_FULL_HISTORY_INPUT=str(registration),RUST_LOG='error')
  if case:env['MEMORY_FULL_HISTORY_CASE']=case
  start=time.monotonic();timeout=False
  with raw.open('xb') as f:
   child=subprocess.Popen([str(BINARY),test,'--exact','--ignored','--nocapture'],env=env,stdout=f,stderr=subprocess.STDOUT)
   try:code=child.wait(timeout=seconds)
   except subprocess.TimeoutExpired:timeout=True;child.kill();code=child.wait(timeout=10)
  elapsed=time.monotonic()-start
 events=[]
 for line in raw.read_text().splitlines():
  if line.startswith('FULL_HISTORY_'):
   try:
    name,body=line.split(' ',1);events.append((name,json.loads(body)))
   except ValueError:pass
 return {'exit_code':code,'external_timeout':timeout,'elapsed_seconds':elapsed,'copy_seconds':copy_seconds,'raw_sha256':sha(raw),'raw_bytes':raw.stat().st_size},events

def audit():
 RAW.mkdir(exist_ok=False)
 identity={'driver_sha256':sha(Path(__file__)),'observer_sha256':sha(BINARY),'profile':'test opt-level=3; native Apple Silicon','snapshot_file_sha256':hashes(),'build_registration_sha256':sha(OUT/'registration.json'),'read_registration_sha256':sha(OUT/'reads-registration.json')}
 save(OUT/'capture-identity.json',identity)
 expected=json.loads((OUT/'registration.json').read_text())
 inventories=[]
 for ordinal in (1,2):
  meta,events=invoke('full_history_formation_inventory',OUT/'registration.json',RAW/f'audit-{ordinal}.log',300)
  bodies=[b for e,b in events if e=='FULL_HISTORY_FORMATION']
  result={'execution':meta,'content_verified':False}
  if meta['exit_code']==0 and len(bodies)==1:
   result['verification']=verify_snapshot(expected,bodies[0]);result['content_verified']=True
   inventories.append(bodies[0]);save(OUT/f'readback-{ordinal}.json',bodies[0])
  save(OUT/f'audit-{ordinal}.json',result)
  print('audit',ordinal,result['content_verified'],round(meta['elapsed_seconds'],3),flush=True)
  if not result['content_verified']:raise RuntimeError('full source readback failed; no reads admitted')
 if inventories[0]!=inventories[1]:raise RuntimeError('readback differs across reopened isolated clones')
 if hashes()!=identity['snapshot_file_sha256']:raise RuntimeError('registered source snapshot changed')
 save(OUT/'snapshot-admission.json',{'complete_source_restored':True,'episodes':len(inventories[0]['episodes']),'reopen_equal':True,'source_snapshot_unchanged':True,'extraction_status_observation':'not retained by running archived backend; do not infer from body hashes'})

def reads():
 identity=json.loads((OUT/'capture-identity.json').read_text())
 if sha(BINARY)!=identity['observer_sha256'] or hashes()!=identity['snapshot_file_sha256']:raise RuntimeError('observer or store changed')
 if not json.loads((OUT/'snapshot-admission.json').read_text())['complete_source_restored']:raise RuntimeError('incomplete source')
 reg=json.loads((OUT/'reads-registration.json').read_text());folder=OUT/'reads';folder.mkdir()
 reader_rows=[];summary=[]
 for case,registered in reg['cases'].items():
  raw=RAW/f'{case}.log';meta,events=invoke('full_history_registered_case_retains_reviewed_source',OUT/'reads-registration.json',raw,reg['limits']['child_seconds'],case)
  compact=[];items=None
  for name,body in events:
   if name=='FULL_HISTORY_QUERY':body={'elapsed_ms':body['elapsed_ms'],'duration_us':body['duration_us'],'sql_sha256':hashlib.sha256(body['sql'].encode()).hexdigest()}
   if name in ('FULL_HISTORY_RECEIPT','FULL_HISTORY_PARTIAL_RECEIPT'):
    receipt=body.get('receipt',body)
    if name=='FULL_HISTORY_RECEIPT':items=receipt['items']
    for stage in receipt['stages']:
     stage['items']=[{'fact_id':i.get('fact_id'),'source_episode':i.get('source_episode'),'content_bytes':len(i['content'].encode()),'content_sha256':hashlib.sha256(i['content'].encode()).hexdigest(),'registered_span_present':registered['required_serialized_span'] in i['content']} for i in stage['items']]
   compact.append({'event':name,'data':body})
  row={'case_id':case,**meta,'process_deadline_exceeded':meta['elapsed_seconds']>reg['limits']['child_seconds'],'delivered_items':items,'events':compact}
  save(folder/f'{case}.json',row)
  summary.append({k:v for k,v in row.items() if k not in ('delivered_items','events')})
  reader_rows.append({'question_id':case,'question_text':registered['query'],'prompt_messages':[], 'retrieval_fault':None if items is not None else row['exit_code'],'memory_context':[{'type':'text','value':i['content']} for i in items or []]})
  print(case,meta['exit_code'],round(meta['elapsed_seconds'],3),'items',len(items or []),flush=True)
 with (OUT/'reader-rows.jsonl').open('x') as f:
  for row in reader_rows:f.write(json.dumps(row,ensure_ascii=False)+'\n')
 save(OUT/'reads-summary.json',{'scheduled':len(reg['cases']),'rows':summary,'source_snapshot_unchanged':hashes()==identity['snapshot_file_sha256']})
if __name__=='__main__':{'audit':audit,'reads':reads}[sys.argv[1]]()
