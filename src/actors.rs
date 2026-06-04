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
    /// Aggressive buy (taker lifted the ask).
    Buy,
    /// Aggressive sell (taker hit the bid).
    Sell,
}

/// A single matched trade from the exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeData {
    /// Instrument symbol (e.g. `"XBTUSDTM"`).
    pub symbol: String,
    /// Exchange identifier (e.g. `"kucoin"`).
    pub exchange: String,
    /// Whether the aggressor was a buyer or seller.
    pub side: TradeSide,
    /// Matched price.
    pub price: f64,
    /// Matched quantity (contracts or base units).
    pub amount: f64,
    /// Timestamp assigned by the exchange (milliseconds).
    pub exchange_ts: i64,
    /// Timestamp when this process received the message (milliseconds).
    pub receipt_ts: i64,
    /// Exchange-assigned trade identifier.
    pub trade_id: String,
}

/// Best bid/ask and last-trade price from the exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickerData {
    /// Instrument symbol.
    pub symbol: String,
    /// Exchange identifier.
    pub exchange: String,
    /// Last traded price.
    pub price: f64,
    /// Current best bid price.
    pub best_bid: f64,
    /// Current best ask price.
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
    /// Instrument symbol.
    pub symbol: String,
    /// Exchange identifier.
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

/// A candlestick / OHLCV bar from a public market data feed.
///
/// Most exchanges push *in-progress* candles every few seconds with the
/// final value flagged via `is_closed`. Skip non-closed candles if you only
/// need finalised bars; consume both if you want intra-bar updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandleData {
    /// Instrument symbol (exchange-specific format, e.g. `"BTCUSDT"` or `"XBT/USD"`).
    pub symbol: String,
    /// Exchange identifier (e.g. `"binance"`).
    pub exchange: String,
    /// Exchange-specific interval label (`"1m"`, `"5m"`, `"1h"`, `"1d"`, …).
    ///
    /// Not normalised across exchanges; each connector emits whatever its
    /// API exposes.
    pub interval: String,
    /// Candle open time as milliseconds since the Unix epoch.
    pub open_ts: i64,
    /// First trade price in the interval.
    pub open: f64,
    /// Highest trade price in the interval.
    pub high: f64,
    /// Lowest trade price in the interval.
    pub low: f64,
    /// Last trade price in the interval. Equals `open` if no trades occurred.
    pub close: f64,
    /// Base-asset volume traded during the interval.
    pub volume: f64,
    /// `true` once the interval has elapsed and the bar is finalised.
    /// `false` for live in-progress updates within the current interval.
    pub is_closed: bool,
    /// Local receipt timestamp in milliseconds.
    pub receipt_ts: i64,
}

/// A funding-rate event from a perpetual futures feed.
///
/// Funding payments accrue at fixed intervals (typically every 8 h on
/// Binance/Bybit, every 4 h on Crypto.com); the rate published here is the
/// one that will be applied at `next_funding_time`. The optional
/// `mark_price` / `index_price` fields capture the bundle some feeds emit
/// together (e.g. Binance's `<symbol>@markPrice` stream).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingData {
    /// Instrument symbol (exchange-specific format).
    pub symbol: String,
    /// Exchange identifier.
    pub exchange: String,
    /// Funding rate that will be applied at `next_funding_time`
    /// (e.g. `0.0001` = 0.01 %, positive = longs pay shorts).
    pub funding_rate: f64,
    /// Milliseconds since the Unix epoch when the next funding payment
    /// settles. Some feeds (Bybit ticker.extended) include the previous
    /// settlement timestamp instead — treat as the most recent
    /// settlement-related time.
    pub next_funding_time: i64,
    /// Mark price at the time of the event, if bundled by the feed.
    pub mark_price: Option<f64>,
    /// Underlying spot index price, if bundled by the feed.
    pub index_price: Option<f64>,
    /// Exchange timestamp in milliseconds.
    pub exchange_ts: i64,
    /// Local receipt timestamp in milliseconds.
    pub receipt_ts: i64,
}

