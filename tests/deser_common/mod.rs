//! Shared proptest strategies for the deserializer-hardening tests.
//!
//! Each exchange's `*_deser_proptest.rs` file pulls this in via
//! `#[path = "deser_common/mod.rs"] mod deser_common;`. It lives in a
//! subdirectory so Cargo does not treat it as its own test binary.
//!
//! The strategies deliberately generate adversarial JSON — wrong-arity
//! arrays, non-numeric strings where numbers are expected, nulls, nested
//! objects, huge/negative numbers — so the fuzz tests can assert the one
//! guarantee that matters on a live-money data path: the hand-rolled
//! deserializers return a `Result`, never panic.
#![allow(dead_code, missing_docs)]
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

use proptest::prelude::*;
use serde_json::{Map, Value};

/// A single "wire-scalar" value: the sort of thing that shows up inside an
/// exchange's positional array — a numeric string, a garbage string, an
/// integer, a float, a bool, or null.
pub fn arb_scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        // Only finite floats — JSON cannot represent NaN/Inf, and
        // `serde_json::Value::from(f64)` maps non-finite to Null anyway.
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(Value::from),
        // Realistic numeric strings, including signs, exponents and junk.
        arb_numeric_string().prop_map(Value::String),
        "\\PC{0,8}".prop_map(Value::String),
    ]
}

/// Strings that plausibly appear where a price/size string is expected —
/// a mix of well-formed decimals and things that must fail to parse.
pub fn arb_numeric_string() -> impl Strategy<Value = String> {
    prop_oneof![
        // Well-formed decimals.
        (0i64..1_000_000_000, 0u32..1_000_000_000).prop_map(|(a, b)| format!("{a}.{b}")),
        // Plain integers.
        any::<i64>().prop_map(|n| n.to_string()),
        // Scientific notation.
        Just("1e309".to_string()), // overflows f64 -> inf on parse
        Just("-0.0".to_string()),
        Just("inf".to_string()),
        Just("NaN".to_string()),
        Just(String::new()),
        Just("abc".to_string()),
        Just("   ".to_string()),
        Just("0x10".to_string()),
    ]
}

/// A JSON array of `min..max` arbitrary scalars — stresses positional
/// (tuple / `visit_seq`) deserializers with wrong arity and bad element types.
pub fn arb_scalar_array(min: usize, max: usize) -> impl Strategy<Value = Value> {
    prop::collection::vec(arb_scalar(), min..max).prop_map(Value::Array)
}

/// Fully arbitrary, bounded-depth JSON — null / bool / number / string /
/// array / object. The broadest adversarial driver: feed it to any
/// deserializer and assert it never panics.
pub fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = arb_scalar();
    leaf.prop_recursive(4, 32, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::vec(("[a-z]{1,5}", inner), 0..6).prop_map(|entries| {
                Value::Object(entries.into_iter().collect::<Map<String, Value>>())
            }),
        ]
    })
}

/// A finite, non-negative price-like `f64` for round-trip structural tests.
/// Rust's `f64` `Display` produces the shortest string that parses back to
/// the exact same value, so callers can assert exact equality.
pub fn arb_price() -> impl Strategy<Value = f64> {
    0.0f64..1_000_000_000.0
}
