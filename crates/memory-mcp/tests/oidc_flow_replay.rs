//! OIDC flow persistence integration over the real SurrealDB Mem registry.
#![cfg(feature = "control-plane")]

use memory_mcp::control::oidc::{
    OidcFlowIntent, OidcNonce, OidcState, PkceCode, identity_subject_verifier, seal_oidc_payload,
    unseal_oidc_payload,
};
use memory_mcp::http::config::BrowserAuthMethod;
use memory_mcp::http::registry::storage::{BrowserPolicyStore, SessionStore};
use memory_mcp::http::registry::surreal_store::SurrealRegistryStore;

#[tokio::test]
async fn a_stored_oidc_request_is_consumed_once_through_the_registry_interface() {
    let store = SurrealRegistryStore::connect_in_memory("oidc_replay", "registry")
        .await
        .expect("migrated in-memory registry");
    let policy = store
        .reconcile_browser_policy(&[BrowserAuthMethod::Oidc], None)
        .await
        .expect("durable OIDC policy");
    let key = [0x73; 32];
    let state = OidcState::new();
    let state_value = state.as_str().to_string();
    let nonce = OidcNonce::new();
    let nonce_value = nonce.as_str().to_string();
    let pkce = PkceCode {
        verifier: "fixed-pkce-verifier".into(),
        challenge: "not-persisted".into(),
    };
    let state_hash =
        hex::encode(identity_subject_verifier(&key, "", state.as_str()).expect("state hash"));
    let (sealed, aead_nonce) =
        seal_oidc_payload(&key, &state, &nonce, &pkce, &OidcFlowIntent::SignIn)
            .expect("seal authorization request");

    store
        .store_oidc_request(&policy, &state_hash, &sealed, &aead_nonce)
        .await
        .expect("store authorization request");
    let consumed = store
        .take_oidc_request(&policy, &state_hash)
        .await
        .expect("consume authorization request")
        .expect("first callback finds the request");
    let flow = unseal_oidc_payload(&key, &consumed.0, &consumed.1).expect("unseal request");
    let replay = store
        .take_oidc_request(&policy, &state_hash)
        .await
        .expect("replayed callback is refused as a missing request");

    assert_eq!(flow.state.as_str(), state_value);
    assert_eq!(flow.nonce.as_str(), nonce_value);
    assert_eq!(flow.pkce.verifier, "fixed-pkce-verifier");
    assert_eq!(flow.intent, OidcFlowIntent::SignIn);
    assert!(replay.is_none());
}
