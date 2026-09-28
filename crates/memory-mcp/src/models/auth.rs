//! Browser authentication methods.
//!
//! A pure shared value: which door a deployment serves. It is a kernel
//! type because the control-transaction ports name it alongside the
//! account and identity payloads, and those live below `http`.

use serde::Deserialize;

pub const AUTH_METHOD_LOCAL: &str = "local";
pub const AUTH_METHOD_OIDC: &str = "oidc";

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAuthMethod {
    Local,
    Oidc,
}

impl BrowserAuthMethod {
    /// Every method a deployment can serve. The set is closed: adding one
    /// touches the durable schema, the router, the login page and the removal
    /// guard, which is why the guard may name the tokens individually.
    pub const ALL: [Self; 2] = [Self::Local, Self::Oidc];

    /// The durable token for this method, as stored in `browser_auth_policy`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => AUTH_METHOD_LOCAL,
            Self::Oidc => AUTH_METHOD_OIDC,
        }
    }

    /// Parse a durable token, or `None` for an unknown method so the caller can
    /// fail closed with its own error type.
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            AUTH_METHOD_LOCAL => Some(Self::Local),
            AUTH_METHOD_OIDC => Some(Self::Oidc),
            _ => None,
        }
    }

    /// The enabled methods in the canonical order every representation uses:
    /// `local` before `oidc`.
    ///
    /// A set has one representation however it was enumerated, which is what
    /// lets the durable row, the configuration and the login page be compared
    /// as values instead of as sets.
    pub fn canonical_set(desired: &[Self]) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|method| desired.contains(method))
            .collect()
    }
}
