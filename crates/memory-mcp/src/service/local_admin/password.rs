use std::sync::Arc;

use argon2::password_hash::{PasswordHash, SaltString};
use argon2::{
    Algorithm, Argon2, Params, PasswordHasher as ArgonPasswordHasher, PasswordVerifier, Version,
};
use rand_core::OsRng;
use rand_core::RngCore;

use crate::service::local_admin::contracts::{LocalAdminError, LocalResult};

/// Bounded password hasher with Argon2id v19, m=19456/t2/p1.
/// Two running blocking jobs, at most eight queued, two-second
/// admission deadline.
#[derive(Clone)]
pub struct PasswordHasher {
    params: Params,
    dummy_phc: String,
    admission: Arc<tokio::sync::Semaphore>,
    running: Arc<tokio::sync::Semaphore>,
}

impl PasswordHasher {
    /// Initialize with supported parameters and a dummy PHC hash.
    pub fn new() -> LocalResult<Self> {
        let params = Params::new(19456, 2, 1, Some(32))
            .map_err(|e| LocalAdminError::InvalidInput(format!("KDF params: {e}")))?;
        // Generate a dummy PHC once at startup for unknown/pending users.
        let mut salt_bytes = [0u8; 16];
        OsRng.fill_bytes(&mut salt_bytes);
        let dummy_salt = SaltString::encode_b64(&salt_bytes)
            .map_err(|e| LocalAdminError::InvalidInput(format!("salt encode: {e}")))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone());
        let dummy_phc = argon2
            .hash_password(b"dummy", &dummy_salt)
            .map_err(|e| LocalAdminError::InvalidInput(format!("dummy KDF: {e}")))?
            .to_string();
        Ok(Self {
            params,
            dummy_phc,
            admission: Arc::new(tokio::sync::Semaphore::new(8)),
            running: Arc::new(tokio::sync::Semaphore::new(2)),
        })
    }

    /// Hash a password with Argon2id. Bounded admission.
    pub async fn hash(&self, password: String) -> LocalResult<String> {
        let _permit =
            tokio::time::timeout(std::time::Duration::from_secs(2), self.admission.acquire())
                .await
                .map_err(|_| LocalAdminError::Unavailable)?
                .map_err(|_| LocalAdminError::Unavailable)?;

        let running = self.running.clone();
        let params = self.params.clone();

        // Acquire running slot before spawn_blocking to queue properly.
        // Permit is held until the spawned task completes.
        let running_permit = running
            .acquire()
            .await
            .map_err(|_| LocalAdminError::Unavailable)?;

        let result = tokio::task::spawn_blocking(move || {
            let mut salt_bytes = [0u8; 16];
            OsRng.fill_bytes(&mut salt_bytes);
            let salt = SaltString::encode_b64(&salt_bytes)
                .map_err(|e| LocalAdminError::InvalidInput(format!("salt encode: {e}")))?;
            let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
            let phc = argon2
                .hash_password(password.as_bytes(), &salt)
                .map_err(|e| LocalAdminError::InvalidInput(format!("KDF hash: {e}")))?
                .to_string();
            Ok::<String, LocalAdminError>(phc)
        })
        .await
        .map_err(|_| LocalAdminError::Unavailable)?;
        drop(running_permit);
        result
    }

    /// Verify a password against a stored PHC hash. If `phc` is None,
    /// performs one dummy verification and returns false.
    pub async fn verify(&self, password: String, phc: Option<String>) -> LocalResult<bool> {
        let phc_str = match phc {
            Some(p) => p,
            None => {
                // Dummy verification for unknown/pending users.
                let params = self.params.clone();
                let dummy = self.dummy_phc.clone();
                return tokio::task::spawn_blocking(move || {
                    let parsed = PasswordHash::new(&dummy)
                        .map_err(|e| LocalAdminError::InvalidInput(format!("dummy PHC: {e}")))?;
                    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
                    let _ = argon2.verify_password(password.as_bytes(), &parsed);
                    Ok::<bool, LocalAdminError>(false)
                })
                .await
                .map_err(|_| LocalAdminError::Unavailable)?;
            }
        };

        // Validate PHC parameters before expensive KDF work.
        validate_phc(&phc_str)?;

        let _permit =
            tokio::time::timeout(std::time::Duration::from_secs(2), self.admission.acquire())
                .await
                .map_err(|_| LocalAdminError::Unavailable)?
                .map_err(|_| LocalAdminError::Unavailable)?;

        let running = self.running.clone();

        let running_permit = running
            .acquire()
            .await
            .map_err(|_| LocalAdminError::Unavailable)?;

        let result = tokio::task::spawn_blocking(move || {
            let parsed = PasswordHash::new(&phc_str)
                .map_err(|e| LocalAdminError::InvalidInput(format!("PHC parse: {e}")))?;
            let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, self_params()?);
            let ok = argon2.verify_password(password.as_bytes(), &parsed).is_ok();
            Ok::<bool, LocalAdminError>(ok)
        })
        .await
        .map_err(|_| LocalAdminError::Unavailable)?;
        drop(running_permit);
        result
    }
}

