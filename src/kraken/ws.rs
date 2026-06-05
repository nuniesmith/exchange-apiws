//! Kraken WebSocket v2 connector — public market-data streams.
//!
//! Implements [`ExchangeConnector`] so a [`KrakenConnector`] drops into
//! [`run_feed`](crate::ws::run_feed) and
//! [`run_feed_supervised`](crate::ws::run_feed_supervised).
//!
//! Kraken v2 uses JSON `{"method":"subscribe","params":{…}}` frames sent
//! after handshake (subscribe-after-connect, like Bybit — opposed to
//! Binance's URL-encoded streams). Heartbeats are JSON too:
//! `{"method":"ping"}` from the client, `{"method":"pong"}` from the
//! server; the server also pushes spontaneous
//! `{"channel":"heartbeat"}` frames roughly every second on idle
//! connections.
//!
//! # Supported channels
//!
//! | Helper | Channel | Emits |
//! |---|---|---|
//! | [`KrakenConnector::trade_subscription`] | `trade` | `DataMessage::Trade` (per element in `data`) |
//! | [`KrakenConnector::ticker_subscription`] | `ticker` | `DataMessage::Ticker` |
//! | [`KrakenConnector::ohlc_subscription`] | `ohlc` | `DataMessage::Candle` |
//! | [`KrakenConnector::book_subscription`] | `book` | `DataMessage::OrderBook` (snapshot then deltas) |
//!
//! # Endpoints
//!
//! - Public: `wss://ws.kraken.com/v2`
//! - Private (executions / balances): `wss://ws-auth.kraken.com/v2` — requires
//!   a WS token from `POST /0/private/GetWebSocketsToken`. Use
//!   [`KrakenConnector::private`] to point at the auth endpoint and pass
//!   the token inside your own `params.token` field on the subscribe frame.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use tokio::sync::{mpsc, watch};
//! use exchange_apiws::actors::{DataMessage, ExchangeConnector};
//! use exchange_apiws::kraken::ws::KrakenConnector;
//! use exchange_apiws::ws::{WsRunnerConfig, run_feed};
//!
//! # async fn example() -> exchange_apiws::Result<()> {
//! let subs = vec![
//!     KrakenConnector::trade_subscription(&["BTC/USD"]),
//!     KrakenConnector::ticker_subscription(&["BTC/USD"]),
//! ];
//! let connector = Arc::new(KrakenConnector::public());
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
    BalanceUpdate, CandleData, DataMessage, ExchangeConnector, OrderBookData, OrderUpdate,
    TickerData, TradeData, TradeSide, WebSocketConfig,
};
use crate::error::Result;

const WS_PUBLIC_URL: &str = "wss://ws.kraken.com/v2";
const WS_PRIVATE_URL: &str = "wss://ws-auth.kraken.com/v2";
const EXCHANGE_NAME: &str = "kraken";
/// Kraken v2 expects a ping every ~30 s on idle connections.
const PING_INTERVAL_SECS: u64 = 30;

// ── Connector ────────────────────────────────────────────────────────────────

/// Kraken v2 WebSocket connector.
///
/// Cheap to clone. Subscription frames are passed to
/// [`run_feed`](crate::ws::run_feed) as the `subscriptions` parameter
/// rather than baked in here — build them with the static `*_subscription`
/// helpers and pass whichever set you want for this session.
#[derive(Debug, Clone)]
pub struct KrakenConnector {
    /// Full WSS URL — public or private endpoint.
    pub url: String,
}

impl KrakenConnector {
    /// Public market-data endpoint (`wss://ws.kraken.com/v2`).
    #[must_use]
    pub fn public() -> Self {
        Self {
            url: WS_PUBLIC_URL.to_string(),
        }
    }

    /// Authenticated endpoint (`wss://ws-auth.kraken.com/v2`). Subscribe
    /// frames for private channels must include a `token` (obtained via
    /// `POST /0/private/GetWebSocketsToken`) — the helpers in this module
    /// build public-channel frames; private-channel callers should compose
    /// their own JSON and pass it as a subscription string.
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

    // ── Subscription builders ───────────────────────────────────────────────

