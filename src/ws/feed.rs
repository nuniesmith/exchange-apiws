//! KuCoin WebSocket connector — message parsing for all public market data topics.
//!
//! # Supported topics
//!
//! | Topic pattern                         | Env     | Subject        | Emits              |
//! |---------------------------------------|---------|----------------|--------------------|
//! | `/contractMarket/execution:{sym}`     | Futures | `match`        | `DataMessage::Trade`     |
//! | `/market/match:{sym}`                 | Spot    | `trade.l3match`| `DataMessage::Trade`     |
//! | `/contractMarket/tickerV2:{sym}`      | Futures | `tickerV2`     | `DataMessage::Ticker`    |
//! | `/market/ticker:{sym}`                | Spot    | `trade.ticker` | `DataMessage::Ticker`    |
//! | `/contractMarket/level2Depth5:{sym}`  | Futures | `level2Depth5` | `DataMessage::OrderBook` (snapshot) |
//! | `/contractMarket/level2Depth50:{sym}` | Futures | `level2Depth50`| `DataMessage::OrderBook` (snapshot) |
//! | `/contractMarket/level2:{sym}`        | Futures | `level2`       | `DataMessage::OrderBook` (delta)    |
//!
//! Build subscription messages with the dedicated helpers on [`KucoinConnector`]
//! rather than constructing topic strings by hand.

use serde_json::Value;
use uuid::Uuid;

use crate::actors::{
    DataMessage, ExchangeConnector, OrderBookData, TickerData, TradeData, TradeSide,
    WebSocketConfig,
};
use crate::client::KucoinEnv;
use crate::error::Result;
use crate::ws::types::{WsMessage, WsToken};

// ── Connector ────────────────────────────────────────────────────────────────

/// KuCoin WebSocket connector.
///
/// Construct from a negotiated token returned by [`crate::client::KuCoinClient::get_ws_token_private`]
/// or [`crate::client::KuCoinClient::get_ws_token_public`].
///
/// ```no_run
/// # use kucoin_apiws::{KuCoinClient, Credentials, KucoinEnv};
/// # use kucoin_apiws::ws::KucoinConnector;
/// # async fn example(client: KuCoinClient) -> kucoin_apiws::Result<()> {
/// let token = client.get_ws_token_public().await?;
/// let connector = KucoinConnector::new(&token, KucoinEnv::LiveFutures)?;
/// # Ok(())
/// # }
/// ```
pub struct KucoinConnector {
    /// Full WSS URL with `?token=…&connectId=…` appended.
    pub negotiated_url: String,
    /// Recommended ping interval from the instance server (seconds).
    pub ping_interval_secs: u64,
    /// Whether this connector targets Futures, Spot, or Unified.
    pub env: KucoinEnv,
}

impl KucoinConnector {
    /// Build a connector from a negotiated WS token.
    pub fn new(token_data: &WsToken, env: KucoinEnv) -> Result<Self> {
        let server = token_data.instance_servers.first().ok_or_else(|| {
            crate::error::BotError::Config("KuCoin returned no instance servers".into())
        })?;

        let negotiated_url = format!(
            "{}?token={}&connectId={}",
            server.endpoint,
            token_data.token,
            Uuid::new_v4()
        );

        Ok(Self {
            negotiated_url,
            // KuCoin returns the interval in milliseconds.
            ping_interval_secs: server.ping_interval / 1000,
            env,
        })
    }

    // ── Subscription builders ─────────────────────────────────────────────────

    /// Subscription for live trade executions on `symbol`.
    ///
    /// Futures: `/contractMarket/execution:{symbol}` (subject: `match`)
    /// Spot:    `/market/match:{symbol}` (subject: `trade.l3match`)
    pub fn trade_subscription(&self, symbol: &str) -> Option<String> {
        let topic = match self.env {
            KucoinEnv::LiveFutures => format!("/contractMarket/execution:{symbol}"),
            _ => format!("/market/match:{symbol}"),
        };
        self.build_sub(topic, false)
    }

    /// Subscription for best-bid/ask ticker updates on `symbol`.
    ///
    /// Futures: `/contractMarket/tickerV2:{symbol}`
    /// Spot:    `/market/ticker:{symbol}`
    pub fn ticker_subscription(&self, symbol: &str) -> Option<String> {
        let topic = match self.env {
            KucoinEnv::LiveFutures => format!("/contractMarket/tickerV2:{symbol}"),
            _ => format!("/market/ticker:{symbol}"),
        };
        self.build_sub(topic, false)
    }

