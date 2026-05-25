//! Binance WebSocket connector — public market-data streams.
//!
//! Implements [`ExchangeConnector`] so a [`BinanceConnector`] plugs straight
//! into [`run_feed`](crate::ws::run_feed) and [`run_feed_supervised`](crate::ws::run_feed_supervised).
//! Binance uses URL-encoded stream names (no JSON subscription frames are
//! sent after connect), so the connector pre-builds the full WSS URL when
//! you construct it and `subscription_message` returns `None`.
//!
//! # Supported streams
//!
//! | Helper | Binance stream | Emits |
//! |--------|----------------|-------|
//! | [`BinanceConnector::trade_stream`] | `<sym>@aggTrade` | `DataMessage::Trade` |
//! | [`BinanceConnector::ticker_stream`] | `<sym>@bookTicker` | `DataMessage::Ticker` |
//! | [`BinanceConnector::kline_stream`] | `<sym>@kline_<interval>` | `DataMessage::Candle` |
//! | [`BinanceConnector::depth_stream`] | `<sym>@depth@100ms` | `DataMessage::OrderBook` (delta) |
//! | [`BinanceConnector::depth_snapshot_stream`] | `<sym>@depth{5\|10\|20}@100ms` | `DataMessage::OrderBook` (snapshot) |
//! | [`BinanceConnector::mark_price_stream`] | `<sym>@markPrice@1s` (futures) | `DataMessage::FundingRate` |
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use tokio::sync::{mpsc, watch};
//! use exchange_apiws::actors::{DataMessage, ExchangeConnector};
//! use exchange_apiws::binance::BinanceConnector;
//! use exchange_apiws::ws::{WsRunnerConfig, run_feed};
//!
//! # async fn example() -> exchange_apiws::Result<()> {
//! let streams = [
//!     BinanceConnector::trade_stream("BTCUSDT"),
//!     BinanceConnector::ticker_stream("BTCUSDT"),
//!     BinanceConnector::kline_stream("BTCUSDT", "1m"),
//! ];
//! let stream_refs: Vec<&str> = streams.iter().map(String::as_str).collect();
//! let connector = Arc::new(BinanceConnector::spot(&stream_refs));
//! let url = connector.ws_url().to_string();
//!
//! let (tx, mut rx) = mpsc::channel::<DataMessage>(1024);
//! let (_sd_tx, sd_rx) = watch::channel(false);
//!
//! tokio::spawn(run_feed(
//!     url,
//!     vec![],
//!     connector,
//!     tx,
//!     WsRunnerConfig::default(),
//!     sd_rx,
//! ));
//! while let Some(msg) = rx.recv().await {
//!     println!("{msg:?}");
//! }
//! # Ok(())
//! # }
//! ```

use serde_json::Value;

use crate::actors::{
    CandleData, DataMessage, ExchangeConnector, FundingData, OrderBookData, TickerData, TradeData,
    TradeSide, WebSocketConfig,
};
use crate::error::Result;

const SPOT_WS_BASE: &str = "wss://stream.binance.com:9443";
const FUTURES_WS_BASE: &str = "wss://fstream.binance.com";
const EXCHANGE_SPOT: &str = "binance";
const EXCHANGE_FUTURES: &str = "binance-futures";

// ── Connector ────────────────────────────────────────────────────────────────

/// Binance WebSocket connector — spot or USDT-M futures.
///
/// Subscription is encoded in the WSS URL at construction time; no separate
/// subscribe frame is sent after connect. Built via [`Self::spot`] /
/// [`Self::futures`] with the helper stream-name builders.
#[derive(Debug, Clone)]
pub struct BinanceConnector {
    /// Full WSS URL, including `?streams=…` for combined streams.
    pub url: String,
    /// Exchange identifier (`"binance"` for spot, `"binance-futures"` for USDT-M).
    pub exchange: &'static str,
}

