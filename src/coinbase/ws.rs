//! Coinbase Advanced Trade public WebSocket connector.
//!
//! Implements [`ExchangeConnector`] for the Coinbase Advanced Trade market-data
//! feed (`wss://advanced-trade-ws.coinbase.com`). Subscriptions use Coinbase's
//! `{"type":"subscribe","product_ids":[…],"channel":…}` envelope; inbound
//! frames carry a top-level `channel` discriminator and an `events` array whose
//! entries hold `tickers` / `trades` sub-arrays.
//!
//! Channels supported: `ticker`, `market_trades`, `level2`.
//! Docs: <https://docs.cloud.coinbase.com/advanced-trade-api/docs/ws-channels>.

use serde_json::Value;

use crate::actors::{
    DataMessage, ExchangeConnector, OrderBookData, TickerData, TradeData, TradeSide,
    WebSocketConfig,
};
use crate::error::Result;

const WS_BASE: &str = "wss://advanced-trade-ws.coinbase.com";
const EXCHANGE_NAME: &str = "coinbase";
/// Coinbase sends periodic heartbeats; a 30 s app-level ping keeps NAT open.
const PING_INTERVAL_SECS: u64 = 30;

/// Coinbase Advanced Trade public channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoinbaseChannel {
    /// Real-time best-bid/ask + last price.
    Ticker,
    /// Trade executions (`market_trades`).
    MarketTrades,
    /// Order book (`level2`).
    Level2,
}

impl CoinbaseChannel {
    /// Wire name for the `channel` field.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ticker => "ticker",
            Self::MarketTrades => "market_trades",
            Self::Level2 => "level2",
        }
    }
}

/// Coinbase public WebSocket connector. One `subscribe` frame per channel is
/// sent on connect for the configured products.
#[derive(Debug, Clone)]
pub struct CoinbaseConnector {
    /// Full WSS URL.
    pub url: String,
    /// Product ids (e.g. `"BTC-USD"`) to subscribe.
    pub product_ids: Vec<String>,
    /// Channels to subscribe for those products.
    pub channels: Vec<CoinbaseChannel>,
}

impl CoinbaseConnector {
    /// Build a connector for `product_ids` subscribed to `channels`.
    #[must_use]
    pub fn new(product_ids: Vec<String>, channels: Vec<CoinbaseChannel>) -> Self {
        Self {
            url: WS_BASE.to_string(),
            product_ids,
            channels,
        }
    }

    /// Build with a caller-supplied URL (tests against a local server).
    #[must_use]
    pub fn with_url(
        url: impl Into<String>,
        product_ids: Vec<String>,
        channels: Vec<CoinbaseChannel>,
    ) -> Self {
        Self {
            url: url.into(),
            product_ids,
            channels,
        }
    }

    /// One subscribe frame for a single channel.
    fn subscribe_frame(&self, channel: CoinbaseChannel) -> String {
        serde_json::json!({
            "type": "subscribe",
            "product_ids": self.product_ids,
            "channel": channel.as_str(),
        })
        .to_string()
    }
}

impl ExchangeConnector for CoinbaseConnector {
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

