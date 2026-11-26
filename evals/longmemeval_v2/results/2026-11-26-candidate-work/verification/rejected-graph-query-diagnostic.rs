mod common;
use memory_mcp::{storage::DbClient,knowledge::queries::build_select_edge_neighbors_for_entities_query};
#[tokio::test]
async fn compare_graph_plans() {
 let (_,db)=common::make_service_with_client().await;
 let ids=(0..32).map(|n|format!("entity:seed{n:03}")).collect::<Vec<_>>();
 let(sql,vars)=build_select_edge_neighbors_for_entities_query(&ids,"2026-05-01T00:00:00Z");
 let _operands=(0..32).map(|n|format!("type::record($entity_{n}_table,$entity_{n}_key)")).collect::<Vec<_>>().join(",");
 let endpoint=sql.split("WHERE (").nth(1).unwrap().split(") AND ").next().unwrap();
 let rest=sql.split_once(") AND ").unwrap().1;
 let staged=format!("SELECT in, out, relation FROM (SELECT in,out,relation,t_valid,t_ingested,t_invalid,t_invalid_ingested FROM edge WHERE ({endpoint})) WHERE {rest}");
 let (condition,order)=rest.split_once(" ORDER BY ").unwrap();
 let filtered=format!("SELECT in, out, relation FROM array::filter((SELECT in,out,relation,t_valid,t_ingested,t_invalid,t_invalid_ingested FROM edge WHERE ({endpoint})), |$e| {}) ORDER BY {order}", condition.replace("t_invalid_ingested","$e.t_invalid_ingested").replace("t_ingested","$e.t_ingested").replace("t_invalid ","$e.t_invalid ").replace("t_valid ","$e.t_valid "));
 for (name,query,prefix) in [("original",sql.clone(),String::new()),("filtered",filtered,String::new())] {
  let plan=db.query(&format!("{prefix}EXPLAIN {query}"),Some(vars.clone()),"org").await;
  println!("GRAPH_PLAN {}",serde_json::json!({"variant":name,"sql":format!("{prefix}{query}"),"result":plan.map_err(|e|e.to_string())}));
 }
}
