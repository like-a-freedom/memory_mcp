//! Random secret generation for one-time tokens.
//!
//! The control plane hands secrets to a caller exactly once:
//! API keys, account-deletion challenges, session cookies, and
//! OIDC nonces all share the same 32-byte random, hex-encoded
//! shape. Centralising the helper keeps every consumer honest
//! about the security parameter (256 bits of entropy, 64 hex
//! characters) and lets tests stub the generator.

/// Generate a 256-bit random token as 64 lowercase hex
/// characters. Uses the operating-system CSPRNG via
/// [`rand::fill`], so it is suitable for one-time secrets
/// (API keys, deletion challenges) and short-lived
/// identifiers (session cookie values, OIDC nonces).
pub(crate) fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_token_is_64_lowercase_hex_chars() {
        let token = random_token();
        assert_eq!(token.len(), 64, "expected 64 hex chars, got {token:?}");
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "expected lowercase hex, got {token:?}"
        );
    }

    #[test]
    fn random_token_produces_distinct_values_across_calls() {
        // The probability of two consecutive 256-bit random
        // tokens colliding is ~ 1 / 2^256, well below the
        // test flakiness threshold; the assertion is
        // overwhelmingly reliable.
        let a = random_token();
        let b = random_token();
        assert_ne!(a, b);
    }
}
