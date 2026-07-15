#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]
#![cfg(feature = "cryptocom")]

//! Property / fuzz tests for Crypto.com's hand-rolled deserializers.
//!
//! Covers the `flex` string-or-number adapter (`flex::string` /
//! `flex::opt_string`, exercised via [`CryptocomWithdrawalAck`] and
//! [`CryptocomBalance`]) plus the market-data shapes: [`CryptocomOrderBook`]
//! (`[price, qty, num_orders]` string triples), [`CryptocomCandle`],
//! [`CryptocomTicker`], and [`CryptocomTrade`].

#[path = "deser_common/mod.rs"]
mod deser_common;

use deser_common::{arb_json, arb_price};
use exchange_apiws::cryptocom::{
    CryptocomBalance, CryptocomCandle, CryptocomOrderBook, CryptocomTicker, CryptocomTrade,
    CryptocomWithdrawalAck,
};
use proptest::prelude::*;
use serde_json::{Value, json};

fn config() -> ProptestConfig {
    ProptestConfig::with_cases(96)
}

proptest! {
    #![proptest_config(config())]

    // ── flex::string (required) — accepts string OR number ───────────────────

    /// `id` given as a JSON string is preserved verbatim.
    #[test]
    fn flex_string_from_string(s in "[A-Za-z0-9._-]{0,20}") {
        let ack: CryptocomWithdrawalAck =
            serde_json::from_value(json!({"id": s})).expect("string id");
        prop_assert_eq!(ack.id, s);
    }

    /// `id` given as a JSON integer normalises to its decimal string, exactly
    /// (no f64 precision loss for 19-digit snowflakes).
    #[test]
    fn flex_string_from_integer(n in any::<u64>()) {
        let ack: CryptocomWithdrawalAck =
            serde_json::from_value(json!({"id": n})).expect("integer id");
        prop_assert_eq!(ack.id, n.to_string());
    }

    /// `id` given as a JSON float normalises via `f64::to_string`.
    #[test]
    fn flex_string_from_float(x in -1e6f64..1e6) {
        let ack: CryptocomWithdrawalAck =
            serde_json::from_value(json!({"id": x})).expect("float id");
        prop_assert_eq!(ack.id, x.to_string());
    }

    /// A required `flex::string` field that is null / bool / array / object is
    /// an error — never a panic, never a bogus value.
    #[test]
    fn flex_string_rejects_non_scalar(
        bad in prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            Just(json!([1, 2])),
            Just(json!({"x": 1})),
        ],
    ) {
        let res: Result<CryptocomWithdrawalAck, _> =
            serde_json::from_value(json!({"id": bad}));
        prop_assert!(res.is_err());
    }

    // ── flex::opt_string — null/absent -> None, string/number -> Some ────────

    #[test]
    fn flex_opt_string_forms(x in arb_price()) {
        // absent field -> None (serde default)
        let absent: CryptocomBalance =
            serde_json::from_value(json!({"currency": "BTC"})).expect("absent");
        prop_assert_eq!(absent.available, None);

        // explicit null -> None
        let nulled: CryptocomBalance =
            serde_json::from_value(json!({"currency": "BTC", "available": null}))
                .expect("null");
        prop_assert_eq!(nulled.available, None);

        // number -> Some(stringified)
        let numbered: CryptocomBalance =
            serde_json::from_value(json!({"currency": "BTC", "available": x}))
                .expect("number");
        prop_assert_eq!(numbered.available, Some(x.to_string()));

        // string -> Some(verbatim)
        let stringed: CryptocomBalance =
            serde_json::from_value(json!({"currency": "BTC", "available": "0.5"}))
                .expect("string");
        prop_assert_eq!(stringed.available, Some("0.5".to_string()));
    }

    #[test]
    fn balance_arbitrary_json_never_panics(v in arb_json()) {
        let _ = serde_json::from_value::<CryptocomBalance>(v);
    }

    #[test]
    fn withdrawal_ack_arbitrary_json_never_panics(v in arb_json()) {
        let _ = serde_json::from_value::<CryptocomWithdrawalAck>(v);
    }

    // ── CryptocomOrderBook ([price, qty, num_orders] string triples) ─────────

    #[test]
    fn orderbook_well_formed_roundtrips(
        levels in prop::collection::vec((arb_price(), arb_price()), 0..8),
    ) {
        let rows: Vec<Value> = levels
            .iter()
            .map(|(p, q)| json!([p.to_string(), q.to_string(), "1"]))
            .collect();
        let row = json!({
            "instrument_name": "BTC_USDT",
            "bids": rows, "asks": rows,
        });
        let book: CryptocomOrderBook = serde_json::from_value(row).expect("well-formed book");
        prop_assert_eq!(book.bids_f64().len(), levels.len());
        for ([p, q], (ep, eq)) in book.bids_f64().iter().zip(levels.iter()) {
            prop_assert_eq!(*p, *ep);
            prop_assert_eq!(*q, *eq);
        }
    }

    /// A level with the wrong number of columns (2 instead of 3) is a parse
    /// error, never a panic.
    #[test]
    fn orderbook_wrong_level_arity_is_err(_seed in 0u8..1) {
        let row = json!({
            "instrument_name": "BTC_USDT",
            "bids": [["1.0", "2.0"]],
            "asks": [],
        });
        let res: Result<CryptocomOrderBook, _> = serde_json::from_value(row);
        prop_assert!(res.is_err());
    }

    #[test]
    fn orderbook_arbitrary_json_never_panics(v in arb_json()) {
        if let Ok(book) = serde_json::from_value::<CryptocomOrderBook>(v) {
            let _ = book.bids_f64();
            let _ = book.asks_f64();
        }
    }

    // ── CryptocomCandle / Ticker / Trade ─────────────────────────────────────

    #[test]
    fn candle_well_formed_roundtrips(
        ts in 0i64..4_000_000_000_000,
        o in arb_price(),
        h in arb_price(),
        l in arb_price(),
        c in arb_price(),
        v in arb_price(),
    ) {
        let row = json!({
            "o": o.to_string(), "h": h.to_string(), "l": l.to_string(),
            "c": c.to_string(), "v": v.to_string(), "t": ts,
        });
        let candle: CryptocomCandle = serde_json::from_value(row).expect("well-formed candle");
        prop_assert_eq!(candle.open_ts, ts);
        prop_assert_eq!(candle.open_f64(), o);
        prop_assert_eq!(candle.close_f64(), c);
        prop_assert!(candle.high_f64() >= 0.0 && candle.volume_f64() >= 0.0);
    }

    #[test]
    fn candle_arbitrary_json_never_panics(v in arb_json()) {
        let _ = serde_json::from_value::<CryptocomCandle>(v);
    }

    #[test]
    fn ticker_arbitrary_json_never_panics(v in arb_json()) {
        let _ = serde_json::from_value::<CryptocomTicker>(v);
    }

    #[test]
    fn trade_arbitrary_json_never_panics(v in arb_json()) {
        let _ = serde_json::from_value::<CryptocomTrade>(v);
    }
}
