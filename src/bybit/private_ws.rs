//! Bybit v5 **private** WebSocket connector — order, execution, position &
//! wallet streams.
//!
//! Authenticates with an `auth` op frame (HMAC-SHA256 over `"GET/realtime" +
//! expires`, see [`BybitCredentials::sign_ws`]) sent right after connect — the
//! WS runner drives this via [`ExchangeConnector::auth_message`], before the
//! subscription — then subscribes to the private `order`, `execution`,
//! `position` and `wallet` topics and normalises each into the matching
//! [`DataMessage`] variant:
//!
//! | Topic | Variant | Notes |
//! |-------|---------|-------|
//! | `order` | [`DataMessage::OrderUpdate`] | order *state* (no per-fill detail) |
//! | `execution` | [`DataMessage::OrderUpdate`] | individual fills (match price/size/id) |
//! | `position` | [`DataMessage::PositionChange`] | one per position element |
//! | `wallet` | [`DataMessage::BalanceUpdate`] | one per coin in each account |
//!
//! Bybit reports order *state* on the `order` topic and individual fills on the
//! `execution` topic. Fills carry the true match price/size, surfaced via
//! [`OrderUpdate::match_price`] / [`OrderUpdate::match_size`] /
//! [`OrderUpdate::trade_id`] — so even a market order (whose resting `price` is
//! `0`) reports the price it actually filled at.
//!
//! ## Sizes
//!
//! [`OrderUpdate`]'s `size` family is `f64`, so quantities are represented
//! exactly — Bybit *inverse* contracts (integer USD) and *linear*/*spot*
//! fractional base units alike. The true fill price rides on `match_price`.
//!
//! [`PositionChange::current_qty`] is `i32` (contract counts) — Bybit's unsigned
//! `size` string is signed here by the position `side` (`Buy`
//! positive, `Sell` negative, empty/`None` → flat). [`BalanceUpdate`] maps the
//! per-coin `availableToWithdraw` / `locked` fields; note that *UNIFIED*
//! accounts report availability at the account level, so per-coin
//! `availableToWithdraw` can be empty (→ `0.0`) there, whereas *CONTRACT*
//! (inverse) accounts populate it.
//!
//! ```no_run
//! # use exchange_apiws::bybit::{BybitCredentials, BybitPrivateConnector};
//! # async fn example() -> exchange_apiws::Result<()> {
//! let creds = BybitCredentials::from_env()?;
//! let connector = BybitPrivateConnector::new(creds);
//! // hand `connector` to the WS runner; it sends the auth frame, subscribes to
//! // `order` + `execution` + `position` + `wallet`, and emits the matching
//! // `DataMessage` variants.
//! # Ok(())
//! # }
//! ```

use serde_json::Value;

use crate::actors::{
    BalanceUpdate, DataMessage, ExchangeConnector, OrderUpdate, PositionChange, TradeSide,
    WebSocketConfig,
};
use crate::bybit::auth::BybitCredentials;
use crate::error::Result;

const EXCHANGE_NAME: &str = "bybit";
const WS_PRIVATE_BASE: &str = "wss://stream.bybit.com/v5/private";
const PING_INTERVAL_SECS: u64 = 20;
/// Private topics this connector subscribes to on connect.
const PRIVATE_TOPICS: [&str; 4] = ["order", "execution", "position", "wallet"];
/// How far ahead (ms) the `auth` frame's `expires` deadline is set.
const AUTH_EXPIRY_MS: u64 = 5_000;

