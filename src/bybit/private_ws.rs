//! Bybit v5 **private** WebSocket connector — order & execution streams.
//!
//! Authenticates with an `auth` op frame (HMAC-SHA256 over `"GET/realtime" +
//! expires`, see [`BybitCredentials::sign_ws`]) sent right after connect — the
//! WS runner drives this via [`ExchangeConnector::auth_message`], before the
//! subscription — then subscribes to the private `order` and `execution` topics
//! and normalises both into [`DataMessage::OrderUpdate`].
//!
//! Bybit reports order *state* on the `order` topic and individual fills on the
//! `execution` topic. Fills carry the true match price/size, surfaced via
//! [`OrderUpdate::match_price`] / [`OrderUpdate::match_size`] /
//! [`OrderUpdate::trade_id`] — so even a market order (whose resting `price` is
//! `0`) reports the price it actually filled at.
//!
//! ## Sizes
//!
//! [`OrderUpdate`]'s `size` family is `u32` (contract counts, matching the
//! KuCoin feed). Bybit *inverse* contracts are integer USD contracts and map
//! cleanly; Bybit *linear*/*spot* quantities are fractional base units and are
//! truncated here. The fill price is always exact via `match_price`; widening
//! the `OrderUpdate` size fields to `f64` is a separate (breaking) change.
//!
//! ```no_run
//! # use exchange_apiws::bybit::{BybitCredentials, BybitPrivateConnector};
//! # async fn example() -> exchange_apiws::Result<()> {
//! let creds = BybitCredentials::from_env()?;
//! let connector = BybitPrivateConnector::new(creds);
//! // hand `connector` to the WS runner; it sends the auth frame, subscribes to
//! // `order` + `execution`, and emits `DataMessage::OrderUpdate`s.
//! # Ok(())
//! # }
//! ```

use serde_json::Value;

use crate::actors::{DataMessage, ExchangeConnector, OrderUpdate, TradeSide, WebSocketConfig};
use crate::bybit::auth::BybitCredentials;
use crate::error::Result;

const EXCHANGE_NAME: &str = "bybit";
const WS_PRIVATE_BASE: &str = "wss://stream.bybit.com/v5/private";
const PING_INTERVAL_SECS: u64 = 20;
/// Private topics this connector subscribes to on connect.
const PRIVATE_TOPICS: [&str; 2] = ["order", "execution"];
/// How far ahead (ms) the `auth` frame's `expires` deadline is set.
const AUTH_EXPIRY_MS: u64 = 5_000;

/// Bybit v5 private WebSocket connector: order + execution → `OrderUpdate`.
pub struct BybitPrivateConnector {
    credentials: BybitCredentials,
    url: String,
}

impl BybitPrivateConnector {
    /// Build a private connector for the given credentials (mainnet private WS).
    pub fn new(credentials: BybitCredentials) -> Self {
        Self {
            credentials,
            url: WS_PRIVATE_BASE.to_string(),
        }
    }

    /// Build a private connector against a specific URL (testnet / a mock server).
    pub fn with_url(credentials: BybitCredentials, url: impl Into<String>) -> Self {
        Self {
            credentials,
            url: url.into(),
        }
    }

    /// The signed `auth` op frame Bybit expects post-connect, with the given
    /// `expires` deadline (epoch-ms). Args are `[api_key, expires, signature]`.
    fn auth_frame(&self, expires: u64) -> String {
        let signature = self.credentials.sign_ws(expires);
        serde_json::json!({
            "op": "auth",
            "args": [self.credentials.api_key, expires, signature],
        })
        .to_string()
    }
}

impl ExchangeConnector for BybitPrivateConnector {
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

    /// `{"op":"subscribe","args":["order","execution"]}` — the private streams.
    fn subscription_message(&self, _symbol: &str) -> Option<String> {
        serde_json::to_string(&serde_json::json!({
            "op": "subscribe",
            "args": PRIVATE_TOPICS,
        }))
        .ok()
    }

    /// Signed `auth` frame, sent by the runner after connect and before the
    /// subscription. `expires` is `now + AUTH_EXPIRY_MS`.
    fn auth_message(&self) -> Option<String> {
        let expires = (chrono::Utc::now().timestamp_millis() as u64) + AUTH_EXPIRY_MS;
        Some(self.auth_frame(expires))
    }

