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

/// The production Argon2id v19 parameters: m=19456 KiB, t=2, p=1.
///
/// One source of truth: [`self_params`] builds from these and [`validate_phc`]
/// refuses any stored hash built with anything else.
const M_COST: u32 = 19456;
const T_COST: u32 = 2;
const P_COST: u32 = 1;

/// Default Argon2id v19 parameters: m=19456, t=2, p=1.
///
/// Returns an error rather than panicking so hostile stored parameters can
/// never abort a request path.
fn self_params() -> LocalResult<Params> {
    Params::new(M_COST, T_COST, P_COST, Some(32))
        .map_err(|e| LocalAdminError::InvalidInput(format!("KDF params: {e}")))
}

/// Validate PHC parameters before expensive KDF work.
///
/// `argon2`'s verifier computes with the parameters stored in the hash, so a
/// wide acceptance range would let one corrupt or hostile row choose the
/// allocation (plan §6: "do not allocate according to arbitrary stored
/// parameters"). The system only ever writes its own parameters, so anything
/// else is corrupt or hostile and is refused before any computation. Corrupt
/// or hostile hashes fail closed without raw error details.
fn validate_phc(phc: &str) -> LocalResult<()> {
    // Must start with the expected Argon2id prefix
    if !phc.starts_with("$argon2id$v=19$m=") {
        return Err(LocalAdminError::InvalidCredentials);
    }

    // Parse the hash to validate structure
    let parsed = PasswordHash::new(phc).map_err(|_| LocalAdminError::InvalidCredentials)?;

    // The stored parameters must be exactly the production set
    let params = Params::try_from(&parsed).map_err(|_| LocalAdminError::InvalidCredentials)?;
    if params.m_cost() != M_COST || params.t_cost() != T_COST || params.p_cost() != P_COST {
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
    async fn hostile_phc_parameters_are_refused_before_any_computation() {
        // Plan §5: "hostile parameters not executed". `argon2` computes with
        // the parameters stored in the hash, so a row carrying m=1 GiB must be
        // refused before any allocation. A completed test is the evidence:
        // computing this PHC would stall for minutes and allocate 1 GiB.
        let hasher = PasswordHasher::new().expect("supported KDF");
        let hostile = concat!(
            "$argon2id$v=19$m=1048576,t=2,p=1$",
            "MTIzNDU2Nzg5MGFiY2RlZg==",
            "$QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE="
        );
        assert!(matches!(
            hasher
                .verify("any password".into(), Some(hostile.to_owned()))
                .await,
            Err(LocalAdminError::InvalidCredentials)
        ));
        // A plausible but non-production cost set is refused the same way.
        let other = concat!(
            "$argon2id$v=19$m=8192,t=2,p=1$",
            "MTIzNDU2Nzg5MGFiY2RlZg==",
            "$QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE="
        );
        assert!(matches!(
            hasher
                .verify("any password".into(), Some(other.to_owned()))
                .await,
            Err(LocalAdminError::InvalidCredentials)
        ));
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