impl BinanceConnector {
    /// Build a spot connector subscribed to the given streams.
    ///
    /// Always uses the combined-stream endpoint (`/stream?streams=…`) so a
    /// single connection handles many topics; the parser unwraps the
    /// `{"stream":…,"data":…}` envelope automatically.
    #[must_use]
    pub fn spot(streams: &[&str]) -> Self {
        Self {
            url: build_combined_url(SPOT_WS_BASE, streams),
            exchange: EXCHANGE_SPOT,
        }
    }

    /// Build a futures (USDT-M) connector subscribed to the given streams.
    #[must_use]
    pub fn futures(streams: &[&str]) -> Self {
        Self {
            url: build_combined_url(FUTURES_WS_BASE, streams),
            exchange: EXCHANGE_FUTURES,
        }
    }

    /// Build a connector with a caller-supplied URL — used by tests pointing
    /// at a local tokio-tungstenite server.
    #[must_use]
    pub fn with_url(url: impl Into<String>, exchange: &'static str) -> Self {
        Self {
            url: url.into(),
            exchange,
        }
    }

    // ── Stream-name builders ────────────────────────────────────────────────

    /// Aggregate trade stream — `<symbol>@aggTrade`.
    ///
    /// Binance lowercases stream names; the helper does this for you.
    #[must_use]
    pub fn trade_stream(symbol: &str) -> String {
        format!("{}@aggTrade", symbol.to_lowercase())
    }

    /// Best bid/ask stream — `<symbol>@bookTicker`.
    #[must_use]
    pub fn ticker_stream(symbol: &str) -> String {
        format!("{}@bookTicker", symbol.to_lowercase())
    }

    /// Kline stream — `<symbol>@kline_<interval>`.
    ///
    /// `interval` follows Binance's labels: `"1m"`, `"3m"`, `"5m"`,
    /// `"15m"`, `"30m"`, `"1h"`, `"2h"`, `"4h"`, `"6h"`, `"8h"`, `"12h"`,
    /// `"1d"`, `"3d"`, `"1w"`, `"1M"`.
    #[must_use]
    pub fn kline_stream(symbol: &str, interval: &str) -> String {
        format!("{}@kline_{interval}", symbol.to_lowercase())
    }

    /// Incremental depth stream — `<symbol>@depth@100ms`.
    ///
    /// Emits delta frames; bootstrap with a REST snapshot via
    /// [`BinanceRestClient::get_orderbook`](crate::binance::BinanceRestClient::get_orderbook)
    /// and apply updates whose `final_update_id` (`u`) is greater than the
    /// snapshot's `lastUpdateId`.
    #[must_use]
    pub fn depth_stream(symbol: &str) -> String {
        format!("{}@depth@100ms", symbol.to_lowercase())
    }

    /// Partial-book depth stream — `<symbol>@depth{N}@100ms`.
    ///
    /// `levels` is clamped to 5, 10, or 20 (the only values Binance accepts).
    /// Each frame is a full snapshot of the top N levels — no bootstrap
    /// required. Use this for top-of-book displays where a local book
    /// isn't worth maintaining.
    #[must_use]
    pub fn depth_snapshot_stream(symbol: &str, levels: u8) -> String {
        let n = match levels {
            0..=5 => 5,
            6..=10 => 10,
            _ => 20,
        };
        format!("{}@depth{n}@100ms", symbol.to_lowercase())
    }

    /// Mark-price + funding-rate stream — `<symbol>@markPrice@1s` (futures).
    ///
    /// Emitted on the USDT-M futures endpoint only.
    #[must_use]
    pub fn mark_price_stream(symbol: &str) -> String {
        format!("{}@markPrice@1s", symbol.to_lowercase())
    }
}

fn build_combined_url(base: &str, streams: &[&str]) -> String {
    let joined = streams.join("/");
    format!("{base}/stream?streams={joined}")
}

// ── ExchangeConnector ────────────────────────────────────────────────────────

