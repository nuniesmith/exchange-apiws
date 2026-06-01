//! OKX v5 public WebSocket connector.
//!
//! Implements [`ExchangeConnector`] for the OKX v5 public market-data feed
//! (`wss://ws.okx.com:8443/ws/v5/public`). Subscriptions use OKX's
//! `{"op":"subscribe","args":[{"channel":…,"instId":…}]}` envelope; inbound
//! frames carry an `arg.channel` discriminator and a `data` array.
//!
//! Channels supported: `trades`, `tickers`, `books` (order book).
//! Docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel>.

use serde_json::Value;

use crate::actors::{
    DataMessage, ExchangeConnector, OrderBookData, TickerData, TradeData, TradeSide,
    WebSocketConfig,
};
use crate::error::Result;

const WS_BASE: &str = "wss://ws.okx.com:8443/ws/v5/public";
const EXCHANGE_NAME: &str = "okx";
/// OKX disconnects after 30 s of silence; ping well inside that.
const PING_INTERVAL_SECS: u64 = 20;

/// A single OKX subscription target (`{"channel":…,"instId":…}`).
#[derive(Debug, Clone)]
pub struct OkxChannel {
    /// Channel name — `"trades"`, `"tickers"`, or `"books"`.
    pub channel: String,
    /// Instrument id — e.g. `"BTC-USDT"`.
    pub inst_id: String,
}

impl OkxChannel {
    /// Trades channel for `inst_id`.
    #[must_use]
    pub fn trades(inst_id: impl Into<String>) -> Self {
        Self {
            channel: "trades".to_string(),
            inst_id: inst_id.into(),
        }
    }

    /// Tickers channel for `inst_id`.
    #[must_use]
    pub fn tickers(inst_id: impl Into<String>) -> Self {
        Self {
            channel: "tickers".to_string(),
            inst_id: inst_id.into(),
        }
    }

    /// Order-book (`books`) channel for `inst_id`.
    #[must_use]
    pub fn books(inst_id: impl Into<String>) -> Self {
        Self {
            channel: "books".to_string(),
            inst_id: inst_id.into(),
        }
    }
}

/// OKX public WebSocket connector. Channels are bundled at construction into a
/// single subscribe frame sent after handshake.
#[derive(Debug, Clone)]
pub struct OkxConnector {
    /// Full WSS URL.
    pub url: String,
    /// Channels subscribed on connect.
    pub channels: Vec<OkxChannel>,
}

impl OkxConnector {
    /// Build a connector subscribed to `channels`.
    #[must_use]
    pub fn new(channels: Vec<OkxChannel>) -> Self {
        Self {
            url: WS_BASE.to_string(),
            channels,
        }
    }

    /// Build with a caller-supplied URL (for tests against a local server).
    #[must_use]
    pub fn with_url(url: impl Into<String>, channels: Vec<OkxChannel>) -> Self {
        Self {
            url: url.into(),
            channels,
        }
    }
}

impl ExchangeConnector for OkxConnector {
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

    /// All configured channels as one `subscribe` frame. `symbol` is unused —
    /// channels are fixed at construction time.
    fn subscription_message(&self, _symbol: &str) -> Option<String> {
        if self.channels.is_empty() {
            return None;
        }
        let args: Vec<Value> = self
            .channels
            .iter()
            .map(|c| serde_json::json!({ "channel": c.channel, "instId": c.inst_id }))
            .collect();
        serde_json::to_string(&serde_json::json!({ "op": "subscribe", "args": args })).ok()
    }