/// Bybit v5 private WebSocket connector: `order` + `execution` → `OrderUpdate`,
/// `position` → `PositionChange`, `wallet` → `BalanceUpdate`.
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

    /// `{"op":"subscribe","args":["order","execution","position","wallet"]}` —
    /// the private streams.
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
        // The `wallet` frame stamps its time once at the top level (`creationTime`,
        // a JSON number) — the per-coin rows carry no timestamp of their own.
        let creation_ts = i64_any(&json, "creationTime");

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
            "position" => data
                .iter()
                .filter_map(parse_position)
                .map(DataMessage::PositionChange)
                .collect(),
            // Each account element fans out to one BalanceUpdate per coin.
            "wallet" => data
                .iter()
                .flat_map(|acct| parse_wallet(acct, creation_ts))
                .map(DataMessage::BalanceUpdate)
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
        size: str_f64(d, "qty"),
        filled_size: str_f64(d, "cumExecQty"),
        remaining_size: str_f64(d, "leavesQty"),
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
    let match_size = str_f64(d, "execQty");
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
        size: str_f64(d, "orderQty"),
        filled_size: match_size,
        remaining_size: 0.0,
        fee: str_f64(d, "execFee"),
        match_price: Some(str_f64(d, "execPrice")),
        match_size: Some(match_size),
        trade_id: nonempty(str_field(d, "execId")),
        exchange_ts: str_i64(d, "execTime").unwrap_or_else(now_ms),
        receipt_ts: now_ms(),
    })
}

/// Parse one Bybit v5 `position`-topic element into a [`PositionChange`].
///
/// Bybit's `size` is unsigned and conveys direction in `side` (`"Buy"` /
/// `"Sell"`, or `""` / `"None"` when flat) — the contract count is signed
/// accordingly. There is no dedicated change-reason field, so `positionStatus`
/// (`"Normal"` / `"Liq"` / `"Adl"`) stands in, defaulting to `"positionChange"`.
fn parse_position(d: &Value) -> Option<PositionChange> {
    let symbol = d.get("symbol")?.as_str()?.to_string();
    let magnitude = str_f64(d, "size") as i32;
    let current_qty = match d.get("side").and_then(Value::as_str).unwrap_or("") {
        s if s.eq_ignore_ascii_case("sell") => -magnitude,
        s if s.eq_ignore_ascii_case("buy") => magnitude,
        _ => 0, // flat: "" / "None"
    };
    Some(PositionChange {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        current_qty,
        avg_entry_price: str_f64(d, "entryPrice"),
        unrealised_pnl: str_f64(d, "unrealisedPnl"),
        realised_pnl: str_f64(d, "cumRealisedPnl"),
        change_reason: nonempty(str_field(d, "positionStatus"))
            .unwrap_or_else(|| "positionChange".to_string()),
        exchange_ts: str_i64(d, "updatedTime").unwrap_or_else(now_ms),
        receipt_ts: now_ms(),
    })
}