/// Unified market data message emitted by any exchange connector.
///
/// Marked `#[non_exhaustive]` so new feed types (e.g. `FundingRate`) can be
/// added in minor releases without breaking downstream `match` arms.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum DataMessage {
    /// A matched trade execution.
    Trade(TradeData),
    /// A best-bid/ask ticker update.
    Ticker(TickerData),
    /// An order book snapshot or incremental delta.
    OrderBook(OrderBookData),
    /// A candlestick (OHLCV) bar — closed or in-progress.
    ///
    /// Emitted by Binance kline, Bybit kline, Kraken OHLC, and Crypto.com
    /// candlestick streams. KuCoin doesn't currently push candle frames
    /// over WS (its WS contractMarket feed is trade-driven; klines are
    /// REST-only via [`crate::KuCoinClient::fetch_klines`]).
    Candle(CandleData),
    /// A funding-rate event from a perpetual futures feed.
    ///
    /// Emitted by Binance markPrice (`<symbol>@markPrice@1s`), Bybit
    /// extended ticker, and other feeds that surface funding alongside
    /// mark/index price. KuCoin's equivalent is already covered by
    /// [`DataMessage::InstrumentEvent`] (subject `"funding.rate"`).
    FundingRate(FundingData),
    // Private-feed events — requires a private WS token.
    /// A fill or status change on one of your orders.
    OrderUpdate(OrderUpdate),
    /// A change to an open position.
    PositionChange(PositionChange),
    /// A wallet or margin balance change.
    BalanceUpdate(BalanceUpdate),
    /// An index price / mark price / premium index event from the instrument feed.
    ///
    /// Emitted on `/contract/instrument:{symbol}` (public).
    InstrumentEvent(InstrumentEvent),
    /// A stop/trigger order status event from the private advanced-orders feed.
    ///
    /// Emitted on `/contractMarket/advancedOrders` (private).
    AdvancedOrderUpdate(AdvancedOrderUpdate),
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

    /// Application-level ping JSON to send at every `ping_interval_secs` tick.
    ///
    /// Different exchanges expect different formats:
    /// - KuCoin: `{"type":"ping"}`
    /// - Bybit: `{"op":"ping"}`
    /// - Binance: server-driven — return `None` so the runner uses only
    ///   protocol-level WS Ping/Pong frames.
    ///
    /// Default implementation returns `None`, matching the
    /// no-application-ping case. Connectors whose servers expect an
    /// application ping override and return their format.
    fn ping_message(&self) -> Option<String> {
        None
    }

    /// Optional JSON auth frame sent once, right after connect and **before**
    /// the subscription. For private streams that authenticate with a
    /// post-connect frame (e.g. Bybit v5's `op:"auth"`). Public connectors, and
    /// those that authenticate via a URL token (e.g. KuCoin), return `None`.
    ///
    /// Default returns `None` — no auth frame.
    fn auth_message(&self) -> Option<String> {
        None
    }

    /// Optional inbound-driven response — given the raw text of an
    /// incoming frame, return a JSON frame the runner should send back
    /// before delivering parsed `DataMessage`s upstream.
    ///
    /// Useful for protocols where a heartbeat is server-initiated and
    /// must be acknowledged with content derived from the inbound frame
    /// (e.g. Crypto.com's `public/heartbeat` requires
    /// `public/respond-heartbeat` echoing the same `id`).
    ///
    /// Default returns `None` — no response. Implementations should
    /// keep the work fast: this is on the recv-loop critical path.
    fn response_for(&self, _raw: &str) -> Option<String> {
        None
    }
}

