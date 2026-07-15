#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]
#![cfg(feature = "binance")]

//! Property / fuzz tests for Binance's hand-rolled deserializers.
//!
//! Covers [`BinanceKline`] (12-element positional string array with a custom
//! `Deserialize` impl) and [`BinanceOrderBook`] (`[price, qty]` string-pair
//! levels). The guarantee under test: adversarial JSON on the market-data
//! path yields a `Result`, never a panic, and never a wrong-but-plausible
//! parse of a malformed row.

#[path = "deser_common/mod.rs"]
mod deser_common;

use deser_common::{arb_json, arb_price, arb_scalar_array};
use exchange_apiws::binance::{BinanceKline, BinanceOrderBook};
use proptest::prelude::*;
use serde_json::{Value, json};

fn config() -> ProptestConfig {
    ProptestConfig::with_cases(96)
}

/// Build a well-formed 12-element Binance kline row with the correct JSON
/// types per position (numbers at 0/6/8, strings elsewhere).
fn well_formed_kline(
    open_time: i64,
    close_time: i64,
    trades: u64,
    o: f64,
    h: f64,
    l: f64,
    c: f64,
    v: f64,
) -> Value {
    json!([
        open_time,
        o.to_string(),
        h.to_string(),
        l.to_string(),
        c.to_string(),
        v.to_string(),
        close_time,
        "0.0",
        trades,
        "0.0",
        "0.0",
        "0"
    ])
}

proptest! {
    #![proptest_config(config())]

    /// Well-formed rows parse, preserve every field exactly, and keep the
    /// non-negative price invariant.
    #[test]
    fn kline_well_formed_roundtrips(
        open_time in 0i64..4_000_000_000_000,
        close_time in 0i64..4_000_000_000_000,
        trades in 0u64..10_000_000,
        o in arb_price(),
        h in arb_price(),
        l in arb_price(),
        c in arb_price(),
        v in arb_price(),
    ) {
        let row = well_formed_kline(open_time, close_time, trades, o, h, l, c, v);
        let k: BinanceKline = serde_json::from_value(row).expect("well-formed kline");
        prop_assert_eq!(k.open_time, open_time);
        prop_assert_eq!(k.close_time, close_time);
        prop_assert_eq!(k.trades, trades);
        prop_assert_eq!(k.open, o);
        prop_assert_eq!(k.high, h);
        prop_assert_eq!(k.low, l);
        prop_assert_eq!(k.close, c);
        prop_assert_eq!(k.volume, v);
        prop_assert!(k.open >= 0.0 && k.high >= 0.0 && k.low >= 0.0 && k.close >= 0.0);
        prop_assert!(k.volume >= 0.0);
    }

    /// A non-numeric string in any price slot must be an error, never a
    /// silent 0 or a panic.
    #[test]
    fn kline_non_numeric_price_is_err(slot in 1usize..6) {
        let mut arr: Vec<Value> = well_formed_kline(1, 2, 3, 1.0, 1.0, 1.0, 1.0, 1.0)
            .as_array()
            .unwrap()
            .clone();
        arr[slot] = Value::String("not-a-number".into());
        let res: Result<BinanceKline, _> = serde_json::from_value(Value::Array(arr));
        prop_assert!(res.is_err());
    }

    /// Wrong-arity positional arrays (0..=20 arbitrary scalars) never panic.
    #[test]
    fn kline_wrong_arity_never_panics(v in arb_scalar_array(0, 20)) {
        let _ = serde_json::from_value::<BinanceKline>(v);
    }

    /// Arbitrary JSON of any shape never panics the kline deserializer.
    #[test]
    fn kline_arbitrary_json_never_panics(v in arb_json()) {
        let s = v.to_string();
        let _ = serde_json::from_str::<BinanceKline>(&s);
        let _ = serde_json::from_value::<BinanceKline>(v);
    }

    /// Arbitrary JSON never panics the order-book deserializer, and the
    /// `*_f64` helpers stay panic-free on whatever survives parsing.
    #[test]
    fn orderbook_arbitrary_json_never_panics(v in arb_json()) {
        if let Ok(book) = serde_json::from_value::<BinanceOrderBook>(v) {
            let _ = book.bids_f64();
            let _ = book.asks_f64();
        }
    }

    /// Well-formed order books parse and the `[price, qty]` helper only keeps
    /// rows whose two columns both parse — never inventing values.
    #[test]
    fn orderbook_well_formed_roundtrips(
        last in 0i64..i64::MAX,
        levels in prop::collection::vec((arb_price(), arb_price()), 0..8),
    ) {
        let to_rows = |ls: &[(f64, f64)]| {
            ls.iter()
                .map(|(p, q)| json!([p.to_string(), q.to_string()]))
                .collect::<Vec<_>>()
        };
        let row = json!({
            "lastUpdateId": last,
            "bids": to_rows(&levels),
            "asks": to_rows(&levels),
        });
        let book: BinanceOrderBook = serde_json::from_value(row).expect("well-formed book");
        prop_assert_eq!(book.last_update_id, last);
        prop_assert_eq!(book.bids.len(), levels.len());
        let parsed = book.bids_f64();
        prop_assert_eq!(parsed.len(), levels.len());
        for ([p, q], (ep, eq)) in parsed.iter().zip(levels.iter()) {
            prop_assert_eq!(*p, *ep);
            prop_assert_eq!(*q, *eq);
            prop_assert!(*p >= 0.0 && *q >= 0.0);
        }
    }
}
