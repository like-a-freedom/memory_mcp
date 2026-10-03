//! Operation guard + response lease + leased body.
//!
//! `OperationGuard` keeps a Tenant Runtime pinned for the
//! duration of an HTTP request. `ResponseLease` couples the
//! pin and the admission permit and is moved into the
//! response body so the permit and pin are not released
//! until the body is fully consumed (including SSE).
//!
//! `LeasedBody<B>` is the `http_body::Body` adapter that
//! holds the lease for the body lifetime. The lease is
//! released when the body emits `None` or `Some(Err(_))`;
//! intermediate frames keep it alive.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};

use http_body::Body;
use http_body::Frame;
use http_body::SizeHint;
use tokio::sync::OwnedSemaphorePermit;

use super::pool::AdmissionPermit;
use super::storage::TenantRuntime;

/// Keeps a Tenant Runtime pinned until the guard is dropped.
pub struct OperationGuard {
    runtime: Arc<TenantRuntime>,
    pin_count: Arc<AtomicU32>,
    _tenant_permit: OwnedSemaphorePermit,
}

impl OperationGuard {
    pub fn new(
        runtime: Arc<TenantRuntime>,
        pin_count: Arc<AtomicU32>,
        tenant_permit: OwnedSemaphorePermit,
    ) -> Self {
        pin_count.fetch_add(1, Ordering::SeqCst);
        Self::from_parts(runtime, pin_count, tenant_permit)
    }

