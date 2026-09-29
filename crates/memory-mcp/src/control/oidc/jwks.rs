//! JWKS cache used by [`crate::control::oidc::OidcClient`].
//!
//! The cache is shared via `Arc` so concurrent ID-token validations
//! hit the in-memory `HashMap`; a refresh is serialized through a
//! `tokio::sync::Mutex` and only re-fetches when the cached keys
//! are older than the configured TTL.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use jsonwebtoken::DecodingKey;
use tokio::sync::Mutex;

use super::flow_material::{AuthError, SUPPORTED_ID_TOKEN_ALGORITHMS};

#[derive(Clone)]
pub struct JwksCache {
    inner: Arc<RwLock<JwksState>>,
    refresh_lock: Arc<Mutex<()>>,
    client: reqwest::Client,
    jwks_uri: String,
    ttl: Duration,
}

struct JwksState {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Option<Instant>,
}

impl JwksState {
    fn empty() -> Self {
        Self {
            keys: HashMap::new(),
            fetched_at: None,
        }
    }
}

impl JwksCache {
    /// Construct a cache from its parts. Used by `OidcClient::new`
    /// after OIDC discovery.
    pub fn from_parts(client: reqwest::Client, jwks_uri: String, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(JwksState::empty())),
            refresh_lock: Arc::new(Mutex::new(())),
            client,
            jwks_uri,
            ttl,
        }
    }
    pub fn find_key(&self, kid: &str) -> Result<Option<DecodingKey>, AuthError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .keys
            .get(kid)
            .cloned())
    }

    pub async fn key_for(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        let fresh = self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.ttl);
        if fresh && let Some(key) = self.find_key(kid)? {
            return Ok(key);
        }
        let _refresh = self.refresh_lock.lock().await;
        let fresh = self
            .inner
            .read()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.ttl);
        if fresh && let Some(key) = self.find_key(kid)? {
            return Ok(key);
        }
        self.refresh().await?;
        self.find_key(kid)?
            .ok_or_else(|| AuthError::Jwks("unknown key id".into()))
    }

    pub async fn refresh(&self) -> Result<(), AuthError> {
        #[derive(serde::Deserialize)]
        struct JwksDocument {
            keys: Vec<Jwk>,
        }
        #[derive(serde::Deserialize)]
        struct Jwk {
            kid: String,
            kty: String,
            n: Option<String>,
            e: Option<String>,
            crv: Option<String>,
            x: Option<String>,
            y: Option<String>,
            alg: Option<String>,
        }
        let document = self
            .client
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(|error| AuthError::Jwks(error.to_string()))?
            .error_for_status()
            .map_err(|error| AuthError::Jwks(error.to_string()))?
            .json::<JwksDocument>()
            .await
            .map_err(|error| AuthError::Jwks(error.to_string()))?;
        let mut keys = HashMap::new();
        for jwk in document.keys.into_iter().take(32) {
            let key = match (
                jwk.kty.as_str(),
                jwk.n.as_deref(),
                jwk.e.as_deref(),
                jwk.crv.as_deref(),
                jwk.x.as_deref(),
                jwk.y.as_deref(),
            ) {
                ("RSA", Some(n), Some(e), _, _, _) => {
                    jsonwebtoken::DecodingKey::from_rsa_components(n, e)
                }
                ("EC", _, _, Some("P-256"), Some(x), Some(y)) => {
                    jsonwebtoken::DecodingKey::from_ec_components(x, y)
                }
                ("OKP", _, _, Some("Ed25519"), Some(x), _) => {
                    jsonwebtoken::DecodingKey::from_ed_components(x)
                }
                _ => continue,
            }
            .map_err(|error| AuthError::Jwks(error.to_string()))?;
            // A key the provider pins to an algorithm we accept has to
            // survive. EC entries stay limited to P-256 above because no other
            // curve is decoded — so adding an ES* algorithm to
            // `SUPPORTED_ID_TOKEN_ALGORITHMS` needs a matching curve arm here,
            // or its key will drop out exactly as this filter used to drop
            // RS384's.
            if jwk
                .alg
                .as_deref()
                .is_none_or(|alg| SUPPORTED_ID_TOKEN_ALGORITHMS.contains(&alg))
            {
                keys.insert(jwk.kid, key);
            }
        }
        let mut state = self
            .inner
            .write()
            .map_err(|_| AuthError::Jwks("JWKS cache lock poisoned".into()))?;
        state.keys = keys;
        state.fetched_at = Some(Instant::now());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};

    /// A real 2048-bit RSA public key, base64url-encoded as JWKS requires.
    /// It exists only to make `DecodingKey::from_rsa_components` succeed;
    /// nothing in these tests performs a signature verification with it.
    const RSA_N: &str = "wVC3GEGwKEX2CE02Zg061Rwu8OlW4ZPUuqyjBvAl5GfMshHGjdOpvh1KzZF8BtEvzuQHCBBeRA0sySMZOU6tBqdkQESdXMyek4fY1pJ8QqX7mX9CI6upQ7NCsrEpw9_xHBfZ87NovfTZ2G1Ooas6Kqo04Shkk0tFChSZK3g25QsXIbEOwkJf2LaIeBcv_J4b4qHmtXg6uy2ymaFKmiZk5ldzpqD2Hop0CaBQtQ0rVYHrBpddxB_A6jWFUkurpGGdrvm0P03arv7i5X7zc9T7vhVaAaBhKWl5QIryeJS9E1Ooty23cK2rjF6l41saz9PkrkEmCSdKqNCxtoqnH1m4Jw";
    const RSA_E: &str = "AQAB";
    const EC_X: &str = "wUHMe1ZQUhm74MZPtYNIgAZE7SmZ82vVtgckt8pof-Y";
    const EC_Y: &str = "lz3mjqFj7qj3ZlOv3_uxkJvoOYtjw6Q_MkSV7rdQjvk";
    const ED_X: &str = "zRGoD-Ue1wXAa2KWYQjtp3ckNvJ_L4yfLCu0Z3gNRYg";

    /// A loopback HTTP server that answers every request with `status` and
    /// `body`, counting how many requests it served. This is the cache's only
    /// boundary, so mocking it here is what lets the TTL and refresh decisions
    /// be asserted against a real network round trip.
    ///
    /// Each connection is handled on its own thread, and the request is read
    /// to completion before the response is written, so a client that pipelines
    /// or that is slow to send cannot stall the accept loop.
    struct StubJwksServer {
        addr: SocketAddr,
        requests: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl StubJwksServer {
        /// Serve `status`/`body` forever on an ephemeral loopback port.
        fn serving(status: u16, body: String) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let addr = listener.local_addr().expect("loopback address");
            let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counter = Arc::clone(&requests);
            let response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let response = Arc::new(response);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { break };
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let response = Arc::clone(&response);
                    std::thread::spawn(move || serve_one(stream, &response));
                }
            });
            Self { addr, requests }
        }

        fn uri(&self) -> String {
            format!("http://{}/jwks", self.addr)
        }

        fn request_count(&self) -> usize {
            self.requests.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Read one HTTP request off `stream` and answer it with `response`.
    /// The request head is read until the blank line that ends the headers,
    /// then any declared body is drained, so the client sees a well-formed
    /// exchange rather than a truncated one.
    fn serve_one(mut stream: std::net::TcpStream, response: &str) {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while let Ok(1) = stream.read(&mut byte) {
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }

    /// A JWKS document holding a single RSA key with the given kid and alg.
    fn rsa_document(kid: &str, alg: Option<&str>) -> String {
        let alg = match alg {
            Some(alg) => format!(r#","alg":"{alg}""#),
            None => String::new(),
        };
        format!(r#"{{"keys":[{{"kid":"{kid}","kty":"RSA","n":"{RSA_N}","e":"{RSA_E}"{alg}}}]}}"#)
    }

    fn cache_for(server: &StubJwksServer, ttl: Duration) -> JwksCache {
        JwksCache::from_parts(reqwest::Client::new(), server.uri(), ttl)
    }

    #[tokio::test]
    async fn find_key_returns_none_before_any_refresh() {
        let cache = JwksCache::from_parts(
            reqwest::Client::new(),
            "http://127.0.0.1:1/jwks".to_string(),
            Duration::from_secs(300),
        );

        let observed = cache.find_key("key-a").expect("cache read");

        assert!(observed.is_none());
    }

    #[tokio::test]
    async fn key_for_fetches_and_returns_the_key() {
        let server = StubJwksServer::serving(200, rsa_document("key-a", Some("RS256")));
        let cache = cache_for(&server, Duration::from_secs(300));

        let observed = cache.key_for("key-a").await;

        assert!(observed.is_ok(), "the cached kid must resolve");
    }

    #[tokio::test]
    async fn key_for_errors_on_an_unknown_key_id() {
        let server = StubJwksServer::serving(200, rsa_document("key-a", Some("RS256")));
        let cache = cache_for(&server, Duration::from_secs(300));

        let observed = cache.key_for("key-z").await;

        assert!(
            observed.is_err(),
            "a kid absent from the document must fail"
        );
    }

    #[tokio::test]
    async fn key_for_serves_a_second_lookup_from_the_cache() {
        let server = StubJwksServer::serving(200, rsa_document("key-a", Some("RS256")));
        let cache = cache_for(&server, Duration::from_secs(300));
        cache.key_for("key-a").await.expect("first lookup");
        let after_first = server.request_count();

        let _ = cache.key_for("key-a").await;

        assert_eq!(
            server.request_count(),
            after_first,
            "a fresh cache must not refetch inside its TTL"
        );
    }

    #[tokio::test]
    async fn key_for_refetches_once_the_ttl_has_expired() {
        let server = StubJwksServer::serving(200, rsa_document("key-a", Some("RS256")));
        // A zero TTL means every entry is already stale on arrival.
        let cache = cache_for(&server, Duration::ZERO);
        cache.key_for("key-a").await.expect("first lookup");
        let after_first = server.request_count();

        let _ = cache.key_for("key-a").await;

        assert_eq!(
            server.request_count(),
            after_first + 1,
            "an expired cache must refetch"
        );
    }

    #[tokio::test]
    async fn refresh_records_the_fetch_time() {
        let server = StubJwksServer::serving(200, rsa_document("key-a", Some("RS256")));
        let cache = cache_for(&server, Duration::ZERO);

        let observed = cache.refresh().await;

        assert!(observed.is_ok());
    }

    #[tokio::test]
    async fn refresh_makes_the_key_findable() {
        let server = StubJwksServer::serving(200, rsa_document("key-a", Some("RS256")));
        let cache = cache_for(&server, Duration::ZERO);

        cache.refresh().await.expect("refresh");

        assert!(cache.find_key("key-a").expect("cache read").is_some());
    }

    #[tokio::test]
    async fn refresh_fails_when_the_provider_returns_a_server_error() {
        let server = StubJwksServer::serving(500, "upstream exploded".to_string());
        let cache = cache_for(&server, Duration::ZERO);

        let observed = cache.refresh().await;

        assert!(observed.is_err(), "a 5xx from the provider must surface");
    }

    #[tokio::test]
    async fn refresh_fails_when_the_body_is_not_a_jwks_document() {
        let server = StubJwksServer::serving(200, "not json".to_string());
        let cache = cache_for(&server, Duration::ZERO);

        let observed = cache.refresh().await;

        assert!(observed.is_err(), "an unparseable body must surface");
    }

    #[tokio::test]
    async fn refresh_fails_when_the_endpoint_is_unreachable() {
        let cache = JwksCache::from_parts(
            reqwest::Client::new(),
            "http://127.0.0.1:1/jwks".to_string(),
            Duration::ZERO,
        );

        let observed = cache.refresh().await;

        assert!(observed.is_err(), "a connection refusal must surface");
    }

    #[tokio::test]
    async fn refresh_keeps_an_ec_p256_key() {
        let body = format!(
            r#"{{"keys":[{{"kid":"ec-1","kty":"EC","crv":"P-256","x":"{EC_X}","y":"{EC_Y}"}}]}}"#
        );
        let server = StubJwksServer::serving(200, body);
        let cache = cache_for(&server, Duration::ZERO);

        cache.refresh().await.expect("refresh");

        assert!(cache.find_key("ec-1").expect("cache read").is_some());
    }

    #[tokio::test]
    async fn refresh_keeps_an_ed25519_key() {
        let body =
            format!(r#"{{"keys":[{{"kid":"ed-1","kty":"OKP","crv":"Ed25519","x":"{ED_X}"}}]}}"#);
        let server = StubJwksServer::serving(200, body);
        let cache = cache_for(&server, Duration::ZERO);

        cache.refresh().await.expect("refresh");

        assert!(cache.find_key("ed-1").expect("cache read").is_some());
    }

    #[tokio::test]
    async fn refresh_skips_a_key_with_an_unsupported_curve() {
        let body = format!(
            r#"{{"keys":[{{"kid":"ec-1","kty":"EC","crv":"P-384","x":"{EC_X}","y":"{EC_Y}"}}]}}"#
        );
        let server = StubJwksServer::serving(200, body);
        let cache = cache_for(&server, Duration::ZERO);

        cache.refresh().await.expect("refresh");

        assert!(
            cache.find_key("ec-1").expect("cache read").is_none(),
            "a curve the decoder cannot build must be skipped, not fatal"
        );
    }

    #[tokio::test]
    async fn refresh_skips_a_key_with_an_unknown_key_type() {
        let body = r#"{"keys":[{"kid":"oct-1","kty":"oct","k":"AAAA"}]}"#;
        let server = StubJwksServer::serving(200, body.to_string());
        let cache = cache_for(&server, Duration::ZERO);

        cache.refresh().await.expect("refresh");

        assert!(cache.find_key("oct-1").expect("cache read").is_none());
    }

    #[tokio::test]
    async fn refresh_keeps_a_key_without_an_alg() {
        let server = StubJwksServer::serving(200, rsa_document("key-a", None));
        let cache = cache_for(&server, Duration::ZERO);

        cache.refresh().await.expect("refresh");

        assert!(cache.find_key("key-a").expect("cache read").is_some());
    }

    #[tokio::test]
    async fn refresh_skips_a_key_pinned_to_a_disallowed_algorithm() {
        let server = StubJwksServer::serving(200, rsa_document("key-a", Some("HS256")));
        let cache = cache_for(&server, Duration::ZERO);

        cache.refresh().await.expect("refresh");

        assert!(
            cache.find_key("key-a").expect("cache read").is_none(),
            "an algorithm outside the allowlist must be skipped"
        );
    }

    /// A provider that signs an ID token with RS384 or RS512 is *allowed* to
    /// do so: `resolve_allowed_algorithms` accepts those under both an explicit
    /// pin and `auto` (Rauthy advertises all four). The cache used to filter
    /// keys down to `{RS256, ES256, EdDSA}` alone, so such a key was dropped
    /// and `key_for` reported `unknown key id` — surfacing as 503 rather than
    /// as the authorization failure it actually was. The filter has to accept
    /// every algorithm the resolver can accept.
    #[tokio::test]
    async fn refresh_keeps_a_key_pinned_to_a_non_default_rsa_algorithm() {
        let server = StubJwksServer::serving(200, rsa_document("key-a", Some("RS384")));
        let cache = cache_for(&server, Duration::ZERO);

        cache.refresh().await.expect("refresh");

        assert!(
            cache.find_key("key-a").expect("cache read").is_some(),
            "RS384 is in the allowed set, so its key must survive the filter"
        );
    }

    /// The regression guard for the three lists that used to disagree
    /// (`client::SUPPORTED`, the config validator's literal, and this filter):
    /// whatever the resolver accepts, the cache must be able to load.
    #[tokio::test]
    async fn refresh_keeps_a_key_for_every_allowed_algorithm() {
        for alg in SUPPORTED_ID_TOKEN_ALGORITHMS {
            let server = StubJwksServer::serving(200, rsa_document("key-a", Some(alg)));
            let cache = cache_for(&server, Duration::ZERO);

            cache.refresh().await.expect("refresh");

            assert!(
                cache.find_key("key-a").expect("cache read").is_some(),
                "{alg} is in SUPPORTED_ID_TOKEN_ALGORITHMS, so the JWKS filter \
                 must not drop its key"
            );
        }
    }

    /// The guard above is RSA-shaped, so on its own it would happily go green
    /// for an `ES384` entry whose key the *curve* arm still drops — the same
    /// silent loss this change removed, one level up. Each EC/OKP entry is
    /// therefore published as the key type a provider would really use, and
    /// must come back. An algorithm added to the constant without a matching
    /// `kty`/`crv` arm fails here rather than in production.
    #[tokio::test]
    async fn refresh_keeps_a_key_of_the_right_type_for_every_ec_algorithm() {
        // The curve each allowed EC/OKP algorithm implies. Every `ES*` entry in
        // the constant must have one: an algorithm with no curve here cannot
        // be published as a real key, and `refresh` would drop it at the
        // `kty`/`crv` arm while the `alg` filter above happily passed it —
        // precisely the silent loss this change removed, one level up. So an
        // unmapped curve is a failure here, not a skipped case.
        let curve_for = |alg: &str| -> Option<&'static str> {
            match alg {
                "ES256" => Some("P-256"),
                "EdDSA" => Some("Ed25519"),
                _ => None,
            }
        };
        for alg in SUPPORTED_ID_TOKEN_ALGORITHMS {
            if alg.starts_with("ES") {
                assert!(
                    curve_for(alg).is_some(),
                    "{alg} is allowed but no EC curve is decoded for it: add the \
                     `kty`/`crv` arm in `refresh`, or drop {alg} from \
                     SUPPORTED_ID_TOKEN_ALGORITHMS"
                );
            }
        }

        for alg in SUPPORTED_ID_TOKEN_ALGORITHMS {
            let Some(crv) = curve_for(alg) else {
                // An RSA entry; the sibling test covers its filter behaviour.
                continue;
            };
            let (kty, x, y) = match crv {
                "P-256" => ("EC", EC_X, Some(EC_Y)),
                "Ed25519" => ("OKP", ED_X, None),
                other => unreachable!("unhandled curve {other}"),
            };
            let y_field = y
                .map(|value| format!(r#","y":"{value}""#))
                .unwrap_or_default();
            let body = format!(
                r#"{{"keys":[{{"kid":"key-{alg}","kty":"{kty}","crv":"{crv}","x":"{x}"{y_field}}}]}}"#
            );
            let server = StubJwksServer::serving(200, body);
            let cache = cache_for(&server, Duration::ZERO);

            cache.refresh().await.expect("refresh");

            assert!(
                cache
                    .find_key(&format!("key-{alg}"))
                    .expect("cache read")
                    .is_some(),
                "{alg} is allowed, so its {crv} key must load — a JWKS \
                 `alg` filter that passes is not enough; the curve arm must \
                 decode it too"
            );
        }
    }

    #[tokio::test]
    async fn refresh_fails_when_a_key_cannot_be_decoded() {
        let body = r#"{"keys":[{"kid":"bad","kty":"RSA","n":"not-base64!!","e":"AQAB"}]}"#;
        let server = StubJwksServer::serving(200, body.to_string());
        let cache = cache_for(&server, Duration::ZERO);

        let observed = cache.refresh().await;

        assert!(
            observed.is_err(),
            "a malformed key must surface rather than silently vanish"
        );
    }
}
