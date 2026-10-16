//! Bounded, reusable provider transport. Errors never retain URLs or bodies.
use crate::error::MemoryError;
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct ProviderSettings {
    pub issuer: String,
    pub client_id: String,
    pub audience: String,
    pub redirect_uri: String,
    pub allowed_algorithm: String,
}
impl ProviderSettings {
    pub(crate) fn new(
        issuer: &str,
        client_id: &str,
        audience: &str,
        redirect_uri: &str,
        allowed_algorithm: &str,
    ) -> Result<Self, MemoryError> {
        for value in [issuer, redirect_uri] {
            validate_url(value).map_err(|_| {
                MemoryError::ConfigInvalid(
                    "OIDC URL must be an absolute HTTP(S) URL without credentials or fragment"
                        .into(),
                )
            })?;
        }
        if client_id.is_empty()
            || audience.is_empty()
            || (allowed_algorithm != "auto"
                && !super::SUPPORTED_ID_TOKEN_ALGORITHMS.contains(&allowed_algorithm))
        {
            return Err(MemoryError::ConfigInvalid(
                "invalid local OIDC settings".into(),
            ));
        }
        Ok(Self {
            issuer: issuer.into(),
            client_id: client_id.into(),
            audience: audience.into(),
            redirect_uri: redirect_uri.into(),
            allowed_algorithm: allowed_algorithm.into(),
        })
    }
}
pub(crate) fn validate_url(value: &str) -> Result<(), ProviderFailure> {
    let url = reqwest::Url::parse(value).map_err(|_| ProviderFailure::invalid())?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ProviderFailure::invalid());
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderFailureKind {
    Transport,
    Timeout,
    Http,
    InvalidResponse,
}
#[derive(Clone, Debug, thiserror::Error)]
#[error("OIDC provider failure: {kind:?}, status={status:?}")]
pub(crate) struct ProviderFailure {
    pub kind: ProviderFailureKind,
    pub status: Option<u16>,
    pub retry_after: Option<Duration>,
}
impl ProviderFailure {
    pub(crate) fn invalid() -> Self {
        Self {
            kind: ProviderFailureKind::InvalidResponse,
            status: None,
            retry_after: None,
        }
    }
    pub(crate) fn auth(&self) -> super::AuthError {
        super::AuthError::Unavailable {
            retry_after: super::retry::retry_delay(0, 0, self),
        }
    }
}
fn transport(error: reqwest::Error) -> ProviderFailure {
    ProviderFailure {
        kind: if error.is_timeout() {
            ProviderFailureKind::Timeout
        } else {
            ProviderFailureKind::Transport
        },
        status: None,
        retry_after: None,
    }
}
pub(crate) fn http_client() -> Result<reqwest::Client, MemoryError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| MemoryError::ConfigInvalid("OIDC HTTP client initialization failed".into()))
}
pub(crate) fn retry_after(value: &str, now: chrono::DateTime<chrono::Utc>) -> Option<Duration> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds.min(300)));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let delta = date.signed_duration_since(now).to_std().ok()?;
    (!delta.is_zero()).then_some(delta.min(Duration::from_secs(300)))
}
pub(crate) async fn response(
    request: reqwest::RequestBuilder,
) -> Result<(reqwest::StatusCode, Vec<u8>, Option<Duration>), ProviderFailure> {
    let mut response = request.send().await.map_err(transport)?;
    let status = response.status();
    let hint = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| retry_after(v, chrono::Utc::now()));
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        if chunk.len() > 262_144usize.saturating_sub(body.len()) {
            return Err(ProviderFailure::invalid());
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body, hint))
}
pub(crate) async fn json(
    request: reqwest::RequestBuilder,
) -> Result<serde_json::Value, ProviderFailure> {
    let (status, body, retry_after) = response(request).await?;
    if !status.is_success() {
        return Err(ProviderFailure {
            kind: ProviderFailureKind::Http,
            status: Some(status.as_u16()),
            retry_after,
        });
    }
    serde_json::from_slice(&body).map_err(|_| ProviderFailure::invalid())
}
