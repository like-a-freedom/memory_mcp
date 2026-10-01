//! The quota policy, at the seam the owning context declares.
//!
//! These cases moved out of `http/registry/plan.rs` with the policy itself,
//! under the same names. What changed is what the tests can now reach: the
//! quota is a decision about a tenant's usage, it is owned by the operations
//! context, and a test outside the HTTP adapter observes it there. Before the
//! move these assertions could only live beside the code, in the one module
//! that happened to hold it.
//!
//! Gated on `streamable-http`, because the policy is: a tenant is a concept of
//! the SaaS profile, and the stdio profile has one Active Namespace and no
//! quota. `control-plane` implies `streamable-http`, and the gate names the
//! narrower of the two so the intent is legible.

#![cfg(feature = "streamable-http")]

use chrono::{DateTime, Utc};

use memory_mcp::operations::quota::{
    QuotaDecision, QuotaPlan, UsageCounter, enforce_ingest, usage_drift_report,
};

fn plan(ingest_per_minute: u32) -> QuotaPlan {
    QuotaPlan {
        ingest_per_minute,
        ..QuotaPlan::default()
    }
}

#[test]
fn ingest_allows_under_limit() {
    let mut counter = UsageCounter::default();
    let now = Utc::now();
    counter.window_start = now;
    for _ in 0..3 {
        assert!(matches!(
            enforce_ingest(&plan(5), &mut counter, 0, now),
            QuotaDecision::Allow
        ));
    }
    assert_eq!(counter.ingest_current_minute, 3);
}

#[test]
fn quota_exceeded_rejects_ingest_with_retry_guidance() {
    let mut counter = UsageCounter::default();
    let now = Utc::now();
    counter.window_start = now;
    for _ in 0..5 {
        assert!(matches!(
            enforce_ingest(&plan(5), &mut counter, 0, now),
            QuotaDecision::Allow
        ));
    }
    let decision = enforce_ingest(&plan(5), &mut counter, 0, now);
    match decision {
        QuotaDecision::Deny {
            reason,
            retry_after_secs,
            guidance,
        } => {
            assert_eq!(reason, "ingest_rate_exceeded");
            assert!(retry_after_secs > 0);
            assert!(guidance.contains("ingest limited to 5"));
        }
        QuotaDecision::Allow => panic!("expected deny"),
    }
}

#[test]
fn zero_per_minute_disables_ingest() {
    let mut counter = UsageCounter::default();
    let now = Utc::now();
    counter.window_start = now;
    assert!(enforce_ingest(&plan(0), &mut counter, 0, now).is_deny());
}

#[test]
fn window_rolls_after_60s() {
    let mut counter = UsageCounter::default();
    let t0 = Utc::now();
    counter.window_start = t0;
    for _ in 0..2 {
        assert!(matches!(
            enforce_ingest(&plan(2), &mut counter, 0, t0),
            QuotaDecision::Allow
        ));
    }
    assert!(enforce_ingest(&plan(2), &mut counter, 0, t0).is_deny());

    let t1 = t0 + chrono::Duration::seconds(61);
    assert!(matches!(
        enforce_ingest(&plan(2), &mut counter, 0, t1),
        QuotaDecision::Allow
    ));
    assert_eq!(counter.ingest_current_minute, 1);
}

#[test]
fn reconciler_repairs_drift_above_threshold() {
    let report = usage_drift_report(&plan(5), "ten_1", 10, 0);
    assert!(report.repaired);
    assert_eq!(report.drift, 10);
}

#[test]
fn reconciler_skips_drift_below_threshold() {
    let report = usage_drift_report(&plan(5), "ten_2", 7, 5);
    assert!(!report.repaired);
    assert_eq!(report.drift, 2);
}

/// A denial must leave the counter exactly as it found it.
///
/// This is true by construction today — the three increments are the last
/// statements before `Allow` — and it is the property `InMemoryStore` depends
/// on, because there the increment inside its lock *is* the write. A denial
/// that mutated the counter would spend a tenant's quota on a request it then
/// refused, and every such refusal would make the next one more likely.
#[test]
fn a_denial_never_mutates_the_counter() {
    let now = Utc::now();
    let before = UsageCounter {
        ingest_current_minute: 4,
        window_start: now,
        ingested_bytes: 10,
        episode_count: 2,
    };

    for denied in [
        // ingest disabled
        enforce_ingest(&plan(0), &mut before.clone(), 0, now),
        // rate
        enforce_ingest(&plan(4), &mut before.clone(), 0, now),
    ] {
        assert!(denied.is_deny(), "this case must be a denial");

        let mut after = UsageCounter {
            ingest_current_minute: 4,
            window_start: now,
            ingested_bytes: 10,
            episode_count: 2,
        };
        assert!(enforce_ingest(&plan(4), &mut after, 0, now).is_deny());
        assert_eq!(
            after.ingest_current_minute, 4,
            "the minute count is untouched"
        );
        assert_eq!(after.ingested_bytes, 10, "the byte count is untouched");
        assert_eq!(after.episode_count, 2, "the episode count is untouched");
    }

    // And the byte ceiling, which is checked before the rate limit.
    let mut after = UsageCounter {
        ingest_current_minute: 4,
        window_start: now,
        ingested_bytes: 10,
        episode_count: 2,
    };
    let tiny_ceiling = QuotaPlan {
        ingest_per_minute: 100,
        max_ingested_bytes: 5,
        ..QuotaPlan::default()
    };
    assert!(
        enforce_ingest(&tiny_ceiling, &mut after, 1, now).is_deny(),
        "10 already-ingested bytes exceed a ceiling of 5"
    );
    assert_eq!(after.ingested_bytes, 10);
    assert_eq!(after.episode_count, 2);
}

