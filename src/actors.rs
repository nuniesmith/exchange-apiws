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
    // Private-feed events — requires a private WS token.
    OrderUpdate(OrderUpdate),
    PositionChange(PositionChange),
    BalanceUpdate(BalanceUpdate),
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

/// A fill or status-change event for an order on the private feed.
///
/// Emitted on `/contractMarket/tradeOrders` (Futures).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderUpdate {
    pub symbol: String,
    pub exchange: String,
    pub order_id: String,
    pub client_oid: Option<String>,
    pub side: TradeSide,
    /// `"market"` or `"limit"`.
    pub order_type: String,
    /// `"open"`, `"filled"`, `"canceled"`, or `"partialFilled"`.
    pub status: String,
    pub price: f64,
    /// Total order size in contracts.
    pub size: u32,
    pub filled_size: u32,
    pub remaining_size: u32,
    pub fee: f64,
    /// Exchange timestamp in milliseconds.
    pub exchange_ts: i64,
    /// Local receipt timestamp in milliseconds.
    pub receipt_ts: i64,
}

/// A position-change event from the private feed.
///
/// Emitted on `/contract/position:{symbol}` (Futures).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionChange {
    pub symbol: String,
    pub exchange: String,
    /// Positive = long, negative = short, 0 = flat.
    pub current_qty: i32,
    pub avg_entry_price: f64,
    pub unrealised_pnl: f64,
    pub realised_pnl: f64,
    /// Why the position changed — e.g. `"positionChange"`, `"liquidation"`, `"funding"`.
    pub change_reason: String,
    pub exchange_ts: i64,
    pub receipt_ts: i64,
}

/// A balance or margin update from the private feed.
///
/// Emitted on `/contractAccount/wallet` (Futures).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceUpdate {
    pub exchange: String,
    pub currency: String,
    pub available_balance: f64,
    pub hold_balance: f64,
    /// Event tag from KuCoin — e.g. `"orderMargin.create"`, `"trade.settled"`.
    pub event: String,
    pub exchange_ts: i64,
    pub receipt_ts: i64,
}