    /// Build a guard from a pin that is already counted.
    ///
    /// The pool reserves the pin under its state lock, before it can wait for
    /// anything, so the slot cannot be unloaded between choosing to use it and
    /// receiving it. Transferring the reservation here — instead of incrementing
    /// again — is what keeps the count equal to the number of live guards.
    pub(crate) fn from_parts(
        runtime: Arc<TenantRuntime>,
        pin_count: Arc<AtomicU32>,
        tenant_permit: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            runtime,
            pin_count,
            _tenant_permit: tenant_permit,
        }
    }

    pub fn runtime(&self) -> &Arc<TenantRuntime> {
        &self.runtime
    }

    #[cfg(test)]
    pub fn pin_counter(&self) -> Arc<AtomicU32> {
        self.pin_count.clone()
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        self.pin_count.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A pin counted against a slot before its runtime is usable.
///
/// Taken while the pool's state lock is held, and either transferred into an
/// [`OperationGuard`] or released by its own `Drop`. A cancelled acquisition
/// therefore returns its pin without any cleanup step that could itself be
/// cancelled.
pub(crate) struct SlotReservation {
    pins: Arc<AtomicU32>,
    transferred: bool,
}

impl SlotReservation {
    /// Count one pin against `pins`.
    pub(crate) fn acquire(pins: &Arc<AtomicU32>) -> Self {
        pins.fetch_add(1, Ordering::SeqCst);
        Self {
            pins: Arc::clone(pins),
            transferred: false,
        }
    }

    /// Hand the pin to the guard that will release it.
    pub(crate) fn into_guard(
        mut self,
        runtime: Arc<TenantRuntime>,
        tenant_permit: OwnedSemaphorePermit,
    ) -> OperationGuard {
        self.transferred = true;
        OperationGuard::from_parts(runtime, Arc::clone(&self.pins), tenant_permit)
    }
}

impl Drop for SlotReservation {
    fn drop(&mut self) {
        if !self.transferred {
            self.pins.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Owns the operation pin and the admission permit for the
/// duration of the response. Lives inside `LeasedBody` so the
/// resources are not released until the body stream ends.
pub struct ResponseLease {
    _operation: Option<Arc<OperationGuard>>,
    _admission: Arc<AdmissionPermit>,
}

/// `Arc` clone handle for the admission permit so it can be
/// stored in request extensions (axum requires `Clone`).
#[derive(Clone)]
pub struct AdmissionPermitRef(pub Arc<AdmissionPermit>);

impl std::ops::Deref for AdmissionPermitRef {
    type Target = AdmissionPermit;
    fn deref(&self) -> &AdmissionPermit {
        &self.0
    }
}

impl ResponseLease {
    pub fn new(operation: Option<Arc<OperationGuard>>, admission: Arc<AdmissionPermit>) -> Self {
        Self {
            _operation: operation,
            _admission: admission,
        }
    }
}

/// `http_body::Body` wrapper that keeps the `ResponseLease`
/// alive for the entire body lifetime. The lease is released
/// on terminal frames (`None` or `Some(Err(_))`).
pub struct LeasedBody<B> {
    inner: Pin<Box<B>>,
    _lease: Option<ResponseLease>,
}

/// Cloneable extension wrappers. `axum::Extension<T>` requires
/// `T: Clone`, but the underlying `OperationGuard` and
/// `AdmissionPermit` are not Clone. The wrapper takes the
/// value by `Arc` clone so the same ownership is shared
/// between the request extensions and the response body
/// wrapper.
#[derive(Clone)]
pub struct OperationGuardRef(pub Arc<OperationGuard>);

impl std::ops::Deref for OperationGuardRef {
    type Target = OperationGuard;
    fn deref(&self) -> &OperationGuard {
        &self.0
    }
}

impl<B> LeasedBody<B> {
    pub fn new(body: B, lease: ResponseLease) -> Self {
        Self {
            inner: Box::pin(body),
            _lease: Some(lease),
        }
    }
}

impl<B> Unpin for LeasedBody<B> where B: Unpin {}

impl<B> Body for LeasedBody<B>
where
    B: Body + Unpin,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        // `LeasedBody<B>` is `Unpin` when `B: Unpin`, so we
        // can `get_mut` on the pin and split-borrow the
        // inner `Pin<Box<B>>`.
        let this = self.get_mut();
        let inner_mut: &mut B = this.inner.as_mut().get_mut();
        let mut inner_pin = Pin::new(inner_mut);
        match inner_pin.as_mut().poll_frame(cx) {
            Poll::Ready(None) => {
                this._lease.take();
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                this._lease.take();
                Poll::Ready(Some(Err(error)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::runtime::pool::AdmissionGate;
    use http_body_util::BodyExt;

    /// A gate with a single request slot, so a second acquire must fail while
    /// the first permit is alive.
    fn single_slot_gate() -> Arc<AdmissionGate> {
        Arc::new(AdmissionGate::new(1))
    }

    /// A `ResponseLease` backed by `permit`, with no operation pin.
    fn lease_for(permit: AdmissionPermit) -> ResponseLease {
        ResponseLease::new(None, Arc::new(permit))
    }

    #[tokio::test]
    async fn a_leased_body_still_delivers_its_frames() {
        let gate = single_slot_gate();
        let permit = gate.try_acquire().expect("permit acquires");
        let body = LeasedBody::new(axum::body::Body::from("ok"), lease_for(permit));

        let collected = body.collect().await.expect("body completes");

        assert_eq!(&collected.to_bytes()[..], b"ok");
    }

    #[tokio::test]
    async fn a_leased_body_releases_its_permit_when_the_stream_ends() {
        let gate = single_slot_gate();
        let permit = gate.try_acquire().expect("permit acquires");
        let body = LeasedBody::new(axum::body::Body::from("ok"), lease_for(permit));
        body.collect().await.expect("body completes");

        let observed = gate.try_acquire().is_ok();

        assert!(observed, "the permit must be returned once the body ends");
    }

    #[tokio::test]
    async fn a_leased_body_keeps_its_permit_while_frames_are_pending() {
        // The body has not been polled, so the lease must still be held.
        let gate = single_slot_gate();
        let permit = gate.try_acquire().expect("permit acquires");
        let _body = LeasedBody::new(axum::body::Body::from("ok"), lease_for(permit));

        let observed = gate.try_acquire().is_ok();

        assert!(!observed, "an unconsumed body must hold its permit");
    }

    #[tokio::test]
    async fn a_leased_body_reports_the_inner_size_hint() {
        let gate = single_slot_gate();
        let permit = gate.try_acquire().expect("permit acquires");
        let body = LeasedBody::new(axum::body::Body::from("ok"), lease_for(permit));

        assert_eq!(body.size_hint().exact(), Some(2));
    }

    #[tokio::test]
    async fn a_permit_ref_keeps_its_permit_alive() {
        // The wrapper is handed to request extensions and to the response
        // body as separate `Arc` clones, so both views must see the same
        // permit; dropping one must not release it.
        let gate = single_slot_gate();
        let permit = gate.try_acquire().expect("permit acquires");
        let permit_ref = AdmissionPermitRef(Arc::new(permit));

        let other_view = permit_ref.clone();
        drop(permit_ref);

        assert!(
            gate.try_acquire().is_err(),
            "the surviving clone still holds the permit"
        );
        drop(other_view);
        assert!(gate.try_acquire().is_ok());
    }

    #[tokio::test]
    async fn dropping_a_permit_ref_returns_the_slot() {
        let gate = single_slot_gate();
        let permit = gate.try_acquire().expect("permit acquires");

        {
            let _permit_ref = AdmissionPermitRef(Arc::new(permit));
        }

        assert!(gate.try_acquire().is_ok());
    }
}
