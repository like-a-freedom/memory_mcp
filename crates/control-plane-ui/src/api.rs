//! API client for the control-plane backend.
//!
//! API-key secrets live in page memory only; never `localStorage`,
//! never URL. `Cache-Control: no-store` is honored by the API.

use serde::{Deserialize, Serialize};

/// Error type for API calls.
///
/// `message` is always copy written for the operator. It is never a raw browser
/// exception (`TypeError: Failed to fetch`), a serde decode error, or any other
/// implementation detail — the pages render this string verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub message: String,
    pub status: u16,
}

impl ApiError {
    /// The request never reached the server.
    ///
    /// `status: 0` is the marker: no HTTP status exists because no response
    /// arrived. A rejected `fetch` is indistinguishable from a dead network at
    /// this layer, so the copy covers both.
    fn transport() -> Self {
        Self {
            message: "The console could not reach the server. Check your connection and try again."
                .to_owned(),
            status: 0,
        }
    }

    /// The server answered, but not with a body this console could read.
    fn unreadable(status: u16) -> Self {
        Self {
            message: "The server sent a response this console could not read. Try again."
                .to_owned(),
            status,
        }
    }
}

/// The backend's error envelope: `{"error":{"code":"…","message":"…"}}`.
///
/// Returns `None` when the body is not that shape, so the caller falls back to
/// status-based copy rather than showing the raw body.
fn server_message(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let message = value.get("error")?.get("message")?.as_str()?.trim();
    (!message.is_empty()).then(|| message.to_owned())
}

/// Copy for a status the server did not explain.
fn fallback_message(status: u16) -> String {
    match status {
        401 | 403 => {
            "You are not signed in, or your session has expired. Sign in again.".to_owned()
        }
        404 => "That no longer exists. Reload the page.".to_owned(),
        409 => "That change conflicts with the current state. Reload and try again.".to_owned(),
        429 => "Too many requests. Wait a moment and try again.".to_owned(),
        500..=599 => "The server could not complete the request. Try again.".to_owned(),
        _ => "The request was refused.".to_owned(),
    }
}

