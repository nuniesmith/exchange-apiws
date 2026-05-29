//! Bybit WebSocket connector — public market-data streams (v5).
//!
//! Implements [`ExchangeConnector`] so a [`BybitConnector`] plugs straight
//! into [`run_feed`](crate::ws::run_feed) and
//! [`run_feed_supervised`](crate::ws::run_feed_supervised).
//!
//! Bybit splits the WS endpoint by product class
//! (`/v5/public/spot`, `/v5/public/linear`, `/v5/public/inverse`) and
//! drives subscriptions via JSON `{"op":"subscribe","args":[…]}` frames
//! sent **after** the WS handshake — complementing Binance's URL-encoded
//! approach. Heartbeats are also JSON: the connector sends
//! `{"op":"ping"}` every 20 s and the server replies `{"op":"pong"}`.
//!
//! # Supported topics
//!
//! | Helper | Bybit topic | Emits |
//! |---|---|---|
//! | [`BybitConnector::trade_topic`] | `publicTrade.<sym>` | `DataMessage::Trade` (one per array element) |
//! | [`BybitConnector::ticker_topic`] | `tickers.<sym>` | `DataMessage::Ticker` (snapshot/delta) |
//! | [`BybitConnector::kline_topic`] | `kline.<interval>.<sym>` | `DataMessage::Candle` (confirm flag) |
//! | [`BybitConnector::orderbook_topic`] | `orderbook.<depth>.<sym>` | `DataMessage::OrderBook` |
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use tokio::sync::{mpsc, watch};
//! use exchange_apiws::actors::{DataMessage, ExchangeConnector};
//! use exchange_apiws::bybit::{BybitCategory, BybitConnector};
//! use exchange_apiws::ws::{WsRunnerConfig, run_feed};
//!
//! # async fn example() -> exchange_apiws::Result<()> {
//! let topics = vec![
//!     BybitConnector::trade_topic("BTCUSDT"),
//!     BybitConnector::orderbook_topic("BTCUSDT", 50),
//! ];
//! let connector = Arc::new(BybitConnector::new(BybitCategory::Linear, topics));
//! let url = connector.ws_url().to_string();
//! let subs = connector.subscription_message("").into_iter().collect();
//!
//! let (tx, mut rx) = mpsc::channel::<DataMessage>(1024);
//! let (_sd_tx, sd_rx) = watch::channel(false);
//!
//! tokio::spawn(run_feed(url, subs, connector, tx, WsRunnerConfig::default(), sd_rx));
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
use crate::bybit::BybitCategory;
use crate::error::Result;

const WS_BASE: &str = "wss://stream.bybit.com/v5/public";
const EXCHANGE_NAME: &str = "bybit";
/// Bybit recommends ping every 20 s; server disconnects after ~50 s of silence.
const PING_INTERVAL_SECS: u64 = 20;

// ── Connector ────────────────────────────────────────────────────────────────

/// Bybit WebSocket connector — one of spot / linear / inverse.
///
/// Topics are bundled at construction time into a single `subscribe` frame
/// sent immediately after handshake; further subscriptions on the same
/// session aren't supported by this connector (build a second one if you
/// need a separate symbol set).
#[derive(Debug, Clone)]
pub struct BybitConnector {
    /// Full WSS URL — `wss://stream.bybit.com/v5/public/{spot|linear|inverse}`.
    pub url: String,
    /// Product class this connector targets.
    pub category: BybitCategory,
    /// Topics that will be subscribed on connect.
    pub topics: Vec<String>,
}

impl BybitConnector {
    /// Build a connector for `category` subscribed to `topics`.
    ///
    /// Each topic is something like `"publicTrade.BTCUSDT"` — use the
    /// `*_topic` helpers below to construct them.
    #[must_use]
    pub fn new(category: BybitCategory, topics: Vec<String>) -> Self {
        Self {
            url: format!("{WS_BASE}/{}", category.as_str()),
            category,
            topics,
        }
    }