    /// Subscription for a full order book depth snapshot on `symbol`.
    ///
    /// `depth` is clamped to either 5 or 50 levels. Each message is a complete
    /// snapshot (`OrderBookData::is_snapshot == true`). Use this when you only
    /// need the top of book and don't want to maintain a local order book.
    ///
    /// Futures: `/contractMarket/level2Depth{5|50}:{symbol}`
    pub fn orderbook_depth_subscription(&self, symbol: &str, depth: u8) -> Option<String> {
        let d = if depth <= 5 { 5u8 } else { 50u8 };
        let topic = match self.env {
            KucoinEnv::LiveFutures => format!("/contractMarket/level2Depth{d}:{symbol}"),
            _ => format!("/spotMarket/level2Depth{d}:{symbol}"),
        };
        self.build_sub(topic, false)
    }

    /// Subscription for full Level 2 incremental order book updates on `symbol`.
    ///
    /// Each message is a delta (`OrderBookData::is_snapshot == false`).
    /// `asks` and `bids` each contain exactly one `[price, qty]` entry; when
    /// `qty == 0.0` the level should be removed from the local book.
    ///
    /// To bootstrap your local book, fetch a REST snapshot first via
    /// [`crate::rest::market::KuCoinClient::get_orderbook_snapshot`], then apply
    /// deltas whose `sequence` is greater than the snapshot's `sequence`.
    ///
    /// Futures: `/contractMarket/level2:{symbol}`
    pub fn orderbook_l2_subscription(&self, symbol: &str) -> Option<String> {
        let topic = match self.env {
            KucoinEnv::LiveFutures => format!("/contractMarket/level2:{symbol}"),
            _ => format!("/market/level2:{symbol}"),
        };
        self.build_sub(topic, false)
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn build_sub(&self, topic: String, private_channel: bool) -> Option<String> {
        let msg = WsMessage {
            id: Uuid::new_v4().to_string(),
            msg_type: "subscribe".to_string(),
            topic: Some(topic),
            subject: None,
            data: None,
            private_channel: Some(private_channel),
            response: Some(true),
        };
        serde_json::to_string(&msg).ok()
    }
}

// ── ExchangeConnector impl ────────────────────────────────────────────────────

impl ExchangeConnector for KucoinConnector {
    fn exchange_name(&self) -> &str {
        "kucoin"
    }

    fn ws_url(&self) -> &str {
        &self.negotiated_url
    }

    /// Returns a [`WebSocketConfig`] using the trade topic as the default
    /// subscription. For other topics, call the dedicated subscription builders
    /// and pass the messages to [`crate::ws::runner::run_feed`] directly.
    fn build_ws_config(&self, symbol: &str) -> WebSocketConfig {
        WebSocketConfig {
            url: self.negotiated_url.clone(),
            exchange: self.exchange_name().to_string(),
            symbol: symbol.to_string(),
            subscription_msg: self.trade_subscription(symbol),
            ping_interval_secs: self.ping_interval_secs,
            reconnect_delay_secs: 5,
            max_reconnect_attempts: 10,
        }
    }

    /// Default subscription is the trade/execution topic.
    /// Use the dedicated builders for ticker or order book subscriptions.
    fn subscription_message(&self, symbol: &str) -> Option<String> {
        self.trade_subscription(symbol)
    }

    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>> {
        let msg: WsMessage = serde_json::from_str(raw)?;

        match msg.msg_type.as_str() {
            "message" => {
                let topic = msg.topic.as_deref().unwrap_or("");
                let data = match msg.data {
                    Some(d) => d,
                    None => return Ok(vec![]),
                };

                let symbol = extract_symbol(topic);
                let exchange = self.exchange_name();

                // Route by topic prefix — more reliable than subject alone
                // since the same subject ("match") can appear on different topics.
                if topic.contains("/contractMarket/execution") || topic.contains("/market/match") {
                    parse_trade(symbol, exchange, &data)
                } else if topic.contains("/contractMarket/tickerV2")
                    || topic.contains("/contractMarket/ticker")
                    || topic.contains("/market/ticker")
                {
                    parse_ticker(symbol, exchange, &data)
                } else if topic.contains("level2Depth") {
                    parse_orderbook_depth(symbol, exchange, &data)
                } else if topic.contains("level2") {
                    parse_level2_delta(symbol, exchange, &data)
                } else {
                    Ok(vec![])
                }
            }
            // Protocol / control frames — the runner handles these.
            "ping" | "pong" | "welcome" | "ack" | "error" => Ok(vec![]),
            _ => Ok(vec![]),
        }
    }
}

// ── Parsers ───────────────────────────────────────────────────────────────────

/// Extract the symbol from a KuCoin topic string (`/prefix:{symbol}`).
fn extract_symbol(topic: &str) -> &str {
    topic.split(':').last().unwrap_or("UNKNOWN")
}

/// Parse a field that KuCoin may send as either a JSON string or a number.
fn str_f64(data: &Value, key: &str) -> f64 {
    data.get(key)
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                s.parse().ok()
            } else {
                v.as_f64()
            }
        })
        .unwrap_or(0.0)
}

