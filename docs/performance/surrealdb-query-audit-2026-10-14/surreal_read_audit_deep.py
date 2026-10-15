import subprocess,json,urllib.request,time,statistics
d=json.loads(subprocess.check_output(['docker','inspect','memory_mcp']))[0]
e=dict(x.split('=',1) for x in d['Config']['Env'] if '=' in x)
base='http://172.24.0.2:8000'
r=urllib.request.Request(base+'/signin',data=json.dumps({'user':e['SURREALDB_CONTROL_USERNAME'],'pass':e['SURREALDB_CONTROL_PASSWORD']}).encode(),headers={'Content-Type':'application/json','Accept':'application/json'})
token=json.loads(urllib.request.urlopen(r,timeout=10).read())['token']
h={'Accept':'application/json','Content-Type':'text/plain','Authorization':'Bearer '+token,'Surreal-NS':e['SURREALDB_CONTROL_NAMESPACE'],'Surreal-DB':e['SURREALDB_CONTROL_DB']}
def sql(q):
 return json.loads(urllib.request.urlopen(urllib.request.Request(base+'/sql',data=q.encode(),headers=h),timeout=10).read())

print('utc',time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()))
print('tenant_readiness',sql("SELECT status,schema_version,namespace_binding FROM tenant WHERE status='ready' LIMIT 1"))
t=sql("SELECT namespace_binding FROM tenant WHERE status='ready' LIMIT 1")[0]['result'][0]['namespace_binding']
h['Surreal-NS']=t['namespace'];h['Surreal-DB']=t['database']
print('ledger',sql('SELECT script_name,status FROM script_migration ORDER BY script_name'))
print('vector_inventory',sql('SELECT array::len(embedding) AS dimension,count() AS n FROM fact GROUP BY dimension'))
ids=sql('SELECT VALUE type::string(id) FROM entity LIMIT 4')[0]['result']
cutoff=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())
visibility="t_valid <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff))"
prefix='LET $cutoff='+json.dumps(cutoff)+'; '
cast=[];constructed=[]
for i,eid in enumerate(ids):
 table,key=eid.split(':',1)
 prefix+='LET $node'+str(i)+'='+json.dumps(eid)+'; LET $table'+str(i)+'='+json.dumps(table)+'; LET $key'+str(i)+'='+json.dumps(key)+'; '
 cast.append('(in=<record>$node'+str(i)+' OR out=<record>$node'+str(i)+')')
 constructed.append('(in=type::record($table'+str(i)+',$key'+str(i)+') OR out=type::record($table'+str(i)+',$key'+str(i)+'))')
queries={
 'graph_batch_cast':'SELECT in,out,relation FROM edge WHERE ('+' OR '.join(cast)+') AND '+visibility+' ORDER BY in ASC,out ASC,t_valid DESC',
 'graph_batch_constructed':'SELECT in,out,relation FROM edge WHERE ('+' OR '.join(constructed)+') AND '+visibility+' ORDER BY in ASC,out ASC,t_valid DESC',
 'episode_id_lookup':"SELECT episode_id FROM episode WHERE episode_id INSIDE ['episode:diagnostic_absent'] ORDER BY t_ref DESC LIMIT 10",
 'source_fact_lookup':"SELECT fact_id FROM fact WHERE source_episode='episode:diagnostic_absent' AND "+visibility+' LIMIT 1',
 'alias_lookup':"SELECT entity_id FROM entity WHERE aliases CONTAINSANY ['diagnostic_absent'] LIMIT 1",
 'canonical_lookup':"SELECT entity_id FROM entity WHERE canonical_name_normalized='diagnostic_absent' LIMIT 1",
 'missing_vector_count':'SELECT count() AS count FROM fact WHERE embedding IS NONE GROUP ALL',
 'claim_by_id':"SELECT claim_id FROM claim WHERE claim_id INSIDE ['diagnostic_absent'] ORDER BY claim_id LIMIT 50",
 'claim_by_source':"SELECT claim_id FROM claim WHERE source_fact_id INSIDE ['fact:diagnostic_absent'] ORDER BY source_fact_id,claim_id LIMIT 50",
 'claim_relations':"SELECT claim_relation_id FROM claim_relation WHERE left_claim_id INSIDE ['diagnostic_absent'] OR right_claim_id INSIDE ['diagnostic_absent'] ORDER BY claim_relation_id LIMIT 50",
 'fact_entity_links':"SELECT fact_id FROM fact WHERE entity_links CONTAINSANY ['entity:diagnostic_absent'] AND "+visibility+' ORDER BY t_valid DESC LIMIT 50',
 'community_members':"SELECT community_id FROM community WHERE member_entities CONTAINSANY ['entity:diagnostic_absent'] ORDER BY community_id",
}
def operators(v):
 if isinstance(v,dict):
  out=[{'operator':v['operator'],'index':v.get('attributes',{}).get('index')}] if 'operator' in v else []
  for x in v.values():out+=operators(x)
  return out
 if isinstance(v,list):return sum((operators(x) for x in v),[])
 return []
import hashlib
for name,q in queries.items():
 plan=sql(prefix+q+' EXPLAIN')[-1]
 wall=[];sqlms=[];hashes=[];counts=[]
 for repeat in range(7):
  start=time.perf_counter();o=sql(prefix+q)[-1];wall.append((time.perf_counter()-start)*1000)
  v=o.get('time','0s'); factor=0.001 if v.endswith('µs') else 1 if v.endswith('ms') else 1000
  sqlms.append(float(v[:-2] if v.endswith(('µs','ms')) else v[:-1])*factor)
  hashes.append(hashlib.sha256(json.dumps(o.get('result'),sort_keys=True).encode()).hexdigest());counts.append(len(o.get('result',[])))
 print(json.dumps({'case':name,'plan':operators(plan),'status':o.get('status'),'wall_median_ms':round(statistics.median(wall),3),'sql_median_ms':round(statistics.median(sqlms),3),'rows':counts,'result_hashes':hashes}))
