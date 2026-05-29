//! Crypto.com WebSocket connector — public market-data streams.
//!
//! Implements [`ExchangeConnector`] so a [`CryptocomConnector`] drops
//! into [`run_feed`](crate::ws::run_feed) and
//! [`run_feed_supervised`](crate::ws::run_feed_supervised).
//!
//! # Heartbeat protocol
//!
//! Crypto.com's heartbeat is **server-initiated**: the server pushes
//! `{"id":<N>,"method":"public/heartbeat"}` periodically and the
//! client must reply with `{"id":<N>,"method":"public/respond-heartbeat"}`
//! echoing the same `id`. The runner's [`ping_message`][crate::actors::ExchangeConnector::ping_message]
//! model doesn't fit (it's tick-driven and stateless), so this
//! connector overrides [`response_for`][crate::actors::ExchangeConnector::response_for]
//! to craft the response from each inbound heartbeat. Application-level
//! pings via `ping_message` are therefore disabled (return `None`).
//!
//! # Supported channels
//!
//! | Helper | Channel | Emits |
//! |---|---|---|
//! | [`CryptocomConnector::trade_channel`] | `trade.<instr>` | `DataMessage::Trade` |
//! | [`CryptocomConnector::ticker_channel`] | `ticker.<instr>` | `DataMessage::Ticker` |
//! | [`CryptocomConnector::candlestick_channel`] | `candlestick.<tf>.<instr>` | `DataMessage::Candle` |
//! | [`CryptocomConnector::book_channel`] | `book.<instr>.<depth>` | `DataMessage::OrderBook` (snapshot then deltas) |
//!
//! # Endpoints
//!
//! - Public: `wss://stream.crypto.com/exchange/v1/market`
//! - Private (`user.order.*`, `user.balance`): `wss://stream.crypto.com/exchange/v1/user`
//!   — requires an auth frame sent immediately after connect. Use
//!   [`CryptocomConnector::private`] and compose your own subscribe
//!   JSON; the parser ignores unknown channels so it won't trip on
//!   private-channel payloads either.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use tokio::sync::{mpsc, watch};
//! use exchange_apiws::actors::{DataMessage, ExchangeConnector};
//! use exchange_apiws::cryptocom::ws::CryptocomConnector;
//! use exchange_apiws::ws::{WsRunnerConfig, run_feed};
//!
//! # async fn example() -> exchange_apiws::Result<()> {
//! let subs = vec![CryptocomConnector::subscribe_frame(
//!     1,
//!     &[
//!         CryptocomConnector::trade_channel("BTC_USDT"),
//!         CryptocomConnector::book_channel("BTC_USDT", 10),
//!     ],
//! )];
//! let connector = Arc::new(CryptocomConnector::public());
//! let url = connector.ws_url().to_string();
//!
//! let (tx, mut rx) = mpsc::channel::<DataMessage>(1024);
//! let (_sd_tx, sd_rx) = watch::channel(false);
//! tokio::spawn(run_feed(url, subs, connector, tx, WsRunnerConfig::default(), sd_rx));
//! while let Some(msg) = rx.recv().await { println!("{msg:?}"); }
//! # Ok(())
//! # }
//! ```

use serde_json::{Value, json};

use crate::actors::{
    CandleData, DataMessage, ExchangeConnector, OrderBookData, TickerData, TradeData, TradeSide,
    WebSocketConfig,
};
use crate::error::Result;

const WS_PUBLIC_URL: &str = "wss://stream.crypto.com/exchange/v1/market";
const WS_PRIVATE_URL: &str = "wss://stream.crypto.com/exchange/v1/user";
const EXCHANGE_NAME: &str = "cryptocom";

// ── Connector ───────────────────────────────────────────────────────────────

/// Crypto.com WebSocket connector.
///
/// Cheap to clone. Subscription frames are passed to
/// [`run_feed`](crate::ws::run_feed) as the `subscriptions` vector —
/// build them with [`Self::subscribe_frame`] and the channel helpers.
#[derive(Debug, Clone)]
pub struct CryptocomConnector {
    /// Full WSS URL — public or private endpoint.
    pub url: String,
}

impl CryptocomConnector {
    /// Public market-data endpoint (`wss://stream.crypto.com/exchange/v1/market`).
    #[must_use]
    pub fn public() -> Self {
        Self {
            url: WS_PUBLIC_URL.to_string(),
        }
    }

