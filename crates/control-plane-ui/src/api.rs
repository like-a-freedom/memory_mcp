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

/// Deletion challenge from POST /api/v1/account/delete.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteChallenge {
    pub message: String,
    pub typed_phrase: String,
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
        let resp = gloo_net::http::Request::get(&format!("{}/api/v1/account", self.base))
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
        let resp = gloo_net::http::Request::post(&format!("{}/api/v1/account/api_keys", self.base))
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
        let resp = gloo_net::http::Request::delete(&format!(
            "{}/api/v1/account/api_keys/{}",
            self.base, id
        ))
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
        let resp = gloo_net::http::Request::post(&format!("{}/api/v1/account/delete", self.base))
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
    pub async fn confirm_delete(&self, phrase: String) -> Result<(), ApiError> {
        let resp =
            gloo_net::http::Request::post(&format!("{}/api/v1/account/delete/confirm", self.base))
                .json(&serde_json::json!({ "typed_phrase": phrase }))
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