impl ExchangeConnector for BinanceConnector {
    fn exchange_name(&self) -> &str {
        self.exchange
    }

    fn ws_url(&self) -> &str {
        &self.url
    }

    fn build_ws_config(&self, symbol: &str) -> WebSocketConfig {
        WebSocketConfig {
            url: self.url.clone(),
            exchange: self.exchange.to_string(),
            symbol: symbol.to_string(),
            subscription_msg: None,
            // Binance pings every ~3 min; the runner handles protocol-level
            // Ping/Pong automatically. Application pings aren't needed.
            ping_interval_secs: 180,
            reconnect_delay_secs: 5,
            max_reconnect_attempts: 5,
        }
    }

    fn subscription_message(&self, _symbol: &str) -> Option<String> {
        // Subscription is URL-based; no per-symbol frame needed.
        None
    }

    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>> {
        let json: Value = serde_json::from_str(raw)?;

        // Combined-stream wrapper: `{"stream":"btcusdt@aggTrade","data":{...}}`.
        // Unwrap to the inner event for routing.
        let (stream_name, inner) =
            if let Some(stream) = json.get("stream").and_then(|v| v.as_str()) {
                let data = json.get("data").cloned().unwrap_or(Value::Null);
                (Some(stream.to_string()), data)
            } else {
                (None, json)
            };

        // Most Binance events identify themselves via the "e" field. The two
        // that don't are bookTicker and the partial-depth snapshot — both
        // are detected by their distinctive field sets below.
        let event_type = inner.get("e").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            "aggTrade" => Ok(parse_agg_trade(self.exchange, &inner)),
            "kline" => Ok(parse_kline(self.exchange, &inner)),
            "markPriceUpdate" => Ok(parse_mark_price(self.exchange, &inner)),
            "depthUpdate" => Ok(parse_depth_update(self.exchange, &inner)),
            "" => {
                // bookTicker has `u`, `s`, `b`, `B`, `a`, `A` (no `e`).
                if inner.get("u").is_some()
                    && inner.get("s").is_some()
                    && inner.get("b").is_some()
                {
                    Ok(parse_book_ticker(self.exchange, &inner))
                } else if inner.get("lastUpdateId").is_some() {
                    // Partial-depth snapshot has no symbol in the frame; pull it
                    // from the combined-stream wrapper if present.
                    let symbol = stream_name
                        .as_ref()
                        .and_then(|s| s.split('@').next())
                        .unwrap_or("")
                        .to_uppercase();
                    Ok(parse_depth_snapshot(self.exchange, &symbol, &inner))
                } else {
                    Ok(vec![])
                }
            }
            _ => Ok(vec![]), // unknown / future event type
        }
    }
}

// ── Parsers ───────────────────────────────────────────────────────────────────

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn str_f64(v: &Value, key: &str) -> f64 {
    v.get(key)
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

fn opt_str_f64(v: &Value, key: &str) -> Option<f64> {
    v.get(key)
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse().ok())
}

