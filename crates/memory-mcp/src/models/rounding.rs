//! Serde helpers for rounding f64 values to 2 decimal places in JSON responses.
//!
//! These are applied to MCP response types via `#[serde(serialize_with = ...)]`
//! so that wire-format numbers are concise and legible without sacrificing
//! internal computation precision.

/// Serialize an `f64` rounded to two decimal places.
pub fn round_2<S: serde::Serializer>(val: &f64, s: S) -> Result<S::Ok, S::Error> {
    let rounded = (val * 100.0).round() / 100.0;
    serde::Serialize::serialize(&rounded, s)
}

/// Serialize an `Option<f64>` rounded to two decimal places when `Some`.
pub fn round_2_opt<S: serde::Serializer>(val: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match val {
        Some(v) => {
            let rounded = (v * 100.0).round() / 100.0;
            serde::Serialize::serialize(&Some(rounded), s)
        }
        None => serde::Serialize::serialize(&None::<f64>, s),
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::{round_2, round_2_opt};

    /// Helper: round an f64 using the same formula the serde functions apply.
    fn round_2_val(v: f64) -> f64 {
        (v * 100.0).round() / 100.0
    }

    /// A bare-bones struct to test serde `round_2` on `f64`.
    #[derive(Serialize)]
    struct Wrapped {
        #[serde(serialize_with = "round_2")]
        val: f64,
    }

    /// A bare-bones struct to test serde `round_2_opt` on `Option<f64>`.
    #[derive(Serialize)]
    struct WrappedOpt {
        #[serde(
            serialize_with = "round_2_opt",
            skip_serializing_if = "Option::is_none"
        )]
        val: Option<f64>,
    }

    #[test]
    fn helper_rounds_standard_value() {
        assert_eq!(round_2_val(0.85), 0.85);
    }

    #[test]
    fn helper_rounds_full_precision_f64() {
        // serde serializes f64 with minimal representation, so
        // 0.8500000000000001 becomes 0.85 after rounding.
        let json = serde_json::to_string(&Wrapped {
            val: 0.8500000000000001,
        })
        .unwrap();
        assert_eq!(json, r#"{"val":0.85}"#);
    }

    #[test]
    fn helper_rounds_one_third() {
        let json = serde_json::to_string(&Wrapped { val: 1.0 / 3.0 }).unwrap();
        assert_eq!(json, r#"{"val":0.33}"#);
    }

    #[test]
    fn helper_rounds_near_one_to_one() {
        let json = serde_json::to_string(&Wrapped { val: 0.999999999 }).unwrap();
        assert_eq!(json, r#"{"val":1.0}"#);
    }

    #[test]
    fn helper_rounds_multi_digit() {
        let json = serde_json::to_string(&Wrapped { val: 1.23456789 }).unwrap();
        assert_eq!(json, r#"{"val":1.23}"#);
    }

    #[test]
    fn helper_rounds_zero() {
        let json = serde_json::to_string(&Wrapped { val: 0.0 }).unwrap();
        assert_eq!(json, r#"{"val":0.0}"#);
    }

    #[test]
    fn helper_rounds_small_below_threshold_to_zero() {
        let json = serde_json::to_string(&Wrapped { val: 0.0049999999 }).unwrap();
        assert_eq!(json, r#"{"val":0.0}"#);
    }

    #[test]
    fn helper_rounds_small_at_threshold_to_0_01() {
        let json = serde_json::to_string(&Wrapped { val: 0.005 }).unwrap();
        assert_eq!(json, r#"{"val":0.01}"#);
    }

    // --- Option<f64> helpers ---

    #[test]
    fn helper_rounds_opt_some() {
        let json = serde_json::to_string(&WrappedOpt { val: Some(0.9) }).unwrap();
        assert_eq!(json, r#"{"val":0.9}"#);
    }

    #[test]
    fn helper_rounds_opt_some_full_precision() {
        let json = serde_json::to_string(&WrappedOpt {
            val: Some(0.3333333333333333),
        })
        .unwrap();
        assert_eq!(json, r#"{"val":0.33}"#);
    }

    #[test]
    fn helper_skips_opt_none() {
        let json = serde_json::to_string(&WrappedOpt { val: None }).unwrap();
        assert_eq!(json, r#"{}"#);
    }
}