/// Parse one Bybit v5 `wallet`-topic account element into one [`BalanceUpdate`]
/// per coin. `creation_ts` is the frame-level `creationTime` (ms); the per-coin
/// rows carry no timestamp. The `event` tag is set to the account type
/// (`"UNIFIED"` / `"CONTRACT"` …) for context.
fn parse_wallet(acct: &Value, creation_ts: Option<i64>) -> Vec<BalanceUpdate> {
    let event = nonempty(str_field(acct, "accountType")).unwrap_or_else(|| "wallet".to_string());
    let exchange_ts = creation_ts.unwrap_or_else(now_ms);
    let receipt_ts = now_ms();
    let Some(coins) = acct.get("coin").and_then(Value::as_array) else {
        return vec![];
    };
    coins
        .iter()
        .filter_map(|c| {
            let currency = nonempty(str_field(c, "coin"))?;
            Some(BalanceUpdate {
                exchange: EXCHANGE_NAME.to_string(),
                currency,
                available_balance: str_f64(c, "availableToWithdraw"),
                hold_balance: str_f64(c, "locked"),
                event: event.clone(),
                exchange_ts,
                receipt_ts,
            })
        })
        .collect()
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

/// Read an i64 that may arrive as a JSON number *or* a numeric string. Bybit
/// stamps `creationTime` as a number on the `wallet` frame, unlike the
/// string-typed per-element time fields (`updatedTime` / `execTime`).
fn i64_any(d: &Value, key: &str) -> Option<i64> {
    d.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
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
    fn subscription_is_all_four_private_topics() {
        let c = connector();
        let v: Value = serde_json::from_str(&c.subscription_message("").unwrap()).unwrap();
        assert_eq!(v["op"], "subscribe");
        let args: Vec<&str> = v["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        assert_eq!(args, vec!["order", "execution", "position", "wallet"]);
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
        assert!((o.size - 100.0).abs() < 1e-9);
        assert!((o.filled_size - 40.0).abs() < 1e-9);
        assert!((o.remaining_size - 60.0).abs() < 1e-9);
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
        assert_eq!(o.match_size, Some(10.0));
        assert!((o.filled_size - 10.0).abs() < 1e-9);
        assert_eq!(o.trade_id.as_deref(), Some("exec-77"));
        assert!((o.fee - 0.05).abs() < 1e-9);
        assert_eq!(o.exchange_ts, 1_700_000_005_000);
    }

    #[test]
    fn parses_position_topic_into_position_change() {
        let c = connector();
        let raw = r#"{
            "topic":"position",
            "creationTime":1700000010000,
            "data":[{
                "symbol":"BTCUSD","side":"Sell","size":"150",
                "entryPrice":"29850.5","unrealisedPnl":"-12.5",
                "cumRealisedPnl":"340.25","positionStatus":"Normal",
                "updatedTime":"1700000009000"
            }]
        }"#;
        let msgs = c.parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        let DataMessage::PositionChange(p) = &msgs[0] else {
            panic!("expected PositionChange, got {:?}", msgs[0]);
        };
        assert_eq!(p.symbol, "BTCUSD");
        assert_eq!(p.exchange, "bybit");
        // Short position: unsigned size 150 signed negative by side=Sell.
        assert_eq!(p.current_qty, -150);
        assert!((p.avg_entry_price - 29850.5).abs() < 1e-9);
        assert!((p.unrealised_pnl + 12.5).abs() < 1e-9);
        assert!((p.realised_pnl - 340.25).abs() < 1e-9);
        assert_eq!(p.change_reason, "Normal");
        assert_eq!(p.exchange_ts, 1_700_000_009_000);
    }

    #[test]
    fn flat_position_has_zero_qty_and_default_reason() {
        let c = connector();
        // Bybit reports an empty `side` once a position is closed, and the
        // `position` frame omits `positionStatus` in some pushes.
        let raw = r#"{"topic":"position","data":[{
            "symbol":"BTCUSD","side":"","size":"0","entryPrice":"0",
            "unrealisedPnl":"0","cumRealisedPnl":"5.0","updatedTime":"1700000009000"
        }]}"#;
        let DataMessage::PositionChange(p) = &c.parse_message(raw).unwrap()[0] else {
            panic!("expected PositionChange");
        };
        assert_eq!(p.current_qty, 0);
        assert_eq!(p.change_reason, "positionChange");
    }

    #[test]
    fn parses_wallet_topic_into_one_balance_update_per_coin() {
        let c = connector();
        // One account element with two coins → two BalanceUpdates, both stamped
        // with the frame-level numeric `creationTime`.
        let raw = r#"{
            "topic":"wallet",
            "creationTime":1700000020000,
            "data":[{
                "accountType":"CONTRACT",
                "coin":[
                    {"coin":"USDT","availableToWithdraw":"1000.5","locked":"250.25"},
                    {"coin":"BTC","availableToWithdraw":"0.5","locked":"0"}
                ]
            }]
        }"#;
        let msgs = c.parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 2);
        let DataMessage::BalanceUpdate(usdt) = &msgs[0] else {
            panic!("expected BalanceUpdate, got {:?}", msgs[0]);
        };
        assert_eq!(usdt.exchange, "bybit");
        assert_eq!(usdt.currency, "USDT");
        assert!((usdt.available_balance - 1000.5).abs() < 1e-9);
        assert!((usdt.hold_balance - 250.25).abs() < 1e-9);
        assert_eq!(usdt.event, "CONTRACT", "event carries the account type");
        assert_eq!(usdt.exchange_ts, 1_700_000_020_000);
        let DataMessage::BalanceUpdate(btc) = &msgs[1] else {
            panic!("expected BalanceUpdate");
        };
        assert_eq!(btc.currency, "BTC");
        assert!((btc.available_balance - 0.5).abs() < 1e-9);
    }

    #[test]
    fn unknown_topic_is_ignored() {
        let c = connector();
        // `greeks` (options portfolio greeks) is a real private topic we don't map.
        let raw = r#"{"topic":"greeks","data":[{"baseCoin":"BTC"}]}"#;
        assert!(c.parse_message(raw).unwrap().is_empty());
    }
}
