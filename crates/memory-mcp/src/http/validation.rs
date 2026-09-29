//! Response body deadline. The Streamable HTTP service also receives a cancellation
//! token, so a timeout stops request-owned work; already durable
//! commits remain durable.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use tokio::time::Sleep;

pub struct DeadlineBody {
    inner: Pin<Box<axum::body::Body>>,
    timer: Option<Pin<Box<Sleep>>>,
    deadline: Option<Instant>,
    finished: bool,
}

impl DeadlineBody {
    pub fn new(body: axum::body::Body, timeout: Option<Duration>) -> Self {
        Self {
            inner: Box::pin(body),
            timer: timeout.map(|value| Box::pin(tokio::time::sleep(value))),
            deadline: timeout.map(|value| Instant::now() + value),
            finished: false,
        }
    }
}

impl Body for DeadlineBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.finished {
            return Poll::Ready(None);
        }
        let expired = self
            .timer
            .as_mut()
            .is_some_and(|timer| timer.as_mut().poll(cx).is_ready())
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline);
        if expired {
            self.finished = true;
            return Poll::Ready(Some(Err(axum::Error::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "HTTP response deadline exceeded",
            )))));
        }
        match self.inner.as_mut().poll_frame(cx) {
            Poll::Ready(None) => {
                self.finished = true;
                Poll::Ready(None)
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.finished || self.inner.as_ref().get_ref().is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.inner.as_ref().get_ref().size_hint()
    }
}

pub fn with_body_deadline(
    response: axum::response::Response,
    timeout: Option<Duration>,
) -> axum::response::Response {
    let (parts, body) = response.into_parts();
    axum::response::Response::from_parts(
        parts,
        axum::body::Body::new(DeadlineBody::new(body, timeout)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::pin::Pin;

    #[tokio::test]
    async fn deadline_body_allows_completion_before_timeout() {
        let body = DeadlineBody::new(axum::body::Body::from("ok"), Some(Duration::from_secs(1)));
        let collected = body.collect().await.expect("body completes");
        assert_eq!(&collected.to_bytes()[..], b"ok");
    }

    #[tokio::test]
    async fn deadline_body_returns_timeout_error_when_expired() {
        let mut body = DeadlineBody::new(axum::body::Body::from("late"), Some(Duration::ZERO));
        // Poll once to drive the timer.
        let frame =
            std::future::poll_fn(|cx| <DeadlineBody as Body>::poll_frame(Pin::new(&mut body), cx))
                .await;
        assert!(frame.is_some());
        assert!(frame.unwrap().is_err());
    }

    #[tokio::test]
    async fn a_body_with_no_timeout_is_never_cut_short() {
        let body = DeadlineBody::new(axum::body::Body::from("ok"), None);

        let collected = body.collect().await.expect("body completes");

        assert_eq!(&collected.to_bytes()[..], b"ok");
    }

    #[tokio::test]
    async fn a_body_that_completes_is_marked_finished() {
        let mut body =
            DeadlineBody::new(axum::body::Body::from("ok"), Some(Duration::from_secs(1)));
        (&mut body).collect().await.expect("body completes");

        assert!(body.finished);
    }

    #[tokio::test]
    async fn a_body_that_has_not_been_polled_is_not_finished() {
        let body = DeadlineBody::new(axum::body::Body::from("ok"), Some(Duration::from_secs(1)));

        assert!(!body.finished);
    }

    #[tokio::test]
    async fn a_finished_body_ends_the_stream() {
        let mut body =
            DeadlineBody::new(axum::body::Body::from("ok"), Some(Duration::from_secs(1)));
        (&mut body).collect().await.expect("body completes");

        assert!(body.is_end_stream());
    }

    #[tokio::test]
    async fn a_body_reports_the_inner_size_hint() {
        let body = DeadlineBody::new(axum::body::Body::from("ok"), Some(Duration::from_secs(1)));

        assert_eq!(body.size_hint().exact(), Some(2));
    }

    #[tokio::test]
    async fn a_body_that_has_timed_out_yields_exactly_one_error() {
        let mut body = DeadlineBody::new(axum::body::Body::from("late"), Some(Duration::ZERO));
        let first =
            std::future::poll_fn(|cx| <DeadlineBody as Body>::poll_frame(Pin::new(&mut body), cx))
                .await;

        let second =
            std::future::poll_fn(|cx| <DeadlineBody as Body>::poll_frame(Pin::new(&mut body), cx))
                .await;

        assert!(first.expect("a frame is produced").is_err());
        assert!(
            second.is_none(),
            "a timed-out body must not keep reporting errors"
        );
    }

    #[tokio::test]
    async fn with_body_deadline_passes_a_body_through_unchanged() {
        let response = axum::response::Response::new(axum::body::Body::from("ok"));

        let observed = with_body_deadline(response, Some(Duration::from_secs(1)));

        assert_eq!(observed.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn with_body_deadline_preserves_the_response_body() {
        let response = axum::response::Response::new(axum::body::Body::from("payload"));

        let observed = with_body_deadline(response, Some(Duration::from_secs(1)));
        let collected = observed
            .into_body()
            .collect()
            .await
            .expect("body completes");

        assert_eq!(&collected.to_bytes()[..], b"payload");
    }

    #[tokio::test]
    async fn with_body_deadline_preserves_response_headers() {
        let response = axum::response::Response::builder()
            .status(axum::http::StatusCode::ACCEPTED)
            .header("x-test", "yes")
            .body(axum::body::Body::from("ok"))
            .expect("response builds");

        let observed = with_body_deadline(response, None);

        assert_eq!(observed.headers().get("x-test").unwrap(), "yes");
    }
}