    /// Build a connector with a caller-supplied URL — used by tests
    /// pointing at a local tokio-tungstenite server.
    #[must_use]
    pub fn with_url(url: impl Into<String>, category: BybitCategory, topics: Vec<String>) -> Self {
        Self {
            url: url.into(),
            category,
            topics,
        }
    }

    // ── Topic builders ──────────────────────────────────────────────────────

    /// Public-trade topic — `publicTrade.<symbol>`.
    #[must_use]
    pub fn trade_topic(symbol: &str) -> String {
        format!("publicTrade.{symbol}")
    }

    /// Ticker topic — `tickers.<symbol>`.
    ///
    /// First frame on a subscription is `type: "snapshot"`; subsequent
    /// frames are `type: "delta"` with only changed fields. The connector
    /// propagates that distinction via `DataMessage::Ticker` — readers
    /// should expect partial updates.
    #[must_use]
    pub fn ticker_topic(symbol: &str) -> String {
        format!("tickers.{symbol}")
    }

    /// Kline topic — `kline.<interval>.<symbol>`.
    ///
    /// `interval` follows Bybit's wire values: `"1"`, `"3"`, `"5"`, `"15"`,
    /// `"30"`, `"60"`, `"120"`, `"240"`, `"360"`, `"720"`, `"D"`, `"W"`, `"M"`.
    #[must_use]
    pub fn kline_topic(symbol: &str, interval: &str) -> String {
        format!("kline.{interval}.{symbol}")
    }

    /// Order-book topic — `orderbook.<depth>.<symbol>`.
    ///
    /// `depth` accepts `1`, `50`, `200`, `500` (varies by product class).
    /// The first frame is a snapshot (`type: "snapshot"`); subsequent
    /// frames are deltas (`type: "delta"`).
    #[must_use]
    pub fn orderbook_topic(symbol: &str, depth: u32) -> String {
        format!("orderbook.{depth}.{symbol}")
    }
}

// ── ExchangeConnector ────────────────────────────────────────────────────────

impl ExchangeConnector for BybitConnector {
    fn exchange_name(&self) -> &str {
        EXCHANGE_NAME
    }

    fn ws_url(&self) -> &str {
        &self.url
    }

    fn build_ws_config(&self, symbol: &str) -> WebSocketConfig {
        WebSocketConfig {
            url: self.url.clone(),
            exchange: EXCHANGE_NAME.to_string(),
            symbol: symbol.to_string(),
            subscription_msg: self.subscription_message(symbol),
            ping_interval_secs: PING_INTERVAL_SECS,
            reconnect_delay_secs: 5,
            max_reconnect_attempts: 5,
        }
    }

    /// Returns the connector's configured topics packaged as a single
    /// `{"op":"subscribe","args":[…]}` frame. `symbol` is unused — topic
    /// selection happens at construction time.
    fn subscription_message(&self, _symbol: &str) -> Option<String> {
        if self.topics.is_empty() {
            return None;
        }
        serde_json::to_string(&serde_json::json!({
            "op": "subscribe",
            "args": &self.topics,
        }))
        .ok()
    }

