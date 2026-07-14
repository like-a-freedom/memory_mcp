use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
