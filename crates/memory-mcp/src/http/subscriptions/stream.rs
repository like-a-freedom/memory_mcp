//! Event-to-notification adapter for subscriptions.
//!
//! Converts a `TenantChangeEvent` into an rmcp
//! `ServerNotification` and sends it through the
//! subscription sink.

use crate::http::subscriptions::outbox::TenantChangeEvent;

/// Send a single change event as a resource updated
/// notification through the subscription sink.
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