fn parse_levels(v: &Value) -> Vec<[f64; 2]> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|row| {
                    let p: f64 = row.get(0)?.as_str()?.parse().ok()?;
                    let q: f64 = row.get(1)?.as_str()?.parse().ok()?;
                    Some([p, q])
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_agg_trade(exchange: &str, data: &Value) -> Vec<DataMessage> {
    let symbol = data["s"].as_str().unwrap_or("").to_string();
    // `m == true` means the buyer was the maker, i.e. an aggressive sell.
    let is_buyer_maker = data["m"].as_bool().unwrap_or(false);
    let side = if is_buyer_maker {
        TradeSide::Sell
    } else {
        TradeSide::Buy
    };

    vec![DataMessage::Trade(TradeData {
        symbol,
        exchange: exchange.to_string(),
        side,
        price: str_f64(data, "p"),
        amount: str_f64(data, "q"),
        exchange_ts: data["T"].as_i64().unwrap_or(0),
        receipt_ts: now_ms(),
        trade_id: data["a"].as_u64().unwrap_or(0).to_string(),
    })]
}

fn parse_kline(exchange: &str, data: &Value) -> Vec<DataMessage> {
    let Some(k) = data.get("k") else {
        return vec![];
    };

    vec![DataMessage::Candle(CandleData {
        symbol: k["s"].as_str().unwrap_or("").to_string(),
        exchange: exchange.to_string(),
        interval: k["i"].as_str().unwrap_or("").to_string(),
        open_ts: k["t"].as_i64().unwrap_or(0),
        open: str_f64(k, "o"),
        high: str_f64(k, "h"),
        low: str_f64(k, "l"),
        close: str_f64(k, "c"),
        volume: str_f64(k, "v"),
        is_closed: k["x"].as_bool().unwrap_or(false),
        receipt_ts: now_ms(),
    })]
}

fn parse_mark_price(exchange: &str, data: &Value) -> Vec<DataMessage> {
    vec![DataMessage::FundingRate(FundingData {
        symbol: data["s"].as_str().unwrap_or("").to_string(),
        exchange: exchange.to_string(),
        funding_rate: str_f64(data, "r"),
        next_funding_time: data["T"].as_i64().unwrap_or(0),
        mark_price: opt_str_f64(data, "p"),
        index_price: opt_str_f64(data, "i"),
        exchange_ts: data["E"].as_i64().unwrap_or(0),
        receipt_ts: now_ms(),
    })]
}

fn parse_depth_update(exchange: &str, data: &Value) -> Vec<DataMessage> {
    vec![DataMessage::OrderBook(OrderBookData {
        symbol: data["s"].as_str().unwrap_or("").to_string(),
        exchange: exchange.to_string(),
        asks: parse_levels(&data["a"]),
        bids: parse_levels(&data["b"]),
        exchange_ts: data["E"].as_i64().unwrap_or(0),
        receipt_ts: now_ms(),
        is_snapshot: false,
    })]
}

fn parse_book_ticker(exchange: &str, data: &Value) -> Vec<DataMessage> {
    // bookTicker doesn't carry an exchange timestamp or last-trade price,
    // so we leave `price` zeroed and use the receipt time as exchange_ts.
    // Callers needing a true exchange timestamp should use the 24h ticker
    // REST endpoint or the @ticker stream (not yet wired).
    let now = now_ms();
    vec![DataMessage::Ticker(TickerData {
        symbol: data["s"].as_str().unwrap_or("").to_string(),
        exchange: exchange.to_string(),
        price: 0.0,
        best_bid: str_f64(data, "b"),
        best_ask: str_f64(data, "a"),
        exchange_ts: now,
        receipt_ts: now,
    })]
}

fn parse_depth_snapshot(exchange: &str, symbol: &str, data: &Value) -> Vec<DataMessage> {
    // Partial-book frames carry no event time; treat receipt time as both.
    let now = now_ms();
    vec![DataMessage::OrderBook(OrderBookData {
        symbol: symbol.to_string(),
        exchange: exchange.to_string(),
        asks: parse_levels(&data["asks"]),
        bids: parse_levels(&data["bids"]),
        exchange_ts: now,
        receipt_ts: now,
        is_snapshot: true,
    })]
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn connector() -> BinanceConnector {
        BinanceConnector::spot(&["btcusdt@aggTrade"])
    }

    #[test]
    fn parse_combined_stream_aggtrade() {
        let raw = r#"{
            "stream": "btcusdt@aggTrade",
            "data": {
                "e": "aggTrade",
                "E": 1700000000000,
                "s": "BTCUSDT",
                "a": 12345,
                "p": "96000.50",
                "q": "0.05",
                "f": 100,
                "l": 102,
                "T": 1700000000050,
                "m": true,
                "M": true
            }
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            DataMessage::Trade(t) => {
                assert_eq!(t.symbol, "BTCUSDT");
                assert_eq!(t.exchange, "binance");
                assert_eq!(t.side, TradeSide::Sell); // m=true → maker is buyer → aggressive sell
                assert!((t.price - 96_000.5).abs() < 1e-9);
                assert!((t.amount - 0.05).abs() < 1e-12);
                assert_eq!(t.exchange_ts, 1_700_000_000_050);
                assert_eq!(t.trade_id, "12345");
            }
            other => panic!("expected Trade, got {other:?}"),
        }
    }

    #[test]
    fn parse_bookticker_into_ticker() {
        // bookTicker has no `e` field and is wrapped by the combined stream.
        let raw = r#"{
            "stream": "btcusdt@bookTicker",
            "data": {
                "u": 400900217,
                "s": "BTCUSDT",
                "b": "96000.10",
                "B": "1.5",
                "a": "96001.00",
                "A": "0.8"
            }
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        match &msgs[0] {
            DataMessage::Ticker(t) => {
                assert_eq!(t.symbol, "BTCUSDT");
                assert!((t.best_bid - 96_000.1).abs() < 1e-9);
                assert!((t.best_ask - 96_001.0).abs() < 1e-9);
                // bookTicker carries no last-trade price.
                assert!((t.price - 0.0).abs() < 1e-12);
            }
            other => panic!("expected Ticker, got {other:?}"),
        }
    }

    #[test]
    fn parse_kline_into_candle() {
        let raw = r#"{
            "stream": "btcusdt@kline_1m",
            "data": {
                "e": "kline",
                "E": 1700000000000,
                "s": "BTCUSDT",
                "k": {
                    "t": 1700000000000,
                    "T": 1700000059999,
                    "s": "BTCUSDT",
                    "i": "1m",
                    "f": 100,
                    "L": 200,
                    "o": "96000.00",
                    "c": "96100.00",
                    "h": "96200.00",
                    "l": "95900.00",
                    "v": "100.5",
                    "n": 250,
                    "x": true,
                    "q": "9650000.0",
                    "V": "50.0",
                    "Q": "4800000.0",
                    "B": "0"
                }
            }
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        match &msgs[0] {
            DataMessage::Candle(c) => {
                assert_eq!(c.symbol, "BTCUSDT");
                assert_eq!(c.interval, "1m");
                assert!((c.open - 96_000.0).abs() < 1e-9);
                assert!((c.close - 96_100.0).abs() < 1e-9);
                assert!(c.is_closed);
            }
            other => panic!("expected Candle, got {other:?}"),
        }
    }

    #[test]
    fn parse_depth_update_into_orderbook_delta() {
        let raw = r#"{
            "stream": "btcusdt@depth@100ms",
            "data": {
                "e": "depthUpdate",
                "E": 1700000000000,
                "s": "BTCUSDT",
                "U": 157,
                "u": 160,
                "b": [["96000.00", "1.5"], ["95999.50", "0.0"]],
                "a": [["96001.00", "0.5"]]
            }
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        match &msgs[0] {
            DataMessage::OrderBook(ob) => {
                assert_eq!(ob.symbol, "BTCUSDT");
                assert!(!ob.is_snapshot);
                assert_eq!(ob.bids.len(), 2);
                assert!((ob.bids[1][1] - 0.0).abs() < 1e-12); // qty 0 = remove level
                assert_eq!(ob.asks.len(), 1);
            }
            other => panic!("expected OrderBook, got {other:?}"),
        }
    }

    #[test]
    fn parse_depth_snapshot_uses_stream_name_for_symbol() {
        // Partial-book snapshot frame has no symbol — must come from stream.
        let raw = r#"{
            "stream": "btcusdt@depth5@100ms",
            "data": {
                "lastUpdateId": 999,
                "bids": [["96000.00", "1.0"]],
                "asks": [["96001.00", "0.5"]]
            }
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        match &msgs[0] {
            DataMessage::OrderBook(ob) => {
                assert_eq!(ob.symbol, "BTCUSDT");
                assert!(ob.is_snapshot);
                assert_eq!(ob.bids.len(), 1);
                assert_eq!(ob.asks.len(), 1);
            }
            other => panic!("expected OrderBook, got {other:?}"),
        }
    }

    #[test]
    fn parse_mark_price_into_funding_rate() {
        let raw = r#"{
            "stream": "btcusdt@markPrice@1s",
            "data": {
                "e": "markPriceUpdate",
                "E": 1700000000000,
                "s": "BTCUSDT",
                "p": "96010.0",
                "i": "96005.0",
                "P": "96012.0",
                "r": "0.0001",
                "T": 1700028800000
            }
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        match &msgs[0] {
            DataMessage::FundingRate(f) => {
                assert_eq!(f.symbol, "BTCUSDT");
                assert!((f.funding_rate - 0.0001).abs() < 1e-9);
                assert_eq!(f.next_funding_time, 1_700_028_800_000);
                assert_eq!(f.mark_price, Some(96_010.0));
                assert_eq!(f.index_price, Some(96_005.0));
            }
            other => panic!("expected FundingRate, got {other:?}"),
        }
    }

    #[test]
    fn unknown_event_returns_empty_vec() {
        let raw = r#"{"e": "someFutureEvent", "s": "BTCUSDT"}"#;
        let msgs = connector().parse_message(raw).expect("parse");
        assert!(msgs.is_empty(), "unknown event should yield no DataMessages");
    }

    #[test]
    fn stream_name_builders_lowercase_symbols() {
        assert_eq!(
            BinanceConnector::trade_stream("BTCUSDT"),
            "btcusdt@aggTrade"
        );
        assert_eq!(
            BinanceConnector::ticker_stream("ETHUSDT"),
            "ethusdt@bookTicker"
        );
        assert_eq!(
            BinanceConnector::kline_stream("BTCUSDT", "5m"),
            "btcusdt@kline_5m"
        );
        assert_eq!(
            BinanceConnector::depth_stream("BTCUSDT"),
            "btcusdt@depth@100ms"
        );
        assert_eq!(
            BinanceConnector::mark_price_stream("BTCUSDT"),
            "btcusdt@markPrice@1s"
        );
    }

    #[test]
    fn depth_snapshot_stream_clamps_levels() {
        assert!(BinanceConnector::depth_snapshot_stream("BTCUSDT", 0).ends_with("@depth5@100ms"));
        assert!(BinanceConnector::depth_snapshot_stream("BTCUSDT", 5).ends_with("@depth5@100ms"));
        assert!(BinanceConnector::depth_snapshot_stream("BTCUSDT", 8).ends_with("@depth10@100ms"));
        assert!(BinanceConnector::depth_snapshot_stream("BTCUSDT", 20).ends_with("@depth20@100ms"));
        assert!(BinanceConnector::depth_snapshot_stream("BTCUSDT", 100).ends_with("@depth20@100ms"));
    }

    #[test]
    fn spot_url_combines_streams() {
        let c = BinanceConnector::spot(&["btcusdt@aggTrade", "btcusdt@bookTicker"]);
        assert!(c.url.starts_with("wss://stream.binance.com:9443/stream?streams="));
        assert!(c.url.contains("btcusdt@aggTrade"));
        assert!(c.url.contains("btcusdt@bookTicker"));
        assert_eq!(c.exchange, "binance");
    }

    #[test]
    fn futures_url_uses_fstream_host() {
        let c = BinanceConnector::futures(&["btcusdt@markPrice@1s"]);
        assert!(c.url.starts_with("wss://fstream.binance.com/stream?streams="));
        assert_eq!(c.exchange, "binance-futures");
    }
}