    /// Coinbase subscribes one channel per frame. The runner sends a single
    /// `subscription_msg`, so we return the first channel's frame and rely on
    /// [`Self::subscription_messages`] for the multi-channel case. When a
    /// single channel is configured (the common case) this is exact.
    fn subscription_message(&self, _symbol: &str) -> Option<String> {
        self.channels.first().map(|&c| self.subscribe_frame(c))
    }

    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>> {
        let json: Value = serde_json::from_str(raw)?;

        // Subscribe acks arrive on the `subscriptions` channel; ignore.
        let channel = json.get("channel").and_then(Value::as_str).unwrap_or("");
        if channel == "subscriptions" || channel.is_empty() {
            return Ok(vec![]);
        }
        let Some(events) = json.get("events").and_then(Value::as_array) else {
            return Ok(vec![]);
        };

        let mut out = Vec::new();
        for ev in events {
            match channel {
                "ticker" => {
                    if let Some(arr) = ev.get("tickers").and_then(Value::as_array) {
                        out.extend(arr.iter().filter_map(parse_ticker));
                    }
                }
                "market_trades" => {
                    if let Some(arr) = ev.get("trades").and_then(Value::as_array) {
                        out.extend(arr.iter().filter_map(parse_trade));
                    }
                }
                "l2_data" | "level2" => {
                    if let Some(msg) = parse_l2(ev) {
                        out.push(msg);
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Coinbase numeric fields are JSON strings.
fn str_f64(v: &Value, key: &str) -> f64 {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

/// Parse an RFC3339 timestamp string into epoch millis (0 on failure).
fn rfc3339_ms(v: &Value, key: &str) -> i64 {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map_or(0, |dt| dt.timestamp_millis())
}

fn parse_ticker(t: &Value) -> Option<DataMessage> {
    let symbol = t.get("product_id").and_then(Value::as_str)?.to_string();
    Some(DataMessage::Ticker(TickerData {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        price: str_f64(t, "price"),
        best_bid: str_f64(t, "best_bid"),
        best_ask: str_f64(t, "best_ask"),
        exchange_ts: now_ms(), // ticker payloads carry no per-tick ts; use receipt
        receipt_ts: now_ms(),
    }))
}

fn parse_trade(t: &Value) -> Option<DataMessage> {
    let symbol = t.get("product_id").and_then(Value::as_str)?.to_string();
    let side = match t.get("side").and_then(Value::as_str).unwrap_or("BUY") {
        s if s.eq_ignore_ascii_case("SELL") => TradeSide::Sell,
        _ => TradeSide::Buy,
    };
    Some(DataMessage::Trade(TradeData {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        side,
        price: str_f64(t, "price"),
        amount: str_f64(t, "size"),
        exchange_ts: rfc3339_ms(t, "time"),
        receipt_ts: now_ms(),
        trade_id: t
            .get("trade_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }))
}

/// Parse a `level2` event into an `OrderBookData`. Coinbase l2 `updates` are
/// `{side, price_level, new_quantity}` rows; `type` is `"snapshot"` or `"update"`.
fn parse_l2(ev: &Value) -> Option<DataMessage> {
    let symbol = ev.get("product_id").and_then(Value::as_str)?.to_string();
    let is_snapshot = ev.get("type").and_then(Value::as_str).unwrap_or("update") == "snapshot";
    let updates = ev.get("updates").and_then(Value::as_array)?;
    let mut bids = Vec::new();
    let mut asks = Vec::new();
    for u in updates {
        let px: f64 = u.get("price_level").and_then(Value::as_str)?.parse().ok()?;
        let qty: f64 = u
            .get("new_quantity")
            .and_then(Value::as_str)?
            .parse()
            .ok()?;
        match u.get("side").and_then(Value::as_str).unwrap_or("") {
            s if s.eq_ignore_ascii_case("bid") || s.eq_ignore_ascii_case("buy") => {
                bids.push([px, qty]);
            }
            _ => asks.push([px, qty]),
        }
    }
    Some(DataMessage::OrderBook(OrderBookData {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        asks,
        bids,
        exchange_ts: now_ms(),
        receipt_ts: now_ms(),
        is_snapshot,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribe_frame_targets_products_and_channel() {
        let c = CoinbaseConnector::new(
            vec!["BTC-USD".to_string()],
            vec![CoinbaseChannel::MarketTrades],
        );
        let v: Value = serde_json::from_str(&c.subscription_message("").unwrap()).unwrap();
        assert_eq!(v["type"], "subscribe");
        assert_eq!(v["channel"], "market_trades");
        assert_eq!(v["product_ids"][0], "BTC-USD");
    }

    #[test]
    fn parses_market_trade() {
        let raw = r#"{"channel":"market_trades","events":[{"type":"update",
            "trades":[{"trade_id":"12345","product_id":"BTC-USD","price":"50000.00",
            "size":"0.001","side":"SELL","time":"2023-01-01T00:00:00.000000Z"}]}]}"#;
        let c = CoinbaseConnector::new(vec![], vec![]);
        let msgs = c.parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            DataMessage::Trade(t) => {
                assert_eq!(t.symbol, "BTC-USD");
                assert_eq!(t.side, TradeSide::Sell);
                assert!((t.price - 50000.0).abs() < 1e-9);
                assert!((t.amount - 0.001).abs() < 1e-9);
                assert_eq!(t.exchange, "coinbase");
                assert!(t.exchange_ts > 0, "rfc3339 time parsed to epoch ms");
            }
            other => panic!("expected Trade, got {other:?}"),
        }
    }

    #[test]
    fn parses_ticker() {
        let raw = r#"{"channel":"ticker","events":[{"type":"update",
            "tickers":[{"type":"ticker","product_id":"BTC-USD","price":"50000.00",
            "best_bid":"49990.00","best_ask":"50010.00"}]}]}"#;
        let c = CoinbaseConnector::new(vec![], vec![]);
        match &c.parse_message(raw).unwrap()[0] {
            DataMessage::Ticker(t) => {
                assert!((t.price - 50000.0).abs() < 1e-9);
                assert!((t.best_bid - 49990.0).abs() < 1e-9);
                assert!((t.best_ask - 50010.0).abs() < 1e-9);
            }
            other => panic!("expected Ticker, got {other:?}"),
        }
    }

    #[test]
    fn subscriptions_ack_yields_nothing() {
        let c = CoinbaseConnector::new(vec![], vec![]);
        assert!(
            c.parse_message(r#"{"channel":"subscriptions","events":[]}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parses_l2_snapshot() {
        let raw = r#"{"channel":"l2_data","events":[{"type":"snapshot","product_id":"BTC-USD",
            "updates":[{"side":"bid","price_level":"49990.00","new_quantity":"1.5"},
                       {"side":"offer","price_level":"50010.00","new_quantity":"2.0"}]}]}"#;
        let c = CoinbaseConnector::new(vec![], vec![]);
        match &c.parse_message(raw).unwrap()[0] {
            DataMessage::OrderBook(ob) => {
                assert!(ob.is_snapshot);
                assert!(
                    (ob.bids[0][0] - 49990.0).abs() < 1e-9 && (ob.bids[0][1] - 1.5).abs() < 1e-9
                );
                assert!(
                    (ob.asks[0][0] - 50010.0).abs() < 1e-9 && (ob.asks[0][1] - 2.0).abs() < 1e-9
                );
            }
            other => panic!("expected OrderBook, got {other:?}"),
        }
    }
}
