//! Compact-mode state for token-efficient serialization.
//!
//! This module is not a service — it is a scoped serialization toggle used
//! by serde helpers on response structs. The guard is dropped after the
//! final JSON serialization completes.

use std::cell::Cell;

thread_local! {
    static COMPACT_MODE: Cell<bool> = const { Cell::new(false) };
}

/// RAII guard for compact mode: sets the flag on entry, restores previous on drop.
/// Must be dropped after the final serde serialization, not before.
pub struct CompactGuard {
    prev: bool,
}

/// Enable or disable compact mode for the remainder of the current function scope.
/// The returned guard must be held until response serialization completes.
pub fn set_compact(compact: bool) -> CompactGuard {
    let prev = COMPACT_MODE.with(|c| c.replace(compact));
    CompactGuard { prev }
}

impl Drop for CompactGuard {
    fn drop(&mut self) {
        COMPACT_MODE.with(|c| c.set(self.prev));
    }
}

/// Check whether compact mode is active.
pub fn is_compact() -> bool {
    COMPACT_MODE.with(|c| c.get())
}

/// serde `skip_serializing_if` fn — skips the field when compact mode is on.
/// Used on fields like `quote` that are redundant in compact output.
pub fn skip_if_compact<T>(_value: &T) -> bool {
    is_compact()
}

/// Custom serializer for `rationale`. Under compact mode, emits only the
/// leading `tier=<tier>` token; otherwise passes the string through.
/// Must be `pub` because `models/request.rs` references it.
pub fn serialize_rationale<S: serde::Serializer>(
    rationale: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if is_compact() {
        let tier = rationale
            .split_whitespace()
            .next()
            .unwrap_or("tier=unknown");
        serializer.serialize_str(tier)
    } else {
        serializer.serialize_str(rationale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_mode_is_off_by_default() {
        // Guard against leaked state from other tests in same thread.
        assert!(!is_compact());
    }

    #[test]
    fn set_compact_true_and_restore_on_drop() {
        assert!(!is_compact());
        {
            let _guard = set_compact(true);
            assert!(is_compact());
        }
        assert!(!is_compact(), "guard must restore previous state on drop");
    }

    #[test]
    fn set_compact_false_is_noop() {
        let _guard = set_compact(false);
        assert!(!is_compact());
    }

    #[test]
    fn nested_guards_restore_correctly() {
        {
            let _outer = set_compact(true);
            assert!(is_compact());
            {
                let _inner = set_compact(false);
                assert!(!is_compact());
            }
            assert!(is_compact(), "inner drop must restore outer value");
        }
        assert!(!is_compact());
    }

    #[test]
    fn skip_if_compact_respects_mode() {
        assert!(!skip_if_compact(&42));
        {
            let _guard = set_compact(true);
            assert!(skip_if_compact(&42));
        }
        assert!(!skip_if_compact(&42));
    }

    // Exercises the serialize_with attribute directly.
    #[derive(serde::Serialize)]
    struct TestRationale {
        #[serde(serialize_with = "serialize_rationale")]
        rationale: String,
    }

    #[test]
    fn serialize_rationale_compact_emits_tier_only() {
        let _guard = set_compact(true);
        let val = serde_json::to_value(TestRationale {
            rationale: "tier=direct fts=0.85 access_count=3 confidence=0.92".to_string(),
        })
        .unwrap();
        assert_eq!(val["rationale"].as_str().unwrap(), "tier=direct");
    }

    #[test]
    fn serialize_rationale_verbose_passes_through() {
        let _guard = set_compact(false);
        let val = serde_json::to_value(TestRationale {
            rationale: "tier=direct fts=0.85 access_count=3 confidence=0.92".to_string(),
        })
        .unwrap();
        assert_eq!(
            val["rationale"].as_str().unwrap(),
            "tier=direct fts=0.85 access_count=3 confidence=0.92"
        );
    }

    #[test]
    fn serialize_rationale_compact_handles_empty() {
        let _guard = set_compact(true);
        let val = serde_json::to_value(TestRationale {
            rationale: String::new(),
        })
        .unwrap();
        assert_eq!(val["rationale"].as_str().unwrap(), "tier=unknown");
    }

    // Exercises the skip_serializing_if attribute directly.
    #[derive(serde::Serialize)]
    struct TestQuote {
        content: String,
        #[serde(skip_serializing_if = "skip_if_compact")]
        quote: String,
    }

    #[test]
    fn quote_skipped_when_compact() {
        let _guard = set_compact(true);
        let val = serde_json::to_value(&TestQuote {
            content: "main content".to_string(),
            quote: "main content".to_string(),
        })
        .unwrap();
        assert!(val.get("content").is_some());
        assert!(
            val.get("quote").is_none(),
            "quote must be omitted in compact"
        );
    }

    #[test]
    fn quote_present_when_verbose() {
        let _guard = set_compact(false);
        let val = serde_json::to_value(&TestQuote {
            content: "main content".to_string(),
            quote: "main content".to_string(),
        })
        .unwrap();
        assert_eq!(val["quote"].as_str().unwrap(), "main content");
    }
}
