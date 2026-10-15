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

cutoff=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())
vec=[1.0]+[0.0]*2047
prefix='LET $cutoff='+json.dumps(cutoff)+'; LET $query_vec='+json.dumps(vec)+'; '
visibility="t_valid <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff))"
queries={
'ann_k20_ef80':'SELECT fact_id,vector::similarity::cosine(embedding,$query_vec) AS sem_score FROM fact WHERE embedding IS NOT NONE AND embedding IS NOT NULL AND '+visibility+' AND embedding <|20,80|> $query_vec ORDER BY sem_score DESC LIMIT 20',
'ann_k50_ef200':'SELECT fact_id,vector::similarity::cosine(embedding,$query_vec) AS sem_score FROM fact WHERE embedding IS NOT NONE AND embedding IS NOT NULL AND '+visibility+' AND embedding <|50,200|> $query_vec ORDER BY sem_score DESC LIMIT 50',
'fts_full_visibility':'SELECT fact_id,search::score(1) AS ft_score FROM fact WHERE '+visibility+' AND (content @1@ "diagnostic_absent" OR index_keys @1@ "diagnostic_absent") ORDER BY ft_score DESC,t_valid DESC,fact_id ASC LIMIT 50',
'backfill_first_full':'SELECT * FROM fact WHERE embedding IS NONE ORDER BY fact_id ASC LIMIT 100',
}
def operators(v):
 if isinstance(v,dict):
  out=[{'operator':v['operator'],'index':v.get('attributes',{}).get('index')}] if 'operator' in v else []
  for x in v.values():out+=operators(x)
  return out
 if isinstance(v,list):return sum((operators(x) for x in v),[])
 return []
for name,q in queries.items():
 plan=sql(prefix+q+' EXPLAIN')[-1]
 times=[];reported=[];rows=[]
 for repeat in range(5):
  start=time.perf_counter();o=sql(prefix+q)[-1];times.append((time.perf_counter()-start)*1000);reported.append(o.get('time'));rows.append(len(o.get('result',[])) if isinstance(o.get('result'),list) else None)
 print(json.dumps({'case':name,'plan':operators(plan),'status':o.get('status'),'wall_median_ms':statistics.median(times),'sql_times':reported,'rows':rows}))