    /// OKX expects a literal `"ping"` text frame; the server replies `"pong"`.
    fn ping_message(&self) -> Option<String> {
        Some("ping".to_string())
    }

    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>> {
        // Heartbeat: server sends the literal string "pong".
        if raw == "pong" {
            return Ok(vec![]);
        }
        let json: Value = serde_json::from_str(raw)?;

        // Subscribe acks / errors carry an `event` field, no `data`.
        if json.get("event").is_some() {
            return Ok(vec![]);
        }

        let channel = json
            .get("arg")
            .and_then(|a| a.get("channel"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(data) = json.get("data").and_then(Value::as_array) else {
            return Ok(vec![]);
        };

        let out = match channel {
            "trades" => data.iter().filter_map(parse_trade).collect(),
            "tickers" => data.iter().filter_map(parse_ticker).collect(),
            "books" => {
                // `books` first push is a snapshot (action="snapshot"), then deltas.
                let is_snapshot = json
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("snapshot")
                    == "snapshot";
                data.iter().map(|d| parse_book(d, is_snapshot)).collect()
            }
            _ => vec![],
        };
        Ok(out)
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Parse a stringified f64 field (`v["k"]` is a JSON string per OKX wire format).
fn str_f64(v: &Value, key: &str) -> f64 {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

fn str_i64(v: &Value, key: &str) -> i64 {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn parse_trade(t: &Value) -> Option<DataMessage> {
    let symbol = t.get("instId").and_then(Value::as_str)?.to_string();
    let side = match t.get("side").and_then(Value::as_str).unwrap_or("buy") {
        s if s.eq_ignore_ascii_case("sell") => TradeSide::Sell,
        _ => TradeSide::Buy,
    };
    Some(DataMessage::Trade(TradeData {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        side,
        price: str_f64(t, "px"),
        amount: str_f64(t, "sz"),
        exchange_ts: str_i64(t, "ts"),
        receipt_ts: now_ms(),
        trade_id: t
            .get("tradeId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }))
}

fn parse_ticker(t: &Value) -> Option<DataMessage> {
    let symbol = t.get("instId").and_then(Value::as_str)?.to_string();
    Some(DataMessage::Ticker(TickerData {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        price: str_f64(t, "last"),
        best_bid: str_f64(t, "bidPx"),
        best_ask: str_f64(t, "askPx"),
        exchange_ts: str_i64(t, "ts"),
        receipt_ts: now_ms(),
    }))
}

/// Parse one `books` entry. OKX levels are `[price, size, _, _]` string arrays.
fn parse_book(d: &Value, is_snapshot: bool) -> DataMessage {
    let levels = |key: &str| -> Vec<[f64; 2]> {
        d.get(key)
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|lvl| {
                        let l = lvl.as_array()?;
                        let px = l.first()?.as_str()?.parse().ok()?;
                        let sz = l.get(1)?.as_str()?.parse().ok()?;
                        Some([px, sz])
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    DataMessage::OrderBook(OrderBookData {
        symbol: d
            .get("instId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        exchange: EXCHANGE_NAME.to_string(),
        asks: levels("asks"),
        bids: levels("bids"),
        exchange_ts: str_i64(d, "ts"),
        receipt_ts: now_ms(),
        is_snapshot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribe_frame_packs_channels() {
        let c = OkxConnector::new(vec![
            OkxChannel::trades("BTC-USDT"),
            OkxChannel::tickers("ETH-USDT"),
        ]);
        let msg = c.subscription_message("").unwrap();
        let v: Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["op"], "subscribe");
        assert_eq!(v["args"][0]["channel"], "trades");
        assert_eq!(v["args"][0]["instId"], "BTC-USDT");
        assert_eq!(v["args"][1]["channel"], "tickers");
    }

    #[test]
    fn parses_trade_frame() {
        let raw = r#"{"arg":{"channel":"trades","instId":"BTC-USDT"},
            "data":[{"instId":"BTC-USDT","tradeId":"130","px":"50000.0",
            "sz":"0.001","side":"sell","ts":"1609459200123"}]}"#;
        let c = OkxConnector::new(vec![]);
        let msgs = c.parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            DataMessage::Trade(t) => {
                assert_eq!(t.symbol, "BTC-USDT");
                assert_eq!(t.side, TradeSide::Sell);
                assert!((t.price - 50000.0).abs() < 1e-9);
                assert!((t.amount - 0.001).abs() < 1e-9);
                assert_eq!(t.exchange_ts, 1_609_459_200_123);
                assert_eq!(t.exchange, "okx");
            }
            other => panic!("expected Trade, got {other:?}"),
        }
    }

    #[test]
    fn parses_ticker_frame() {
        let raw = r#"{"arg":{"channel":"tickers","instId":"BTC-USDT"},
            "data":[{"instId":"BTC-USDT","last":"50000.0","bidPx":"49990.0",
            "askPx":"50010.0","ts":"1609459200123"}]}"#;
        let c = OkxConnector::new(vec![]);
        match &c.parse_message(raw).unwrap()[0] {
            DataMessage::Ticker(t) => {
                assert!((t.best_bid - 49990.0).abs() < 1e-9);
                assert!((t.best_ask - 50010.0).abs() < 1e-9);
                assert!((t.price - 50000.0).abs() < 1e-9);
            }
            other => panic!("expected Ticker, got {other:?}"),
        }
    }

    #[test]
    fn event_and_pong_frames_yield_nothing() {
        let c = OkxConnector::new(vec![]);
        assert!(c.parse_message("pong").unwrap().is_empty());
        assert!(
            c.parse_message(r#"{"event":"subscribe","arg":{"channel":"trades"}}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parses_books_snapshot() {
        let raw = r#"{"arg":{"channel":"books","instId":"BTC-USDT"},"action":"snapshot",
            "data":[{"instId":"BTC-USDT","asks":[["50010.0","1.0","0","1"]],
            "bids":[["49990.0","2.0","0","1"]],"ts":"1609459200123"}]}"#;
        let c = OkxConnector::new(vec![]);
        match &c.parse_message(raw).unwrap()[0] {
            DataMessage::OrderBook(ob) => {
                assert!(ob.is_snapshot);
                assert!(
                    (ob.asks[0][0] - 50010.0).abs() < 1e-9 && (ob.asks[0][1] - 1.0).abs() < 1e-9
                );
                assert!(
                    (ob.bids[0][0] - 49990.0).abs() < 1e-9 && (ob.bids[0][1] - 2.0).abs() < 1e-9
                );
            }
            other => panic!("expected OrderBook, got {other:?}"),
        }
    }
}
