//! Secret-slot resolution shared by the HTTP config loader and the admin CLI.
//!
//! One resolver for both consumers, so the CLI and the server can never
//! disagree about what "derived" means. Per slot: an explicit value wins,
//! then the `MEMORY_MCP_HTTP_SECRET_KEY` root derivation under a purpose
//! label, then the caller's fallback (which preserves the caller's original
//! missing-value contract). Secret material is never invented.

use crate::error::MemoryError;

/// Root secret variable name.
pub const ROOT_SECRET_ENV: &str = "MEMORY_MCP_HTTP_SECRET_KEY";

/// The documented strength floor for the root secret, in bytes.
pub const MIN_ROOT_SECRET_BYTES: usize = 32;

/// Read `MEMORY_MCP_HTTP_SECRET_KEY`; empty or whitespace means unset. A root
/// shorter than the documented floor is refused rather than silently expanded
/// into every secret slot.
pub fn read_root_secret() -> Result<Option<String>, MemoryError> {
    let root = std::env::var(ROOT_SECRET_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if let Some(root) = &root {
        validate_root_secret(root)?;
    }
    Ok(root)
}

/// The documented strength floor for the root secret.
pub fn validate_root_secret(root: &str) -> Result<(), MemoryError> {
    if root.len() < MIN_ROOT_SECRET_BYTES {
        return Err(MemoryError::ConfigInvalid(format!(
            "{ROOT_SECRET_ENV} must be at least {MIN_ROOT_SECRET_BYTES} bytes of secret material"
        )));
    }
    Ok(())
}

/// Purpose-separated derivation from `MEMORY_MCP_HTTP_SECRET_KEY`: one root
/// secret yields every slot, each under the absent variable's name as its
/// label, so derived slots are never zero and never equal to a sibling.
///
/// Gated on `streamable-http` because that profile owns `hmac`; outside it
/// the root secret is validated but not expanded.
#[cfg(feature = "streamable-http")]
pub fn derive_key_from_root_secret(root: &str, label: &str) -> Result<[u8; 32], MemoryError> {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    validate_root_secret(root)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(root.as_bytes())
        .map_err(|_| MemoryError::ConfigInvalid("invalid root secret".into()))?;
    mac.update(b"memory_mcp_http_secret_key\0");
    mac.update(label.as_bytes());
    Ok(mac.finalize().into_bytes().into())
}

/// Resolve one 32-byte HMAC key slot: an explicit value wins, then the
/// root-secret derivation, then the caller's fallback (which preserves the
/// original `ConfigMissing` contract).
#[cfg(feature = "streamable-http")]
pub fn resolve_key_slot(
    supplied: Option<String>,
    env_name: &str,
    root_secret: Option<&str>,
    fallback: impl FnOnce() -> Result<[u8; 32], MemoryError>,
) -> Result<[u8; 32], MemoryError> {
    match supplied {
        Some(raw) => {
            let bytes =
                hex::decode(raw).map_err(|_| MemoryError::ConfigInvalid(env_name.into()))?;
            bytes
                .try_into()
                .map_err(|_| MemoryError::ConfigInvalid(env_name.into()))
        }
        None => match root_secret {
            Some(root) => derive_key_from_root_secret(root, env_name),
            None => fallback(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 32-byte root secret, exactly the documented strength floor.
    const STRONG_ROOT: &str = "0123456789abcdef0123456789abcdef";
    /// 31 bytes: one below the floor, so validation must refuse it.
    const WEAK_ROOT: &str = "0123456789abcdef0123456789abcde";

    /// Run `body` with `MEMORY_MCP_HTTP_SECRET_KEY` set to `value`, restoring
    /// the ambient environment afterwards. Every test that touches the root
    /// secret must go through here: the process environment is shared, so the
    /// `config::env_lock` serializes these against every other env-reading
    /// test in the crate.
    fn with_root_secret<R>(value: Option<&str>, body: impl FnOnce() -> R) -> R {
        let _guard = crate::config::env_lock().lock().expect("secrets env lock");
        let saved = std::env::var(ROOT_SECRET_ENV).ok();
        unsafe {
            match value {
                Some(value) => std::env::set_var(ROOT_SECRET_ENV, value),
                None => std::env::remove_var(ROOT_SECRET_ENV),
            }
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        unsafe {
            match saved {
                Some(saved) => std::env::set_var(ROOT_SECRET_ENV, saved),
                None => std::env::remove_var(ROOT_SECRET_ENV),
            }
        }
        outcome.expect("secrets test body")
    }

    #[test]
    fn read_root_secret_returns_none_when_unset() {
        let observed = with_root_secret(None, read_root_secret);

        assert_eq!(observed.expect("unset root secret reads cleanly"), None);
    }

    #[test]
    fn read_root_secret_returns_none_when_blank() {
        let observed = with_root_secret(Some("   "), read_root_secret);

        assert_eq!(observed.expect("blank root secret reads cleanly"), None);
    }

    #[test]
    fn read_root_secret_trims_surrounding_whitespace() {
        let padded = format!("  {STRONG_ROOT}  ");

        let observed = with_root_secret(Some(&padded), read_root_secret);

        assert_eq!(
            observed.expect("padded strong root secret reads cleanly"),
            Some(STRONG_ROOT.to_string())
        );
    }

    #[test]
    fn read_root_secret_rejects_a_root_shorter_than_the_floor_after_trimming() {
        let observed = with_root_secret(Some("  abc  "), read_root_secret);

        assert!(
            observed.is_err(),
            "whitespace must not pad a root up to the strength floor"
        );
    }

    #[test]
    fn validate_root_secret_accepts_exactly_the_strength_floor() {
        let observed = validate_root_secret(STRONG_ROOT);

        assert!(observed.is_ok(), "32 bytes must satisfy the floor");
    }

    #[test]
    fn validate_root_secret_rejects_one_byte_below_the_floor() {
        let observed = validate_root_secret(WEAK_ROOT);

        assert!(observed.is_err(), "31 bytes must be refused");
    }

    #[test]
    fn validate_root_secret_rejects_the_empty_string() {
        let observed = validate_root_secret("");

        assert!(observed.is_err(), "an empty root must be refused");
    }

    #[test]
    fn read_root_secret_rejects_a_weak_root() {
        let observed = with_root_secret(Some(WEAK_ROOT), read_root_secret);

        assert!(observed.is_err(), "a weak root must surface as an error");
    }

    #[test]
    fn read_root_secret_accepts_a_strong_root() {
        let observed = with_root_secret(Some(STRONG_ROOT), read_root_secret);

        assert_eq!(
            observed.expect("strong root secret reads cleanly"),
            Some(STRONG_ROOT.to_string())
        );
    }

    #[test]
    #[cfg(feature = "streamable-http")]
    fn derive_key_from_root_secret_is_deterministic() {
        let first =
            derive_key_from_root_secret(STRONG_ROOT, "SLOT_A").expect("derivation succeeds");
        let second =
            derive_key_from_root_secret(STRONG_ROOT, "SLOT_A").expect("derivation succeeds");

        assert_eq!(
            first, second,
            "the same root and label must derive the same key"
        );
    }

    #[test]
    #[cfg(feature = "streamable-http")]
    fn derive_key_from_root_secret_differs_per_label() {
        let first =
            derive_key_from_root_secret(STRONG_ROOT, "SLOT_A").expect("derivation succeeds");
        let second =
            derive_key_from_root_secret(STRONG_ROOT, "SLOT_B").expect("derivation succeeds");

        assert_ne!(first, second, "a sibling slot must never equal another");
    }

    #[test]
    #[cfg(feature = "streamable-http")]
    fn derive_key_from_root_secret_rejects_a_weak_root() {
        let observed = derive_key_from_root_secret(WEAK_ROOT, "SLOT_A");

        assert!(observed.is_err(), "a weak root must not be expanded");
    }

    #[test]
    #[cfg(feature = "streamable-http")]
    fn resolve_key_slot_prefers_an_explicit_value_over_the_root() {
        let supplied = "aa".repeat(32);
        let expected: [u8; 32] = [0xaa; 32];

        let observed = resolve_key_slot(Some(supplied), "SLOT_A", Some(STRONG_ROOT), || {
            panic!("an explicit value must never reach the fallback")
        })
        .expect("explicit slot resolves");

        assert_eq!(observed, expected);
    }

    #[test]
    #[cfg(feature = "streamable-http")]
    fn resolve_key_slot_rejects_an_explicit_value_of_the_wrong_width() {
        let supplied = "aabbcc".to_string();

        let observed = resolve_key_slot(Some(supplied), "SLOT_A", None, || {
            panic!("fallback must not run")
        });

        assert!(
            observed.is_err(),
            "a short slot must be refused, not padded"
        );
    }

    #[test]
    #[cfg(feature = "streamable-http")]
    fn resolve_key_slot_derives_from_the_root_when_no_value_is_supplied() {
        let expected =
            derive_key_from_root_secret(STRONG_ROOT, "SLOT_A").expect("derivation succeeds");

        let observed = resolve_key_slot(None, "SLOT_A", Some(STRONG_ROOT), || {
            panic!("a present root must never reach the fallback")
        })
        .expect("derived slot resolves");

        assert_eq!(observed, expected);
    }

    #[test]
    #[cfg(feature = "streamable-http")]
    fn resolve_key_slot_falls_back_when_neither_value_nor_root_is_present() {
        let expected = [0x5a; 32];

        let observed =
            resolve_key_slot(None, "SLOT_A", None, || Ok(expected)).expect("slot resolves");

        assert_eq!(observed, expected);
    }
}