/// Turn a non-success response into an [`ApiError`] the page can render.
async fn read_failure(resp: gloo_net::http::Response) -> ApiError {
    let status = resp.status();
    let message = resp
        .text()
        .await
        .ok()
        .and_then(|body| server_message(&body))
        .unwrap_or_else(|| fallback_message(status));
    ApiError { message, status }
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

    /// Join `base` and `path` into one same-origin request path.
    ///
    /// Callers pass `/` (the bundle is served from the root), which has to join
    /// as `/api/v1/account` and never as `//api/v1/account`. A leading `//` is
    /// not an absolute path: the URL spec reads it as a *scheme-relative*
    /// reference, so the browser resolves `//api/v1/account` to
    /// `scheme://api/v1/account`, looks up a host literally named `api`, and
    /// rejects the request with `TypeError: Failed to fetch` before any byte
    /// reaches the server. That failure is invisible to a server-side test —
    /// the request is simply never sent — so the join lives in one place and is
    /// pinned from both sides by `mod tests`.
    fn endpoint(&self, path: &str) -> String {
        let base = self.base.trim_end_matches('/');
        format!("{base}/{}", path.trim_start_matches('/'))
    }

    /// GET /api/v1/account — read account metadata.
    pub async fn me(&self) -> Result<AccountMeta, ApiError> {
        #[derive(Deserialize)]
        struct AccountResponse {
            account: AccountMeta,
        }
        let resp = gloo_net::http::Request::get(&self.endpoint("/api/v1/account"))
            .send()
            .await
            .map_err(|_| ApiError::transport())?;
        if !resp.ok() {
            return Err(read_failure(resp).await);
        }
        resp.json::<AccountResponse>()
            .await
            .map(|body| body.account)
            .map_err(|_| ApiError::unreadable(resp.status()))
    }

    async fn csrf(&self) -> Result<String, ApiError> {
        #[derive(Deserialize)]
        struct CsrfResponse {
            csrf_token: String,
        }
        let resp = gloo_net::http::Request::get(&self.endpoint("/api/v1/account/csrf"))
            .send()
            .await
            .map_err(|_| ApiError::transport())?;
        if !resp.ok() {
            return Err(read_failure(resp).await);
        }
        resp.json::<CsrfResponse>()
            .await
            .map(|body| body.csrf_token)
            .map_err(|_| ApiError::unreadable(resp.status()))
    }

    /// GET /api/v1/account/api_keys — list API keys.
    pub async fn list_keys(&self) -> Result<Vec<ApiKeyMeta>, ApiError> {
        let resp = gloo_net::http::Request::get(&self.endpoint("/api/v1/account/api_keys"))
            .send()
            .await
            .map_err(|_| ApiError::transport())?;
        if !resp.ok() {
            return Err(read_failure(resp).await);
        }
        resp.json()
            .await
            .map_err(|_| ApiError::unreadable(resp.status()))
    }

    /// POST /api/v1/account/api_keys — create a new API key.
    pub async fn create_key(&self, name: String) -> Result<CreateApiKeyResponse, ApiError> {
        let csrf = self.csrf().await?;
        let resp = gloo_net::http::Request::post(&self.endpoint("/api/v1/account/api_keys"))
            .header("X-CSRF-Token", &csrf)
            .json(&serde_json::json!({ "name": name }))
            .map_err(|_| ApiError::transport())?
            .send()
            .await
            .map_err(|_| ApiError::transport())?;
        if !resp.ok() {
            return Err(read_failure(resp).await);
        }
        resp.json()
            .await
            .map_err(|_| ApiError::unreadable(resp.status()))
    }

    /// DELETE /api/v1/account/api_keys/:id — revoke an API key.
    pub async fn revoke_key(&self, id: String) -> Result<(), ApiError> {
        let csrf = self.csrf().await?;
        let resp = gloo_net::http::Request::delete(
            &self.endpoint(&format!("/api/v1/account/api_keys/{id}")),
        )
        .header("X-CSRF-Token", &csrf)
        .send()
        .await
        .map_err(|_| ApiError::transport())?;
        if resp.ok() {
            Ok(())
        } else {
            Err(read_failure(resp).await)
        }
    }

    /// POST /auth/oidc/logout — revoke the current browser session.
    pub async fn logout(&self) -> Result<(), ApiError> {
        let csrf = self.csrf().await?;
        let resp = gloo_net::http::Request::post(&self.endpoint("/auth/oidc/logout"))
            .header("X-CSRF-Token", &csrf)
            .send()
            .await
            .map_err(|_| ApiError::transport())?;
        if resp.ok() {
            Ok(())
        } else {
            Err(read_failure(resp).await)
        }
    }

    /// POST /api/v1/account/delete — start deletion flow.
    pub async fn start_delete(&self) -> Result<DeleteChallenge, ApiError> {
        let csrf = self.csrf().await?;
        let resp = gloo_net::http::Request::post(&self.endpoint("/api/v1/account/delete"))
            .header("X-CSRF-Token", &csrf)
            .send()
            .await
            .map_err(|_| ApiError::transport())?;
        if !resp.ok() {
            return Err(read_failure(resp).await);
        }
        resp.json()
            .await
            .map_err(|_| ApiError::unreadable(resp.status()))
    }

    /// POST /api/v1/account/delete/confirm — confirm deletion.
    pub async fn confirm_delete(
        &self,
        confirmation_token: String,
        phrase: String,
    ) -> Result<(), ApiError> {
        let csrf = self.csrf().await?;
        let resp =
            gloo_net::http::Request::post(&self.endpoint("/api/v1/account/delete/confirm"))
                .header("X-CSRF-Token", &csrf)
                .json(&serde_json::json!({ "confirmation_token": confirmation_token, "typed_phrase": phrase }))
                .map_err(|_| ApiError::transport())?
                .send()
                .await
                .map_err(|_| ApiError::transport())?;
        if resp.ok() {
            Ok(())
        } else {
            Err(read_failure(resp).await)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiClient, fallback_message, server_message};

    /// The regression that motivated `endpoint`. `format!("{}/api/v1/account", "/")`
    /// is `//api/v1/account`, which is a scheme-relative URL, so the browser
    /// resolved it to `scheme://api/v1/account` and every account-surface
    /// request was rejected with `TypeError: Failed to fetch` before it left the
    /// page. No server-side test could see it: nothing was ever sent.
    #[test]
    fn a_root_base_joins_without_doubling_the_slash() {
        let client = ApiClient::new("/".to_owned());
        assert_eq!(client.endpoint("/api/v1/account"), "/api/v1/account");
        assert_eq!(client.endpoint("api/v1/account"), "/api/v1/account");
        assert!(
            !client.endpoint("/api/v1/account").starts_with("//"),
            "a double slash is a scheme-relative URL, not an absolute path"
        );
    }

    #[test]
    fn an_empty_base_and_a_root_base_agree() {
        let empty = ApiClient::new(String::new());
        let root = ApiClient::new("/".to_owned());
        for path in [
            "/api/v1/account",
            "/auth/oidc/logout",
            "/api/v1/account/delete",
        ] {
            assert_eq!(empty.endpoint(path), root.endpoint(path));
        }
    }

    #[test]
    fn an_absolute_base_is_preserved() {
        let client = ApiClient::new("https://console.example".to_owned());
        assert_eq!(
            client.endpoint("/api/v1/account"),
            "https://console.example/api/v1/account"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_base_does_not_double() {
        let client = ApiClient::new("https://console.example/".to_owned());
        assert_eq!(
            client.endpoint("/api/v1/account"),
            "https://console.example/api/v1/account"
        );
    }

    #[test]
    fn every_request_path_is_absolute_and_same_origin() {
        // The paths the client actually builds. None may be scheme-relative.
        let client = ApiClient::new("/".to_owned());
        for path in [
            "/api/v1/account",
            "/api/v1/account/csrf",
            "/api/v1/account/api_keys",
            "/api/v1/account/api_keys/key:1",
            "/api/v1/account/delete",
            "/api/v1/account/delete/confirm",
            "/auth/oidc/logout",
        ] {
            let url = client.endpoint(path);
            assert!(url.starts_with('/'), "{url} is not same-origin");
            assert!(!url.starts_with("//"), "{url} is scheme-relative");
            assert!(!url.contains("//"), "{url} contains a doubled slash");
        }
    }

    #[test]
    fn the_server_error_envelope_is_decoded() {
        assert_eq!(
            server_message(r#"{"error":{"code":"not_found","message":"not found"}}"#),
            Some("not found".to_owned())
        );
        assert_eq!(
            server_message(r#"{"error":{"code":"conflict","message":"  spaced  "}}"#),
            Some("spaced".to_owned())
        );
    }

    #[test]
    fn a_body_without_the_envelope_is_not_shown_to_the_operator() {
        // Anything that is not the documented envelope must fall back to
        // status-based copy rather than surface the raw body.
        assert_eq!(server_message("not found"), None);
        assert_eq!(server_message("<html>nope</html>"), None);
        assert_eq!(server_message(r#"{"error":{"message":"   "}}"#), None);
        assert_eq!(server_message(r#"{"error":"a string"}"#), None);
        assert_eq!(server_message(""), None);
    }

    #[test]
    fn fallback_copy_names_the_status_class_and_never_leaks_internals() {
        for status in [401, 403] {
            assert!(fallback_message(status).contains("Sign in again"));
        }
        assert!(fallback_message(404).contains("no longer exists"));
        assert!(fallback_message(409).contains("Reload"));
        assert!(fallback_message(429).contains("Wait"));
        assert!(fallback_message(500).contains("server"));
        assert!(fallback_message(503).contains("server"));

        for status in [400, 401, 403, 404, 409, 429, 500, 503] {
            let message = fallback_message(status);
            assert!(!message.is_empty(), "status {status} has no copy");
            assert!(
                !message.contains("TypeError") && !message.contains("missing field"),
                "status {status} leaks an implementation detail: {message}"
            );
        }
    }
}
