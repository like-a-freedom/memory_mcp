//! API client for the control-plane backend.
//!
//! API-key secrets live in page memory only; never `localStorage`,
//! never URL. `Cache-Control: no-store` is honored by the API.

use serde::{Deserialize, Serialize};

/// Error type for API calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub message: String,
    pub status: u16,
}

/// Account metadata from GET /api/v1/account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountMeta {
    pub id: String,
    pub status: String,
    pub tenant_id: String,
    pub created_at: String,
}

/// API key metadata (without secret).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyMeta {
    pub id: String,
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
}

/// Response from POST /api/v1/account/api_keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApiKeyResponse {
    pub id: String,
    pub secret: String,
    pub name: String,
    pub expires_at: Option<String>,
}

/// Deletion challenge from POST /api/v1/account/delete. The token is held in
/// page memory only and is never put in a URL or browser storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteChallenge {
    pub confirmation_token: String,
    pub typed_phrase: String,
    pub export_available: bool,
    pub recovery_available: bool,
    pub expires_at: String,
}

/// API client for the control-plane backend.
#[derive(Clone)]
pub struct ApiClient {
    base: String,
}

impl ApiClient {
    pub fn new(base: String) -> Self {
        Self { base }
    }

    /// GET /api/v1/account — read account metadata.
    pub async fn me(&self) -> Result<AccountMeta, ApiError> {
        #[derive(Deserialize)]
        struct AccountResponse {
            account: AccountMeta,
        }
        let resp = gloo_net::http::Request::get(&format!("{}/api/v1/account", self.base))
            .send()
            .await
            .map_err(|e| ApiError {
                message: e.to_string(),
                status: 0,
            })?;
        resp.json::<AccountResponse>()
            .await
            .map(|body| body.account)
            .map_err(|e| ApiError {
                message: e.to_string(),
                status: resp.status(),
            })
    }

    async fn csrf(&self) -> Result<String, ApiError> {
        #[derive(Deserialize)]
        struct CsrfResponse {
            csrf_token: String,
        }
        let resp = gloo_net::http::Request::get(&format!("{}/api/v1/account/csrf", self.base))
            .send()
            .await
            .map_err(|e| ApiError {
                message: e.to_string(),
                status: 0,
            })?;
        resp.json::<CsrfResponse>()
            .await
            .map(|body| body.csrf_token)
            .map_err(|e| ApiError {
                message: e.to_string(),
                status: resp.status(),
            })
    }

    /// GET /api/v1/account/api_keys — list API keys.
    pub async fn list_keys(&self) -> Result<Vec<ApiKeyMeta>, ApiError> {
        let resp = gloo_net::http::Request::get(&format!("{}/api/v1/account/api_keys", self.base))
            .send()
            .await
            .map_err(|e| ApiError {
                message: e.to_string(),
                status: 0,
            })?;
        resp.json().await.map_err(|e| ApiError {
            message: e.to_string(),
            status: resp.status(),
        })
    }

    /// POST /api/v1/account/api_keys — create a new API key.
    pub async fn create_key(&self, name: String) -> Result<CreateApiKeyResponse, ApiError> {
        let csrf = self.csrf().await?;
        let resp = gloo_net::http::Request::post(&format!("{}/api/v1/account/api_keys", self.base))
            .header("X-CSRF-Token", &csrf)
            .json(&serde_json::json!({ "name": name }))
            .map_err(|e| ApiError {
                message: e.to_string(),
                status: 0,
            })?
            .send()
            .await
            .map_err(|e| ApiError {
                message: e.to_string(),
                status: 0,
            })?;
        resp.json().await.map_err(|e| ApiError {
            message: e.to_string(),
            status: resp.status(),
        })
    }

    /// DELETE /api/v1/account/api_keys/:id — revoke an API key.
    pub async fn revoke_key(&self, id: String) -> Result<(), ApiError> {
        let csrf = self.csrf().await?;
        let resp = gloo_net::http::Request::delete(&format!(
            "{}/api/v1/account/api_keys/{}",
            self.base, id
        ))
        .header("X-CSRF-Token", &csrf)
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
            status: 0,
        })?;
        if resp.ok() {
            Ok(())
        } else {
            Err(ApiError {
                message: "revoke failed".into(),
                status: resp.status(),
            })
        }
    }

    /// POST /api/v1/account/delete — start deletion flow.
    pub async fn start_delete(&self) -> Result<DeleteChallenge, ApiError> {
        let csrf = self.csrf().await?;
        let resp = gloo_net::http::Request::post(&format!("{}/api/v1/account/delete", self.base))
            .header("X-CSRF-Token", &csrf)
            .send()
            .await
            .map_err(|e| ApiError {
                message: e.to_string(),
                status: 0,
            })?;
        resp.json().await.map_err(|e| ApiError {
            message: e.to_string(),
            status: resp.status(),
        })
    }

    /// POST /api/v1/account/delete/confirm — confirm deletion.
    pub async fn confirm_delete(
        &self,
        confirmation_token: String,
        phrase: String,
    ) -> Result<(), ApiError> {
        let csrf = self.csrf().await?;
        let resp =
            gloo_net::http::Request::post(&format!("{}/api/v1/account/delete/confirm", self.base))
                .header("X-CSRF-Token", &csrf)
                .json(&serde_json::json!({ "confirmation_token": confirmation_token, "typed_phrase": phrase }))
                .map_err(|e| ApiError {
                    message: e.to_string(),
                    status: 0,
                })?
                .send()
                .await
                .map_err(|e| ApiError {
                    message: e.to_string(),
                    status: 0,
                })?;
        if resp.ok() {
            Ok(())
        } else {
            Err(ApiError {
                message: "confirm failed".into(),
                status: resp.status(),
            })
        }
    }
}
