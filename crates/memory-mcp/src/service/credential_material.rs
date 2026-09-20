//! Credential material helpers for local admin auth and client provisioning.
//!
//! Provides HMAC-based key fingerprinting, random byte generation, and API
//! key material generation. Independent of control-plane adapters: the
//! service layer must not reach into `control::secret`, so the generators the
//! local-admin service and HTTP adapter both need live here.

use hmac::{Hmac, KeyInit, Mac};
use rand_core::RngCore;
use sha2::Sha256;

use crate::http::registry::models::{KeyedVerifier, new_api_key_id};

type HmacSha256 = Hmac<Sha256>;

/// Compute an HMAC-SHA256 fingerprint of data under a purpose-separated label.
///
/// The label prevents cross-purpose key reuse even if the underlying key is
/// the same. Returns the raw 32-byte MAC output.
///
/// HMAC accepts a key of any length, so the constructor cannot fail for any
/// reachable input. It is still handled explicitly — exactly as
/// [`KeyedVerifier::compute`] does — so a dependency contract violation fails
/// closed on an inert fingerprint instead of panicking the server. Callers use
/// the result to select a fixed throttle/binding slot, where an inert value
/// collapses traffic into one slot and therefore denies rather than admits.
pub fn hmac_fingerprint(key: &[u8; 32], label: &[u8], data: &[u8]) -> [u8; 32] {
    let Ok(mut mac) = HmacSha256::new_from_slice(key) else {
        return [0u8; 32];
    };
    mac.update(label);
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Generate a random 32-byte value from the operating-system CSPRNG.
pub fn random_32() -> [u8; 32] {
    let mut buf = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut buf);
    buf
}

/// A 256-bit random one-time secret as 64 lowercase hex characters.
///
/// The same shape the OIDC key workflow issues, so one data-plane parser
/// accepts credentials minted by either workflow.
pub fn random_token_hex() -> String {
    hex::encode(random_32())
}

/// Generate API key material: `(key_id, verifier, full_credential)`.
///
/// - `key_id`: a new `ak_…` id, the value persisted as the key's public id;
/// - `verifier`: HMAC-SHA256 of the secret under the deployment pepper, which
///   is the only form of the secret that is ever stored;
/// - `full_credential`: the one-time `mem_sk_<key_id>_<secret>` string the
///   data plane accepts, handed to the operator exactly once.
///
/// The helper does not choose a name, account, expiry or `created_at` and
/// creates no records or audit rows: those are the caller's transaction.
pub fn generate_api_key_material(pepper: &[u8]) -> (String, KeyedVerifier, String) {
    let key_id = new_api_key_id();
    let secret = random_token_hex();
    let verifier = KeyedVerifier::compute(pepper, secret.as_bytes());
    let full_credential = format!("mem_sk_{key_id}_{secret}");
    (key_id, verifier, full_credential)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::principal::api_keys::ApiKeyCredential;

    #[test]
    fn hmac_fingerprint_deterministic() {
        let key = random_32();
        let data = b"test data";
        let fp1 = hmac_fingerprint(&key, b"label", data);
        let fp2 = hmac_fingerprint(&key, b"label", data);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn hmac_fingerprint_label_separation() {
        let key = random_32();
        let data = b"same data";
        let fp1 = hmac_fingerprint(&key, b"label_a", data);
        let fp2 = hmac_fingerprint(&key, b"label_b", data);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn random_32_different_each_time() {
        let a = random_32();
        let b = random_32();
        assert_ne!(a, b);
    }

    #[test]
    fn generated_credential_parses_as_a_data_plane_credential() {
        let pepper = random_32();
        let (key_id, verifier, credential) = generate_api_key_material(&pepper);

        let parsed = ApiKeyCredential::parse(&credential)
            .expect("the issued credential must be the shape the data plane accepts");
        assert_eq!(parsed.key_id(), key_id, "key id and credential must agree");
        assert!(
            verifier.verify(&pepper, parsed.secret()),
            "the stored verifier must verify the credential's own secret"
        );
    }

    #[test]
    fn generated_material_is_unique_per_call() {
        let pepper = random_32();
        let (first_id, _, first_credential) = generate_api_key_material(&pepper);
        let (second_id, _, second_credential) = generate_api_key_material(&pepper);
        assert_ne!(first_id, second_id);
        assert_ne!(first_credential, second_credential);
    }

    #[test]
    fn a_credential_only_verifies_under_its_own_pepper() {
        let pepper = random_32();
        let other = random_32();
        let (_, verifier, credential) = generate_api_key_material(&pepper);
        let parsed = ApiKeyCredential::parse(&credential).expect("parse");
        assert!(verifier.verify(&pepper, parsed.secret()));
        assert!(
            !verifier.verify(&other, parsed.secret()),
            "a different pepper must not verify"
        );
        assert!(
            !verifier.verify(&pepper, credential.as_bytes()),
            "the whole credential string is not the verified secret"
        );
    }
}
