//! Exchange-agnostic connector traits and normalized data types.
//!
//! Any exchange integration (KuCoin, Binance, …) implements [`ExchangeConnector`].
//! The runner in `ws::runner` drives the connection lifecycle; this module only
//! defines the contract and the shared data model.

use serde::{Deserialize, Serialize};

use crate::error::Result;

// ── WebSocket config ──────────────────────────────────────────────────────────

/// Unified parameters for maintaining one WebSocket connection.
///
/// Build via [`ExchangeConnector::build_ws_config`] or construct directly.
/// The runner uses these values for ping scheduling and reconnect backoff.
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// Full WSS URL including token query params.
    pub url: String,
    /// Human-readable exchange identifier (e.g. `"kucoin"`).
    pub exchange: String,
    /// Primary symbol for this connection (informational).
    pub symbol: String,
    /// Optional default subscription message sent on connect.
    pub subscription_msg: Option<String>,
    /// How often to send an application-level ping (seconds).
    pub ping_interval_secs: u64,
    /// Base reconnect delay in seconds (doubled on each attempt).
    pub reconnect_delay_secs: u64,
    /// Give up after this many consecutive failed reconnect attempts.
    pub max_reconnect_attempts: u32,
}

// ── Normalized data types ─────────────────────────────────────────────────────

/// Trade side as received from the exchange feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeSide {
    Buy,
    Sell,
}

/// A single matched trade from the exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeData {
    pub symbol: String,
    pub exchange: String,
    pub side: TradeSide,
    pub price: f64,
    pub amount: f64,
    /// Timestamp assigned by the exchange (milliseconds).
    pub exchange_ts: i64,
    /// Timestamp when this process received the message (milliseconds).
    pub receipt_ts: i64,
    pub trade_id: String,
}

/// Best bid/ask and last-trade price from the exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickerData {
    pub symbol: String,
    pub exchange: String,
    pub price: f64,
    pub best_bid: f64,
    pub best_ask: f64,
    /// Timestamp assigned by the exchange (milliseconds).
    pub exchange_ts: i64,
    /// Timestamp when this process received the message (milliseconds).
    pub receipt_ts: i64,
}

/// Order book snapshot or incremental delta.
///
/// When `is_snapshot` is `true` this carries a full level-N snapshot.
/// When `false` it is a delta: each entry is `[price, qty]` where `qty == 0.0`
/// signals that the level should be removed from the local book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBookData {
    pub symbol: String,
    pub exchange: String,
    /// Ask levels as `[price, qty]` pairs.
    pub asks: Vec<[f64; 2]>,
    /// Bid levels as `[price, qty]` pairs.
    pub bids: Vec<[f64; 2]>,
    /// Timestamp assigned by the exchange (milliseconds).
    pub exchange_ts: i64,
    /// Timestamp when this process received the message (milliseconds).
    pub receipt_ts: i64,
    /// `true` for a full snapshot, `false` for an incremental delta.
    pub is_snapshot: bool,
}

/// Unified market data message emitted by any exchange connector.
#[derive(Debug, Clone)]
pub enum DataMessage {
    Trade(TradeData),
    Ticker(TickerData),
    OrderBook(OrderBookData),
}

// ── Connector trait ───────────────────────────────────────────────────────────

/// Interface that every exchange WebSocket integration must implement.
///
/// Implement this trait to add a new exchange. The runner in `ws::runner`
/// will handle the connection lifecycle; only parsing and subscription
/// message construction are exchange-specific.
pub trait ExchangeConnector: Send + Sync {
    /// Short, lowercase exchange identifier — e.g. `"kucoin"`.
    fn exchange_name(&self) -> &str;

    /// Full WSS URL including any required token query parameters.
    fn ws_url(&self) -> &str;

    /// Build a [`WebSocketConfig`] for the given primary symbol.
    fn build_ws_config(&self, symbol: &str) -> WebSocketConfig;

    /// Serialised JSON subscription message for the given symbol, or `None`
    /// if subscriptions are not needed (e.g. the URL already encodes the topic).
    fn subscription_message(&self, symbol: &str) -> Option<String>;

    /// Parse a raw text frame into zero or more normalized [`DataMessage`]s.
    ///
    /// Return `Ok(vec![])` for control frames or topics the connector does
    /// not handle. Only return `Err` for unrecoverable parse failures.
    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>>;
}
