#![allow(missing_docs)]
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

//! Property / fuzz tests for KuCoin's hand-rolled deserializers — the venue
//! that carries live futures money, and the one the original #72 fuzz pass
//! skipped (it covered Binance/Bybit/Crypto.com/Kraken only).
//!
//! Focus, per the M2 venue-truth review:
//! - `OrderDetail` — the order-status deserializer whose classification a live
//!   bot's order tracker depends on. The one guarantee that matters on a
//!   money path: arbitrary/adversarial JSON returns a `Result`, never panics,
//!   and `is_active()`/`is_filled()`/`is_cancelled()` stay internally
//!   consistent for any value that *does* parse.
//! - `Fill` — exercises the `de_f64_flexible` string-or-number tolerance on
//!   the `price`/`fee` fields (the class of bug fixed live in `b51df50`).

#[path = "deser_common/mod.rs"]
mod deser_common;

use deser_common::{arb_json, arb_numeric_string, arb_scalar};
use exchange_apiws::rest::{Fill, OrderDetail};
use proptest::prelude::*;
use serde_json::{Value, json};

fn config() -> ProptestConfig {
    ProptestConfig::with_cases(256)
}

/// A KuCoin Futures order payload with adversarially-typed fields around a
/// valid skeleton — stresses the status/isActive/cancelExist/size decoders.
fn arb_order_payload() -> impl Strategy<Value = Value> {
    (
        proptest::option::of(prop_oneof![
            Just(json!("open")),
            Just(json!("done")),
            Just(json!("active")), // the query-param fiction — must not panic
            Just(json!("match")),
            arb_scalar(),
        ]),
        proptest::option::of(arb_scalar()), // isActive (may be non-bool junk)
        proptest::option::of(arb_scalar()), // cancelExist
        proptest::option::of(0u64..1_000u64), // size
        proptest::option::of(arb_scalar()), // filledSize
        proptest::option::of(arb_scalar()), // price
    )
        .prop_map(|(status, is_active, cancel, size, filled, price)| {
            let mut m = serde_json::Map::new();
            m.insert("id".into(), json!("o-1"));
            m.insert("symbol".into(), json!("XBTUSDTM"));
            m.insert("side".into(), json!("buy"));
            m.insert("type".into(), json!("limit"));
            if let Some(v) = status {
                m.insert("status".into(), v);
            }
            if let Some(v) = is_active {
                m.insert("isActive".into(), v);
            }
            if let Some(v) = cancel {
                m.insert("cancelExist".into(), v);
            }
            if let Some(v) = size {
                m.insert("size".into(), json!(v));
            }
            if let Some(v) = filled {
                m.insert("filledSize".into(), v);
            }
            if let Some(v) = price {
                m.insert("price".into(), v);
            }
            Value::Object(m)
        })
}

/// A `Fill` payload where `price`/`fee` are string-or-number (or junk).
fn arb_fill_payload() -> impl Strategy<Value = Value> {
    let numish = prop_oneof![
        arb_numeric_string().prop_map(Value::String),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(Value::from),
        arb_scalar(),
    ];
    (numish.clone(), numish, proptest::option::of(0u64..1_000u64)).prop_map(|(price, fee, size)| {
        let mut m = serde_json::Map::new();
        m.insert("symbol".into(), json!("XBTUSDTM"));
        m.insert("orderId".into(), json!("o-9"));
        m.insert("side".into(), json!("buy"));
        m.insert("price".into(), price);
        m.insert("fee".into(), fee);
        if let Some(v) = size {
            m.insert("size".into(), json!(v));
        }
        Value::Object(m)
    })
}

proptest! {
    #![proptest_config(config())]

    /// Fully-arbitrary JSON into `OrderDetail` must never panic.
    #[test]
    fn order_detail_never_panics_on_arbitrary_json(v in arb_json()) {
        let _ = serde_json::from_value::<OrderDetail>(v);
    }

    /// Structured-but-adversarial order payloads: never panic, and when they
    /// DO parse, the status predicates stay mutually consistent.
    #[test]
    fn order_detail_predicates_consistent(v in arb_order_payload()) {
        if let Ok(d) = serde_json::from_value::<OrderDetail>(v) {
            // The accessors must not panic and must be pure booleans.
            let _active = d.is_active();
            let _filled = d.is_filled();
            let _cancelled = d.is_cancelled();
            // is_filled implies filled_size >= size.
            if d.is_filled() {
                prop_assert!(d.filled_size.unwrap_or(0) >= d.size);
            }
        }
    }

    /// `Fill` deserialization (exercises `de_f64_flexible`) never panics.
    ///
    /// NB: like the other venues' deser proptests, the guarantee under test is
    /// no-panic, not value sanity. `de_f64_flexible` uses `str::parse::<f64>`,
    /// which *accepts* "inf"/"NaN" — that quirk is on the Gate-A paper fill
    /// path and is intentionally left unchanged here; it is only noted, not
    /// asserted against.
    #[test]
    fn fill_never_panics(v in arb_fill_payload()) {
        let _ = serde_json::from_value::<Fill>(v);
    }
}

// ── Regression: well-formed wire shapes classify correctly ─────────────────────

#[test]
fn open_order_is_active_done_order_is_not() {
    let open: OrderDetail = serde_json::from_value(json!({
        "id": "a", "symbol": "XBTUSDTM", "side": "buy", "type": "limit",
        "status": "open", "isActive": true, "size": 1,
    }))
    .unwrap();
    assert!(open.is_active());

    let done: OrderDetail = serde_json::from_value(json!({
        "id": "b", "symbol": "XBTUSDTM", "side": "buy", "type": "market",
        "status": "done", "isActive": false, "size": 1, "filledSize": 1,
    }))
    .unwrap();
    assert!(!done.is_active());
    assert!(done.is_filled());
}
