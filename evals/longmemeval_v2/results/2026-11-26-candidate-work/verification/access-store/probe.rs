mod common;
use memory_mcp::{memory::fact_access_store::FactAccessStore,storage::DbClient};
use serde_json::json;
#[tokio::test]
async fn access_probe(){
 let (_,db)=common::make_service_with_client().await;
 let keys=(0..5000).map(|i|format!("entry-{i}")).collect::<Vec<_>>();
 db.create("fact:probe",json!({"fact_id":"fact:probe","fact_type":"note","content":"archive ".repeat(8192),"quote":"archive","source_episode":"episode:probe","t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z","confidence":0.9,"index_keys":keys,"entity_links":[],"scope":"org","policy_tags":[],"provenance":{"source":"manual"}}),"org",&["t_valid","t_ingested"]).await.unwrap();
 let start=std::time::Instant::now();
 FactAccessStore::new(db.clone(),"org").record_fact_access("fact:probe",1).await.unwrap();
 println!("PROBE owner {:?}",start.elapsed());
 let start=std::time::Instant::now();
 db.query("UPDATE fact:probe SET access_count=type::int(math::clamp(type::decimal(access_count ?? 0) + type::decimal($boost), type::decimal($low), type::decimal($high))), last_accessed=time::now() RETURN NONE",Some(json!({"boost":1,"low":i64::MIN,"high":i64::MAX})),"org").await.unwrap();
 println!("PROBE decimal {:?}",start.elapsed());
 let start=std::time::Instant::now();
 db.query("UPDATE fact:probe SET access_count += 1, last_accessed=time::now() RETURN NONE",None,"org").await.unwrap();
 println!("PROBE plain {:?}",start.elapsed());
}
#[tokio::test]
#[ignore]
async fn frozen_store_access_probe(){
 use memory_mcp::{config::SurrealConfigBuilder,storage::SurrealDbClient};
 use std::sync::Arc;
 let cfg=SurrealConfigBuilder::new().embedded(true).credentials("root","root").db_name("memory").namespace("main").data_dir(std::env::var("ACCESS_PROBE_STORE").unwrap()).build().unwrap();
 let db=Arc::new(SurrealDbClient::connect(&cfg).await.unwrap());
 let store=FactAccessStore::new(db.clone(),"main");
 for id in ["72dae2359d272b755a877236","37f2974c09fc83ab98421ce5","fe6ba23116a9aa827b8d73c4","07f50ea549db79b8a856800e","fe3c2170d50df2f639856bb1"] {
  let id=format!("fact:{id}");
  let value=db.select_one(&id,"main").await.unwrap().unwrap();
  println!("PROBE_ID {} source_bytes={} keys={}",id,value["content"].as_str().unwrap().len(),value["index_keys"].as_array().unwrap().len());
  let start=std::time::Instant::now();
  store.record_fact_access(&id,1).await.unwrap();
  println!("PROBE_ACCESS {} {:?}",id,start.elapsed());
 }
}
