//! Durable subscriptions and transactional outbox.
//!
//! The durable outbox is the correctness source for subscription delivery. A
//! listener may use a wake hint in the future, but it always polls this log so
//! a lost cross-replica wake cannot lose a committed change.

pub mod outbox;
pub mod scheduler;
pub mod stream;

use std::collections::BTreeSet;

use crate::error::MemoryError;
use crate::http::subscriptions::outbox::TenantChangeEvent;
use crate::storage::client::BoundDbClient;

const PUBLIC_APP_NAMES: [&str; 5] = [
    "inspector",
    "diff",
    "ingestion_review",
    "lifecycle",
    "graph",
];
const APP_ROOT_PREFIX: &str = "ui://memory/apps/";
const APP_SESSION_PREFIX: &str = "ui://memory/app/";
const MAX_RESOURCE_URI_LEN: usize = 512;

/// The only filter shape the HTTP subscription implementation accepts.
///
/// The tenant binding is carried with the validated filter so a filter cannot
/// accidentally be passed to a store for another tenant. Resource identities
/// are exact MCP resource URIs; record IDs and arbitrary URLs are not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSubscriptionFilter {
    tenant_id: String,
    resource_subscriptions: Vec<String>,
}

/// Name used by the handler/store seam in the completion design.
pub type AcceptedSubscriptionFilter = ValidatedSubscriptionFilter;

impl ValidatedSubscriptionFilter {
    /// Validate a client request for one immutable tenant identity.
    pub fn for_tenant(
        tenant_id: impl Into<String>,
        requested: &rmcp::model::SubscriptionFilter,
    ) -> Result<Self, MemoryError> {
        let tenant_id = tenant_id.into();
        if tenant_id.trim().is_empty() {
            return Err(MemoryError::Validation(
                "subscription tenant binding is missing".to_string(),
            ));
        }

        if requested.tools_list_changed.is_some()
            || requested.prompts_list_changed.is_some()
            || requested.resources_list_changed.is_some()
        {
            return Err(MemoryError::Validation(
                "only tenant-owned App/resource subscriptions are supported".to_string(),
            ));
        }

        let requested_resources = requested.resource_subscriptions.as_deref().ok_or_else(|| {
            MemoryError::Validation(
                "resource_subscriptions must contain at least one resource URI".to_string(),
            )
        })?;
        if requested_resources.is_empty() {
            return Err(MemoryError::Validation(
                "resource_subscriptions must contain at least one resource URI".to_string(),
            ));
        }

        let mut resources = BTreeSet::new();
        for uri in requested_resources {
            if !is_supported_resource_uri(uri) {
                return Err(MemoryError::Validation(format!(
                    "unsupported subscription resource URI: {uri}"
                )));
            }
            resources.insert(uri.clone());
        }

        Ok(Self {
            tenant_id,
            resource_subscriptions: resources.into_iter().collect(),
        })
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub fn resource_subscriptions(&self) -> &[String] {
        &self.resource_subscriptions
    }

    /// Convert back to rmcp's accepted filter after validation.
    pub fn to_rmcp_filter(&self) -> rmcp::model::SubscriptionFilter {
        rmcp::model::SubscriptionFilter::builder()
            .resource_subscriptions(self.resource_subscriptions.clone())
            .build()
    }

    /// Return the App/session parts for an accepted concrete session URI.
    ///
    /// This is intentionally private to the subscription module's crate API so
    /// ownership checks cannot be bypassed by callers constructing raw paths.
    fn session_target(uri: &str) -> Option<(&str, &str)> {
        let rest = uri.strip_prefix(APP_SESSION_PREFIX)?;
        let (app, session_id) = rest.split_once('/')?;
        if is_public_app(app)
            && !session_id.is_empty()
            && !session_id.contains('/')
            && session_id
                .chars()
                .all(|character| !character.is_control() && !character.is_whitespace())
        {
            Some((app, session_id))
        } else {
            None
        }
    }
}

fn is_public_app(app: &str) -> bool {
    PUBLIC_APP_NAMES.contains(&app)
}

fn is_supported_resource_uri(uri: &str) -> bool {
    if uri.is_empty() || uri.len() > MAX_RESOURCE_URI_LEN {
        return false;
    }
    if let Some(app) = uri.strip_prefix(APP_ROOT_PREFIX) {
        return is_public_app(app) && !app.contains('/');
    }
    ValidatedSubscriptionFilter::session_target(uri).is_some()
}

/// Tenant-bound subscription store over the namespace's durable outbox.
#[async_trait::async_trait]
pub trait SubscriptionStore: Send + Sync + 'static {
    /// Current durable sequence number for the bound tenant namespace.
    async fn current_sequence(&self) -> Result<u64, MemoryError>;

