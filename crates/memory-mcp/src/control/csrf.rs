//! CSRF token issuance and verification.
//!
//! CSRF tokens are HMACs of `(account_id, session_id, csrf_key)`.
//! Required on all `/api/v1` state-changing endpoints.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::error::MemoryError;

type HmacSha256 = Hmac<Sha256>;

/// Compute a CSRF token from account_id + session_id.
pub fn compute_csrf(
    key: &[u8; 32],
    account_id: &str,
    session_id: &str,
) -> Result<String, MemoryError> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| MemoryError::ConfigInvalid("invalid CSRF key".into()))?;
    mac.update(account_id.as_bytes());
    mac.update(b"\0");
    mac.update(session_id.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Verify a CSRF token matches the expected value.
pub fn verify_csrf(
    key: &[u8; 32],
    account_id: &str,
    session_id: &str,
    token: &str,
) -> Result<bool, MemoryError> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| MemoryError::ConfigInvalid("invalid CSRF key".into()))?;
    mac.update(account_id.as_bytes());
    mac.update(b"\0");
    mac.update(session_id.as_bytes());
    let token_bytes =
        hex::decode(token).map_err(|_| MemoryError::ConfigInvalid("invalid CSRF hex".into()))?;
    Ok(mac.verify_slice(&token_bytes).is_ok())
}

/// Extract CSRF token from request headers or form body.
/// Returns `None` if not present.
pub fn extract_csrf_token(headers: &axum::http::HeaderMap, form: Option<&str>) -> Option<String> {
    // Prefer X-CSRF-Token header.
    if let Some(value) = headers.get("x-csrf-token")
        && let Ok(s) = value.to_str()
    {
        return Some(s.to_string());
    }
    // Fall back to form field.
    form.and_then(|body| {
        body.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            if key == "csrf_token" {
                Some(value.to_string())
            } else {
                None
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csrf_token_is_deterministic() {
        let key = [0xABu8; 32];
        let a = compute_csrf(&key, "acc1", "sess1").unwrap();
        let b = compute_csrf(&key, "acc1", "sess1").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn csrf_token_differs_for_different_accounts() {
        let key = [0xABu8; 32];
        let a = compute_csrf(&key, "acc1", "sess1").unwrap();
        let b = compute_csrf(&key, "acc2", "sess1").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn csrf_token_differs_for_different_sessions() {
        let key = [0xABu8; 32];
        let a = compute_csrf(&key, "acc1", "sess1").unwrap();
        let b = compute_csrf(&key, "acc1", "sess2").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn verify_csrf_roundtrip() {
        let key = [0xABu8; 32];
        let token = compute_csrf(&key, "acc1", "sess1").unwrap();
        assert!(verify_csrf(&key, "acc1", "sess1", &token).unwrap());
    }

    #[test]
    fn verify_csrf_rejects_tampered() {
        let key = [0xABu8; 32];
        let token = compute_csrf(&key, "acc1", "sess1").unwrap();
        let mut tampered = token.clone();
        tampered.pop();
        tampered.push('0');
        assert!(!verify_csrf(&key, "acc1", "sess1", &tampered).unwrap());
    }

    #[test]
    fn verify_csrf_rejects_wrong_account() {
        let key = [0xABu8; 32];
        let token = compute_csrf(&key, "acc1", "sess1").unwrap();
        assert!(!verify_csrf(&key, "acc2", "sess1", &token).unwrap());
    }

    #[test]
    fn verify_csrf_rejects_a_token_signed_with_another_key() {
        let token = compute_csrf(&[0xABu8; 32], "acc1", "sess1").unwrap();

        let observed = verify_csrf(&[0xCDu8; 32], "acc1", "sess1", &token);

        assert!(
            !observed.unwrap(),
            "the key is part of the token's identity"
        );
    }

    #[test]
    fn verify_csrf_rejects_a_token_that_is_not_hex() {
        let observed = verify_csrf(&[0xABu8; 32], "acc1", "sess1", "not hex");

        assert!(observed.is_err());
    }

    #[test]
    fn verify_csrf_rejects_an_empty_token() {
        let observed = verify_csrf(&[0xABu8; 32], "acc1", "sess1", "");

        assert!(!observed.unwrap());
    }

    #[test]
    fn a_token_is_hex_encoded_hmac_sha256() {
        let observed = compute_csrf(&[0xABu8; 32], "acc1", "sess1").expect("token computes");

        assert_eq!(observed.len(), 64, "sha256 renders as 64 hex characters");
    }

    #[test]
    fn the_account_and_session_names_are_separated_by_a_nul_byte() {
        // Without the separator, ("ab","c") and ("a","bc") would collide.
        let observed = compute_csrf(&[0xABu8; 32], "ab", "c").expect("token computes");

        assert_ne!(observed, compute_csrf(&[0xABu8; 32], "a", "bc").unwrap());
    }

    #[test]
    fn the_token_is_read_from_the_csrf_header() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "x-csrf-token",
            axum::http::HeaderValue::from_static("from-header"),
        );

        let observed = extract_csrf_token(&headers, None);

        assert_eq!(observed.as_deref(), Some("from-header"));
    }

    #[test]
    fn the_header_wins_over_the_form_field() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "x-csrf-token",
            axum::http::HeaderValue::from_static("from-header"),
        );

        let observed = extract_csrf_token(&headers, Some("csrf_token=from-form"));

        assert_eq!(
            observed.as_deref(),
            Some("from-header"),
            "a forged form field must not override the header"
        );
    }

    #[test]
    fn the_token_falls_back_to_the_form_field() {
        let observed = extract_csrf_token(&axum::http::HeaderMap::new(), Some("csrf_token=abc123"));

        assert_eq!(observed.as_deref(), Some("abc123"));
    }

    #[test]
    fn the_form_field_is_found_among_other_fields() {
        let observed = extract_csrf_token(
            &axum::http::HeaderMap::new(),
            Some("first=1&csrf_token=abc123&last=2"),
        );

        assert_eq!(observed.as_deref(), Some("abc123"));
    }

    #[test]
    fn a_form_without_a_csrf_field_yields_no_token() {
        let observed = extract_csrf_token(&axum::http::HeaderMap::new(), Some("first=1&last=2"));

        assert!(observed.is_none());
    }

    #[test]
    fn an_absent_header_and_form_yield_no_token() {
        let observed = extract_csrf_token(&axum::http::HeaderMap::new(), None);

        assert!(observed.is_none());
    }
}