    /// Private (user) endpoint
    /// (`wss://stream.crypto.com/exchange/v1/user`) — requires an
    /// auth frame after connect. Callers must compose private
    /// subscribe JSON themselves.
    #[must_use]
    pub fn private() -> Self {
        Self {
            url: WS_PRIVATE_URL.to_string(),
        }
    }

    /// Build with a caller-supplied URL — used by tests pointing at a
    /// local tokio-tungstenite server.
    #[must_use]
    pub fn with_url(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }

    // ── Channel-name helpers ────────────────────────────────────────────────

    /// `trade.<instrument>`.
    #[must_use]
    pub fn trade_channel(instrument: &str) -> String {
        format!("trade.{instrument}")
    }

    /// `ticker.<instrument>`.
    #[must_use]
    pub fn ticker_channel(instrument: &str) -> String {
        format!("ticker.{instrument}")
    }

    /// `candlestick.<timeframe>.<instrument>`.
    ///
    /// `timeframe` follows Crypto.com's wire values (`"1m"`, `"5m"`,
    /// `"15m"`, `"30m"`, `"1h"`, `"4h"`, `"6h"`, `"12h"`, `"1D"`,
    /// `"7D"`, `"14D"`, `"1M"`).
    #[must_use]
    pub fn candlestick_channel(instrument: &str, timeframe: &str) -> String {
        format!("candlestick.{timeframe}.{instrument}")
    }

    /// `book.<instrument>.<depth>`.
    ///
    /// `depth` accepts `10`, `50`, `150`. First frame after subscribe
    /// is `"snapshot"`; subsequent frames are `"update"` (deltas).
    #[must_use]
    pub fn book_channel(instrument: &str, depth: u32) -> String {
        format!("book.{instrument}.{depth}")
    }

    /// Build a `{"id":N,"method":"subscribe","params":{"channels":[…]}}`
    /// frame for the given list of channel strings.
    #[must_use]
    pub fn subscribe_frame(id: i64, channels: &[String]) -> String {
        let channels_ref: Vec<&str> = channels.iter().map(String::as_str).collect();
        json!({
            "id": id,
            "method": "subscribe",
            "params": {"channels": channels_ref},
        })
        .to_string()
    }
}

// ── ExchangeConnector ───────────────────────────────────────────────────────