    /// Validate resource ownership before a listener starts.
    ///
    /// In-memory/test stores can rely on the URI validation and keep the
    /// default. The durable implementation verifies concrete App sessions in
    /// its tenant namespace.
    async fn validate_filter(
        &self,
        _filter: &AcceptedSubscriptionFilter,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    /// Fetch matching events after `after_sequence`. The filter is already
    /// validated and must remain explicit at this boundary to prevent an
    /// unfiltered tenant-wide read from becoming the default behavior.
    async fn next_batch(
        &self,
        after_sequence: u64,
        filter: &AcceptedSubscriptionFilter,
    ) -> Result<Vec<TenantChangeEvent>, MemoryError>;
}

/// Durable subscription store over one tenant namespace.
pub struct DurableSubscriptionStore {
    db: std::sync::Arc<BoundDbClient>,
    tenant_id: String,
}

impl DurableSubscriptionStore {
    pub fn new(db: std::sync::Arc<BoundDbClient>, tenant_id: String) -> Self {
        Self { db, tenant_id }
    }

    fn ensure_tenant<'a>(
        &'a self,
        filter: &'a AcceptedSubscriptionFilter,
    ) -> Result<&'a [String], MemoryError> {
        if filter.tenant_id() != self.tenant_id {
            return Err(MemoryError::Auth(
                "subscription tenant binding mismatch".to_string(),
            ));
        }
        Ok(filter.resource_subscriptions())
    }
}

#[async_trait::async_trait]
impl SubscriptionStore for DurableSubscriptionStore {
    async fn current_sequence(&self) -> Result<u64, MemoryError> {
        let row = self
            .db
            .query_first(
                "SELECT VALUE value FROM tenant_change_sequence:default LIMIT 1",
                None,
            )
            .await?;
        let Some(row) = row else {
            return Ok(0);
        };
        let value = row
            .as_u64()
            .or_else(|| row.as_i64().and_then(|value| u64::try_from(value).ok()))
            .or_else(|| {
                row.get("value")
                    .and_then(serde_json::Value::as_u64)
                    .or_else(|| {
                        row.get("value")
                            .and_then(serde_json::Value::as_i64)
                            .and_then(|value| u64::try_from(value).ok())
                    })
            })
            .ok_or_else(|| {
                MemoryError::Storage("subscription sequence row has no non-negative value".into())
            })?;
        Ok(value)
    }

    async fn validate_filter(
        &self,
        filter: &AcceptedSubscriptionFilter,
    ) -> Result<(), MemoryError> {
        let resources = self.ensure_tenant(filter)?;
        for uri in resources {
            let Some((app, handle)) = AcceptedSubscriptionFilter::session_target(uri) else {
                continue;
            };
            let rows = self
                .db
                .query_rows(
                    "SELECT handle FROM app_session \
                     WHERE tenant_id = $tenant_id AND app = $app AND handle = $handle \
                       AND absolute_expiry > time::now() LIMIT 1",
                    Some(serde_json::json!({
                        "tenant_id": self.tenant_id,
                        "app": app,
                        "handle": handle,
                    })),
                )
                .await?;
            if rows.is_empty() {
                return Err(MemoryError::Auth(
                    "subscription resource is not owned by the tenant".to_string(),
                ));
            }
        }
        Ok(())
    }

