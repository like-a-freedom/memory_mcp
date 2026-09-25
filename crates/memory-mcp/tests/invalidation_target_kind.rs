//! Invalidation targets a fact. A record id naming any other kind
//! must be refused before the close owner is reached.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use memory_mcp::MemoryError;
use memory_mcp::memory::api::{
    InvalidationPort, InvalidationRequest, RateLimitPort, StoredRecord, invalidate_fact,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct CloseCall {
    fact_id: String,
    reason: String,
}

struct RecordingPort {
    known: Mutex<Vec<String>>,
    close_calls: Mutex<Vec<CloseCall>>,
    claim_calls: Mutex<Vec<String>>,
    cache_invalidated: Mutex<usize>,
}

impl RecordingPort {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            known: Mutex::new(vec!["fact:abc123".to_owned()]),
            close_calls: Mutex::new(Vec::new()),
            claim_calls: Mutex::new(Vec::new()),
            cache_invalidated: Mutex::new(0),
        })
    }

    fn close_calls(&self) -> Vec<CloseCall> {
        self.close_calls.lock().expect("close lock").clone()
    }
}

#[async_trait::async_trait]
impl InvalidationPort for RecordingPort {
    async fn find_record(&self, record_id: &str) -> Result<StoredRecord, MemoryError> {
        Ok(
            if self
                .known
                .lock()
                .expect("known lock")
                .iter()
                .any(|id| id == record_id)
            {
                StoredRecord::Present
            } else {
                StoredRecord::Absent
            },
        )
    }

    async fn close_record(
        &self,
        fact_id: &str,
        _t_invalid: DateTime<Utc>,
        _t_invalid_ingested: Option<DateTime<Utc>>,
        reason: &str,
    ) -> Result<(), MemoryError> {
        self.close_calls
            .lock()
            .expect("close lock")
            .push(CloseCall {
                fact_id: fact_id.to_owned(),
                reason: reason.to_owned(),
            });
        Ok(())
    }

    async fn close_claims_for_fact(&self, fact_id: &str) -> Result<(), MemoryError> {
        self.claim_calls
            .lock()
            .expect("claim lock")
            .push(fact_id.to_owned());
        Ok(())
    }

    fn claim_pipeline_is_wired(&self) -> bool {
        true
    }

    async fn invalidate_assembled_context(&self) -> Result<(), MemoryError> {
        *self.cache_invalidated.lock().expect("cache lock") += 1;
        Ok(())
    }
}

struct AllowAll;

impl RateLimitPort for AllowAll {
    fn check(&self, _caller: Option<&str>) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn request(fact_id: &str) -> InvalidationRequest {
    InvalidationRequest {
        fact_id: fact_id.to_owned(),
        t_invalid: "2026-01-02T00:00:00Z".parse().expect("valid timestamp"),
        reason: "superseded by a correction".to_owned(),
        caller_id: Some("user-1".to_owned()),
    }
}

#[tokio::test]
async fn a_well_formed_fact_id_is_still_closed_exactly_once() {
    let port = RecordingPort::new();

    invalidate_fact(port.as_ref(), &AllowAll, &request("fact:abc123"))
        .await
        .expect("invalidation succeeds");

    let calls = port.close_calls();
    assert_eq!(calls.len(), 1, "the close is issued exactly once");
    assert_eq!(calls[0].fact_id, "fact:abc123");
    assert_eq!(calls[0].reason, "superseded by a correction");
}

#[tokio::test]
async fn an_episode_id_is_refused_and_never_closed() {
    let port = RecordingPort::new();
    // The generic accessor would have found this episode, passed the
    // existence check, and closed it. The episode table has no
    // `t_invalid` field, so that write is a schema violation.
    port.known
        .lock()
        .expect("known lock")
        .push("episode:xyz".to_owned());

    let error = invalidate_fact(port.as_ref(), &AllowAll, &request("episode:xyz"))
        .await
        .expect_err("an episode is not a fact and must not be invalidated");

    assert!(
        matches!(&error, MemoryError::Validation(message) if message.contains("episode:xyz")),
        "the refusal must name the offending id, got {error:?}"
    );
    assert!(
        port.close_calls().is_empty(),
        "no close may be issued for a non-fact record"
    );
    assert_eq!(*port.cache_invalidated.lock().expect("cache lock"), 0);
}

#[tokio::test]
async fn every_non_fact_table_is_refused_before_any_write() {
    // `edge` and `triple` do define `t_invalid`, so for those the
    // generic path would have performed a real, silent write. That
    // is the case that matters most.
    for id in [
        "edge:abc",
        "triple:abc",
        "entity:abc",
        "community:abc",
        "event_log:abc",
        "query_log:abc",
    ] {
        let port = RecordingPort::new();
        port.known.lock().expect("known lock").push(id.to_owned());

        let error = invalidate_fact(port.as_ref(), &AllowAll, &request(id))
            .await
            .expect_err(&format!("{id} must be refused"));

        assert!(
            matches!(&error, MemoryError::Validation(_)),
            "{id} must be a validation refusal, got {error:?}"
        );
        assert!(
            port.close_calls().is_empty(),
            "{id} must never reach the close owner"
        );
    }
}

#[tokio::test]
async fn a_bare_id_is_refused_as_a_malformed_record_id() {
    let port = RecordingStoreBareId::new();
    let error = invalidate_fact(port.as_ref(), &AllowAll, &request("474b2d8b81b3feabf"))
        .await
        .expect_err("a bare id is not a fact record id");

    assert!(matches!(&error, MemoryError::Validation(_)));
    assert!(port.close_calls().is_empty());
}

/// Minimal port used only to prove a bare id never reaches storage.
struct RecordingStoreBareId {
    close_calls: Mutex<Vec<String>>,
}

impl RecordingStoreBareId {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            close_calls: Mutex::new(Vec::new()),
        })
    }

    fn close_calls(&self) -> Vec<String> {
        self.close_calls.lock().expect("close lock").clone()
    }
}

#[async_trait::async_trait]
impl InvalidationPort for RecordingStoreBareId {
    async fn find_record(&self, _record_id: &str) -> Result<StoredRecord, MemoryError> {
        Ok(StoredRecord::Present)
    }

    async fn close_record(
        &self,
        fact_id: &str,
        _t_invalid: DateTime<Utc>,
        _t_invalid_ingested: Option<DateTime<Utc>>,
        _reason: &str,
    ) -> Result<(), MemoryError> {
        self.close_calls
            .lock()
            .expect("close lock")
            .push(fact_id.to_owned());
        Ok(())
    }

    async fn close_claims_for_fact(&self, _fact_id: &str) -> Result<(), MemoryError> {
        Ok(())
    }

    fn claim_pipeline_is_wired(&self) -> bool {
        false
    }

    async fn invalidate_assembled_context(&self) -> Result<(), MemoryError> {
        Ok(())
    }
}
