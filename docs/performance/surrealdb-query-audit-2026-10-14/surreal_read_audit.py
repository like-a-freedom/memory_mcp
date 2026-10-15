import subprocess,json,urllib.request,time,statistics
d=json.loads(subprocess.check_output(['docker','inspect','memory_mcp']))[0]
e=dict(x.split('=',1) for x in d['Config']['Env'] if '=' in x)
base='http://172.24.0.2:8000'
r=urllib.request.Request(base+'/signin',data=json.dumps({'user':e['SURREALDB_CONTROL_USERNAME'],'pass':e['SURREALDB_CONTROL_PASSWORD']}).encode(),headers={'Content-Type':'application/json','Accept':'application/json'})
token=json.loads(urllib.request.urlopen(r,timeout=10).read())['token']
h={'Accept':'application/json','Content-Type':'text/plain','Authorization':'Bearer '+token,'Surreal-NS':e['SURREALDB_CONTROL_NAMESPACE'],'Surreal-DB':e['SURREALDB_CONTROL_DB']}
def sql(q):
 return json.loads(urllib.request.urlopen(urllib.request.Request(base+'/sql',data=q.encode(),headers=h),timeout=10).read())
tenants=sql("SELECT namespace_binding FROM tenant WHERE status='ready' LIMIT 1")[0]['result']
print('utc',time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()))
h['Surreal-NS']=tenants[0]['namespace_binding']['namespace'];h['Surreal-DB']=tenants[0]['namespace_binding']['database']
if not h['Surreal-DB']: h['Surreal-DB']='memory'
for table in ['fact','episode','entity','edge','community','claim','claim_relation','task_artifact','tenant_task']:
 out=sql('INFO FOR TABLE '+table)[0]
 print('schema',table,out.get('status'),out.get('result',{}).get('indexes',{}))
print('inventory',sql('SELECT count() AS n FROM edge GROUP ALL;SELECT count() AS n FROM entity GROUP ALL;SELECT count() AS n FROM community GROUP ALL;SELECT count() AS n FROM claim GROUP ALL;SELECT count() AS n FROM claim_relation GROUP ALL'))
entity=sql('SELECT VALUE type::string(id) FROM entity LIMIT 1')[0]['result']
eid=entity[0] if entity else 'entity:diagnostic_absent'
parts=eid.split(':',1)
prefix='LET $cutoff=time::now(); LET $node='+json.dumps(eid)+'; LET $table='+json.dumps(parts[0])+'; LET $key='+json.dumps(parts[1])+'; '
qs={
 'edge_cast': 'SELECT in,out,relation FROM edge WHERE (in=<record>$node OR out=<record>$node) ORDER BY in ASC,out ASC,t_valid DESC',
 'edge_constructed': 'SELECT in,out,relation FROM edge WHERE (in=type::record($table,$key) OR out=type::record($table,$key)) ORDER BY in ASC,out ASC,t_valid DESC',
 'facts_scan':'SELECT fact_id,content,t_valid,t_ingested,t_invalid,t_invalid_ingested,policy_tags FROM fact',
 'facts_browse':'SELECT fact_id FROM fact WHERE t_valid <= $cutoff ORDER BY t_valid DESC,fact_id ASC LIMIT 50',
 'facts_fts': 'SELECT fact_id,search::score(1) AS ft_score FROM fact WHERE content @1@ "diagnostic_absent" OR index_keys @1@ "diagnostic_absent" ORDER BY ft_score DESC LIMIT 50',
 'entity_batch':'SELECT id FROM entity WHERE canonical_name_normalized IN ["diagnostic_absent"] OR aliases CONTAINSANY ["diagnostic_absent"]',
 'episode_substring':'SELECT episode_id FROM episode WHERE string::contains(string::lowercase(content),"diagnostic_absent") ORDER BY t_ref DESC,episode_id ASC LIMIT 10',
 'episode_scan':'SELECT id,t_ref,t_ingested,metadata FROM episode',
 'backfill_first':'SELECT fact_id FROM fact WHERE embedding IS NONE ORDER BY fact_id ASC LIMIT 100',
 'backfill_cursor':'SELECT fact_id FROM (SELECT fact_id FROM fact WHERE embedding IS NONE) WHERE fact_id > "fact:diagnostic_absent" ORDER BY fact_id ASC LIMIT 100',
 'artifacts_upper':"SELECT id FROM task_artifact WHERE state='committed' ORDER BY id DESC LIMIT 1",
 'tasks_due':"SELECT id FROM tenant_task WHERE state='queued' OR (state='running' AND lease_expires_at<=time::now()) LIMIT 1",
}
for name,q in qs.items():
 explanation=sql(prefix+q+' EXPLAIN')[-1]
 times=[];counts=[]
 for repeat in range(5):
  start=time.perf_counter();o=sql(prefix+q)[-1];times.append((time.perf_counter()-start)*1000);counts.append(len(o.get('result',[])) if isinstance(o.get('result'),list) else None)
 print(json.dumps({'case':name,'plan':explanation,'wall_median_ms':round(statistics.median(times),3),'rows':counts,'status':o.get('status'),'sql_time':o.get('time')},ensure_ascii=False))