    /// `{"method":"subscribe","params":{"channel":"trade","symbol":[…]}}`.
    #[must_use]
    pub fn trade_subscription(pairs: &[&str]) -> String {
        json!({
            "method": "subscribe",
            "params": {"channel": "trade", "symbol": pairs},
        })
        .to_string()
    }

    /// `{"method":"subscribe","params":{"channel":"ticker","symbol":[…]}}`.
    #[must_use]
    pub fn ticker_subscription(pairs: &[&str]) -> String {
        json!({
            "method": "subscribe",
            "params": {"channel": "ticker", "symbol": pairs},
        })
        .to_string()
    }

    /// `{"method":"subscribe","params":{"channel":"ohlc","symbol":[…],"interval":N}}`.
    ///
    /// `interval` is in minutes — `1`, `5`, `15`, `30`, `60`, `240`,
    /// `1440`, `10080`, `21600`.
    #[must_use]
    pub fn ohlc_subscription(pairs: &[&str], interval_mins: u32) -> String {
        json!({
            "method": "subscribe",
            "params": {"channel": "ohlc", "symbol": pairs, "interval": interval_mins},
        })
        .to_string()
    }

    /// `{"method":"subscribe","params":{"channel":"book","symbol":[…],"depth":N}}`.
    ///
    /// `depth` accepts `10`, `25`, `100`, `500`, or `1000`. First frame
    /// after subscribe is `type: "snapshot"`; subsequent frames are
    /// `type: "update"` (deltas).
    #[must_use]
    pub fn book_subscription(pairs: &[&str], depth: u32) -> String {
        json!({
            "method": "subscribe",
            "params": {"channel": "book", "symbol": pairs, "depth": depth},
        })
        .to_string()
    }

    /// Private `executions` channel — order state + fills →
    /// [`DataMessage::OrderUpdate`]. `token` comes from
    /// [`KrakenPrivateClient::get_websockets_token`](crate::kraken::KrakenPrivateClient::get_websockets_token);
    /// subscribe on the [`private`](Self::private) endpoint.
    #[must_use]
    pub fn executions_subscription(token: &str) -> String {
        json!({
            "method": "subscribe",
            "params": {"channel": "executions", "token": token},
        })
        .to_string()
    }

    /// Private `balances` channel — wallet balances →
    /// [`DataMessage::BalanceUpdate`]. `token` as for
    /// [`executions_subscription`](Self::executions_subscription).
    #[must_use]
    pub fn balances_subscription(token: &str) -> String {
        json!({
            "method": "subscribe",
            "params": {"channel": "balances", "token": token},
        })
        .to_string()
    }
}

// ── ExchangeConnector ────────────────────────────────────────────────────────

