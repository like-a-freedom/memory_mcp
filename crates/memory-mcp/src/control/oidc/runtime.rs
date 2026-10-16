//! Configured OIDC method: discovery is owned background work, never startup I/O.
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
    settings: Option<ProviderSettings>,
    http: reqwest::Client,
    clock: Arc<dyn OidcClock>,
    state: RwLock<RuntimeState>,
}
struct RuntimeState {
    client: Option<Arc<OidcClient>>,
    retry_at: Instant,
    invalid_response: bool,
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
            settings: Some(settings),
            http: super::provider::http_client()?,
            clock: Arc::new(SystemOidcClock),
            state: RwLock::new(RuntimeState {
                client: None,
                retry_at: Instant::now(),
                invalid_response: false,
            }),
        }))
    }
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn ready(client: Arc<OidcClient>) -> Arc<Self> {
        Arc::new(Self {
            settings: None,
            http: reqwest::Client::new(),
            clock: Arc::new(SystemOidcClock),
            state: RwLock::new(RuntimeState {
                client: Some(client),
                retry_at: Instant::now(),
                invalid_response: false,
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
    pub(crate) async fn run(self: Arc<Self>, cancel: CancellationToken) {
        let Some(settings) = &self.settings else {
            return;
        };
        let mut attempt = 0;
        loop {
            let result = tokio::select! {biased; _=cancel.cancelled()=>return,result=OidcClient::discover(settings,&self.http)=>result};
            match result {
                Ok(client) => {
                    let client = Arc::new(client);
                    if let Ok(mut state) = self.state.write() {
                        state.client = Some(client.clone());
                        state.invalid_response = false;
                    } else {
                        return;
                    }
                    client.jwks().maintain(cancel).await;
                    return;
                }
                Err(failure) => {
                    let delay = super::retry::retry_delay(attempt, rand::random(), &failure);
                    attempt = attempt.saturating_add(1);
                    let deadline = self.clock.now() + delay;
                    if let Ok(mut state) = self.state.write() {
                        state.retry_at = deadline;
                        state.invalid_response =
                            failure.kind == ProviderFailureKind::InvalidResponse;
                    } else {
                        return;
                    }
                    tokio::select! {biased; _=cancel.cancelled()=>return,_=self.clock.sleep_until(deadline)=>{}}
                }
            }
        }
    }
}
