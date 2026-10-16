use super::diagnostics::{
    MetadataStatus, OidcHealth, ProviderComponent, ProviderDiagnostics, ProviderState,
};
use super::{
    OidcClient,
    provider::{ProviderFailureKind, ProviderSettings},
    retry::{OidcClock, SystemOidcClock},
};
use crate::{error::MemoryError, http::config::OidcBrowserConfig};
use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, thiserror::Error)]
#[error("OIDC metadata temporarily unavailable")]
pub struct OidcUnavailable {
    pub retry_after: Duration,
}
pub struct OidcRuntime {
    diagnostics: ProviderDiagnostics,
    settings: Option<ProviderSettings>,
    http: reqwest::Client,
    clock: Arc<dyn OidcClock>,
    state: RwLock<RuntimeState>,
}
struct RuntimeState {
    client: Option<Arc<OidcClient>>,
    retry_at: Instant,
    metadata: MetadataStatus,
}
impl OidcRuntime {
    pub fn new(config: &OidcBrowserConfig) -> Result<Arc<Self>, MemoryError> {
        let settings = ProviderSettings::new(
            &config.issuer,
            &config.client_id,
            &config.audience,
            &config.redirect_uri,
            &config.allowed_alg,
        )?;
        Ok(Arc::new(Self {
            diagnostics: ProviderDiagnostics::default(),
            settings: Some(settings),
            http: super::provider::http_client()?,
            clock: Arc::new(SystemOidcClock),
            state: RwLock::new(RuntimeState {
                client: None,
                retry_at: Instant::now(),
                metadata: MetadataStatus::Initializing,
            }),
        }))
    }
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn ready(client: Arc<OidcClient>) -> Arc<Self> {
        Arc::new(Self {
            diagnostics: ProviderDiagnostics::default(),
            settings: None,
            http: reqwest::Client::new(),
            clock: Arc::new(SystemOidcClock),
            state: RwLock::new(RuntimeState {
                client: Some(client),
                retry_at: Instant::now(),
                metadata: MetadataStatus::Initializing,
            }),
        })
    }
    pub fn client(&self) -> Result<Arc<OidcClient>, OidcUnavailable> {
        let state = self.state.read().map_err(|_| OidcUnavailable {
            retry_after: Duration::from_secs(1),
        })?;
        state.client.clone().ok_or_else(|| OidcUnavailable {
            retry_after: state
                .retry_at
                .saturating_duration_since(self.clock.now())
                .max(Duration::from_secs(1)),
        })
    }
    pub fn health(&self) -> OidcHealth {
        match self.state.read() {
            Ok(state) => match &state.client {
                Some(client) => OidcHealth {
                    enabled: true,
                    metadata: MetadataStatus::Ready,
                    jwks: client.jwks().status(),
                },
                None => OidcHealth {
                    enabled: true,
                    metadata: state.metadata,
                    jwks: super::JwksStatus::Empty,
                },
            },
            Err(_) => OidcHealth {
                enabled: true,
                metadata: MetadataStatus::Backoff,
                jwks: super::JwksStatus::Empty,
            },
        }
    }
    pub(crate) async fn run(self: Arc<Self>, cancel: CancellationToken) {
        let Some(settings) = &self.settings else {
            return;
        };
        self.diagnostics.ready(ProviderComponent::Metadata, false);
        self.diagnostics.ready(ProviderComponent::Jwks, false);
        self.diagnostics.transition(
            ProviderComponent::Metadata,
            ProviderState::Initializing,
            "startup",
            None,
            Duration::ZERO,
        );
        let mut attempt = 0;
        loop {
            let result = tokio::select! {biased; _=cancel.cancelled()=>return,result=OidcClient::discover(settings,&self.http)=>result};
            match result {
                Ok(client) => {
                    let client = Arc::new(client);
                    if let Ok(mut state) = self.state.write() {
                        state.client = Some(client.clone());
                        state.metadata = MetadataStatus::Ready;
                    } else {
                        return;
                    }
                    self.diagnostics.ready(ProviderComponent::Metadata, true);
                    self.diagnostics.transition(
                        ProviderComponent::Metadata,
                        ProviderState::Ready,
                        "success",
                        None,
                        Duration::ZERO,
                    );
                    client.jwks().maintain(cancel).await;
                    return;
                }
                Err(failure) => {
                    let delay = super::retry::retry_delay(attempt, rand::random(), &failure);
                    attempt = attempt.saturating_add(1);
                    self.diagnostics.transition(
                        ProviderComponent::Metadata,
                        if failure.kind == ProviderFailureKind::InvalidResponse {
                            ProviderState::InvalidResponse
                        } else {
                            ProviderState::Backoff
                        },
                        failure.reason(),
                        failure.status,
                        delay,
                    );
                    let deadline = self.clock.now() + delay;
                    if let Ok(mut state) = self.state.write() {
                        state.retry_at = deadline;
                        state.metadata = if failure.kind == ProviderFailureKind::InvalidResponse {
                            MetadataStatus::InvalidResponse
                        } else {
                            MetadataStatus::Backoff
                        };
                    } else {
                        return;
                    }
                    tokio::select! {biased; _=cancel.cancelled()=>return,_=self.clock.sleep_until(deadline)=>{}}
                }
            }
        }
    }
}
