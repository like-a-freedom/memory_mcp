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