impl ExchangeConnector for CryptocomConnector {
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
            subscription_msg: None,
            // The recv loop wakes on every server heartbeat (roughly
            // every 30 s) so app pings aren't needed. The tick still
            // drives the idle check.
            ping_interval_secs: 30,
            reconnect_delay_secs: 5,
            max_reconnect_attempts: 5,
        }
    }

    /// Subscriptions are pre-built by the caller and passed as the
    /// `subscriptions` vector to `run_feed`.
    fn subscription_message(&self, _symbol: &str) -> Option<String> {
        None
    }

    /// Crypto.com's heartbeat is server-initiated — no client ping is
    /// needed. The required response is generated inside
    /// [`Self::response_for`] instead.
    fn ping_message(&self) -> Option<String> {
        None
    }

    /// When the inbound frame is a `public/heartbeat`, emit a
    /// `public/respond-heartbeat` echoing the server's `id`. Otherwise
    /// `None`.
    fn response_for(&self, raw: &str) -> Option<String> {
        // Cheap-path early return: if the frame doesn't contain the
        // heartbeat method substring there's no point JSON-parsing it.
        if !raw.contains("public/heartbeat") {
            return None;
        }
        let json: Value = serde_json::from_str(raw).ok()?;
        if json.get("method").and_then(Value::as_str)? != "public/heartbeat" {
            return None;
        }
        let id = json.get("id").and_then(Value::as_i64)?;
        Some(json!({"id": id, "method": "public/respond-heartbeat"}).to_string())
    }

    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>> {
        let json: Value = serde_json::from_str(raw)?;

        // Heartbeat — runner has already handled the response via
        // `response_for`, so nothing more to do here.
        if json.get("method").and_then(Value::as_str) == Some("public/heartbeat") {
            return Ok(vec![]);
        }

        // Subscribe acks and other method-response frames have a
        // top-level `method` and no `result.data` of interest.
        let Some(result) = json.get("result") else {
            return Ok(vec![]);
        };
        let channel = result.get("channel").and_then(Value::as_str).unwrap_or("");
        let Some(data) = result.get("data").and_then(Value::as_array) else {
            return Ok(vec![]);
        };
        let instrument = result
            .get("instrument_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let is_snapshot = result
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("update")
            == "snapshot";

        match channel {
            "trade" => Ok(parse_trades(data, &instrument)),
            "ticker" => Ok(parse_tickers(data, &instrument)),
            "candlestick" => Ok(parse_candles(data, &instrument)),
            "book" => Ok(parse_books(data, &instrument, is_snapshot)),
            _ => Ok(vec![]),
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Crypto.com sometimes sends a numeric field as a JSON string and
/// sometimes as a number; accept both.
fn flexible_f64(v: &Value, key: &str) -> f64 {
    match v.get(key) {
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

// ── Parsers ─────────────────────────────────────────────────────────────────

fn parse_trades(data: &[Value], instrument_fallback: &str) -> Vec<DataMessage> {
    data.iter()
        .map(|t| {
            let side = match t.get("s").and_then(Value::as_str).unwrap_or("buy") {
                s if s.eq_ignore_ascii_case("sell") => TradeSide::Sell,
                _ => TradeSide::Buy,
            };
            let symbol = t
                .get("i")
                .and_then(Value::as_str)
                .unwrap_or(instrument_fallback)
                .to_string();
            DataMessage::Trade(TradeData {
                symbol,
                exchange: EXCHANGE_NAME.to_string(),
                side,
                price: flexible_f64(t, "p"),
                amount: flexible_f64(t, "q"),
                exchange_ts: t.get("t").and_then(Value::as_i64).unwrap_or(0),
                receipt_ts: now_ms(),
                trade_id: t.get("d").and_then(Value::as_str).unwrap_or("").to_string(),
            })
        })
        .collect()
}

fn parse_tickers(data: &[Value], instrument_fallback: &str) -> Vec<DataMessage> {
    let now = now_ms();
    data.iter()
        .map(|t| {
            let symbol = t
                .get("i")
                .and_then(Value::as_str)
                .unwrap_or(instrument_fallback)
                .to_string();
            DataMessage::Ticker(TickerData {
                symbol,
                exchange: EXCHANGE_NAME.to_string(),
                // Crypto.com's "a" is the latest trade price, NOT ask.
                price: flexible_f64(t, "a"),
                best_bid: flexible_f64(t, "b"),
                // And "k" is the best ask (unintuitively).
                best_ask: flexible_f64(t, "k"),
                exchange_ts: t.get("t").and_then(Value::as_i64).unwrap_or(now),
                receipt_ts: now,
            })
        })
        .collect()
}

fn parse_candles(data: &[Value], instrument_fallback: &str) -> Vec<DataMessage> {
    let now = now_ms();
    data.iter()
        .map(|c| {
            // Some pushes carry the symbol on the candle; others rely on
            // the surrounding result.instrument_name.
            let symbol = c
                .get("i")
                .and_then(Value::as_str)
                .unwrap_or(instrument_fallback)
                .to_string();
            // The interval label rides on the parent result.channel name
            // (e.g. "candlestick.1m.BTC_USDT") but isn't always
            // duplicated on the candle. Pull from the candle when present.
            let interval = c
                .get("interval")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            DataMessage::Candle(CandleData {
                symbol,
                exchange: EXCHANGE_NAME.to_string(),
                interval,
                open_ts: c.get("t").and_then(Value::as_i64).unwrap_or(now),
                open: flexible_f64(c, "o"),
                high: flexible_f64(c, "h"),
                low: flexible_f64(c, "l"),
                close: flexible_f64(c, "c"),
                volume: flexible_f64(c, "v"),
                // Crypto.com doesn't expose a "closed" flag on the
                // candlestick channel — every push updates the bar
                // in-place until the next interval begins.
                is_closed: false,
                receipt_ts: now,
            })
        })
        .collect()
}

fn parse_books(data: &[Value], instrument_fallback: &str, is_snapshot: bool) -> Vec<DataMessage> {
    let now = now_ms();
    data.iter()
        .map(|b| {
            DataMessage::OrderBook(OrderBookData {
                symbol: instrument_fallback.to_string(),
                exchange: EXCHANGE_NAME.to_string(),
                asks: parse_levels(b.get("asks")),
                bids: parse_levels(b.get("bids")),
                exchange_ts: b.get("t").and_then(Value::as_i64).unwrap_or(now),
                receipt_ts: now,
                is_snapshot,
            })
        })
        .collect()
}

/// Crypto.com book levels are `[price_str, qty_str, num_orders_str]`
/// triples; drop the num-orders column for cross-exchange-friendly
/// output.
fn parse_levels(v: Option<&Value>) -> Vec<[f64; 2]> {
    v.and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|lvl| {
                    let p: f64 = lvl.get(0)?.as_str()?.parse().ok()?;
                    let q: f64 = lvl.get(1)?.as_str()?.parse().ok()?;
                    Some([p, q])
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn connector() -> CryptocomConnector {
        CryptocomConnector::public()
    }

    #[test]
    fn ws_url_picks_public_or_private() {
        assert!(CryptocomConnector::public().url.ends_with("/market"));
        assert!(CryptocomConnector::private().url.ends_with("/user"));
    }

    #[test]
    fn channel_builders_format() {
        assert_eq!(
            CryptocomConnector::trade_channel("BTC_USDT"),
            "trade.BTC_USDT"
        );
        assert_eq!(
            CryptocomConnector::ticker_channel("BTC_USDT"),
            "ticker.BTC_USDT"
        );
        assert_eq!(
            CryptocomConnector::candlestick_channel("BTC_USDT", "1m"),
            "candlestick.1m.BTC_USDT"
        );
        assert_eq!(
            CryptocomConnector::book_channel("BTC_USDT", 10),
            "book.BTC_USDT.10"
        );
    }

    #[test]
    fn subscribe_frame_carries_all_channels() {
        let frame = CryptocomConnector::subscribe_frame(
            7,
            &[
                CryptocomConnector::trade_channel("BTC_USDT"),
                CryptocomConnector::book_channel("BTC_USDT", 10),
            ],
        );
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "subscribe");
        assert_eq!(v["params"]["channels"][0], "trade.BTC_USDT");
        assert_eq!(v["params"]["channels"][1], "book.BTC_USDT.10");
    }

    #[test]
    fn ping_message_is_none() {
        // Heartbeats are server-initiated; ping_message MUST stay None.
        assert!(connector().ping_message().is_none());
    }

    #[test]
    fn response_for_heartbeat_echoes_id() {
        let raw = r#"{"id":1234,"method":"public/heartbeat"}"#;
        let resp = connector().response_for(raw).expect("response");
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["id"], 1234);
        assert_eq!(v["method"], "public/respond-heartbeat");
    }

    #[test]
    fn response_for_non_heartbeat_is_none() {
        assert!(
            connector()
                .response_for(r#"{"id":1,"method":"subscribe","code":0}"#)
                .is_none()
        );
        assert!(
            connector()
                .response_for(r#"{"result":{"channel":"trade","data":[]}}"#)
                .is_none()
        );
    }

    #[test]
    fn response_for_short_circuits_on_non_heartbeat_text() {
        // Substring check guards against full JSON parse on every frame.
        // Confirm the early-return path returns None for a frame that
        // doesn't even mention "public/heartbeat".
        let raw = r#"{"result":{"channel":"book","data":[]}}"#;
        assert!(connector().response_for(raw).is_none());
    }

    #[test]
    fn parse_subscribe_ack_returns_empty() {
        let raw = r#"{"id":1,"method":"subscribe","code":0}"#;
        assert!(connector().parse_message(raw).expect("parse").is_empty());
    }

    #[test]
    fn parse_heartbeat_returns_empty() {
        // The response is sent via response_for; parse_message just
        // shouldn't surface anything upstream.
        let raw = r#"{"id":1,"method":"public/heartbeat"}"#;
        assert!(connector().parse_message(raw).expect("parse").is_empty());
    }

    #[test]
    fn parse_trade_uses_inner_symbol_and_emits_one_per_element() {
        let raw = r#"{
            "id":-1,"method":"subscribe","code":0,
            "result":{
                "instrument_name":"BTC_USDT","channel":"trade","subscription":"trade.BTC_USDT",
                "data":[
                    {"i":"BTC_USDT","s":"buy","p":"96000","q":"0.05","t":1700000000000,"d":"id-1"},
                    {"i":"BTC_USDT","s":"sell","p":"96005","q":"0.10","t":1700000000500,"d":"id-2"}
                ]
            }
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        assert_eq!(msgs.len(), 2);
        match &msgs[0] {
            DataMessage::Trade(t) => {
                assert_eq!(t.symbol, "BTC_USDT");
                assert_eq!(t.exchange, "cryptocom");
                assert_eq!(t.side, TradeSide::Buy);
                assert!((t.price - 96_000.0).abs() < 1e-9);
                assert_eq!(t.trade_id, "id-1");
            }
            other => panic!("expected Trade, got {other:?}"),
        }
        match &msgs[1] {
            DataMessage::Trade(t) => assert_eq!(t.side, TradeSide::Sell),
            _ => panic!("expected Trade"),
        }
    }

    #[test]
    fn parse_ticker_routes_letter_fields() {
        let raw = r#"{
            "result":{
                "instrument_name":"BTC_USDT","channel":"ticker","subscription":"ticker.BTC_USDT",
                "data":[{
                    "i":"BTC_USDT","a":"96000.0","b":"95999.0","k":"96001.0",
                    "h":"96500.0","l":"95500.0","v":"100.5","vv":"9650000",
                    "c":"0.005","t":1700000000000
                }]
            }
        }"#;
        match &connector().parse_message(raw).expect("parse")[0] {
            DataMessage::Ticker(t) => {
                assert_eq!(t.symbol, "BTC_USDT");
                assert!((t.price - 96_000.0).abs() < 1e-9);
                assert!((t.best_bid - 95_999.0).abs() < 1e-9);
                assert!((t.best_ask - 96_001.0).abs() < 1e-9);
            }
            other => panic!("expected Ticker, got {other:?}"),
        }
    }

    #[test]
    fn parse_candlestick_emits_one_per_element() {
        let raw = r#"{
            "result":{
                "instrument_name":"BTC_USDT","channel":"candlestick","subscription":"candlestick.1m.BTC_USDT",
                "data":[
                    {"o":"96000","h":"96100","l":"95900","c":"96050","v":"10.5","t":1700000000000}
                ]
            }
        }"#;
        match &connector().parse_message(raw).expect("parse")[0] {
            DataMessage::Candle(c) => {
                assert_eq!(c.symbol, "BTC_USDT");
                assert!((c.open - 96_000.0).abs() < 1e-9);
                assert!((c.close - 96_050.0).abs() < 1e-9);
                // No "closed" flag on this channel.
                assert!(!c.is_closed);
            }
            other => panic!("expected Candle, got {other:?}"),
        }
    }

    #[test]
    fn parse_book_snapshot_then_delta() {
        let snap = r#"{
            "result":{
                "instrument_name":"BTC_USDT","channel":"book","subscription":"book.BTC_USDT.10","type":"snapshot",
                "data":[{
                    "asks":[["96001","0.5","1"]],
                    "bids":[["96000","1.5","2"],["95999","2.0","3"]],
                    "t":1700000000000,"s":1
                }]
            }
        }"#;
        let delta = r#"{
            "result":{
                "instrument_name":"BTC_USDT","channel":"book","subscription":"book.BTC_USDT.10","type":"update",
                "data":[{
                    "asks":[],
                    "bids":[["95998","0","0"]],
                    "t":1700000000100,"s":2
                }]
            }
        }"#;
        let c = connector();
        match &c.parse_message(snap).expect("snap")[0] {
            DataMessage::OrderBook(ob) => {
                assert_eq!(ob.symbol, "BTC_USDT");
                assert!(ob.is_snapshot);
                assert_eq!(ob.bids.len(), 2);
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
    fn parse_unknown_channel_returns_empty() {
        // Private channels (user.order, user.balance) will appear here
        // when callers subscribe to them; the parser ignores them
        // rather than erroring so custom subs keep working.
        let raw = r#"{"result":{"channel":"user.order","data":[{"x":"y"}],"instrument_name":"BTC_USDT"}}"#;
        assert!(connector().parse_message(raw).expect("parse").is_empty());
    }
}
