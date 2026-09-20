use std::sync::Arc;

use argon2::password_hash::phc::PasswordHash;
use argon2::{
    Algorithm, Argon2, Params, PasswordHasher as ArgonPasswordHasher, PasswordVerifier, Version,
};

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
        // Generate a dummy PHC once at startup for unknown/pending users. The
        // hasher draws the salt from the system RNG itself, so there is no salt
        // material to hold here.
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone());
        let dummy_phc = argon2
            .hash_password(b"dummy")
            .map_err(|e| LocalAdminError::InvalidInput(format!("dummy KDF: {e}")))?
            .to_string();
        Ok(Self {
            params,
            dummy_phc,
            admission: Arc::new(tokio::sync::Semaphore::new(8)),
            running: Arc::new(tokio::sync::Semaphore::new(2)),
        })
    }

    /// Build a hasher with explicit bounds. Test-only: production always uses
    /// the reviewed two-running/eight-queued shape.
    #[cfg(test)]
    pub(crate) fn with_bounds(running: usize, admission: usize) -> LocalResult<Self> {
        let mut hasher = Self::new()?;
        hasher.admission = Arc::new(tokio::sync::Semaphore::new(admission));
        hasher.running = Arc::new(tokio::sync::Semaphore::new(running));
        Ok(hasher)
    }

    /// The admission semaphore, so a test can hold every slot and prove the
    /// fail-closed deadline. Test-only.
    #[cfg(test)]
    pub(crate) fn admission_semaphore(&self) -> Arc<tokio::sync::Semaphore> {
        self.admission.clone()
    }

    /// Hash a password with Argon2id. Bounded admission.
    pub async fn hash(&self, password: String) -> LocalResult<String> {
        let _permit = self.admit().await?;
        let params = self.params.clone();

        self.run_with_running_permit(move |_permit| {
            let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
            let phc = argon2
                .hash_password(password.as_bytes())
                .map_err(|e| LocalAdminError::InvalidInput(format!("KDF hash: {e}")))?
                .to_string();
            Ok::<String, LocalAdminError>(phc)
        })
        .await
    }

    /// Verify a password against a stored PHC hash. If `phc` is None,
    /// performs one dummy verification and returns false.
    ///
    /// The dummy path takes the *same* admission and running bounds as a real
    /// verification: an unknown username must not be able to spawn unbounded
    /// blocking KDF work, so the work it does is bounded exactly like the work
    /// it imitates.
    pub async fn verify(&self, password: String, phc: Option<String>) -> LocalResult<bool> {
        let _permit = self.admit().await?;

        match phc {
            // Dummy verification for unknown/pending users.
            None => {
                let params = self.params.clone();
                let dummy = self.dummy_phc.clone();
                self.run_with_running_permit(move |_permit| {
                    let parsed = PasswordHash::new(&dummy)
                        .map_err(|e| LocalAdminError::InvalidInput(format!("dummy PHC: {e}")))?;
                    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
                    let _ = argon2.verify_password(password.as_bytes(), &parsed);
                    Ok::<bool, LocalAdminError>(false)
                })
                .await
            }
            Some(phc_str) => {
                // Validate PHC parameters before expensive KDF work.
                validate_phc(&phc_str)?;
                self.run_with_running_permit(move |_permit| {
                    let parsed = PasswordHash::new(&phc_str)
                        .map_err(|e| LocalAdminError::InvalidInput(format!("PHC parse: {e}")))?;
                    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, self_params()?);
                    let ok = argon2.verify_password(password.as_bytes(), &parsed).is_ok();
                    Ok::<bool, LocalAdminError>(ok)
                })
                .await
            }
        }
    }

    /// Take one of the eight admission slots, or fail closed with a
    /// sanitized `503` after the two-second deadline.
    ///
    /// The wait is bounded rather than instant: a burst beyond the queue is
    /// rejected after the same two-second deadline every admission obeys, and
    /// a saturated queue can never grow without limit.
    async fn admit(&self) -> LocalResult<tokio::sync::OwnedSemaphorePermit> {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.admission.clone().acquire_owned(),
        )
        .await
        .map_err(|_| LocalAdminError::Unavailable)?
        .map_err(|_| LocalAdminError::Unavailable)
    }

    /// Run `job` on the blocking pool while holding one of the two *running*
    /// permits.
    ///
    /// The permit travels **into** the blocking closure, so it is released when
    /// the KDF work actually stops rather than when the awaiting future is
    /// dropped. Cancelling a request therefore cannot free capacity for work
    /// that is still executing on the pool.
    async fn run_with_running_permit<T, F>(&self, job: F) -> LocalResult<T>
    where
        F: FnOnce(tokio::sync::OwnedSemaphorePermit) -> LocalResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let running_permit = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.running.clone().acquire_owned(),
        )
        .await
        .map_err(|_| LocalAdminError::Unavailable)?
        .map_err(|_| LocalAdminError::Unavailable)?;

        tokio::task::spawn_blocking(move || job(running_permit))
            .await
            .map_err(|_| LocalAdminError::Unavailable)?
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
///
/// Binds algorithm, version, memory, time and parallelism. The derived output
/// length is deliberately not bound here: `argon2`'s verifier compares the full
/// PHC string, so a foreign output length cannot verify against a stored hash
/// that was produced with `Some(32)`. Corrupt or hostile hashes fail closed
/// without raw error details.
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
    use crate::service::local_admin::contracts::LocalAdminError;

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

    #[tokio::test]
    async fn a_saturated_admission_queue_fails_closed() {
        // Plan §5: "KDF timeout/queue/cancellation". One slot, held by the
        // test, so every admission must give up after the bounded deadline
        // instead of queueing without limit.
        let hasher = PasswordHasher::with_bounds(1, 1).expect("supported KDF");
        // Hash first, while admission is still available, so the case below
        // has real stored material to verify against.
        let phc = hasher
            .hash("a sufficiently long passphrase".to_owned())
            .await
            .expect("hash for the saturation case");
        let held = hasher
            .admission_semaphore()
            .try_acquire_owned()
            .expect("hold the only admission slot");

        assert!(matches!(
            hasher
                .verify("a sufficiently long passphrase".into(), Some(phc))
                .await,
            Err(LocalAdminError::Unavailable)
        ));
        // The dummy path for an unknown username is bounded by the same
        // deadline, so it cannot be used to spawn unbounded KDF work.
        assert!(matches!(
            hasher
                .verify("a sufficiently long passphrase".into(), None)
                .await,
            Err(LocalAdminError::Unavailable)
        ));

        drop(held);
        assert!(
            hasher
                .verify("a sufficiently long passphrase".into(), None)
                .await
                .is_ok(),
            "admission recovers once capacity returns"
        );
    }
}
