import subprocess,json,urllib.request,time,statistics
d=json.loads(subprocess.check_output(['docker','inspect','memory_mcp']))[0]
e=dict(x.split('=',1) for x in d['Config']['Env'] if '=' in x)
base='http://172.24.0.2:8000'
r=urllib.request.Request(base+'/signin',data=json.dumps({'user':e['SURREALDB_CONTROL_USERNAME'],'pass':e['SURREALDB_CONTROL_PASSWORD']}).encode(),headers={'Content-Type':'application/json','Accept':'application/json'})
token=json.loads(urllib.request.urlopen(r,timeout=10).read())['token']
h={'Accept':'application/json','Content-Type':'text/plain','Authorization':'Bearer '+token,'Surreal-NS':e['SURREALDB_CONTROL_NAMESPACE'],'Surreal-DB':e['SURREALDB_CONTROL_DB']}
def sql(q):
 return json.loads(urllib.request.urlopen(urllib.request.Request(base+'/sql',data=q.encode(),headers=h),timeout=10).read())

t=sql("SELECT namespace_binding FROM tenant WHERE status='ready' LIMIT 1")[0]['result'][0]['namespace_binding'];h['Surreal-NS']=t['namespace'];h['Surreal-DB']=t['database']
print('sequence',sql('SELECT * FROM tenant_change_sequence:default; SELECT count() AS n FROM tenant_change_sequence GROUP ALL;SELECT event_seq FROM tenant_change_event ORDER BY event_seq DESC LIMIT 1'))
for table in ['episode','tenant_task']:
 info=sql('INFO FOR TABLE '+table)[0]['result']
 print('fields',table,sorted(info.get('fields',{})))