impl ExchangeConnector for KrakenConnector {
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
            // Subscriptions are passed to run_feed directly; this struct's
            // single-message field is unused for Kraken.
            subscription_msg: None,
            ping_interval_secs: PING_INTERVAL_SECS,
            reconnect_delay_secs: 5,
            max_reconnect_attempts: 5,
        }
    }

    /// Returns `None` — Kraken needs one subscribe frame per channel, so
    /// the caller passes the list as the `subscriptions` vector to
    /// [`run_feed`](crate::ws::run_feed).
    fn subscription_message(&self, _symbol: &str) -> Option<String> {
        None
    }

    /// Kraken v2 expects `{"method":"ping"}`; server replies
    /// `{"method":"pong"}`.
    fn ping_message(&self) -> Option<String> {
        Some(r#"{"method":"ping"}"#.to_string())
    }

    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>> {
        let json: Value = serde_json::from_str(raw)?;

        // Method responses (subscribe ack, pong) — no payload to surface.
        if json.get("method").is_some() {
            return Ok(vec![]);
        }

        let channel = json.get("channel").and_then(Value::as_str).unwrap_or("");
        let Some(data) = json.get("data").and_then(Value::as_array) else {
            // Heartbeat: `{"channel":"heartbeat"}` (no data array)
            return Ok(vec![]);
        };
        let is_snapshot =
            json.get("type").and_then(Value::as_str).unwrap_or("update") == "snapshot";

        match channel {
            "trade" => Ok(parse_trades(data)),
            "ticker" => Ok(parse_tickers(data)),
            "ohlc" => Ok(parse_klines(data)),
            "book" => Ok(parse_books(data, is_snapshot)),
            "executions" => Ok(parse_executions(data)),
            "balances" => Ok(parse_balances(data)),
            // status, heartbeat (rare with data), …
            _ => Ok(vec![]),
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Parse an RFC-3339 timestamp string into milliseconds-since-epoch.
/// Falls back to `now_ms()` on malformed input (defensive — Kraken hasn't
/// been seen sending invalid timestamps, but garbage-in-garbage-out would
/// poison downstream consumers).
fn iso_to_ms(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).map_or_else(|_| now_ms(), |dt| dt.timestamp_millis())
}

fn f64_field(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn nonempty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

// ── Parsers ──────────────────────────────────────────────────────────────────

/// `trade` channel `data` is an array of trade objects. One
/// `DataMessage::Trade` per element.
fn parse_trades(data: &[Value]) -> Vec<DataMessage> {
    data.iter()
        .map(|t| {
            // Kraken v2 sends side as "buy" or "sell" (lowercase).
            let side = match t.get("side").and_then(Value::as_str).unwrap_or("buy") {
                s if s.eq_ignore_ascii_case("sell") => TradeSide::Sell,
                _ => TradeSide::Buy,
            };
            let ts = t
                .get("timestamp")
                .and_then(Value::as_str)
                .map_or_else(now_ms, iso_to_ms);
            // trade_id arrives as a JSON integer.
            let trade_id = t
                .get("trade_id")
                .and_then(Value::as_u64)
                .map(|n| n.to_string())
                .unwrap_or_default();
            DataMessage::Trade(TradeData {
                symbol: t["symbol"].as_str().unwrap_or("").to_string(),
                exchange: EXCHANGE_NAME.to_string(),
                side,
                price: f64_field(t, "price"),
                amount: f64_field(t, "qty"),
                exchange_ts: ts,
                receipt_ts: now_ms(),
                trade_id,
            })
        })
        .collect()
}

/// `ticker` channel `data` is an array of ticker objects (one per
/// subscribed symbol). Snapshot includes all fields; subsequent updates
/// may omit unchanged ones — readers should expect partial fields.
fn parse_tickers(data: &[Value]) -> Vec<DataMessage> {
    let now = now_ms();
    data.iter()
        .map(|t| {
            DataMessage::Ticker(TickerData {
                symbol: t["symbol"].as_str().unwrap_or("").to_string(),
                exchange: EXCHANGE_NAME.to_string(),
                price: f64_field(t, "last"),
                best_bid: f64_field(t, "bid"),
                best_ask: f64_field(t, "ask"),
                exchange_ts: now, // Kraken's ticker frame carries no timestamp.
                receipt_ts: now,
            })
        })
        .collect()
}

/// `ohlc` channel `data` is an array of candle objects. `interval` is in
/// minutes — converted to a string label so the unified `CandleData` matches
/// downstream conventions (Binance / Bybit / KuCoin all use string labels).
fn parse_klines(data: &[Value]) -> Vec<DataMessage> {
    data.iter()
        .map(|c| {
            let interval = c
                .get("interval")
                .and_then(Value::as_u64)
                .map(|n| n.to_string())
                .unwrap_or_default();
            // Kraken doesn't expose an "is closed" flag on its OHLC feed —
            // every update is a snapshot of the still-forming bar; the
            // server emits the FINAL state of each bar one last time when
            // the interval closes. Marking everything as "not closed" is
            // honest about that ambiguity.
            DataMessage::Candle(CandleData {
                symbol: c["symbol"].as_str().unwrap_or("").to_string(),
                exchange: EXCHANGE_NAME.to_string(),
                interval,
                open_ts: c
                    .get("interval_begin")
                    .and_then(Value::as_str)
                    .map_or_else(now_ms, iso_to_ms),
                open: f64_field(c, "open"),
                high: f64_field(c, "high"),
                low: f64_field(c, "low"),
                close: f64_field(c, "close"),
                volume: f64_field(c, "volume"),
                is_closed: false,
                receipt_ts: now_ms(),
            })
        })
        .collect()
}

/// `book` channel `data` is an array of book objects with `bids` / `asks`
/// arrays of `{price, qty}` records. `is_snapshot` distinguishes the
/// initial snapshot from subsequent deltas.
fn parse_books(data: &[Value], is_snapshot: bool) -> Vec<DataMessage> {
    let now = now_ms();
    data.iter()
        .map(|b| {
            let bids = parse_level_objects(b.get("bids"));
            let asks = parse_level_objects(b.get("asks"));
            DataMessage::OrderBook(OrderBookData {
                symbol: b["symbol"].as_str().unwrap_or("").to_string(),
                exchange: EXCHANGE_NAME.to_string(),
                asks,
                bids,
                exchange_ts: now, // No exchange timestamp on this frame.
                receipt_ts: now,
                is_snapshot,
            })
        })
        .collect()
}

/// Convert Kraken's `[{price, qty}, ...]` array to `[price, qty]` pairs.
fn parse_level_objects(v: Option<&Value>) -> Vec<[f64; 2]> {
    v.and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|lvl| {
                    let p = lvl.get("price")?.as_f64()?;
                    let q = lvl.get("qty")?.as_f64()?;
                    Some([p, q])
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── Unit tests ───────────────────────────────────────────────────────────────

/// `executions` channel `data` is an array of execution reports — order state
/// changes and fills. One [`DataMessage::OrderUpdate`] per element.
fn parse_executions(data: &[Value]) -> Vec<DataMessage> {
    data.iter()
        .filter_map(parse_execution)
        .map(DataMessage::OrderUpdate)
        .collect()
}

fn parse_execution(d: &Value) -> Option<OrderUpdate> {
    let symbol = d.get("symbol")?.as_str()?.to_string();
    let size = f64_field(d, "order_qty");
    let filled_size = f64_field(d, "cum_qty");
    // A fill carries `exec_type:"trade"` with `last_*` / `exec_id`.
    let is_trade = d.get("exec_type").and_then(Value::as_str) == Some("trade");
    Some(OrderUpdate {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        order_id: str_field(d, "order_id"),
        // Kraken's client ref is `cl_ord_id` (string) or the legacy numeric
        // `order_userref`.
        client_oid: nonempty(str_field(d, "cl_ord_id")).or_else(|| {
            d.get("order_userref")
                .and_then(Value::as_i64)
                .filter(|&r| r != 0)
                .map(|r| r.to_string())
        }),
        side: side_of(d),
        order_type: d
            .get("order_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase(),
        status: map_order_status(d.get("order_status").and_then(Value::as_str).unwrap_or("")),
        price: f64_field(d, "limit_price"),
        size,
        filled_size,
        remaining_size: (size - filled_size).max(0.0),
        fee: sum_fees(d),
        match_price: is_trade.then(|| f64_field(d, "last_price")),
        match_size: is_trade.then(|| f64_field(d, "last_qty")),
        trade_id: if is_trade {
            nonempty(str_field(d, "exec_id"))
        } else {
            None
        },
        exchange_ts: d
            .get("timestamp")
            .and_then(Value::as_str)
            .map_or_else(now_ms, iso_to_ms),
        receipt_ts: now_ms(),
    })
}

/// `balances` channel `data` is an array of per-asset balances. One
/// [`DataMessage::BalanceUpdate`] per element.
///
/// Kraken's v2 `balances` channel reports only the total `balance` per asset —
/// there's no available/hold split on this channel — so `available_balance`
/// carries the total and `hold_balance` is `0.0`.
fn parse_balances(data: &[Value]) -> Vec<DataMessage> {
    data.iter()
        .filter_map(|b| {
            let currency = nonempty(str_field(b, "asset"))?;
            Some(DataMessage::BalanceUpdate(BalanceUpdate {
                exchange: EXCHANGE_NAME.to_string(),
                currency,
                available_balance: f64_field(b, "balance"),
                hold_balance: 0.0,
                event: "balances".to_string(),
                exchange_ts: now_ms(),
                receipt_ts: now_ms(),
            }))
        })
        .collect()
}

fn side_of(d: &Value) -> TradeSide {
    match d.get("side").and_then(Value::as_str).unwrap_or("buy") {
        s if s.eq_ignore_ascii_case("sell") => TradeSide::Sell,
        _ => TradeSide::Buy,
    }
}

/// Sum the `qty` across Kraken's `fees` array (`[{"asset":"USD","qty":0.1}]`).
fn sum_fees(d: &Value) -> f64 {
    d.get("fees")
        .and_then(Value::as_array)
        .map_or(0.0, |arr| arr.iter().map(|f| f64_field(f, "qty")).sum())
}

/// Map Kraken's v2 `order_status` to the crate's vocabulary.
fn map_order_status(status: &str) -> String {
    match status {
        "partially_filled" => "partialFilled",
        "filled" => "filled",
        "canceled" | "expired" => "canceled",
        // new / pending_new / anything else → resting.
        _ => "open",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector() -> KrakenConnector {
        KrakenConnector::public()
    }

    #[test]
    fn ws_url_picks_public_or_private() {
        assert!(KrakenConnector::public().url.ends_with("ws.kraken.com/v2"));
        assert!(
            KrakenConnector::private()
                .url
                .ends_with("ws-auth.kraken.com/v2")
        );
    }

    #[test]
    fn ping_uses_kraken_method_format() {
        assert_eq!(
            connector().ping_message().as_deref(),
            Some(r#"{"method":"ping"}"#)
        );
    }

    #[test]
    fn subscription_builders_emit_canonical_shape() {
        let sub = KrakenConnector::trade_subscription(&["BTC/USD", "ETH/USD"]);
        let v: Value = serde_json::from_str(&sub).unwrap();
        assert_eq!(v["method"], "subscribe");
        assert_eq!(v["params"]["channel"], "trade");
        assert_eq!(v["params"]["symbol"][0], "BTC/USD");
        assert_eq!(v["params"]["symbol"][1], "ETH/USD");

        let ohlc = KrakenConnector::ohlc_subscription(&["BTC/USD"], 5);
        let v: Value = serde_json::from_str(&ohlc).unwrap();
        assert_eq!(v["params"]["channel"], "ohlc");
        assert_eq!(v["params"]["interval"], 5);

        let book = KrakenConnector::book_subscription(&["BTC/USD"], 100);
        let v: Value = serde_json::from_str(&book).unwrap();
        assert_eq!(v["params"]["channel"], "book");
        assert_eq!(v["params"]["depth"], 100);
    }

    #[test]
    fn parse_method_response_returns_empty() {
        let raw = r#"{"method":"subscribe","success":true,"result":{"channel":"trade","symbol":"BTC/USD"}}"#;
        assert!(connector().parse_message(raw).expect("parse").is_empty());
    }

    #[test]
    fn parse_pong_returns_empty() {
        let raw = r#"{"method":"pong"}"#;
        assert!(connector().parse_message(raw).expect("parse").is_empty());
    }

    #[test]
    fn parse_heartbeat_returns_empty() {
        let raw = r#"{"channel":"heartbeat"}"#;
        assert!(connector().parse_message(raw).expect("parse").is_empty());
    }

    #[test]
    fn parse_trade_emits_one_per_array_element() {
        let raw = r#"{
            "channel":"trade","type":"snapshot",
            "data":[
                {"symbol":"BTC/USD","side":"buy","qty":0.1,"price":96000.0,"ord_type":"market","trade_id":1,"timestamp":"2026-05-25T12:00:00.000000Z"},
                {"symbol":"BTC/USD","side":"sell","qty":0.05,"price":96005.0,"ord_type":"limit","trade_id":2,"timestamp":"2026-05-25T12:00:00.500000Z"}
            ]
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        assert_eq!(msgs.len(), 2);
        match &msgs[0] {
            DataMessage::Trade(t) => {
                assert_eq!(t.symbol, "BTC/USD");
                assert_eq!(t.exchange, "kraken");
                assert_eq!(t.side, TradeSide::Buy);
                assert!((t.price - 96_000.0).abs() < 1e-9);
                assert!((t.amount - 0.1).abs() < 1e-12);
                assert_eq!(t.trade_id, "1");
                assert!(t.exchange_ts > 1_700_000_000_000); // 2026 timestamp
            }
            other => panic!("expected Trade, got {other:?}"),
        }
        match &msgs[1] {
            DataMessage::Trade(t) => assert_eq!(t.side, TradeSide::Sell),
            _ => panic!("expected Trade"),
        }
    }

    #[test]
    fn parse_ticker_into_ticker_data() {
        let raw = r#"{
            "channel":"ticker","type":"snapshot",
            "data":[{
                "symbol":"BTC/USD","bid":95999.0,"ask":96001.0,
                "bid_qty":1.0,"ask_qty":1.5,"last":96000.0,
                "volume":100.5,"high":96500.0,"low":95500.0,
                "vwap":95800.0,"change":250.0,"change_pct":0.26
            }]
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        match &msgs[0] {
            DataMessage::Ticker(t) => {
                assert_eq!(t.symbol, "BTC/USD");
                assert!((t.price - 96_000.0).abs() < 1e-9);
                assert!((t.best_bid - 95_999.0).abs() < 1e-9);
                assert!((t.best_ask - 96_001.0).abs() < 1e-9);
            }
            other => panic!("expected Ticker, got {other:?}"),
        }
    }

    #[test]
    fn parse_ohlc_into_candle() {
        let raw = r#"{
            "channel":"ohlc","type":"snapshot",
            "data":[{
                "symbol":"BTC/USD","interval":1,
                "open":96000.0,"high":96100.0,"low":95900.0,"close":96050.0,
                "trades":100,"volume":10.5,"vwap":96025.0,
                "interval_begin":"2026-05-25T12:00:00.000000Z"
            }]
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        match &msgs[0] {
            DataMessage::Candle(c) => {
                assert_eq!(c.symbol, "BTC/USD");
                assert_eq!(c.interval, "1");
                assert!((c.open - 96_000.0).abs() < 1e-9);
                assert!((c.close - 96_050.0).abs() < 1e-9);
                // Kraken v2 OHLC frames don't carry a "closed" flag.
                assert!(!c.is_closed);
                assert!(c.open_ts > 1_700_000_000_000);
            }
            other => panic!("expected Candle, got {other:?}"),
        }
    }

    #[test]
    fn parse_book_snapshot_and_delta() {
        let snap = r#"{
            "channel":"book","type":"snapshot",
            "data":[{
                "symbol":"BTC/USD",
                "bids":[{"price":96000.0,"qty":1.5},{"price":95999.0,"qty":2.0}],
                "asks":[{"price":96001.0,"qty":0.5}],
                "checksum":12345
            }]
        }"#;
        let delta = r#"{
            "channel":"book","type":"update",
            "data":[{
                "symbol":"BTC/USD",
                "bids":[{"price":95998.0,"qty":0.0}],
                "asks":[],
                "checksum":12346
            }]
        }"#;
        let c = connector();
        match &c.parse_message(snap).expect("snap")[0] {
            DataMessage::OrderBook(ob) => {
                assert!(ob.is_snapshot);
                assert_eq!(ob.bids.len(), 2);
                assert!((ob.asks[0][0] - 96_001.0).abs() < 1e-9);
            }
            _ => panic!("snapshot was not OrderBook"),
        }
        match &c.parse_message(delta).expect("delta")[0] {
            DataMessage::OrderBook(ob) => {
                assert!(!ob.is_snapshot);
                // qty 0 means "remove this level" — preserved as-is for the caller.
                assert!((ob.bids[0][1] - 0.0).abs() < 1e-12);
            }
            _ => panic!("delta was not OrderBook"),
        }
    }

    #[test]
    fn parse_unknown_channel_returns_empty() {
        // `status` carries data but isn't mapped to a DataMessage.
        let raw = r#"{"channel":"status","data":[{"system":"online"}]}"#;
        assert!(connector().parse_message(raw).expect("parse").is_empty());
    }

    #[test]
    fn private_subscription_builders_carry_channel_and_token() {
        let ex = KrakenConnector::executions_subscription("tok-123");
        let v: Value = serde_json::from_str(&ex).unwrap();
        assert_eq!(v["method"], "subscribe");
        assert_eq!(v["params"]["channel"], "executions");
        assert_eq!(v["params"]["token"], "tok-123");

        let bal = KrakenConnector::balances_subscription("tok-123");
        let v: Value = serde_json::from_str(&bal).unwrap();
        assert_eq!(v["params"]["channel"], "balances");
        assert_eq!(v["params"]["token"], "tok-123");
    }

    #[test]
    fn parse_executions_order_state_into_order_update() {
        // exec_type "new" → resting order, no fill detail.
        let raw = r#"{
            "channel":"executions","type":"snapshot","data":[{
                "order_id":"O1","cl_ord_id":"my-oid","symbol":"BTC/USD","side":"sell",
                "order_type":"limit","exec_type":"new","order_status":"partially_filled",
                "limit_price":30000.5,"order_qty":100,"cum_qty":40,
                "fees":[{"asset":"USD","qty":0.12}],"timestamp":"2023-09-25T07:48:36.925Z"
            }]
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        assert_eq!(msgs.len(), 1);
        let DataMessage::OrderUpdate(o) = &msgs[0] else {
            panic!("expected OrderUpdate, got {:?}", msgs[0]);
        };
        assert_eq!(o.symbol, "BTC/USD");
        assert_eq!(o.exchange, "kraken");
        assert_eq!(o.order_id, "O1");
        assert_eq!(o.client_oid.as_deref(), Some("my-oid"));
        assert_eq!(o.side, TradeSide::Sell);
        assert_eq!(o.order_type, "limit");
        assert_eq!(o.status, "partialFilled");
        assert!((o.price - 30000.5).abs() < 1e-9);
        assert!((o.size - 100.0).abs() < 1e-9);
        assert!((o.filled_size - 40.0).abs() < 1e-9);
        assert!((o.remaining_size - 60.0).abs() < 1e-9);
        assert!((o.fee - 0.12).abs() < 1e-9);
        assert_eq!(o.match_price, None, "non-trade exec has no fill price");
        assert_eq!(o.exchange_ts, 1_695_628_116_925);
    }

    #[test]
    fn parse_executions_trade_carries_match_details() {
        // exec_type "trade" → a fill with last_* + exec_id; numeric userref.
        let raw = r#"{
            "channel":"executions","type":"update","data":[{
                "order_id":"O9","order_userref":555,"symbol":"ETH/USD","side":"buy",
                "order_type":"market","exec_type":"trade","order_status":"filled",
                "order_qty":10,"cum_qty":10,"last_qty":10,"last_price":2500.25,
                "exec_id":"T-77","fees":[{"asset":"USD","qty":0.05}],
                "timestamp":"2023-09-25T07:48:40.000Z"
            }]
        }"#;
        let DataMessage::OrderUpdate(o) = &connector().parse_message(raw).expect("parse")[0] else {
            panic!("expected OrderUpdate");
        };
        assert_eq!(o.order_id, "O9");
        assert_eq!(
            o.client_oid.as_deref(),
            Some("555"),
            "numeric userref → string"
        );
        assert_eq!(o.status, "filled");
        assert!((o.match_price.unwrap() - 2500.25).abs() < 1e-9);
        assert_eq!(o.match_size, Some(10.0));
        assert_eq!(o.trade_id.as_deref(), Some("T-77"));
        assert!((o.fee - 0.05).abs() < 1e-9);
    }

    #[test]
    fn parse_balances_one_update_per_asset() {
        let raw = r#"{
            "channel":"balances","type":"snapshot","data":[
                {"asset":"USD","balance":1000.5},
                {"asset":"XBT","balance":0.5}
            ]
        }"#;
        let msgs = connector().parse_message(raw).expect("parse");
        assert_eq!(msgs.len(), 2);
        let DataMessage::BalanceUpdate(usd) = &msgs[0] else {
            panic!("expected BalanceUpdate, got {:?}", msgs[0]);
        };
        assert_eq!(usd.exchange, "kraken");
        assert_eq!(usd.currency, "USD");
        assert!((usd.available_balance - 1000.5).abs() < 1e-9);
        assert!((usd.hold_balance - 0.0).abs() < 1e-12);
        assert_eq!(usd.event, "balances");
        let DataMessage::BalanceUpdate(xbt) = &msgs[1] else {
            panic!("expected BalanceUpdate");
        };
        assert_eq!(xbt.currency, "XBT");
        assert!((xbt.available_balance - 0.5).abs() < 1e-9);
    }
}
