use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::service::MemoryError;

macro_rules! define_id_type {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    };
}

define_id_type!(EpisodeId, "Unique identifier for an episode.");
define_id_type!(EntityId, "Unique identifier for an entity.");
define_id_type!(FactId, "Unique identifier for a fact.");
define_id_type!(CommunityId, "Unique identifier for a community.");
define_id_type!(EdgeId, "Unique identifier for an edge.");

// ─── Validated Claim IDs ──────────────────────────────────────────────────────

macro_rules! define_validated_id {
    ($name:ident, $doc:expr, $prefix:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, JsonSchema)]
        pub struct $name(String);

        impl $name {
            /// The table prefix this ID belongs to.
            pub const PREFIX: &'static str = $prefix;

            /// Construct from a string that already carries the correct prefix.
            /// Returns `Err` when the prefix is wrong or the body is empty.
            pub fn new(raw: &str) -> Result<Self, MemoryError> {
                Self::from_str(raw)
            }

            /// Internal constructor from a pre-validated string (must include prefix).
            /// Only callable within the crate.
            #[allow(dead_code)]
            pub(crate) fn from_raw(raw: String) -> Self {
                Self(raw)
            }

            /// The raw string without the prefix.
            #[must_use]
            pub fn body(&self) -> &str {
                &self.0[$prefix.len()..]
            }
        }

        impl std::str::FromStr for $name {
            type Err = MemoryError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                if !s.starts_with($prefix) {
                    return Err(MemoryError::Validation(format!(
                        "{}: expected prefix '{}', got '{}'",
                        stringify!($name),
                        $prefix,
                        &s[..s.len().min(24)]
                    )));
                }
                if s.len() == $prefix.len() {
                    return Err(MemoryError::Validation(format!(
                        "{}: empty body after prefix",
                        stringify!($name)
                    )));
                }
                Ok(Self(s.to_string()))
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                Self::from_str(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

define_validated_id!(ClaimId, "Validated identifier for a claim.", "claim:");
define_validated_id!(
    ClaimRelationId,
    "Validated identifier for a claim relation.",
    "claim_relation:"
);
define_validated_id!(
    ClaimJobId,
    "Validated identifier for a claim job.",
    "claim_job:"
);
