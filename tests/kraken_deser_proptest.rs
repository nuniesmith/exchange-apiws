#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]
#![cfg(feature = "kraken")]

//! Property / fuzz tests for Kraken's hand-rolled deserializers — the most
//! bespoke set in the crate, and the one a past release silently broke.
//!
//! Covers the positional-array types ([`KrakenCandle`], [`KrakenTrade`] via
//! `visit_seq`, [`KrakenSpreadTick`]), the fixed-arity tuple ticker
//! ([`KrakenTicker`]), the `(price, volume, ts)` order book
//! ([`KrakenOrderBook`]), and the pair-keyed map-splitting types
//! ([`KrakenOhlc`], [`KrakenRecentTrades`], [`KrakenSpread`]).

#[path = "deser_common/mod.rs"]
mod deser_common;

use deser_common::{arb_json, arb_price, arb_scalar_array};
use exchange_apiws::kraken::{
    KrakenCandle, KrakenOhlc, KrakenOrderBook, KrakenRecentTrades, KrakenSpread, KrakenSpreadTick,
    KrakenTicker, KrakenTrade,
};
use proptest::prelude::*;
use serde_json::{Value, json};

fn config() -> ProptestConfig {
    ProptestConfig::with_cases(96)
}

/// Well-formed OHLC row: `[time, o, h, l, c, vwap, vol, count]` with the
/// correct JSON types (numbers at 0 and 7, strings between).
fn candle_row(time: i64, count: u64, o: f64, h: f64, l: f64, c: f64, vw: f64, v: f64) -> Value {
    json!([
        time,
        o.to_string(),
        h.to_string(),
        l.to_string(),
        c.to_string(),
        vw.to_string(),
        v.to_string(),
        count,
    ])
}

/// Well-formed trade row up to the optional id: `[price, vol, time, side, type, misc]`.
fn trade_row6(price: f64, vol: f64, time: f64) -> Vec<Value> {
    vec![
        json!(price.to_string()),
        json!(vol.to_string()),
        json!(time),
        json!("b"),
        json!("l"),
        json!(""),
    ]
}