/// Try multiple field names in order, returning the first non-zero value.
fn first_f64(data: &Value, keys: &[&str]) -> f64 {
    for key in keys {
        let v = str_f64(data, key);
        if v != 0.0 {
            return v;
        }
    }
    0.0
}

fn parse_trade(symbol: &str, exchange: &str, data: &Value) -> Result<Vec<DataMessage>> {
    let side = match data["side"].as_str().unwrap_or("buy") {
        s if s.eq_ignore_ascii_case("sell") => TradeSide::Sell,
        _ => TradeSide::Buy,
    };

    // Futures `ts` is nanoseconds; spot `time` is a millisecond string.
    let exchange_ts = data["ts"]
        .as_i64()
        .map(|ns| ns / 1_000_000)
        .or_else(|| data["time"].as_str().and_then(|t| t.parse::<i64>().ok()))
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    let trade_id = data["tradeId"]
        .as_str()
        .or_else(|| data["makerOrderId"].as_str())
        .unwrap_or("")
        .to_string();

    Ok(vec![DataMessage::Trade(TradeData {
        symbol: symbol.to_string(),
        exchange: exchange.to_string(),
        side,
        price: str_f64(data, "price"),
        amount: str_f64(data, "size"),
        exchange_ts,
        receipt_ts: chrono::Utc::now().timestamp_millis(),
        trade_id,
    })])
}

fn parse_ticker(symbol: &str, exchange: &str, data: &Value) -> Result<Vec<DataMessage>> {
    // Futures tickerV2 uses bestBidPrice / bestAskPrice; spot uses bestBid / bestAsk.
    let best_bid = first_f64(data, &["bestBidPrice", "bestBid"]);
    let best_ask = first_f64(data, &["bestAskPrice", "bestAsk"]);

    // Futures `ts` is nanoseconds; spot `time` is milliseconds.
    let exchange_ts = data["ts"]
        .as_i64()
        .map(|ts| {
            // Nanoseconds are orders of magnitude larger than ms timestamps.
            if ts > 1_700_000_000_000_i64 * 1_000_000 {
                ts / 1_000_000
            } else {
                ts
            }
        })
        .or_else(|| data["time"].as_i64())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    Ok(vec![DataMessage::Ticker(TickerData {
        symbol: symbol.to_string(),
        exchange: exchange.to_string(),
        price: str_f64(data, "price"),
        best_bid,
        best_ask,
        exchange_ts,
        receipt_ts: chrono::Utc::now().timestamp_millis(),
    })])
}

fn parse_orderbook_depth(symbol: &str, exchange: &str, data: &Value) -> Result<Vec<DataMessage>> {
    let parse_levels = |arr: &Value| -> Vec<[f64; 2]> {
        arr.as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let price = row.get(0).and_then(|v| {
                            v.as_str()
                                .and_then(|s| s.parse().ok())
                                .or_else(|| v.as_f64())
                        })?;
                        let qty = row.get(1).and_then(|v| {
                            v.as_str()
                                .and_then(|s| s.parse().ok())
                                .or_else(|| v.as_f64())
                        })?;
                        Some([price, qty])
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let exchange_ts = data["ts"]
        .as_i64()
        .or_else(|| data["timestamp"].as_i64())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    Ok(vec![DataMessage::OrderBook(OrderBookData {
        symbol: symbol.to_string(),
        exchange: exchange.to_string(),
        asks: parse_levels(&data["asks"]),
        bids: parse_levels(&data["bids"]),
        exchange_ts,
        receipt_ts: chrono::Utc::now().timestamp_millis(),
        is_snapshot: true,
    })])
}

fn parse_level2_delta(symbol: &str, exchange: &str, data: &Value) -> Result<Vec<DataMessage>> {
    // KuCoin level2 incremental format: `change: "price,side,qty"` where qty=0 means remove.
    let change_str = data["change"].as_str().unwrap_or("");
    let mut parts = change_str.splitn(3, ',');

    let price: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let side = parts.next().unwrap_or("sell");
    let qty: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);

    if price == 0.0 {
        return Ok(vec![]);
    }

    let entry = [price, qty];
    let (asks, bids) = if side.eq_ignore_ascii_case("sell") {
        (vec![entry], vec![])
    } else {
        (vec![], vec![entry])
    };

    let exchange_ts = data["timestamp"]
        .as_i64()
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    Ok(vec![DataMessage::OrderBook(OrderBookData {
        symbol: symbol.to_string(),
        exchange: exchange.to_string(),
        asks,
        bids,
        exchange_ts,
        receipt_ts: chrono::Utc::now().timestamp_millis(),
        is_snapshot: false,
    })])
}