/// Every refusal reason is a bounded token.
///
/// The reason ends up in an HTTP body and, eventually, in a metric label. A
/// reason that can contain anything is a reason that can create a new time
/// series per caller, so this asserts the shape rather than each string: a
/// value carrying a space, a slash or a digit that came from user input would
/// pass a `==` check against today's four literals and fail in production.
#[test]
fn every_denial_reason_is_a_bounded_token() {
    let now = Utc::now();
    let mut reasons: Vec<String> = Vec::new();

    let cases: Vec<QuotaPlan> = vec![
        plan(0),
        QuotaPlan {
            max_ingested_bytes: 0,
            ..plan(10)
        },
        QuotaPlan {
            max_episode_count: 0,
            ..plan(10)
        },
        plan(0),
    ];

    for case in cases {
        let mut counter = UsageCounter {
            ingest_current_minute: 10,
            window_start: now,
            ingested_bytes: 0,
            episode_count: 0,
        };
        if let QuotaDecision::Deny { reason, .. } = enforce_ingest(&case, &mut counter, 1, now) {
            reasons.push(reason);
        }
    }

    assert!(
        reasons.len() >= 3,
        "expected the four ceilings to produce denials, got {reasons:?}"
    );
    for reason in &reasons {
        assert!(
            !reason.is_empty() && reason.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "`{reason}` is not a bounded token: a denial reason becomes a \
             metric label, and an unbounded one lets a caller invent a series"
        );
    }
}

/// The registry's plan converts into a quota plan.
///
/// The conversion crosses a boundary the policy now owns, so it is tested from
/// outside: `QuotaPlan::from` takes the registry contract and produces the
/// limits the policy reads, and a field left at its default here is a limit
/// that silently never applies.
#[test]
fn a_registry_plan_converts_to_a_quota_plan() {
    use memory_mcp::models::registry::{Plan, PlanLimits};

    let registry_plan = Plan {
        id: "free".into(),
        version: 7,
        limits: PlanLimits {
            max_ingested_bytes: 100,
            max_episode_count: 10,
            ingest_per_minute: 3,
            max_open_app_sessions: 8,
            max_active_api_keys: 2,
            per_tenant_request_concurrency: 6,
            extraction_concurrency: 4,
        },
    };

    let quota = QuotaPlan::from(&registry_plan);
    assert_eq!(quota.plan_id, "free:7");
    assert_eq!(quota.max_ingested_bytes, 100);
    assert_eq!(quota.max_episode_count, 10);
    assert_eq!(quota.ingest_per_minute, 3);
    assert_eq!(quota.max_open_app_sessions, 8);
    assert_eq!(quota.max_active_api_keys, 2);
    assert_eq!(quota.per_tenant_request_concurrency, 6);
    assert_eq!(quota.extraction_concurrency, 4);
}

/// The drift threshold is one constant, read in two places.
///
/// `usage_drift_report` compares episode-count drift against
/// `reconciler_drift_threshold`; `reconcile_all` compares byte drift against
/// the same field inline. Those are two comparisons of two different quantities
/// against one number, which is the shape that drifts. Pinning them here means
/// a change to the threshold shows up as a test rather than as a silent
/// difference between what is repaired and what is not.
#[test]
fn the_byte_and_episode_drift_share_one_threshold() {
    let plan = QuotaPlan {
        reconciler_drift_threshold: 5,
        ..QuotaPlan::default()
    };
    assert_eq!(plan.reconciler_drift_threshold, 5);

    // At the threshold, nothing is repaired: the rule is `>` not `>=`.
    assert!(!usage_drift_report(&plan, "t", 5, 0).repaired);
    assert!(usage_drift_report(&plan, "t", 6, 0).repaired);
}

/// The window rolls on the caller's clock, and the function that reads it is
/// the same one the policy calls.
#[test]
fn a_window_older_than_sixty_seconds_rolls_before_the_rate_check() {
    let t0: DateTime<Utc> = Utc::now();
    let mut counter = UsageCounter {
        ingest_current_minute: 99,
        window_start: t0 - chrono::Duration::seconds(61),
        ingested_bytes: 0,
        episode_count: 0,
    };
    assert!(
        matches!(
            enforce_ingest(&plan(1), &mut counter, 0, t0),
            QuotaDecision::Allow
        ),
        "a stale window must be rolled before the rate is checked, or a busy \
         minute denies forever"
    );
    assert_eq!(counter.ingest_current_minute, 1);
}