/// A fill or status-change event for an order on the private feed.
///
/// Emitted on `/contractMarket/tradeOrders` (Futures).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderUpdate {
    /// Instrument symbol.
    pub symbol: String,
    /// Exchange identifier.
    pub exchange: String,
    /// Exchange-assigned order identifier.
    pub order_id: String,
    /// Client-supplied order identifier, if provided at placement.
    pub client_oid: Option<String>,
    /// Order side (buy or sell).
    pub side: TradeSide,
    /// `"market"` or `"limit"`.
    pub order_type: String,
    /// `"open"`, `"filled"`, `"canceled"`, or `"partialFilled"`.
    pub status: String,
    /// Order limit price (0.0 for market orders).
    pub price: f64,
    /// Total order size in contracts.
    pub size: u32,
    /// Number of contracts filled so far.
    pub filled_size: u32,
    /// Number of contracts still open.
    pub remaining_size: u32,
    /// Cumulative fee charged for fills so far.
    pub fee: f64,
    /// Per-execution **match price** — the actual fill price for this execution.
    /// `Some` only on `type:"match"` events; `None` otherwise. Carries the true
    /// fill price even for market orders, where [`price`](Self::price) is `0.0`.
    pub match_price: Option<f64>,
    /// Per-execution **match size**, in contracts. `Some` only on `match` events.
    pub match_size: Option<u32>,
    /// Exchange trade id for this execution. `Some` only on `match` events — a
    /// stable key for de-duplicating fills off the feed.
    pub trade_id: Option<String>,
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
    /// Instrument symbol.
    pub symbol: String,
    /// Exchange identifier.
    pub exchange: String,
    /// Positive = long, negative = short, 0 = flat.
    pub current_qty: i32,
    /// Volume-weighted average entry price.
    pub avg_entry_price: f64,
    /// Current unrealised profit/loss in quote currency.
    pub unrealised_pnl: f64,
    /// Cumulative realised profit/loss in quote currency.
    pub realised_pnl: f64,
    /// Why the position changed — e.g. `"positionChange"`, `"liquidation"`, `"funding"`.
    pub change_reason: String,
    /// Exchange timestamp in milliseconds.
    pub exchange_ts: i64,
    /// Local receipt timestamp in milliseconds.
    pub receipt_ts: i64,
}

/// A balance or margin update from the private feed.
///
/// Emitted on `/contractAccount/wallet` (Futures).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceUpdate {
    /// Exchange identifier.
    pub exchange: String,
    /// Settlement currency (e.g. `"USDT"` or `"XBT"`).
    pub currency: String,
    /// Balance available for new orders or withdrawal.
    pub available_balance: f64,
    /// Balance locked in open orders or positions.
    pub hold_balance: f64,
    /// Event tag from KuCoin — e.g. `"orderMargin.create"`, `"trade.settled"`.
    pub event: String,
    /// Exchange timestamp in milliseconds.
    pub exchange_ts: i64,
    /// Local receipt timestamp in milliseconds.
    pub receipt_ts: i64,
}

/// An instrument event from the public `/contract/instrument:{symbol}` feed.
///
/// KuCoin pushes three subjects on this topic:
/// - `"mark.index.price"` — mark price and underlying index price update.
/// - `"funding.rate"` — current + predicted funding rate update.
/// - `"premium.index"` — the premium index used to compute the funding rate.
///
/// All three are surfaced in a single struct with `Option` fields; populate
/// only the fields that arrive in the specific subject.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentEvent {
    /// Instrument symbol.
    pub symbol: String,
    /// Exchange identifier.
    pub exchange: String,
    /// Subject tag from KuCoin identifying which metric changed.
    /// One of `"mark.index.price"`, `"funding.rate"`, or `"premium.index"`.
    pub subject: String,
    /// Current mark price.
    pub mark_price: Option<f64>,
    /// Underlying spot index price.
    pub index_price: Option<f64>,
    /// Current funding rate (e.g. `0.0001` = 0.01 %).
    pub funding_rate: Option<f64>,
    /// Predicted next-period funding rate.
    pub predicted_funding_rate: Option<f64>,
    /// Premium index value.
    pub premium_index: Option<f64>,
    /// Exchange timestamp in milliseconds.
    pub exchange_ts: i64,
    /// Local receipt timestamp in milliseconds.
    pub receipt_ts: i64,
}

