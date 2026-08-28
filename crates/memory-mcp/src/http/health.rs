//! `/health/live` and `/health/ready` handlers.
//!
//! Phase 3: trivial 200 OK. Task 3.9 adds the registry/admission probe
//! to `ready`. Task 5.6 (or later) widens ready to its full form.

pub async fn live() -> &'static str {
    "ok"
}

pub async fn ready() -> &'static str {
    "ok"
}
