//! Memory invalidation use case: the single bi-temporal close
//! path, expressed against a narrow port rather than a shared
//! service container.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use memory_mcp::MemoryError;
use memory_mcp::memory::api::{
    InvalidationPort, InvalidationRequest, RateLimitPort, StoredRecord, invalidate_fact,
};

#[derive(Debug, Clone, PartialEq)]
struct CloseCall {
    fact_id: String,
    t_invalid: String,
    t_invalid_ingested: Option<String>,
    reason: String,
}

struct RecordingPort {
    existing: Mutex<Option<String>>,
    close_calls: Mutex<Vec<CloseCall>>,
    claim_calls: Mutex<Vec<String>>,
    cache_invalidated: Mutex<usize>,
    claim_pipeline_wired: bool,
}

impl RecordingPort {
    fn with_fact(fact_id: &str) -> Arc<Self> {
        Arc::new(Self {
            existing: Mutex::new(Some(fact_id.to_owned())),
            close_calls: Mutex::new(Vec::new()),
            claim_calls: Mutex::new(Vec::new()),
            cache_invalidated: Mutex::new(0),
            claim_pipeline_wired: true,
        })
    }

    fn without_fact() -> Arc<Self> {
        Arc::new(Self {
            existing: Mutex::new(None),
            close_calls: Mutex::new(Vec::new()),
            claim_calls: Mutex::new(Vec::new()),
            cache_invalidated: Mutex::new(0),
            claim_pipeline_wired: true,
        })
    }

    fn close_calls(&self) -> Vec<CloseCall> {
        self.close_calls.lock().expect("close lock").clone()
    }
}

#[async_trait::async_trait]
impl InvalidationPort for RecordingPort {
    async fn find_record(&self, record_id: &str) -> Result<StoredRecord, MemoryError> {
        Ok(match self.existing.lock().expect("existing lock").clone() {
            Some(fact) if fact == record_id => StoredRecord::Present,
            _ => StoredRecord::Absent,
        })
    }

    async fn close_record(
        &self,
        fact_id: &str,
        t_invalid: DateTime<Utc>,
        t_invalid_ingested: Option<DateTime<Utc>>,
        reason: &str,
    ) -> Result<(), MemoryError> {
        self.close_calls
            .lock()
            .expect("close lock")
            .push(CloseCall {
                fact_id: fact_id.to_owned(),
                t_invalid: t_invalid.to_rfc3339(),
                t_invalid_ingested: t_invalid_ingested.map(|value| value.to_rfc3339()),
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
        self.claim_pipeline_wired
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

struct DenyAll;

impl RateLimitPort for DenyAll {
    fn check(&self, _caller: Option<&str>) -> Result<(), MemoryError> {
        Err(MemoryError::Validation("rate limit exceeded".into()))
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
async fn invalidation_closes_both_bi_temporal_fields_through_one_port() {
    let port = RecordingPort::with_fact("fact:abc123");

    invalidate_fact(port.as_ref(), &AllowAll, &request("fact:abc123"))
        .await
        .expect("invalidation succeeds");

    let calls = port.close_calls();
    assert_eq!(calls.len(), 1, "the close is issued exactly once");
    assert_eq!(calls[0].fact_id, "fact:abc123");
    assert_eq!(
        calls[0].t_invalid, "2026-01-02T00:00:00+00:00",
        "the caller-supplied valid time is the recorded invalid time"
    );
    assert_eq!(
        calls[0].t_invalid_ingested, None,
        "transaction time defaults to server-side now, not the caller"
    );
    assert_eq!(calls[0].reason, "superseded by a correction");
}

#[tokio::test]
async fn invalidation_closes_derived_claims_and_drops_cached_context() {
    let port = RecordingPort::with_fact("fact:abc123");

    invalidate_fact(port.as_ref(), &AllowAll, &request("fact:abc123"))
        .await
        .expect("invalidation succeeds");

    assert_eq!(
        *port.claim_calls.lock().expect("claim lock"),
        vec!["fact:abc123".to_owned()],
        "derived claims are closed with the fact"
    );
    assert_eq!(
        *port.cache_invalidated.lock().expect("cache lock"),
        1,
        "the assembled-context cache is invalidated exactly once"
    );
}

#[tokio::test]
async fn a_missing_fact_is_not_closed_and_caches_stay_intact() {
    let port = RecordingPort::without_fact();

    let error = invalidate_fact(port.as_ref(), &AllowAll, &request("fact:missing"))
        .await
        .expect_err("a missing fact cannot be invalidated");

    assert!(
        matches!(&error, MemoryError::NotFound(message) if message.contains("fact_id not found")),
        "expected not-found, got {error:?}"
    );
    assert!(
        port.close_calls().is_empty(),
        "nothing is closed for a fact that does not exist"
    );
    assert_eq!(*port.cache_invalidated.lock().expect("cache lock"), 0);
}

#[tokio::test]
async fn a_refused_caller_never_reaches_the_close_owner() {
    let port = RecordingPort::with_fact("fact:abc123");

    let error = invalidate_fact(port.as_ref(), &DenyAll, &request("fact:abc123"))
        .await
        .expect_err("a refused caller cannot invalidate");

    assert!(matches!(&error, MemoryError::Validation(_)));
    assert!(
        port.close_calls().is_empty(),
        "a rate-limited caller must not invalidate anything"
    );
}

#[tokio::test]
async fn a_missing_claim_pipeline_still_closes_the_fact() {
    let port = Arc::new(RecordingPort {
        existing: Mutex::new(Some("fact:abc123".to_owned())),
        close_calls: Mutex::new(Vec::new()),
        claim_calls: Mutex::new(Vec::new()),
        cache_invalidated: Mutex::new(0),
        claim_pipeline_wired: false,
    });

    invalidate_fact(port.as_ref(), &AllowAll, &request("fact:abc123"))
        .await
        .expect("the fact is still invalidated without claims");

    assert_eq!(port.close_calls().len(), 1);
    assert!(
        port.claim_calls.lock().expect("claim lock").is_empty(),
        "no claim close is attempted when the pipeline is unwired"
    );
}
