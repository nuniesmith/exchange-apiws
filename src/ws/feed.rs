//! KuCoin WebSocket connector — subscription builders and message parser.
//!
//! # Supported topics
//!
//! ## Public
//!
//! | Topic                                 | Env     | Emits                    |
//! |---------------------------------------|---------|--------------------------|
//! | `/contractMarket/execution:{sym}`     | Futures | `DataMessage::Trade`     |
//! | `/market/match:{sym}`                 | Spot    | `DataMessage::Trade`     |
//! | `/contractMarket/tickerV2:{sym}`      | Futures | `DataMessage::Ticker`    |
//! | `/market/ticker:{sym}`                | Spot    | `DataMessage::Ticker`    |
//! | `/contractMarket/level2Depth5:{sym}`  | Futures | `DataMessage::OrderBook` (snapshot) |
//! | `/contractMarket/level2Depth50:{sym}` | Futures | `DataMessage::OrderBook` (snapshot) |
//! | `/contractMarket/level2:{sym}`        | Futures | `DataMessage::OrderBook` (delta)    |
//!
//! ## Private (requires private WS token)
//!
//! | Topic                          | Env     | Emits                         |
//! |--------------------------------|---------|-------------------------------|
//! | `/contractMarket/tradeOrders`  | Futures | `DataMessage::OrderUpdate`    |
//! | `/contract/position:{sym}`     | Futures | `DataMessage::PositionChange` |
//! | `/contractAccount/wallet`      | Futures | `DataMessage::BalanceUpdate`  |
//!
//! Build subscription messages with the dedicated helpers on [`KucoinConnector`]
//! rather than constructing topic strings by hand.

use serde_json::Value;
use uuid::Uuid;

use crate::actors::{
    BalanceUpdate, DataMessage, ExchangeConnector, OrderBookData, OrderUpdate, PositionChange,
    TickerData, TradeData, TradeSide, WebSocketConfig,
};
use crate::client::KucoinEnv;
use crate::error::Result;
use crate::ws::types::{WsMessage, WsToken};

// ── Connector ────────────────────────────────────────────────────────────────

/// KuCoin WebSocket connector.
///
/// Construct from a negotiated token returned by
/// [`KuCoinClient::get_ws_token_private`][crate::client::KuCoinClient::get_ws_token_private]
/// or [`KuCoinClient::get_ws_token_public`][crate::client::KuCoinClient::get_ws_token_public].
///
/// ```no_run
/// # use exchange_apiws::{KuCoinClient, Credentials, KucoinEnv};
/// # use exchange_apiws::ws::KucoinConnector;
/// # async fn example(client: KuCoinClient) -> exchange_apiws::Result<()> {
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
            crate::error::ExchangeError::Config(
                "KuCoin returned no instance servers".into(),
            )
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

    // ── Public subscription builders ─────────────────────────────────────────

    /// Subscribe to live trade executions for `symbol`.
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

    /// Subscribe to best-bid/ask ticker updates for `symbol`.
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

    /// Subscribe to a full order book depth snapshot for `symbol`.
    ///
    /// `depth` is clamped to 5 or 50 levels. Each message is a complete snapshot
    /// (`OrderBookData::is_snapshot == true`). Use this when you only need the
    /// top of book without maintaining a local book.
    ///
    /// Futures: `/contractMarket/level2Depth{5|50}:{symbol}`
    /// Spot:    `/spotMarket/level2Depth{5|50}:{symbol}`
    pub fn orderbook_depth_subscription(&self, symbol: &str, depth: u8) -> Option<String> {
        let d = if depth <= 5 { 5u8 } else { 50u8 };
        let topic = match self.env {
            KucoinEnv::LiveFutures => format!("/contractMarket/level2Depth{d}:{symbol}"),
            _ => format!("/spotMarket/level2Depth{d}:{symbol}"),
        };
        self.build_sub(topic, false)
    }

    /// Subscribe to full Level 2 incremental order book updates for `symbol`.
    ///
    /// Each message is a delta (`OrderBookData::is_snapshot == false`).
    /// `asks`/`bids` each contain one `[price, qty]` entry; `qty == 0.0` means
    /// remove that price level from your local book.
    ///
    /// To bootstrap, fetch a REST snapshot via
    /// [`get_orderbook_snapshot`][crate::rest::market::KuCoinClient::get_orderbook_snapshot],
    /// then apply deltas with `sequence` greater than the snapshot's `sequence`.
    ///
    /// Futures: `/contractMarket/level2:{symbol}`
    /// Spot:    `/market/level2:{symbol}`
    pub fn orderbook_l2_subscription(&self, symbol: &str) -> Option<String> {
        let topic = match self.env {
            KucoinEnv::LiveFutures => format!("/contractMarket/level2:{symbol}"),
            _ => format!("/market/level2:{symbol}"),
        };
        self.build_sub(topic, false)
    }

    // ── Private subscription builders ─────────────────────────────────────────

    /// Subscribe to private order fill and status-change events.
    ///
    /// Requires a **private** WS token from
    /// [`get_ws_token_private`][crate::client::KuCoinClient::get_ws_token_private].
    ///
    /// Emits `DataMessage::OrderUpdate` on fills and status transitions
    /// (`open`, `partialFilled`, `filled`, `canceled`).
    ///
    /// Topic: `/contractMarket/tradeOrders` (Futures)
    pub fn order_updates_subscription(&self) -> Option<String> {
        self.build_sub("/contractMarket/tradeOrders".to_string(), true)
    }

    /// Subscribe to position changes for `symbol`.
    ///
    /// Requires a **private** WS token.
    ///
    /// Emits `DataMessage::PositionChange` whenever the position size, mark
    /// price, or unrealised PnL changes materially.
    ///
    /// Topic: `/contract/position:{symbol}` (Futures)
    pub fn position_subscription(&self, symbol: &str) -> Option<String> {
        self.build_sub(format!("/contract/position:{symbol}"), true)
    }

    /// Subscribe to wallet/balance updates.
    ///
    /// Requires a **private** WS token.
    ///
    /// Emits `DataMessage::BalanceUpdate` on margin movements, funding
    /// settlements, and order-margin changes.
    ///
    /// Topic: `/contractAccount/wallet` (Futures)
    pub fn balance_subscription(&self) -> Option<String> {
        self.build_sub("/contractAccount/wallet".to_string(), true)
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

                // Public topics — route by topic prefix.
                if topic.contains("/contractMarket/execution")
                    || topic.contains("/market/match")
                {
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
                // Private topics
                } else if topic.contains("/contractMarket/tradeOrders") {
                    parse_order_update(exchange, &data)
                } else if topic.contains("/contract/position") {
                    parse_position_change(symbol, exchange, &data)
                } else if topic.contains("/contractAccount/wallet") {
                    parse_balance_update(exchange, &data)
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

// ── Helpers ───────────────────────────────────────────────────────────────────

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

fn str_u32(data: &Value, key: &str) -> u32 {
    data.get(key)
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                s.parse().ok()
            } else {
                v.as_u64().map(|n| n as u32)
            }
        })
        .unwrap_or(0)
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