    /// Bybit expects `{"op":"ping"}`; the server responds `{"op":"pong"}`.
    fn ping_message(&self) -> Option<String> {
        Some(r#"{"op":"ping"}"#.to_string())
    }

    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>> {
        let json: Value = serde_json::from_str(raw)?;

        // Op responses (subscribe ack, pong) — no payload to surface.
        if json.get("op").is_some() {
            return Ok(vec![]);
        }

        let topic = json.get("topic").and_then(Value::as_str).unwrap_or("");
        let Some(data) = json.get("data") else {
            return Ok(vec![]);
        };
        let is_snapshot = json
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("snapshot")
            == "snapshot";

        if topic.starts_with("publicTrade.") {
            Ok(parse_trade_batch(data))
        } else if topic.starts_with("tickers.") {
            Ok(parse_ticker(data))
        } else if topic.starts_with("kline.") {
            Ok(parse_kline_batch(data))
        } else if topic.starts_with("orderbook.") {
            Ok(parse_orderbook(data, is_snapshot))
        } else {
            Ok(vec![])
        }
    }
}

// ── Parsers ──────────────────────────────────────────────────────────────────

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn str_f64(v: &Value, key: &str) -> f64 {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

fn opt_str_f64(v: &Value, key: &str) -> Option<f64> {
    v.get(key)
        .and_then(Value::as_str)
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

/// `publicTrade.<sym>` data is an array of trade objects.
fn parse_trade_batch(data: &Value) -> Vec<DataMessage> {
    let Some(arr) = data.as_array() else {
        return vec![];
    };
    arr.iter()
        .map(|t| {
            // Bybit's side is "Buy" or "Sell" (capitalised); fall back to
            // Buy on parse failure to match the existing KuCoin behaviour.
            let side = match t.get("S").and_then(Value::as_str).unwrap_or("Buy") {
                s if s.eq_ignore_ascii_case("Sell") => TradeSide::Sell,
                _ => TradeSide::Buy,
            };
            DataMessage::Trade(TradeData {
                symbol: t["s"].as_str().unwrap_or("").to_string(),
                exchange: EXCHANGE_NAME.to_string(),
                side,
                price: str_f64(t, "p"),
                amount: str_f64(t, "v"),
                exchange_ts: t["T"].as_i64().unwrap_or(0),
                receipt_ts: now_ms(),
                trade_id: t.get("i").and_then(Value::as_str).unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// `tickers.<sym>` data is a single object. Snapshot includes all fields;
/// delta includes only changed ones (any field may be absent).
fn parse_ticker(data: &Value) -> Vec<DataMessage> {
    let symbol = data["symbol"].as_str().unwrap_or("").to_string();
    let last_price = str_f64(data, "lastPrice");
    let bid = opt_str_f64(data, "bid1Price").unwrap_or(0.0);
    let ask = opt_str_f64(data, "ask1Price").unwrap_or(0.0);
    let now = now_ms();
    vec![DataMessage::Ticker(TickerData {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        price: last_price,
        best_bid: bid,
        best_ask: ask,
        exchange_ts: now,
        receipt_ts: now,
    })]
}

/// `kline.<interval>.<sym>` data is an array of kline objects. `confirm: true`
/// marks the bar as closed.
fn parse_kline_batch(data: &Value) -> Vec<DataMessage> {
    let Some(arr) = data.as_array() else {
        return vec![];
    };
    arr.iter()
        .map(|k| {
            DataMessage::Candle(CandleData {
                // The kline object doesn't include the symbol; callers can
                // recover it from the topic if needed. Bybit's wire format
                // stores it in the parent frame's topic string only.
                symbol: String::new(),
                exchange: EXCHANGE_NAME.to_string(),
                interval: k["interval"].as_str().unwrap_or("").to_string(),
                open_ts: k["start"].as_i64().unwrap_or(0),
                open: str_f64(k, "open"),
                high: str_f64(k, "high"),
                low: str_f64(k, "low"),
                close: str_f64(k, "close"),
                volume: str_f64(k, "volume"),
                is_closed: k["confirm"].as_bool().unwrap_or(false),
                receipt_ts: now_ms(),
            })
        })
        .collect()
}

/// `orderbook.<depth>.<sym>` data is a single object with `s`, `b`, `a`.
fn parse_orderbook(data: &Value, is_snapshot: bool) -> Vec<DataMessage> {
    let symbol = data["s"].as_str().unwrap_or("").to_string();
    let now = now_ms();
    vec![DataMessage::OrderBook(OrderBookData {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        asks: parse_levels(&data["a"]),
        bids: parse_levels(&data["b"]),
        exchange_ts: now,
        receipt_ts: now,
        is_snapshot,
    })]
}

// Tickers carry futures-specific fields on Linear/Inverse; the Ticker
// variant only surfaces last/bid/ask which are common to all categories.
// Callers who want mark/index/funding can recover them via REST today;
// a richer Ticker variant could be added if there's demand.
#[allow(dead_code)]
fn parse_ticker_funding(data: &Value) -> Option<FundingData> {
    let funding_rate = opt_str_f64(data, "fundingRate")?;
    let next_funding = data
        .get("nextFundingTime")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    Some(FundingData {
        symbol: data["symbol"].as_str().unwrap_or("").to_string(),
        exchange: EXCHANGE_NAME.to_string(),
        funding_rate,
        next_funding_time: next_funding,
        mark_price: opt_str_f64(data, "markPrice"),
        index_price: opt_str_f64(data, "indexPrice"),
        exchange_ts: now_ms(),
        receipt_ts: now_ms(),
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn connector() -> BybitConnector {
        BybitConnector::new(
            BybitCategory::Linear,
            vec!["publicTrade.BTCUSDT".into(), "kline.1.BTCUSDT".into()],
        )
    }

    #[test]
    fn subscription_message_packs_topics() {
        let c = connector();
        let sub = c.subscription_message("ignored").expect("sub");
        let parsed: Value = serde_json::from_str(&sub).unwrap();
        assert_eq!(parsed["op"], "subscribe");
        let args = parsed["args"].as_array().unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "publicTrade.BTCUSDT");
    }

    #[test]
    fn empty_topics_returns_no_subscription() {
        let c = BybitConnector::new(BybitCategory::Spot, vec![]);
        assert!(c.subscription_message("BTCUSDT").is_none());
    }

    #[test]
    fn ping_uses_bybit_op_format() {
        assert_eq!(
            connector().ping_message().as_deref(),
            Some(r#"{"op":"ping"}"#)
        );
    }

    #[test]
    fn parse_op_ack_returns_empty() {
        let raw = r#"{"op":"subscribe","conn_id":"x","ret_msg":"","success":true,"req_id":"y"}"#;
        let msgs = connector().parse_message(raw).expect("parse");
        assert!(msgs.is_empty());
    }

    #[test]
    fn parse_pong_returns_empty() {
        let raw = r#"{"op":"pong","ret_msg":"pong","success":true}"#;
        let msgs = connector().parse_message(raw).expect("parse");
        assert!(msgs.is_empty());
    }

    #[test]
    fn parse_public_trade_emits_one_per_array_element() {
        let raw = r#"{
            "topic": "publicTrade.BTCUSDT",
            "type": "snapshot",
            "ts": 1700000000000,
            "data": [
                {"T":1700000000050,"s":"BTCUSDT","S":"Buy","v":"0.1","p":"96000.0","L":"PlusTick","i":"id-1","BT":false},
                {"T":1700000000080,"s":"BTCUSDT","S":"Sell","v":"0.05","p":"96005.0","L":"MinusTick","i":"id-2","BT":false}
            ]
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        assert_eq!(msgs.len(), 2);
        match &msgs[0] {
            DataMessage::Trade(t) => {
                assert_eq!(t.symbol, "BTCUSDT");
                assert_eq!(t.exchange, "bybit");
                assert_eq!(t.side, TradeSide::Buy);
                assert!((t.price - 96_000.0).abs() < 1e-9);
                assert_eq!(t.trade_id, "id-1");
            }
            other => panic!("expected Trade, got {other:?}"),
        }
        match &msgs[1] {
            DataMessage::Trade(t) => assert_eq!(t.side, TradeSide::Sell),
            _ => panic!("expected Trade variant"),
        }
    }

    #[test]
    fn parse_ticker_into_ticker_data() {
        let raw = r#"{
            "topic":"tickers.BTCUSDT",
            "type":"snapshot",
            "ts":1700000000000,
            "data":{
                "symbol":"BTCUSDT",
                "lastPrice":"96000.0",
                "bid1Price":"95999.0","bid1Size":"1.0",
                "ask1Price":"96001.0","ask1Size":"1.5",
                "markPrice":"96010.0","indexPrice":"96005.0",
                "fundingRate":"0.0001","nextFundingTime":"1700028800000"
            }
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        match &msgs[0] {
            DataMessage::Ticker(t) => {
                assert_eq!(t.symbol, "BTCUSDT");
                assert!((t.price - 96_000.0).abs() < 1e-9);
                assert!((t.best_bid - 95_999.0).abs() < 1e-9);
                assert!((t.best_ask - 96_001.0).abs() < 1e-9);
            }
            other => panic!("expected Ticker, got {other:?}"),
        }
    }

    #[test]
    fn parse_kline_into_candle() {
        let raw = r#"{
            "topic":"kline.1.BTCUSDT",
            "type":"snapshot",
            "ts":1700000000050,
            "data":[{
                "start":1700000000000,"end":1700000059999,"interval":"1",
                "open":"96000.0","close":"96100.0","high":"96200.0","low":"95900.0",
                "volume":"10.0","turnover":"961000.0","confirm":true,"timestamp":1700000000050
            }]
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        match &msgs[0] {
            DataMessage::Candle(c) => {
                assert_eq!(c.interval, "1");
                assert_eq!(c.open_ts, 1_700_000_000_000);
                assert!((c.open - 96_000.0).abs() < 1e-9);
                assert!(c.is_closed);
            }
            other => panic!("expected Candle, got {other:?}"),
        }
    }

    #[test]
    fn parse_orderbook_snapshot_then_delta() {
        let snap = r#"{
            "topic":"orderbook.50.BTCUSDT",
            "type":"snapshot",
            "ts":1700000000000,
            "data":{"s":"BTCUSDT","b":[["96000.0","1.5"]],"a":[["96001.0","0.5"]],"u":1,"seq":1}
        }"#;
        let delta = r#"{
            "topic":"orderbook.50.BTCUSDT",
            "type":"delta",
            "ts":1700000000100,
            "data":{"s":"BTCUSDT","b":[["95999.5","0.0"]],"a":[],"u":2,"seq":2}
        }"#;
        let c = connector();
        match &c.parse_message(snap).expect("snap")[0] {
            DataMessage::OrderBook(ob) => {
                assert_eq!(ob.symbol, "BTCUSDT");
                assert!(ob.is_snapshot);
                assert_eq!(ob.asks.len(), 1);
            }
            _ => panic!("snapshot was not OrderBook"),
        }
        match &c.parse_message(delta).expect("delta")[0] {
            DataMessage::OrderBook(ob) => {
                assert!(!ob.is_snapshot);
                assert!((ob.bids[0][1] - 0.0).abs() < 1e-12); // qty 0 = remove
            }
            _ => panic!("delta was not OrderBook"),
        }
    }

    #[test]
    fn topic_builders_format() {
        assert_eq!(
            BybitConnector::trade_topic("BTCUSDT"),
            "publicTrade.BTCUSDT"
        );
        assert_eq!(BybitConnector::ticker_topic("BTCUSDT"), "tickers.BTCUSDT");
        assert_eq!(
            BybitConnector::kline_topic("BTCUSDT", "1"),
            "kline.1.BTCUSDT"
        );
        assert_eq!(
            BybitConnector::orderbook_topic("BTCUSDT", 50),
            "orderbook.50.BTCUSDT"
        );
    }

    #[test]
    fn ws_url_per_category() {
        assert!(
            BybitConnector::new(BybitCategory::Spot, vec![])
                .url
                .ends_with("/v5/public/spot")
        );
        assert!(
            BybitConnector::new(BybitCategory::Linear, vec![])
                .url
                .ends_with("/v5/public/linear")
        );
        assert!(
            BybitConnector::new(BybitCategory::Inverse, vec![])
                .url
                .ends_with("/v5/public/inverse")
        );
    }

    #[test]
    fn ticker_funding_extractor_round_trip() {
        let data = serde_json::json!({
            "symbol":"BTCUSDT",
            "markPrice":"96010.0","indexPrice":"96005.0",
            "fundingRate":"0.0001","nextFundingTime":"1700028800000"
        });
        let f = parse_ticker_funding(&data).expect("funding");
        assert_eq!(f.exchange, "bybit");
        assert_eq!(f.next_funding_time, 1_700_028_800_000);
        assert_eq!(f.mark_price, Some(96_010.0));
    }
}
