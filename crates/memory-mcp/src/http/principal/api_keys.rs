//! API key parser (ADR-0052, plan §4.3).
//!
//! The structured key format is `mem_sk_<key_id>_<secret>` where
//! `key_id = ak_<uuid v4>`. Splitting on `_` yields
//! `["mem", "sk", "ak", "<uuid>", <secret parts>...]`; the parser
//! must reassemble `key_id` from the `ak` marker + uuid part, and
//! the secret may itself contain `_`. The secret length and
//! character set are constrained to detect malformed credentials
//! early without leaking information about the rejected shape.

use subtle::ConstantTimeEq;

use crate::error::MemoryError;

const MAX_LEN: usize = 200;
const MIN_SECRET_LEN: usize = 32;

#[derive(Debug, Clone)]
pub struct ApiKeyCredential {
    key_id: String,
    secret: Vec<u8>,
}

impl ApiKeyCredential {
    pub fn parse(raw: &str) -> Result<Self, MemoryError> {
        if raw.len() > MAX_LEN {
            return Err(MemoryError::Auth("api key length".into()));
        }
        let mut parts = raw.split('_');
        let prefix = parts
            .next()
            .ok_or_else(|| MemoryError::Auth("api key prefix".into()))?;
        let kind = parts
            .next()
            .ok_or_else(|| MemoryError::Auth("api key kind".into()))?;
        let marker = parts
            .next()
            .ok_or_else(|| MemoryError::Auth("api key marker".into()))?;
        if prefix != "mem" || kind != "sk" || marker != "ak" {
            return Err(MemoryError::Auth("api key prefix".into()));
        }
        let uuid_part = parts
            .next()
            .ok_or_else(|| MemoryError::Auth("api key id".into()))?;
        // uuid v4 canonical shape: 8-4-4-4-12 hex digits with hyphens.
        if !is_canonical_uuid(uuid_part) {
            return Err(MemoryError::Auth("api key id shape".into()));
        }
        let key_id = format!("ak_{uuid_part}");
        // The secret is everything remaining; it may contain underscores.
        let secret: String = parts.collect::<Vec<_>>().join("_");
        if secret.is_empty() {
            return Err(MemoryError::Auth("api key secret".into()));
        }
        if secret.len() < MIN_SECRET_LEN
            || !secret
                .chars()
                .all(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_'))
        {
            return Err(MemoryError::Auth("api key secret".into()));
        }
        Ok(Self {
            key_id,
            secret: secret.into_bytes(),
        })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }
    pub fn secret(&self) -> &[u8] {
        &self.secret
    }
    pub fn constant_time_eq(&self, other: &Self) -> bool {
        self.secret.ct_eq(&other.secret).into()
    }
}

fn is_canonical_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, &b) in bytes.iter().enumerate() {
        let is_hyphen_pos = i == 8 || i == 13 || i == 18 || i == 23;
        if is_hyphen_pos {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn well_formed_key() -> String {
        // 36-char uuid v4 shape + 40-char urlsafe secret
        "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_Ab3defghij0123456789Ab3defghij0123456789"
            .to_string()
    }

    #[test]
    fn parses_well_formed_key() {
        let key = ApiKeyCredential::parse(&well_formed_key()).unwrap();
        assert_eq!(key.key_id(), "ak_01234567-89ab-4cde-8f01-23456789abcd");
        assert_eq!(key.secret().len(), 40);
    }

    #[test]
    fn secret_may_contain_underscores() {
        let raw = "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_secret_with_underscores_and_32_plus_chars";
        let key = ApiKeyCredential::parse(raw).unwrap();
        assert_eq!(
            std::str::from_utf8(key.secret()).unwrap(),
            "secret_with_underscores_and_32_plus_chars"
        );
    }

    #[test]
    fn rejects_wrong_prefix() {
        assert!(ApiKeyCredential::parse("sk_mem_xxx").is_err());
        assert!(ApiKeyCredential::parse("mem_sk_").is_err());
        assert!(ApiKeyCredential::parse("mem_sk_onlyone").is_err());
        assert!(ApiKeyCredential::parse(
            "mem_tk_ak_01234567-89ab-4cde-8f01-23456789abcd_abcdefabcdefabcdefabcdefabcdefabcd"
        )
        .is_err());
        assert!(ApiKeyCredential::parse(
            "mem_sk_xx_01234567-89ab-4cde-8f01-23456789abcd_abcdefabcdefabcdefabcdefabcdefabcd"
        )
        .is_err());
    }

    #[test]
    fn rejects_over_max_length() {
        let s = "a".repeat(1024);
        assert!(ApiKeyCredential::parse(&format!(
            "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_{s}"
        ))
        .is_err());
    }

    #[test]
    fn rejects_non_urlsafe_characters() {
        assert!(ApiKeyCredential::parse(
            "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_secret with space padding padding"
        )
        .is_err());
    }

    #[test]
    fn rejects_short_secret() {
        assert!(ApiKeyCredential::parse(
            "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_tooshort"
        )
        .is_err());
    }

    #[test]
    fn rejects_missing_secret() {
        assert!(
            ApiKeyCredential::parse("mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd").is_err()
        );
    }

    #[test]
    fn constant_time_eq_for_secrets() {
        let a = ApiKeyCredential::parse(&well_formed_key()).unwrap();
        let b = ApiKeyCredential::parse(&well_formed_key()).unwrap();
        let c = ApiKeyCredential::parse(
            "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_Bb3defghij0123456789Ab3defghij0123456789",
        )
        .unwrap();
        assert!(a.constant_time_eq(&b));
        assert!(!a.constant_time_eq(&c));
    }

    #[test]
    fn rejects_bad_uuid_shape() {
        // 35 chars (missing one)
        assert!(ApiKeyCredential::parse(
            "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abc_Ab3defghij0123456789Ab3defghij0123456789"
        )
        .is_err());
        // 37 chars (one extra)
        assert!(ApiKeyCredential::parse(
            "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcde_Ab3defghij0123456789Ab3defghij0123456789"
        )
        .is_err());
        // wrong hyphen position
        assert!(ApiKeyCredential::parse(
            "mem_sk_ak_0123456789ab-4cde-8f01-23456789abcd_Ab3defghij0123456789Ab3defghij0123456789"
        )
        .is_err());
    }
}