    /// Bybit expects `{"op":"ping"}`; the server responds `{"op":"pong"}`.
    fn ping_message(&self) -> Option<String> {
        Some(r#"{"op":"ping"}"#.to_string())
    }

    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>> {
        let json: Value = serde_json::from_str(raw)?;

        // Op responses (auth ack, subscribe ack, pong) carry no payload.
        if json.get("op").is_some() {
            return Ok(vec![]);
        }

        let topic = json.get("topic").and_then(Value::as_str).unwrap_or("");
        let Some(data) = json.get("data").and_then(Value::as_array) else {
            return Ok(vec![]);
        };

        let out = match topic {
            "order" => data
                .iter()
                .filter_map(parse_order)
                .map(DataMessage::OrderUpdate)
                .collect(),
            "execution" => data
                .iter()
                .filter_map(parse_execution)
                .map(DataMessage::OrderUpdate)
                .collect(),
            _ => vec![],
        };
        Ok(out)
    }
}

/// Parse one Bybit v5 `order`-topic element (order state; no per-fill details).
fn parse_order(d: &Value) -> Option<OrderUpdate> {
    let symbol = d.get("symbol")?.as_str()?.to_string();
    Some(OrderUpdate {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        order_id: str_field(d, "orderId"),
        client_oid: nonempty(str_field(d, "orderLinkId")),
        side: side_of(d),
        order_type: order_type_of(d),
        status: map_status(d.get("orderStatus").and_then(Value::as_str).unwrap_or("")),
        price: str_f64(d, "price"),
        size: str_f64(d, "qty") as u32,
        filled_size: str_f64(d, "cumExecQty") as u32,
        remaining_size: str_f64(d, "leavesQty") as u32,
        fee: str_f64(d, "cumExecFee"),
        match_price: None,
        match_size: None,
        trade_id: None,
        exchange_ts: str_i64(d, "updatedTime").unwrap_or_else(now_ms),
        receipt_ts: now_ms(),
    })
}

/// Parse one Bybit v5 `execution`-topic element — an individual fill — carrying
/// the match price/size/trade-id.
fn parse_execution(d: &Value) -> Option<OrderUpdate> {
    let symbol = d.get("symbol")?.as_str()?.to_string();
    let match_size = str_f64(d, "execQty") as u32;
    Some(OrderUpdate {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        order_id: str_field(d, "orderId"),
        client_oid: nonempty(str_field(d, "orderLinkId")),
        side: side_of(d),
        order_type: order_type_of(d),
        // A fill event; whether it fully closed the order is conveyed by the
        // `order` topic — treat each execution as a partial fill here.
        status: "partialFilled".to_string(),
        price: str_f64(d, "orderPrice"),
        size: str_f64(d, "orderQty") as u32,
        filled_size: match_size,
        remaining_size: 0,
        fee: str_f64(d, "execFee"),
        match_price: Some(str_f64(d, "execPrice")),
        match_size: Some(match_size),
        trade_id: nonempty(str_field(d, "execId")),
        exchange_ts: str_i64(d, "execTime").unwrap_or_else(now_ms),
        receipt_ts: now_ms(),
    })
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn str_field(d: &Value, key: &str) -> String {
    d.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn nonempty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

fn str_f64(d: &Value, key: &str) -> f64 {
    d.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

fn str_i64(d: &Value, key: &str) -> Option<i64> {
    d.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
}

fn side_of(d: &Value) -> TradeSide {
    match d.get("side").and_then(Value::as_str).unwrap_or("Buy") {
        s if s.eq_ignore_ascii_case("sell") => TradeSide::Sell,
        _ => TradeSide::Buy,
    }
}

fn order_type_of(d: &Value) -> String {
    d.get("orderType")
        .and_then(Value::as_str)
        .unwrap_or("Market")
        .to_lowercase()
}

/// Map Bybit's `orderStatus` to the crate's `open` / `partialFilled` /
/// `filled` / `canceled` vocabulary.
fn map_status(status: &str) -> String {
    match status {
        "PartiallyFilled" => "partialFilled",
        "Filled" => "filled",
        "Cancelled" | "PartiallyFilledCanceled" | "Deactivated" | "Rejected" => "canceled",
        // New / Created / Untriggered / Triggered / anything else → resting.
        _ => "open",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector() -> BybitPrivateConnector {
        BybitPrivateConnector::new(BybitCredentials::new("test-key", "test-secret"))
    }

    #[test]
    fn auth_message_is_signed_op_auth_frame() {
        let c = connector();
        let frame = c.auth_message().expect("private connector requires auth");
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["op"], "auth");
        let args = v["args"].as_array().unwrap();
        assert_eq!(args[0], "test-key");
        assert!(
            args[1].as_u64().unwrap() > 0,
            "expires is a positive epoch-ms"
        );
        // HMAC-SHA256 hex is 64 chars.
        assert_eq!(args[2].as_str().unwrap().len(), 64);
    }

    #[test]
    fn subscription_is_order_and_execution() {
        let c = connector();
        let v: Value = serde_json::from_str(&c.subscription_message("").unwrap()).unwrap();
        assert_eq!(v["op"], "subscribe");
        let args: Vec<&str> = v["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        assert_eq!(args, vec!["order", "execution"]);
    }

    #[test]
    fn op_acks_yield_no_messages() {
        let c = connector();
        assert!(
            c.parse_message(r#"{"op":"auth","success":true}"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            c.parse_message(r#"{"op":"subscribe","success":true}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parses_order_topic_into_order_update() {
        let c = connector();
        let raw = r#"{
            "topic":"order",
            "data":[{
                "symbol":"BTCUSD","orderId":"abc123","orderLinkId":"my-oid",
                "side":"Sell","orderType":"Limit","orderStatus":"PartiallyFilled",
                "price":"30000.5","qty":"100","cumExecQty":"40","leavesQty":"60",
                "cumExecFee":"0.12","updatedTime":"1700000000000"
            }]
        }"#;
        let msgs = c.parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        let DataMessage::OrderUpdate(o) = &msgs[0] else {
            panic!("expected OrderUpdate, got {:?}", msgs[0]);
        };
        assert_eq!(o.symbol, "BTCUSD");
        assert_eq!(o.exchange, "bybit");
        assert_eq!(o.order_id, "abc123");
        assert_eq!(o.client_oid.as_deref(), Some("my-oid"));
        assert_eq!(o.side, TradeSide::Sell);
        assert_eq!(o.order_type, "limit");
        assert_eq!(o.status, "partialFilled");
        assert!((o.price - 30000.5).abs() < 1e-9);
        assert_eq!(o.size, 100);
        assert_eq!(o.filled_size, 40);
        assert_eq!(o.remaining_size, 60);
        assert!((o.fee - 0.12).abs() < 1e-9);
        assert_eq!(o.match_price, None, "order topic has no per-fill price");
        assert_eq!(o.exchange_ts, 1_700_000_000_000);
    }

    #[test]
    fn parses_execution_topic_with_match_details() {
        let c = connector();
        let raw = r#"{
            "topic":"execution",
            "data":[{
                "symbol":"ETHUSD","orderId":"ord-9","orderLinkId":"",
                "side":"Buy","orderType":"Market","execPrice":"2500.25",
                "execQty":"10","execFee":"0.05","execId":"exec-77",
                "execTime":"1700000005000"
            }]
        }"#;
        let msgs = c.parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        let DataMessage::OrderUpdate(o) = &msgs[0] else {
            panic!("expected OrderUpdate");
        };
        assert_eq!(o.symbol, "ETHUSD");
        assert_eq!(o.order_id, "ord-9");
        assert_eq!(o.client_oid, None, "empty orderLinkId → None");
        assert_eq!(o.side, TradeSide::Buy);
        assert_eq!(o.status, "partialFilled");
        // The true fill price/size/id ride on the match_* fields.
        assert!((o.match_price.unwrap() - 2500.25).abs() < 1e-9);
        assert_eq!(o.match_size, Some(10));
        assert_eq!(o.filled_size, 10);
        assert_eq!(o.trade_id.as_deref(), Some("exec-77"));
        assert!((o.fee - 0.05).abs() < 1e-9);
        assert_eq!(o.exchange_ts, 1_700_000_005_000);
    }

    #[test]
    fn unknown_topic_is_ignored() {
        let c = connector();
        let raw = r#"{"topic":"wallet","data":[{"coin":"USDT"}]}"#;
        assert!(c.parse_message(raw).unwrap().is_empty());
    }
}