proptest! {
    #![proptest_config(config())]

    // ── KrakenCandle (positional tuple) ──────────────────────────────────────

    #[test]
    fn candle_well_formed_roundtrips(
        time in 0i64..4_000_000_000,
        count in 0u64..10_000_000,
        o in arb_price(),
        h in arb_price(),
        l in arb_price(),
        c in arb_price(),
        vw in arb_price(),
        v in arb_price(),
    ) {
        let row = candle_row(time, count, o, h, l, c, vw, v);
        let k: KrakenCandle = serde_json::from_value(row).expect("well-formed candle");
        prop_assert_eq!(k.time, time);
        prop_assert_eq!(k.count, count);
        prop_assert_eq!(k.open_f64(), o);
        prop_assert_eq!(k.close_f64(), c);
        prop_assert!(k.high_f64() >= 0.0 && k.volume_f64() >= 0.0);
    }

    #[test]
    fn candle_wrong_arity_never_panics(v in arb_scalar_array(0, 16)) {
        let _ = serde_json::from_value::<KrakenCandle>(v);
    }

    #[test]
    fn candle_arbitrary_json_never_panics(v in arb_json()) {
        let s = v.to_string();
        let _ = serde_json::from_str::<KrakenCandle>(&s);
        let _ = serde_json::from_value::<KrakenCandle>(v);
    }

    // ── KrakenTrade (visit_seq with optional + ignored trailing fields) ──────

    /// The 6-field form (no trade id) parses and leaves `trade_id` None.
    #[test]
    fn trade_six_fields_parses(price in arb_price(), vol in arb_price(), time in arb_price()) {
        let row = Value::Array(trade_row6(price, vol, time));
        let t: KrakenTrade = serde_json::from_value(row).expect("6-field trade");
        prop_assert_eq!(t.price_f64(), price);
        prop_assert_eq!(t.volume_f64(), vol);
        prop_assert_eq!(t.trade_id, None);
        prop_assert!(t.is_buy());
    }

    /// The 7-field form carries the trade id; any *further* trailing fields
    /// Kraken appends are ignored for forward-compatibility (still `Ok`, id
    /// preserved). This is the key drift-tolerance property.
    #[test]
    fn trade_seven_plus_fields_forward_compatible(
        price in arb_price(),
        vol in arb_price(),
        time in arb_price(),
        id in any::<u64>(),
        extra in prop::collection::vec(arb_json(), 0..4),
    ) {
        let mut row = trade_row6(price, vol, time);
        row.push(json!(id));
        row.extend(extra);
        let t: KrakenTrade = serde_json::from_value(Value::Array(row)).expect("7+ field trade");
        prop_assert_eq!(t.trade_id, Some(id));
        prop_assert_eq!(t.price_f64(), price);
    }

    /// Too few fields (<6) is a length error, never a panic.
    #[test]
    fn trade_too_short_is_err(n in 0usize..6) {
        let row: Vec<Value> = trade_row6(1.0, 1.0, 1.0).into_iter().take(n).collect();
        let res: Result<KrakenTrade, _> = serde_json::from_value(Value::Array(row));
        prop_assert!(res.is_err());
    }

    #[test]
    fn trade_wrong_arity_never_panics(v in arb_scalar_array(0, 16)) {
        let _ = serde_json::from_value::<KrakenTrade>(v);
    }

    #[test]
    fn trade_arbitrary_json_never_panics(v in arb_json()) {
        let s = v.to_string();
        let _ = serde_json::from_str::<KrakenTrade>(&s);
        let _ = serde_json::from_value::<KrakenTrade>(v);
    }

    // ── KrakenSpreadTick (positional tuple) ──────────────────────────────────

    #[test]
    fn spread_tick_well_formed_roundtrips(
        time in 0i64..4_000_000_000,
        bid in arb_price(),
        ask in arb_price(),
    ) {
        let row = json!([time, bid.to_string(), ask.to_string()]);
        let s: KrakenSpreadTick = serde_json::from_value(row).expect("well-formed spread tick");
        prop_assert_eq!(s.time, time);
        prop_assert_eq!(s.bid_f64(), bid);
        prop_assert_eq!(s.ask_f64(), ask);
    }

    #[test]
    fn spread_tick_arbitrary_json_never_panics(v in arb_json()) {
        let _ = serde_json::from_value::<KrakenSpreadTick>(v);
    }

    // ── KrakenTicker (fixed-arity tuple fields) ──────────────────────────────

    #[test]
    fn ticker_well_formed_roundtrips(
        ask in arb_price(),
        bid in arb_price(),
        last in arb_price(),
        trades_today in any::<u64>(),
    ) {
        let two = |x: f64| json!([x.to_string(), x.to_string()]);
        let three = |x: f64| json!([x.to_string(), x.to_string(), x.to_string()]);
        let row = json!({
            "a": three(ask), "b": three(bid), "c": two(last),
            "v": two(1.0), "p": two(1.0), "t": [trades_today, 0u64],
            "l": two(1.0), "h": two(1.0), "o": "1.0",
        });
        let t: KrakenTicker = serde_json::from_value(row).expect("well-formed ticker");
        prop_assert_eq!(t.ask_price(), ask);
        prop_assert_eq!(t.bid_price(), bid);
        prop_assert_eq!(t.last_price(), last);
        prop_assert_eq!(t.t[0], trades_today);
    }

    /// A wrong-arity tuple field (2 elements where 3 are required) errors.
    #[test]
    fn ticker_wrong_tuple_arity_is_err(_seed in 0u8..1) {
        let two = json!(["1", "1"]);
        let row = json!({
            "a": two, "b": ["1","1","1"], "c": ["1","1"],
            "v": ["1","1"], "p": ["1","1"], "t": [0u64, 0u64],
            "l": ["1","1"], "h": ["1","1"], "o": "1",
        });
        let res: Result<KrakenTicker, _> = serde_json::from_value(row);
        prop_assert!(res.is_err());
    }

    #[test]
    fn ticker_arbitrary_json_never_panics(v in arb_json()) {
        let _ = serde_json::from_value::<KrakenTicker>(v);
    }

    // ── KrakenOrderBook ((price, volume, ts) rows) ───────────────────────────

    #[test]
    fn orderbook_well_formed_roundtrips(
        levels in prop::collection::vec((arb_price(), arb_price(), 0.0f64..2e9), 0..8),
    ) {
        let rows: Vec<Value> = levels
            .iter()
            .map(|(p, v, ts)| json!([p.to_string(), v.to_string(), ts]))
            .collect();
        let row = json!({"asks": rows, "bids": rows});
        let book: KrakenOrderBook = serde_json::from_value(row).expect("well-formed book");
        prop_assert_eq!(book.bids_f64().len(), levels.len());
        for ([p, v], (ep, ev, _)) in book.bids_f64().iter().zip(levels.iter()) {
            prop_assert_eq!(*p, *ep);
            prop_assert_eq!(*v, *ev);
        }
    }

    #[test]
    fn orderbook_arbitrary_json_never_panics(v in arb_json()) {
        if let Ok(book) = serde_json::from_value::<KrakenOrderBook>(v) {
            let _ = book.bids_f64();
            let _ = book.asks_f64();
        }
    }

    // ── Pair-keyed map splitters (OHLC / Trades / Spread) ────────────────────

    #[test]
    fn ohlc_well_formed_roundtrips(
        n in 0usize..5,
        last in any::<i64>(),
    ) {
        let candles: Vec<Value> = (0..n)
            .map(|i| candle_row(i as i64, 1, 1.0, 2.0, 0.5, 1.5, 1.0, 3.0))
            .collect();
        let row = json!({ "XXBTZUSD": candles, "last": last });
        let ohlc: KrakenOhlc = serde_json::from_value(row).expect("well-formed ohlc");
        prop_assert_eq!(&ohlc.pair, "XXBTZUSD");
        prop_assert_eq!(ohlc.candles.len(), n);
        prop_assert_eq!(ohlc.last, last);
    }

    #[test]
    fn recent_trades_well_formed_roundtrips(n in 0usize..5) {
        let trades: Vec<Value> = (0..n)
            .map(|_| Value::Array(trade_row6(1.0, 2.0, 3.0)))
            .collect();
        let row = json!({ "XXBTZUSD": trades, "last": "123456789" });
        let rt: KrakenRecentTrades = serde_json::from_value(row).expect("well-formed trades");
        prop_assert_eq!(&rt.pair, "XXBTZUSD");
        prop_assert_eq!(rt.trades.len(), n);
        prop_assert_eq!(&rt.last, "123456789");
    }

    #[test]
    fn spread_well_formed_roundtrips(n in 0usize..5, last in any::<i64>()) {
        let ticks: Vec<Value> = (0..n).map(|i| json!([i as i64, "1.0", "2.0"])).collect();
        let row = json!({ "XXBTZUSD": ticks, "last": last });
        let sp: KrakenSpread = serde_json::from_value(row).expect("well-formed spread");
        prop_assert_eq!(&sp.pair, "XXBTZUSD");
        prop_assert_eq!(sp.spreads.len(), n);
        prop_assert_eq!(sp.last, last);
    }

    /// Missing the `last` cursor is an error for every pair-keyed type.
    #[test]
    fn pair_keyed_missing_last_is_err(_seed in 0u8..1) {
        let ohlc: Result<KrakenOhlc, _> =
            serde_json::from_value(json!({ "XXBTZUSD": [] }));
        prop_assert!(ohlc.is_err());
        let sp: Result<KrakenSpread, _> =
            serde_json::from_value(json!({ "XXBTZUSD": [] }));
        prop_assert!(sp.is_err());
    }

    /// A map with only `last` (no pair entry) is an error, never a panic.
    #[test]
    fn pair_keyed_missing_pair_is_err(_seed in 0u8..1) {
        let ohlc: Result<KrakenOhlc, _> = serde_json::from_value(json!({ "last": 5 }));
        prop_assert!(ohlc.is_err());
    }

    #[test]
    fn pair_keyed_arbitrary_json_never_panics(v in arb_json()) {
        let _ = serde_json::from_value::<KrakenOhlc>(v.clone());
        let _ = serde_json::from_value::<KrakenRecentTrades>(v.clone());
        let _ = serde_json::from_value::<KrakenSpread>(v);
    }
}