/// A stop/trigger order lifecycle event from the private
/// `/contractMarket/advancedOrders` feed.
///
/// KuCoin emits this whenever a stop order is placed, triggered, cancelled,
/// or fails to trigger. Use `status` to differentiate:
/// - `"open"` — stop order accepted and waiting for the trigger price.
/// - `"triggered"` — trigger fired; a new regular order was placed.
/// - `"cancel"` — stop order cancelled before triggering.
/// - `"fail"` — trigger fired but order placement failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedOrderUpdate {
    /// Instrument symbol.
    pub symbol: String,
    /// Exchange identifier.
    pub exchange: String,
    /// Exchange-assigned stop order identifier.
    pub order_id: String,
    /// Client-supplied order identifier, if provided at placement.
    pub client_oid: Option<String>,
    /// Lifecycle status: `"open"`, `"triggered"`, `"cancel"`, or `"fail"`.
    pub status: String,
    /// Order side (buy or sell).
    pub side: TradeSide,
    /// `"market"` or `"limit"` — the type of order placed on trigger.
    pub order_type: String,
    /// Stop direction — `"up"` or `"down"`.
    pub stop: Option<String>,
    /// Trigger price.
    pub stop_price: Option<f64>,
    /// Limit price (present for stop-limit orders only).
    pub price: Option<f64>,
    /// Order quantity in contracts.
    pub size: u32,
    /// Exchange timestamp in milliseconds.
    pub exchange_ts: i64,
    /// Local receipt timestamp in milliseconds.
    pub receipt_ts: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip a CandleData through JSON to catch any field-name drift.
    #[test]
    fn candle_data_serde_round_trip() {
        let c = CandleData {
            symbol: "BTCUSDT".into(),
            exchange: "binance".into(),
            interval: "1m".into(),
            open_ts: 1_700_000_000_000,
            open: 96_000.0,
            high: 96_500.0,
            low: 95_800.0,
            close: 96_300.0,
            volume: 12.5,
            is_closed: true,
            receipt_ts: 1_700_000_060_001,
        };
        let json = serde_json::to_string(&c).expect("serialise");
        let back: CandleData = serde_json::from_str(&json).expect("deserialise");
        // Spot-check key fields — full equality requires PartialEq we don't derive.
        assert_eq!(back.symbol, c.symbol);
        assert_eq!(back.interval, c.interval);
        assert_eq!(back.open_ts, c.open_ts);
        assert!((back.close - c.close).abs() < 1e-9);
        assert_eq!(back.is_closed, c.is_closed);
    }

    /// FundingData with optional mark/index fields populated.
    #[test]
    fn funding_data_serde_round_trip_with_optionals() {
        let f = FundingData {
            symbol: "BTCUSDT".into(),
            exchange: "binance".into(),
            funding_rate: 0.000_1,
            next_funding_time: 1_700_028_800_000,
            mark_price: Some(96_010.5),
            index_price: Some(96_005.0),
            exchange_ts: 1_700_000_000_000,
            receipt_ts: 1_700_000_000_010,
        };
        let json = serde_json::to_string(&f).expect("serialise");
        let back: FundingData = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.symbol, f.symbol);
        assert!((back.funding_rate - f.funding_rate).abs() < 1e-12);
        assert_eq!(back.next_funding_time, f.next_funding_time);
        assert_eq!(back.mark_price, f.mark_price);
        assert_eq!(back.index_price, f.index_price);
    }

    /// FundingData with optionals absent (e.g. Bybit's bare funding tick).
    #[test]
    fn funding_data_serde_round_trip_without_optionals() {
        let f = FundingData {
            symbol: "BTCUSDT".into(),
            exchange: "bybit".into(),
            funding_rate: -0.000_05,
            next_funding_time: 1_700_028_800_000,
            mark_price: None,
            index_price: None,
            exchange_ts: 1_700_000_000_000,
            receipt_ts: 1_700_000_000_010,
        };
        let json = serde_json::to_string(&f).expect("serialise");
        let back: FundingData = serde_json::from_str(&json).expect("deserialise");
        assert!(back.mark_price.is_none());
        assert!(back.index_price.is_none());
        assert!((back.funding_rate - f.funding_rate).abs() < 1e-12);
    }

    /// Constructing the new variants and routing through a match — compile-
    /// time smoke test for downstream consumers.
    #[test]
    fn data_message_new_variants_match() {
        let candle = DataMessage::Candle(CandleData {
            symbol: "BTCUSDT".into(),
            exchange: "binance".into(),
            interval: "1m".into(),
            open_ts: 0,
            open: 0.0,
            high: 0.0,
            low: 0.0,
            close: 0.0,
            volume: 0.0,
            is_closed: false,
            receipt_ts: 0,
        });
        let funding = DataMessage::FundingRate(FundingData {
            symbol: "BTCUSDT".into(),
            exchange: "binance".into(),
            funding_rate: 0.0,
            next_funding_time: 0,
            mark_price: None,
            index_price: None,
            exchange_ts: 0,
            receipt_ts: 0,
        });

        for msg in [candle, funding] {
            match msg {
                DataMessage::Candle(c) => assert_eq!(c.exchange, "binance"),
                DataMessage::FundingRate(f) => assert_eq!(f.exchange, "binance"),
                // #[non_exhaustive] forces a catch-all even though we covered both.
                _ => unreachable!("unexpected variant"),
            }
        }
    }
}
