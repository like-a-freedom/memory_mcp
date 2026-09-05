//! [`OidcClient`] — discovery, authorization URL, code exchange,
//! and ID-token validation.

use crate::error::MemoryError;

use super::flow_material::{AccessClaims, AuthError, OidcNonce, OidcState, OidcTokens, PkceCode};
use super::jwks::JwksCache;

#[derive(Clone)]
pub struct OidcClient {
    issuer: String,
    client_id: String,
    audience: String,
    redirect_uri: String,
    allowed_algorithm: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks: JwksCache,
}

impl OidcClient {
    /// Perform OIDC discovery and initialize the JWKS cache.
    pub async fn new(
        issuer: &str,
        client_id: &str,
        audience: &str,
        redirect_uri: &str,
        allowed_algorithm: &str,
    ) -> Result<Self, MemoryError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| MemoryError::ConfigInvalid(e.to_string()))?;

        let issuer = issuer.trim_end_matches('/');
        let discovery_url = format!("{issuer}/.well-known/openid-configuration");
        let discovery: serde_json::Value = client
            .get(&discovery_url)
            .send()
            .await
            .map_err(|e| MemoryError::ConfigInvalid(format!("OIDC discovery failed: {e}")))?
            .error_for_status()
            .map_err(|e| MemoryError::ConfigInvalid(format!("OIDC discovery failed: {e}")))?
            .json()
            .await
            .map_err(|e| MemoryError::ConfigInvalid(format!("OIDC discovery parse failed: {e}")))?;

        if discovery.get("issuer").and_then(|value| value.as_str()) != Some(issuer) {
            return Err(MemoryError::ConfigInvalid(
                "OIDC discovery issuer does not match configured issuer".into(),
            ));
        }
        let authorization_endpoint = discovery
            .get("authorization_endpoint")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                MemoryError::ConfigInvalid("OIDC discovery missing authorization_endpoint".into())
            })?
            .to_string();
        let token_endpoint = discovery
            .get("token_endpoint")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                MemoryError::ConfigInvalid("OIDC discovery missing token_endpoint".into())
            })?
            .to_string();
        let jwks_uri = discovery
            .get("jwks_uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MemoryError::ConfigInvalid("OIDC discovery missing jwks_uri".into()))?
            .to_string();

        let jwks = JwksCache::from_parts(
            client.clone(),
            jwks_uri,
            std::time::Duration::from_secs(300),
        );

        Ok(Self {
            issuer: issuer.to_string(),
            client_id: client_id.to_string(),
            audience: audience.to_string(),
            redirect_uri: redirect_uri.to_string(),
            allowed_algorithm: allowed_algorithm.to_string(),
            authorization_endpoint,
            token_endpoint,
            jwks,
        })
    }

    /// Build the authorization URL with PKCE and nonce.
    pub fn authorize_url(
        &self,
        state: &OidcState,
        pkce: &PkceCode,
        nonce: &OidcNonce,
    ) -> Result<String, MemoryError> {
        use oauth2::basic::BasicClient;
        use oauth2::{
            AuthUrl, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
        };

        let auth_url = AuthUrl::new(self.authorization_endpoint.clone()).map_err(|error| {
            MemoryError::ConfigInvalid(format!("invalid OIDC authorization URL: {error}"))
        })?;
        let redirect_url = RedirectUrl::new(self.redirect_uri.clone()).map_err(|error| {
            MemoryError::ConfigInvalid(format!("invalid OIDC redirect URL: {error}"))
        })?;
        let client = BasicClient::new(ClientId::new(self.client_id.clone()))
            .set_auth_uri(auth_url)
            .set_redirect_uri(redirect_url);

        let pkce_challenge = PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(
            pkce.verifier.clone(),
        ));

        let (mut url, _csrf) = client
            .authorize_url(|| CsrfToken::new(state.as_str().to_string()))
            .add_scope(Scope::new("openid".to_string()))
            .set_pkce_challenge(pkce_challenge)
            .url();

        url.query_pairs_mut().append_pair("nonce", nonce.as_str());

        Ok(url.to_string())
    }

    /// Exchange an authorization code for tokens using raw reqwest.
    pub async fn exchange_code(
        &self,
        code: String,
        pkce: PkceCode,
    ) -> Result<OidcTokens, AuthError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| AuthError::Provider(e.to_string()))?;

        let params = [
            ("grant_type", "authorization_code".to_string()),
            ("code", code),
            ("redirect_uri", self.redirect_uri.clone()),
            ("client_id", self.client_id.clone()),
            ("code_verifier", pkce.verifier),
        ];

        let form_body = params
            .iter()
            .map(|(key, value)| {
                format!(
                    "{}={}",
                    form_urlencode_component(key),
                    form_urlencode_component(value),
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        let resp = client
            .post(&self.token_endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form_body)
            .send()
            .await
            .map_err(|e| AuthError::Provider(e.to_string()))?
            .error_for_status()
            .map_err(|e| AuthError::Provider(e.to_string()))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AuthError::Provider(e.to_string()))?;

        let id_token = body
            .get("id_token")
            .and_then(|v| v.as_str())
            .ok_or(AuthError::MalformedToken)?
            .to_string();

        Ok(OidcTokens { id_token })
    }

    /// Validate an ID token: issuer, audience, algorithm, expiry, JWKS signature.
    pub async fn validate_id_token(&self, token: &str) -> Result<AccessClaims, AuthError> {
        use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};

        let header = jsonwebtoken::decode_header(token).map_err(AuthError::Jwt)?;

        let kid = header.kid.ok_or(AuthError::MissingKeyId)?;

        let alg = match header.alg {
            Algorithm::RS256 => "RS256",
            Algorithm::ES256 => "ES256",
            Algorithm::EdDSA => "EdDSA",
            _ => return Err(AuthError::DisallowedAlgorithm),
        };

        if alg != self.allowed_algorithm {
            return Err(AuthError::DisallowedAlgorithm);
        }

        let key: DecodingKey = self.jwks.key_for(&kid).await?;

        let validation_algorithm = match alg {
            "RS256" => Algorithm::RS256,
            "ES256" => Algorithm::ES256,
            "EdDSA" => Algorithm::EdDSA,
            _ => return Err(AuthError::DisallowedAlgorithm),
        };
        let mut validation = Validation::new(validation_algorithm);
        validation.set_audience(&[&self.audience]);
        validation.set_issuer(&[&self.issuer]);
        validation.validate_exp = true;

        let token_data =
            decode::<AccessClaims>(token, &key, &validation).map_err(AuthError::Jwt)?;

        Ok(token_data.claims)
    }
}

fn form_urlencode_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push('+'),
            other => {
                encoded.push('%');
                encoded.push(HEX[(other >> 4) as usize] as char);
                encoded.push(HEX[(other & 0x0F) as usize] as char);
            }
        }
    }
    encoded
}