    async fn next_batch(
        &self,
        after_sequence: u64,
        filter: &AcceptedSubscriptionFilter,
    ) -> Result<Vec<TenantChangeEvent>, MemoryError> {
        let resources = self.ensure_tenant(filter)?;
        let rows = self
            .db
            .query_rows(
                "SELECT * FROM tenant_change_event \
                 WHERE event_seq > $after AND resource_id IN $resource_ids \
                 ORDER BY event_seq ASC LIMIT 256",
                Some(serde_json::json!({
                    "after": after_sequence,
                    "resource_ids": resources,
                })),
            )
            .await?;
        serde_json::from_value(serde_json::Value::Array(rows))
            .map_err(|error| MemoryError::Storage(format!("subscription batch parse: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::client::{DbClient, SurrealDbClient};

    async fn fresh_store() -> DurableSubscriptionStore {
        let client =
            SurrealDbClient::connect_in_memory("subscription_test", "subscription_test", "warn")
                .await
                .expect("memory client");
        client
            .query(
                "DEFINE TABLE tenant_change_sequence SCHEMAFULL; \
                 DEFINE FIELD value ON tenant_change_sequence TYPE int DEFAULT 0; \
                 UPSERT tenant_change_sequence:default SET value = 0; \
                 DEFINE TABLE tenant_change_event SCHEMAFULL; \
                 DEFINE FIELD event_seq ON tenant_change_event TYPE int; \
                 DEFINE FIELD resource_id ON tenant_change_event TYPE string; \
                 DEFINE FIELD rev ON tenant_change_event TYPE int; \
                 DEFINE FIELD change_kind ON tenant_change_event TYPE string; \
                 DEFINE FIELD created_at ON tenant_change_event TYPE datetime; \
                 DEFINE TABLE app_session SCHEMAFULL; \
                 DEFINE FIELD tenant_id ON app_session TYPE string; \
                 DEFINE FIELD app ON app_session TYPE string; \
                 DEFINE FIELD handle ON app_session TYPE string; \
                 DEFINE FIELD absolute_expiry ON app_session TYPE datetime;",
                None,
                "subscription_test",
            )
            .await
            .expect("subscription schema");
        DurableSubscriptionStore::new(
            std::sync::Arc::new(crate::storage::client::BoundDbClient::new(
                std::sync::Arc::new(client),
                "subscription_test",
            )),
            "tenant_a".to_string(),
        )
    }

    fn root_filter(tenant_id: &str, app: &str) -> AcceptedSubscriptionFilter {
        ValidatedSubscriptionFilter::for_tenant(
            tenant_id,
            &rmcp::model::SubscriptionFilter::builder()
                .resource_subscription(format!("ui://memory/apps/{app}"))
                .build(),
        )
        .expect("valid root filter")
    }

    #[test]
    fn filter_rejects_non_resource_fields_and_unknown_uris() {
        let unsupported = rmcp::model::SubscriptionFilter::builder()
            .tools_list_changed()
            .resource_subscription("ui://memory/apps/graph")
            .build();
        assert!(ValidatedSubscriptionFilter::for_tenant("tenant_a", &unsupported).is_err());

        let unknown = rmcp::model::SubscriptionFilter::builder()
            .resource_subscription("fact:abc")
            .build();
        assert!(ValidatedSubscriptionFilter::for_tenant("tenant_a", &unknown).is_err());
    }

    #[test]
    fn filter_deduplicates_known_app_resources() {
        let requested = rmcp::model::SubscriptionFilter::builder()
            .resource_subscriptions([
                "ui://memory/apps/graph",
                "ui://memory/apps/graph",
                "ui://memory/app/graph/session-1",
            ])
            .build();
        let filter = ValidatedSubscriptionFilter::for_tenant("tenant_a", &requested)
            .expect("known resources");
        assert_eq!(filter.resource_subscriptions().len(), 2);
        assert_eq!(
            filter
                .to_rmcp_filter()
                .resource_subscriptions
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn durable_batch_filters_without_cursor_starvation() {
        let store = fresh_store().await;
        store
            .db
            .query(
                "CREATE tenant_change_event:one SET event_seq = 1, resource_id = 'ui://memory/apps/inspector', rev = 1, change_kind = 'updated', created_at = time::now(); \
                 CREATE tenant_change_event:two SET event_seq = 2, resource_id = 'ui://memory/apps/graph', rev = 1, change_kind = 'updated', created_at = time::now();",
                None,
            )
            .await
            .expect("seed events");

        let filter = root_filter("tenant_a", "graph");
        let events = store.next_batch(0, &filter).await.expect("filtered batch");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 2);
        assert_eq!(store.current_sequence().await.expect("sequence"), 0);
    }

    #[tokio::test]
    async fn durable_store_rejects_filter_for_another_tenant() {
        let store = fresh_store().await;
        let filter = root_filter("tenant_b", "graph");
        assert!(matches!(
            store.next_batch(0, &filter).await,
            Err(MemoryError::Auth(_))
        ));
    }

    #[tokio::test]
    async fn concrete_session_filter_requires_live_tenant_owned_session() {
        let store = fresh_store().await;
        let filter = ValidatedSubscriptionFilter::for_tenant(
            "tenant_a",
            &rmcp::model::SubscriptionFilter::builder()
                .resource_subscription("ui://memory/app/graph/session-1")
                .build(),
        )
        .expect("valid session URI");
        assert!(matches!(
            store.validate_filter(&filter).await,
            Err(MemoryError::Auth(_))
        ));

        store
            .db
            .query(
                "CREATE app_session SET tenant_id = 'tenant_a', app = 'graph', handle = 'session-1', absolute_expiry = time::now() + 1h",
                None,
            )
            .await
            .expect("seed session");
        store.validate_filter(&filter).await.expect("owned session");
    }
}