// ── Public parsers ────────────────────────────────────────────────────────────

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
    // Futures tickerV2: bestBidPrice / bestAskPrice; spot: bestBid / bestAsk.
    let best_bid = first_f64(data, &["bestBidPrice", "bestBid"]);
    let best_ask = first_f64(data, &["bestAskPrice", "bestAsk"]);

    let exchange_ts = data["ts"]
        .as_i64()
        .map(|ts| {
            // Nanosecond timestamps are much larger than millisecond ones.
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

fn parse_orderbook_depth(
    symbol: &str,
    exchange: &str,
    data: &Value,
) -> Result<Vec<DataMessage>> {
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

fn parse_level2_delta(
    symbol: &str,
    exchange: &str,
    data: &Value,
) -> Result<Vec<DataMessage>> {
    // KuCoin level2 incremental format: `change: "price,side,qty"` where qty=0 → remove level.
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

// ── Private parsers ───────────────────────────────────────────────────────────

fn parse_order_update(exchange: &str, data: &Value) -> Result<Vec<DataMessage>> {
    let side = match data["side"].as_str().unwrap_or("buy") {
        s if s.eq_ignore_ascii_case("sell") => TradeSide::Sell,
        _ => TradeSide::Buy,
    };

    let exchange_ts = data["ts"]
        .as_i64()
        .map(|ns| ns / 1_000_000)
        .or_else(|| data["updatedAt"].as_i64())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    Ok(vec![DataMessage::OrderUpdate(OrderUpdate {
        symbol: data["symbol"].as_str().unwrap_or("").to_string(),
        exchange: exchange.to_string(),
        order_id: data["orderId"].as_str().unwrap_or("").to_string(),
        client_oid: data["clientOid"].as_str().map(str::to_string),
        side,
        order_type: data["type"].as_str().unwrap_or("market").to_string(),
        status: data["status"].as_str().unwrap_or("").to_string(),
        price: str_f64(data, "price"),
        size: str_u32(data, "size"),
        filled_size: str_u32(data, "filledSize"),
        remaining_size: str_u32(data, "remainSize"),
        fee: str_f64(data, "fee"),
        exchange_ts,
        receipt_ts: chrono::Utc::now().timestamp_millis(),
    })])
}

fn parse_position_change(
    symbol: &str,
    exchange: &str,
    data: &Value,
) -> Result<Vec<DataMessage>> {
    let exchange_ts = data["changeReason"]
        .as_str()
        .and(data["currentTimestamp"].as_i64())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    Ok(vec![DataMessage::PositionChange(PositionChange {
        symbol: symbol.to_string(),
        exchange: exchange.to_string(),
        current_qty: data["currentQty"].as_i64().unwrap_or(0) as i32,
        avg_entry_price: str_f64(data, "avgEntryPrice"),
        unrealised_pnl: str_f64(data, "unrealisedPnl"),
        realised_pnl: str_f64(data, "realisedPnl"),
        change_reason: data["changeReason"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        exchange_ts,
        receipt_ts: chrono::Utc::now().timestamp_millis(),
    })])
}

fn parse_balance_update(exchange: &str, data: &Value) -> Result<Vec<DataMessage>> {
    let exchange_ts = data["timestamp"]
        .as_i64()
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    Ok(vec![DataMessage::BalanceUpdate(BalanceUpdate {
        exchange: exchange.to_string(),
        currency: data["currency"].as_str().unwrap_or("").to_string(),
        available_balance: str_f64(data, "availableBalance"),
        hold_balance: str_f64(data, "holdBalance"),
        event: data["event"].as_str().unwrap_or("unknown").to_string(),
        exchange_ts,
        receipt_ts: chrono::Utc::now().timestamp_millis(),
    })])
}
