#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]
#![cfg(feature = "bybit")]

//! Property / fuzz tests for Bybit's hand-rolled deserializers.
//!
//! Covers [`BybitKline`] (7-element positional string array, custom
//! `Deserialize`), [`BybitOrderBook`] (`[price, qty]` string pairs), and the
//! `str_f64` / `str_i64` / `opt_str_f64` serde adapters that accept a JSON
//! string *or* number (exercised via [`BybitTicker`] and [`BybitTrade`]).

#[path = "deser_common/mod.rs"]
mod deser_common;

use deser_common::{arb_json, arb_price, arb_scalar, arb_scalar_array};
use exchange_apiws::bybit::{BybitKline, BybitOrderBook, BybitTicker, BybitTrade};
use proptest::prelude::*;
use serde_json::{Value, json};

fn config() -> ProptestConfig {
    ProptestConfig::with_cases(96)
}

/// Well-formed Bybit kline: 7 JSON strings `[start, o, h, l, c, vol, turnover]`.
fn well_formed_kline(start: i64, o: f64, h: f64, l: f64, c: f64, v: f64, t: f64) -> Value {
    json!([
        start.to_string(),
        o.to_string(),
        h.to_string(),
        l.to_string(),
        c.to_string(),
        v.to_string(),
        t.to_string(),
    ])
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn kline_well_formed_roundtrips(
        start in 0i64..4_000_000_000_000,
        o in arb_price(),
        h in arb_price(),
        l in arb_price(),
        c in arb_price(),
        v in arb_price(),
        t in arb_price(),
    ) {
        let row = well_formed_kline(start, o, h, l, c, v, t);
        let k: BybitKline = serde_json::from_value(row).expect("well-formed kline");
        prop_assert_eq!(k.start_time, start);
        prop_assert_eq!(k.open, o);
        prop_assert_eq!(k.high, h);
        prop_assert_eq!(k.low, l);
        prop_assert_eq!(k.close, c);
        prop_assert_eq!(k.volume, v);
        prop_assert_eq!(k.turnover, t);
        prop_assert!(k.open >= 0.0 && k.volume >= 0.0);
    }

    /// A non-numeric string in any slot is an error, never a silent value.
    #[test]
    fn kline_non_numeric_is_err(slot in 0usize..7) {
        let mut arr: Vec<Value> = well_formed_kline(1, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0)
            .as_array()
            .unwrap()
            .clone();
        arr[slot] = Value::String("nope".into());
        let res: Result<BybitKline, _> = serde_json::from_value(Value::Array(arr));
        prop_assert!(res.is_err());
    }

    #[test]
    fn kline_wrong_arity_never_panics(v in arb_scalar_array(0, 16)) {
        let _ = serde_json::from_value::<BybitKline>(v);
    }

    #[test]
    fn kline_arbitrary_json_never_panics(v in arb_json()) {
        let s = v.to_string();
        let _ = serde_json::from_str::<BybitKline>(&s);
        let _ = serde_json::from_value::<BybitKline>(v);
    }

    #[test]
    fn orderbook_arbitrary_json_never_panics(v in arb_json()) {
        if let Ok(book) = serde_json::from_value::<BybitOrderBook>(v) {
            let _ = book.bids_f64();
            let _ = book.asks_f64();
        }
    }

    #[test]
    fn orderbook_well_formed_roundtrips(
        ts in 0i64..i64::MAX,
        u in 0i64..i64::MAX,
        levels in prop::collection::vec((arb_price(), arb_price()), 0..8),
    ) {
        let rows: Vec<Value> = levels
            .iter()
            .map(|(p, q)| json!([p.to_string(), q.to_string()]))
            .collect();
        let row = json!({
            "s": "BTCUSDT",
            "b": rows,
            "a": rows,
            "ts": ts,
            "u": u,
        });
        let book: BybitOrderBook = serde_json::from_value(row).expect("well-formed book");
        prop_assert_eq!(&book.symbol, "BTCUSDT");
        prop_assert_eq!(book.ts, ts);
        prop_assert_eq!(book.bids_f64().len(), levels.len());
    }

    // ── str_f64 / opt_str_f64 adapters (via BybitTicker) ─────────────────────

    /// `lastPrice` (required, `str_f64`) accepts both a JSON string and a JSON
    /// number and yields the same `f64`. Empty string maps to `0.0` by design.
    #[test]
    fn ticker_str_f64_accepts_string_or_number(x in arb_price()) {
        let as_string = json!({"symbol": "S", "lastPrice": x.to_string()});
        let as_number = json!({"symbol": "S", "lastPrice": x});
        let a: BybitTicker = serde_json::from_value(as_string).expect("string form");
        let b: BybitTicker = serde_json::from_value(as_number).expect("number form");
        prop_assert_eq!(a.last_price, x);
        prop_assert_eq!(b.last_price, x);

        let empty: BybitTicker =
            serde_json::from_value(json!({"symbol": "S", "lastPrice": ""})).expect("empty");
        prop_assert_eq!(empty.last_price, 0.0);
    }

    /// `opt_str_f64` fields: null/absent/empty -> None; string/number -> Some.
    #[test]
    fn ticker_opt_str_f64_forms(x in arb_price()) {
        let null_form: BybitTicker =
            serde_json::from_value(json!({"symbol":"S","lastPrice":"1","bid1Price":null}))
                .expect("null");
        prop_assert_eq!(null_form.bid1_price, None);

        let empty_form: BybitTicker =
            serde_json::from_value(json!({"symbol":"S","lastPrice":"1","bid1Price":""}))
                .expect("empty");
        prop_assert_eq!(empty_form.bid1_price, None);

        let num_form: BybitTicker =
            serde_json::from_value(json!({"symbol":"S","lastPrice":"1","bid1Price":x}))
                .expect("number");
        prop_assert_eq!(num_form.bid1_price, Some(x));
    }

    /// A garbage (non-numeric, non-empty) string in a numeric field errors.
    /// The `z`-prefix keeps proptest from generating `inf`/`nan`, which
    /// `f64::from_str` would legitimately accept.
    #[test]
    fn ticker_str_f64_garbage_is_err(s in "z[a-z]{0,5}") {
        let res: Result<BybitTicker, _> =
            serde_json::from_value(json!({"symbol":"S","lastPrice": s}));
        prop_assert!(res.is_err());
    }

    #[test]
    fn ticker_arbitrary_json_never_panics(v in arb_json()) {
        let _ = serde_json::from_value::<BybitTicker>(v);
    }

    // ── str_i64 adapter (via BybitTrade) ─────────────────────────────────────

    /// `time` (required, `str_i64`) accepts a JSON string or number as `i64`.
    #[test]
    fn trade_str_i64_accepts_string_or_number(
        time in any::<i64>(),
        price in arb_price(),
        size in arb_price(),
    ) {
        let base = |t: Value| {
            json!({
                "execId": "e", "symbol": "S",
                "price": price.to_string(), "size": size.to_string(),
                "side": "Buy", "time": t,
            })
        };
        let s: BybitTrade =
            serde_json::from_value(base(json!(time.to_string()))).expect("string time");
        let n: BybitTrade = serde_json::from_value(base(json!(time))).expect("number time");
        prop_assert_eq!(s.time, time);
        prop_assert_eq!(n.time, time);
    }

    /// A non-integer string in `time` errors rather than truncating silently.
    #[test]
    fn trade_str_i64_non_integer_is_err(s in prop_oneof!["[a-z]{1,5}", "[0-9]+\\.[0-9]+"]) {
        let res: Result<BybitTrade, _> = serde_json::from_value(json!({
            "execId": "e", "symbol": "S",
            "price": "1", "size": "1", "side": "Buy", "time": s,
        }));
        prop_assert!(res.is_err());
    }

    #[test]
    fn trade_arbitrary_json_never_panics(v in arb_json()) {
        let _ = serde_json::from_value::<BybitTrade>(v);
    }
}

/// Adapter round-trip over a fixed adversarial scalar set (fast, deterministic).
#[test]
fn ticker_scalar_matrix_never_panics() {
    proptest!(|(v in arb_scalar())| {
        let _ = serde_json::from_value::<BybitTicker>(json!({"symbol":"S","lastPrice": v}));
    });
}
