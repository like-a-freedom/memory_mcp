use memory_mcp::{config::SurrealConfigBuilder, storage::{DbClient,SurrealDbClient}, knowledge::queries::build_select_edge_neighbors_for_entities_query};
use std::time::Instant;
#[tokio::test]
#[ignore = "requires disposable frozen snapshot clone"]
async fn diagnose_graph_snapshot() {
 let config=SurrealConfigBuilder::new().embedded(true).credentials("root","root").db_name("memory").namespace("main").data_dir(std::env::var("MEMORY_FULL_HISTORY_STORE").unwrap()).build().unwrap();
 let db=SurrealDbClient::connect(&config).await.unwrap();
 let fact=db.query("SELECT array::map(entity_links, |$id| <string>$id) AS links FROM fact LIMIT 1",None,"main").await.unwrap();
 let mut ids=fact[0]["links"].as_array().unwrap().iter().filter_map(|v|v.as_str().map(str::to_owned)).collect::<Vec<_>>();
 ids.sort();ids.dedup();ids.truncate(32);assert_eq!(ids.len(),32);
 let(sql,vars)=build_select_edge_neighbors_for_entities_query(&ids,"2026-11-26T00:00:00Z");
 let(endpoint,rest)=sql.split_once("WHERE (").unwrap().1.split_once(") AND ").unwrap();
 let(condition,order)=rest.split_once(" ORDER BY ").unwrap();
 let condition=condition.replace("t_invalid_ingested","$e.t_invalid_ingested").replace("t_ingested","$e.t_ingested").replace("t_invalid ","$e.t_invalid ").replace("t_valid ","$e.t_valid ");
 let candidate=format!("SELECT in, out, relation FROM array::filter((SELECT in,out,relation,t_valid,t_ingested,t_invalid,t_invalid_ingested FROM edge WHERE ({endpoint})), |$e| {condition}) ORDER BY {order}");
 let start=Instant::now();let original=db.query(&sql,Some(vars.clone()),"main").await.unwrap();
 println!("GRAPH_READ {}",serde_json::json!({"variant":"original","rows":original.as_array().unwrap().len(),"elapsed_ms":start.elapsed().as_secs_f64()*1000.0}));
 let start=Instant::now();let changed=db.query(&candidate,Some(vars),"main").await.unwrap();
 println!("GRAPH_READ {}",serde_json::json!({"variant":"materialized_endpoint_filter","rows":changed.as_array().unwrap().len(),"elapsed_ms":start.elapsed().as_secs_f64()*1000.0,"same_rows_order":changed==original}));
 assert_eq!(changed,original);
}
