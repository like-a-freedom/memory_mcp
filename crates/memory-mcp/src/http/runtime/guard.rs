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

use super::pool::AdmissionPermit;
use super::storage::TenantRuntime;

/// Keeps a Tenant Runtime pinned until the guard is dropped.
pub struct OperationGuard {
    runtime: Arc<TenantRuntime>,
    pin_count: Arc<AtomicU32>,
}

impl OperationGuard {
    pub fn new(runtime: Arc<TenantRuntime>, pin_count: Arc<AtomicU32>) -> Self {
        pin_count.fetch_add(1, Ordering::SeqCst);
        Self { runtime, pin_count }
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
