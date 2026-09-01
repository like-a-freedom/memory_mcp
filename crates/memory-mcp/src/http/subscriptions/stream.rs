//! Bounded event-to-notification delivery for subscriptions.

use std::collections::VecDeque;
use std::time::Duration;

use crate::http::subscriptions::outbox::TenantChangeEvent;

/// Maximum number of distinct resource invalidations buffered for one listener.
pub const DEFAULT_QUEUE_CAPACITY: usize = 64;
/// A sink that does not accept a frame within this bound is disconnected.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// Result of adding an event to a coalescing queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePush {
    Enqueued,
    Coalesced,
}

/// The queue is bounded by distinct resources, not raw event count.
///
/// Repeated updates for one resource replace an older queued update only when
/// the replacement has at least as high a revision. This preserves the
/// invalidation contract while preventing a burst from consuming unbounded
/// memory.
#[derive(Debug, Clone)]
pub struct CoalescingQueue {
    capacity: usize,
    events: VecDeque<TenantChangeEvent>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum QueueError {
    #[error("subscription queue is full")]
    Full,
}

impl CoalescingQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            events: VecDeque::new(),
        }
    }

    pub fn push(&mut self, event: TenantChangeEvent) -> Result<QueuePush, QueueError> {
        if let Some(existing) = self
            .events
            .iter_mut()
            .find(|existing| existing.resource_id == event.resource_id)
        {
            if event.revision > existing.revision
                || (event.revision == existing.revision && event.sequence > existing.sequence)
            {
                *existing = event;
            }
            return Ok(QueuePush::Coalesced);
        }
        if self.events.len() >= self.capacity {
            return Err(QueueError::Full);
        }
        self.events.push_back(event);
        Ok(QueuePush::Enqueued)
    }

    pub fn pop_front(&mut self) -> Option<TenantChangeEvent> {
        self.events.pop_front()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Error returned by the bounded sink adapter.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DeliveryError {
    #[error("subscription sink send timed out")]
    Timeout,
    #[error("subscription sink closed: {0}")]
    SinkClosed(String),
}

/// Send a single change event as a resource-updated notification.
pub async fn send_invalidation(
    sink: &rmcp::service::SubscriptionSink,
    event: TenantChangeEvent,
) -> Result<(), rmcp::service::SubscriptionSendError> {
    use rmcp::model::{
        ResourceUpdatedNotification, ResourceUpdatedNotificationParam, ServerNotification,
    };
    sink.send(ServerNotification::ResourceUpdatedNotification(
        ResourceUpdatedNotification::new(ResourceUpdatedNotificationParam::new(event.resource_id)),
    ))
    .await
}

/// Send with an explicit slow-consumer bound. A closed sink is terminal for a
/// subscription and is surfaced as a typed delivery error to the handler.
pub async fn send_invalidation_with_timeout(
    sink: &rmcp::service::SubscriptionSink,
    event: TenantChangeEvent,
) -> Result<(), DeliveryError> {
    match tokio::time::timeout(SEND_TIMEOUT, send_invalidation(sink, event)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(DeliveryError::SinkClosed(error.to_string())),
        Err(_) => Err(DeliveryError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn event(sequence: u64, resource_id: &str, revision: u64) -> TenantChangeEvent {
        TenantChangeEvent {
            sequence,
            resource_id: resource_id.to_string(),
            revision,
            change_kind: "updated".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn coalesces_same_resource_to_highest_revision() {
        let mut queue = CoalescingQueue::new(2);
        assert_eq!(
            queue.push(event(1, "ui://memory/apps/graph", 1)),
            Ok(QueuePush::Enqueued)
        );
        assert_eq!(
            queue.push(event(2, "ui://memory/apps/graph", 2)),
            Ok(QueuePush::Coalesced)
        );
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.pop_front().map(|item| item.sequence), Some(2));
    }

    #[test]
    fn older_coalesced_event_cannot_replace_newer_revision() {
        let mut queue = CoalescingQueue::new(2);
        queue
            .push(event(2, "ui://memory/apps/graph", 2))
            .expect("enqueue");
        queue
            .push(event(3, "ui://memory/apps/graph", 1))
            .expect("coalesce");
        assert_eq!(queue.pop_front().map(|item| item.revision), Some(2));
    }

    #[test]
    fn full_queue_returns_slow_consumer_error_for_new_resource() {
        let mut queue = CoalescingQueue::new(1);
        queue
            .push(event(1, "ui://memory/apps/graph", 1))
            .expect("enqueue");
        assert_eq!(
            queue.push(event(2, "ui://memory/apps/inspector", 1)),
            Err(QueueError::Full)
        );
        assert_eq!(queue.len(), 1);
    }
}
