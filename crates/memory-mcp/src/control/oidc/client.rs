//! [`OidcClient`] — discovery, authorization URL, code exchange,
//! and ID-token validation.

use crate::error::MemoryError;

use super::flow_material::{AccessClaims, AuthError, OidcNonce, OidcState, OidcTokens, PkceCode};
use super::jwks::JwksCache;

#[derive(Clone)]
pub struct OidcClient {
    /// The issuer exactly as the provider publishes it in discovery — not the
    /// configured spelling. `jsonwebtoken` compares the `iss` claim exactly,
    /// so the published form is authoritative (see `resolve_published_issuer`).
    issuer: String,
    client_id: String,
    audience: String,
    redirect_uri: String,
    /// The algorithms accepted for ID tokens: an explicit
    /// `MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG` is a single pin; `auto` is the safe
    /// set intersected with the provider's advertisement at discovery.
    allowed_algorithms: Vec<&'static str>,
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

        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            normalize_issuer(issuer)
        );
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

        let issuer = resolve_published_issuer(issuer, &discovery)?;
        let allowed_algorithms = resolve_allowed_algorithms(allowed_algorithm, &discovery)?;
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
            issuer,
            client_id: client_id.to_string(),
            audience: audience.to_string(),
            redirect_uri: redirect_uri.to_string(),
            allowed_algorithms,
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
            Algorithm::RS384 => "RS384",
            Algorithm::RS512 => "RS512",
            Algorithm::ES256 => "ES256",
            Algorithm::EdDSA => "EdDSA",
            _ => return Err(AuthError::DisallowedAlgorithm),
        };

        if !self.allowed_algorithms.contains(&alg) {
            return Err(AuthError::DisallowedAlgorithm);
        }

        let key: DecodingKey = self.jwks.key_for(&kid).await?;

        // Keep in lockstep with the allowlist in `http::config::validate`.
        let validation_algorithm = match alg {
            "RS256" => Algorithm::RS256,
            "RS384" => Algorithm::RS384,
            "RS512" => Algorithm::RS512,
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

/// Normalize an issuer identifier for comparison.
///
/// Providers disagree about a trailing slash on the issuer path — Rauthy >=
/// 0.35, for example, always publishes `https://host/auth/v1/` and cannot be
/// configured otherwise — so every config-vs-provider issuer comparison
/// normalizes both sides. Validation of tokens uses the provider's published
/// form verbatim instead.
pub(super) fn normalize_issuer(issuer: &str) -> &str {
    issuer.trim_end_matches('/')
}

/// Compare two issuer identifiers modulo a trailing slash.
pub(super) fn issuers_match(left: &str, right: &str) -> bool {
    normalize_issuer(left) == normalize_issuer(right)
}

/// Check a discovery document's `issuer` against the configured one and
/// return the *published* form. The published form is authoritative for
/// ID-token `iss` validation (`validate_id_token`) and for identity keying,
/// because `jsonwebtoken` compares `iss` exactly as published.
fn resolve_published_issuer(
    configured_issuer: &str,
    discovery: &serde_json::Value,
) -> Result<String, MemoryError> {
    let published = discovery
        .get("issuer")
        .and_then(|value| value.as_str())
        .ok_or_else(|| MemoryError::ConfigInvalid("OIDC discovery missing issuer".into()))?;
    if !issuers_match(published, configured_issuer) {
        return Err(MemoryError::ConfigInvalid(
            "OIDC discovery issuer does not match configured issuer".into(),
        ));
    }
    Ok(published.to_string())
}

/// Resolve the accepted ID-token algorithms. An explicit
/// `MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG` stays a single pin; `auto` (the
/// default) intersects the provider's advertised set (discovery
/// `id_token_signing_alg_values_supported`) with the safe set below, and a
/// provider that advertises nothing usable is a startup error rather than a
/// silent widening.
fn resolve_allowed_algorithms(
    configured: &str,
    discovery: &serde_json::Value,
) -> Result<Vec<&'static str>, MemoryError> {
    const SUPPORTED: [&str; 5] = ["RS256", "RS384", "RS512", "ES256", "EdDSA"];
    if configured != crate::http::config::AUTO_OIDC_ALG {
        let pinned = SUPPORTED
            .iter()
            .copied()
            .find(|name| *name == configured)
            .ok_or_else(|| {
                MemoryError::ConfigInvalid(format!("OIDC allowed algorithm: {configured}"))
            })?;
        return Ok(vec![pinned]);
    }
    let Some(advertised) = discovery
        .get("id_token_signing_alg_values_supported")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(SUPPORTED.to_vec());
    };
    let resolved: Vec<&'static str> = SUPPORTED
        .iter()
        .copied()
        .filter(|name| advertised.iter().any(|value| value.as_str() == Some(name)))
        .collect();
    if resolved.is_empty() {
        return Err(MemoryError::ConfigInvalid(
            "OIDC provider advertises no supported ID-token signing algorithm".into(),
        ));
    }
    Ok(resolved)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn discovery_with_issuer(issuer: &str) -> serde_json::Value {
        serde_json::json!({ "issuer": issuer })
    }

    /// Rauthy >= 0.35 publishes its issuer with a trailing slash and refuses
    /// to drop it, so a bare configuration must still pass — and the published
    /// slashed form is what later `iss` validation compares against.
    #[test]
    fn a_published_trailing_slash_matches_a_bare_configuration() {
        let published = resolve_published_issuer(
            "https://idp.example.com/auth/v1",
            &discovery_with_issuer("https://idp.example.com/auth/v1/"),
        )
        .expect("slash-insensitive comparison");
        assert_eq!(published, "https://idp.example.com/auth/v1/");
    }

    /// The reverse spelling (the operator copies the published form including
    /// the slash) must pass too, and the published form stays authoritative.
    #[test]
    fn a_configured_trailing_slash_matches_a_bare_published_issuer() {
        let published = resolve_published_issuer(
            "https://idp.example.com/auth/v1/",
            &discovery_with_issuer("https://idp.example.com/auth/v1"),
        )
        .expect("slash-insensitive comparison");
        assert_eq!(published, "https://idp.example.com/auth/v1");
    }

    #[test]
    fn a_different_issuer_is_rejected() {
        let result = resolve_published_issuer(
            "https://idp.example.com/auth/v1",
            &discovery_with_issuer("https://other.example.com/auth/v1"),
        );
        assert!(matches!(result, Err(MemoryError::ConfigInvalid(_))));
    }

    #[test]
    fn a_discovery_document_without_an_issuer_is_rejected() {
        let result = resolve_published_issuer("https://idp.example.com", &serde_json::json!({}));
        assert!(matches!(result, Err(MemoryError::ConfigInvalid(_))));
    }

    /// The RFC 9207 authorization-response `iss` is compared against the
    /// configured issuer with the same normalization.
    #[test]
    fn issuer_comparison_normalizes_trailing_slashes() {
        assert!(issuers_match(
            "https://idp.example.com/",
            "https://idp.example.com"
        ));
        assert!(issuers_match(
            "https://idp.example.com",
            "https://idp.example.com/"
        ));
        assert!(!issuers_match(
            "https://idp.example.com/",
            "https://other.example.com/"
        ));
    }

    /// `auto` intersects the provider's advertised algorithms with the safe
    /// set — the shape Rauthy publishes (`RS256, RS384, RS512, EdDSA`) is
    /// accepted whole, anything outside the safe set is dropped.
    #[test]
    fn auto_algorithms_intersect_the_advertisement_with_the_safe_set() {
        let advertised = serde_json::json!({"id_token_signing_alg_values_supported": ["RS256", "RS384", "RS512", "EdDSA"]});
        assert_eq!(
            resolve_allowed_algorithms("auto", &advertised).expect("advertised"),
            vec!["RS256", "RS384", "RS512", "EdDSA"]
        );
        let mixed =
            serde_json::json!({"id_token_signing_alg_values_supported": ["HS256", "RS256"]});
        assert_eq!(
            resolve_allowed_algorithms("auto", &mixed).expect("mixed"),
            vec!["RS256"]
        );
    }

    /// A provider that does not advertise its algorithms gets the safe set.
    #[test]
    fn auto_algorithms_fall_back_to_the_safe_set_when_the_provider_is_silent() {
        assert_eq!(
            resolve_allowed_algorithms("auto", &serde_json::json!({})).expect("silent"),
            vec!["RS256", "RS384", "RS512", "ES256", "EdDSA"]
        );
    }

    #[test]
    fn auto_algorithms_fail_startup_when_nothing_safe_is_advertised() {
        let unsafe_only =
            serde_json::json!({"id_token_signing_alg_values_supported": ["HS256", "PS256"]});
        assert!(matches!(
            resolve_allowed_algorithms("auto", &unsafe_only),
            Err(MemoryError::ConfigInvalid(_))
        ));
    }

    /// `jsonwebtoken` 11 verifies signatures only when the build enables
    /// exactly one crypto-provider feature (`rust_crypto` or `aws_lc_rs`);
    /// with neither, `decode` panics instead of returning — under the release
    /// profile's `panic = "abort"` that killed the whole process on the first
    /// OIDC callback. This pins the wiring: a valid RS256 token fed through
    /// the production JWKS path (`from_rsa_components`) must verify.
    #[test]
    fn signature_verification_decodes_a_valid_rs256_token() {
        use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};

        const N: &str = "jFDolAoizo_h2RyMsAYYCP0yJsg_oEZNlRDkY41tS23Ns0KH9xo0S2x26YSOzro5PjByxjuxPUczAKfjT5a8fCnqzGWZzetcpunRTD1GNqa0yoojJlWpIMSQsPBwkpPdZxlzokcKpWZT_zuSSQPitxW1JQeLs6mXdkEduHv5xnxuSs6YA4usiWjqCAprMf7QGl_y_NIjmZ3fOzqRfo8sZmV-5C9lrKtVnk28DtuFS6tjuZgNULCRWSQtek0JVfKs0D2_yrfyBoUKTDGggMAN3ytVokl3xwA2U9CCiIP2iNQoR-6lU0c78IPWksBWTWMt0LMtNZtu412Ur1c4EaS71w";
        const E: &str = "AQAB";
        const TOKEN: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6ImZpeHR1cmUta2V5IiwidHlwIjoiSldUIn0.eyJpc3MiOiJodHRwczovL2lkcC5leGFtcGxlLmNvbSIsInN1YiI6ImZpeHR1cmUtdXNlciIsImF1ZCI6Im1lbW9yeS1tY3AiLCJleHAiOjQxMDI0NDQ4MDB9.RQUb1jTpNpHx5geL9PYGUwTtWwsRHWC4SggqOgWB43_vCL8w2Br9qdnjm-icqzvUWXAl6nW-AgXbnjC89682M3wMGlHxggShZeX_rwg-QXu_ZmCkAALFXZKXbvssvqHiP4K4UbuKSbx93RZ2Tzm-I2-64H5EfwMf8o2UaI_hXuH50oH3eKVmEUcLwRqajN7SsYn0d6IKQsYQGNa8MmZvRBGloFdxwLidTh-eQXrsvNZo4Odbr7UhkEWwA0RP1SsVLDmAO9Exk4ptGnJ75j6GlvY1F7ONipYO5EW13n_fYMvfJ0ypkiaK4TWFoZ6Zu7z8rTHqTbY9oJHP8GyMdIIqjw";

        let key = DecodingKey::from_rsa_components(N, E).expect("fixture JWKS key");
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["https://idp.example.com"]);
        validation.set_audience(&["memory-mcp"]);

        let token = decode::<serde_json::Value>(TOKEN, &key, &validation)
            .expect("RS256 signature verification must work");
        assert_eq!(token.claims["sub"], serde_json::json!("fixture-user"));
    }

    /// An explicit `MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG` stays a single pin, no
    /// matter what the provider advertises.
    #[test]
    fn an_explicit_algorithm_stays_a_single_pin() {
        assert_eq!(
            resolve_allowed_algorithms(
                "EdDSA",
                &serde_json::json!({"id_token_signing_alg_values_supported": ["RS256"]})
            )
            .expect("pinned"),
            vec!["EdDSA"]
        );
        assert!(matches!(
            resolve_allowed_algorithms("PS256", &serde_json::json!({})),
            Err(MemoryError::ConfigInvalid(_))
        ));
    }
}
