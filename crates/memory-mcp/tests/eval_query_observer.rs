#![cfg(feature = "eval-support")]

use memory_mcp::eval_support::{QueryObserver, SqlObservation};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct RecordingObserver {
    observations: Mutex<Vec<SqlObservation>>,
}

impl QueryObserver for RecordingObserver {
    fn observe(&self, obs: &SqlObservation) {
        self.observations.lock().expect("lock").push(obs.clone());
    }
}

#[tokio::test]
async fn successful_query_reaches_the_observer_once() {
    let client = SurrealDbClient::connect_in_memory("obs_test", "main", "warn")
        .await
        .expect("connect");
    let recorder = Arc::new(RecordingObserver::default());
    client.set_eval_query_observer(Arc::clone(&recorder) as Arc<dyn QueryObserver>);

    DbClient::query(&client, "RETURN 1", None, "main")
        .await
        .expect("query");

    let observations = recorder.observations.lock().expect("lock").clone();
    assert_eq!(
        observations.len(),
        1,
        "exactly one observation, got {observations:?}"
    );
    assert!(observations[0].micros > 0, "micros must be measured");
    assert_eq!(observations[0].op, "db.execute_query");
    assert!(observations[0].sql.contains("RETURN 1"));
}

#[tokio::test]
async fn second_setter_call_is_a_noop_and_queries_still_run() {
    let client = SurrealDbClient::connect_in_memory("obs_test_twice", "main", "warn")
        .await
        .expect("connect");
    let first = Arc::new(RecordingObserver::default());
    let second = Arc::new(RecordingObserver::default());
    client.set_eval_query_observer(Arc::clone(&first) as Arc<dyn QueryObserver>);
    client.set_eval_query_observer(Arc::clone(&second) as Arc<dyn QueryObserver>);

    DbClient::query(&client, "RETURN 1", None, "main")
        .await
        .expect("first query");
    DbClient::query(&client, "RETURN 2", None, "main")
        .await
        .expect("second query");

    assert_eq!(
        first.observations.lock().expect("lock").len(),
        2,
        "the first observer keeps receiving"
    );
    assert_eq!(
        second.observations.lock().expect("lock").len(),
        0,
        "the second setter call must be a no-op"
    );
}