/// Default Argon2id v19 parameters: m=19456, t=2, p=1.
///
/// Returns an error rather than panicking so hostile stored parameters can
/// never abort a request path.
fn self_params() -> LocalResult<Params> {
    Params::new(19456, 2, 1, Some(32))
        .map_err(|e| LocalAdminError::InvalidInput(format!("KDF params: {e}")))
}

/// Validate PHC parameters before expensive KDF work.
/// Binds algorithm, version, memory, time, parallelism, and output size.
/// Corrupt or hostile hashes fail closed without raw error details.
fn validate_phc(phc: &str) -> LocalResult<()> {
    // Must start with the expected Argon2id prefix
    if !phc.starts_with("$argon2id$v=19$m=") {
        return Err(LocalAdminError::InvalidCredentials);
    }

    // Parse the hash to validate structure
    let parsed = PasswordHash::new(phc).map_err(|_| LocalAdminError::InvalidCredentials)?;

    // Verify parameters are within bounds using the argon2 crate
    let params = Params::try_from(&parsed).map_err(|_| LocalAdminError::InvalidCredentials)?;

    let m_cost = params.m_cost();
    let t_cost = params.t_cost();
    let p_cost = params.p_cost();

    // Bound memory: must be reasonable (1MB - 1GB in KiB)
    if !(1024..=1048576).contains(&m_cost) {
        return Err(LocalAdminError::InvalidCredentials);
    }

    // Bound time: must be reasonable (1 - 100)
    if !(1..=100).contains(&t_cost) {
        return Err(LocalAdminError::InvalidCredentials);
    }

    // Bound parallelism: must be reasonable (1 - 64)
    if !(1..=64).contains(&p_cost) {
        return Err(LocalAdminError::InvalidCredentials);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::PasswordHasher;

    #[tokio::test]
    async fn local_admin_password_hashes_are_salted() {
        let hasher = PasswordHasher::new().expect("supported KDF");
        let password = "correct horse battery staple".to_owned();
        let first = hasher.hash(password.clone()).await.expect("hash");
        let second = hasher.hash(password.clone()).await.expect("hash");
        assert_ne!(first, second);
        assert!(first.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(
            hasher
                .verify(password.clone(), Some(first))
                .await
                .expect("verify")
        );
        assert!(
            !hasher
                .verify("a different password".into(), Some(second))
                .await
                .expect("verify")
        );
        assert!(!hasher.verify(password, None).await.expect("dummy"));
    }

    #[tokio::test]
    async fn local_admin_rejects_corrupt_phc() {
        let hasher = PasswordHasher::new().expect("supported KDF");
        let result = hasher
            .verify("password".into(), Some("not-a-phc-hash".into()))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn local_admin_rejects_wrong_algorithm_phc() {
        let hasher = PasswordHasher::new().expect("supported KDF");
        // bcrypt-style hash
        let result = hasher
            .verify(
                "password".into(),
                Some("$2b$10$abcdefghijklmnopqrstuuABCDEFGHIJKLMNOPQRSTUVWXYZ012".into()),
            )
            .await;
        assert!(result.is_err());
    }
}
