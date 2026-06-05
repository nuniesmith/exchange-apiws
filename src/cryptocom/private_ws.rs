//! Crypto.com **user** (private) WebSocket — order / trade / balance channels.
//!
//! Connects to `wss://stream.crypto.com/exchange/v1/user`, authenticates with a
//! signed `public/auth` frame (the runner sends [`ExchangeConnector::auth_message`]
//! right after connect, before the subscription — same hook the Bybit private
//! connector uses), then subscribes to the three private channels and normalises
//! them into the unified feed:
//!
//! | Channel | → Variant | Notes |
//! |---------|-----------|-------|
//! | `user.order` | [`DataMessage::OrderUpdate`] | order state (no per-fill detail) |
//! | `user.trade` | [`DataMessage::OrderUpdate`] | individual fills (carry `match_price`/`size`/`trade_id`) |
//! | `user.balance` | [`DataMessage::BalanceUpdate`] | one per `position_balances` entry |
//!
//! Like the rest of the crate, [`OrderUpdate`]'s `size` family is `f64`, so
//! fractional spot quantities are preserved exactly.
//!
//! ## Schema note
//!
//! Field names follow Crypto.com's **Exchange API v1** user channels. A couple of
//! fields are read with fallbacks where v1 has used more than one spelling
//! (`order_type`/`type`, `limit_price`/`price`, `fees`/`fee`), so the parser is
//! robust to either. `user.balance` has no per-row timestamp, so its events are
//! stamped with the receipt time.
//!
//! ```no_run
//! # use exchange_apiws::cryptocom::{CryptocomCredentials, CryptocomUserConnector};
//! # async fn example() -> exchange_apiws::Result<()> {
//! let creds = CryptocomCredentials::from_env()?;
//! let connector = CryptocomUserConnector::new(creds);
//! // hand `connector` to the WS runner; it sends the signed `public/auth`
//! // frame, subscribes to user.order/user.trade/user.balance, and emits the
//! // matching `DataMessage` variants.
//! # Ok(())
//! # }
//! ```

use serde_json::{Value, json};

use crate::actors::{
    BalanceUpdate, DataMessage, ExchangeConnector, OrderUpdate, TradeSide, WebSocketConfig,
};
use crate::cryptocom::auth::{CryptocomCredentials, sign_cryptocom_request};
use crate::cryptocom::ws::CryptocomConnector;
use crate::error::Result;

const EXCHANGE_NAME: &str = "cryptocom";
const WS_PRIVATE_URL: &str = "wss://stream.crypto.com/exchange/v1/user";
/// Private channels this connector subscribes to after auth.
const USER_CHANNELS: [&str; 3] = ["user.order", "user.trade", "user.balance"];

/// Crypto.com v1 user (private) WebSocket connector: `user.order` +
/// `user.trade` → `OrderUpdate`, `user.balance` → `BalanceUpdate`.
pub struct CryptocomUserConnector {
    credentials: CryptocomCredentials,
    url: String,
}

impl CryptocomUserConnector {
    /// Build a connector for the given credentials (mainnet user endpoint).
    pub fn new(credentials: CryptocomCredentials) -> Self {
        Self {
            credentials,
            url: WS_PRIVATE_URL.to_string(),
        }
    }

    /// Build against a specific URL (a local tokio-tungstenite test server).
    pub fn with_url(credentials: CryptocomCredentials, url: impl Into<String>) -> Self {
        Self {
            credentials,
            url: url.into(),
        }
    }

    /// The signed `public/auth` frame. Crypto.com signs
    /// `method || id || api_key || params || nonce` (empty params here) and
    /// places the hex HMAC in the `sig` field alongside `api_key` + `nonce`.
    fn auth_frame(&self, id: i64, nonce: i64) -> Option<String> {
        let sig = sign_cryptocom_request(
            "public/auth",
            id,
            &self.credentials.api_key,
            &Value::Null,
            nonce,
            &self.credentials.api_secret,
        )
        .ok()?;
        Some(
            json!({
                "id": id,
                "method": "public/auth",
                "api_key": self.credentials.api_key,
                "sig": sig,
                "nonce": nonce,
            })
            .to_string(),
        )
    }
}

impl ExchangeConnector for CryptocomUserConnector {
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
            subscription_msg: None,
            // Server-initiated heartbeats (~30 s) wake the recv loop; the tick
            // just drives the idle check (see `response_for`).
            ping_interval_secs: 30,
            reconnect_delay_secs: 5,
            max_reconnect_attempts: 5,
        }
    }

    /// `{"id":N,"method":"subscribe","params":{"channels":["user.order",…]}}`.
    fn subscription_message(&self, _symbol: &str) -> Option<String> {
        let channels: Vec<String> = USER_CHANNELS.iter().map(|c| (*c).to_string()).collect();
        Some(CryptocomConnector::subscribe_frame(now_ms(), &channels))
    }

    /// Signed `public/auth` frame, sent by the runner after connect and before
    /// the subscription. `id` and `nonce` are both the current epoch-ms.
    fn auth_message(&self) -> Option<String> {
        let nonce = now_ms();
        self.auth_frame(nonce, nonce)
    }

    /// Crypto.com's heartbeat is server-initiated — reply to `public/heartbeat`
    /// with `public/respond-heartbeat` echoing the server's `id`.
    fn response_for(&self, raw: &str) -> Option<String> {
        if !raw.contains("public/heartbeat") {
            return None;
        }
        let json: Value = serde_json::from_str(raw).ok()?;
        if json.get("method").and_then(Value::as_str)? != "public/heartbeat" {
            return None;
        }
        let id = json.get("id").and_then(Value::as_i64)?;
        Some(json!({"id": id, "method": "public/respond-heartbeat"}).to_string())
    }

    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>> {
        let json: Value = serde_json::from_str(raw)?;

        // Heartbeats (handled via `response_for`) and method-ack frames (auth /
        // subscribe success) carry no `result.data`.
        let Some(result) = json.get("result") else {
            return Ok(vec![]);
        };
        let channel = result.get("channel").and_then(Value::as_str).unwrap_or("");
        let Some(data) = result.get("data").and_then(Value::as_array) else {
            return Ok(vec![]);
        };

        let out = match channel {
            "user.order" => data
                .iter()
                .filter_map(parse_user_order)
                .map(DataMessage::OrderUpdate)
                .collect(),
            "user.trade" => data
                .iter()
                .filter_map(parse_user_trade)
                .map(DataMessage::OrderUpdate)
                .collect(),
            // Each balance snapshot fans out to one update per position balance.
            "user.balance" => data
                .iter()
                .flat_map(parse_user_balance)
                .map(DataMessage::BalanceUpdate)
                .collect(),
            _ => vec![],
        };
        Ok(out)
    }
}

/// Parse a v1 `user.order` element (order state; no per-fill detail) into an
/// [`OrderUpdate`].
fn parse_user_order(d: &Value) -> Option<OrderUpdate> {
    let symbol = d.get("instrument_name")?.as_str()?.to_string();
    let size = str_f64(d, "quantity");
    let filled_size = str_f64(d, "cumulative_quantity");
    let status = d.get("status").and_then(Value::as_str).unwrap_or("");
    Some(OrderUpdate {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        order_id: id_string(d, "order_id"),
        client_oid: nonempty(str_field(d, "client_oid")),
        side: side_of(d),
        order_type: str_any(d, &["order_type", "type"]).to_lowercase(),
        status: map_order_status(status, filled_size),
        price: str_f64_any(d, &["limit_price", "price"]),
        size,
        filled_size,
        remaining_size: (size - filled_size).max(0.0),
        fee: str_f64(d, "cumulative_fee"),
        match_price: None,
        match_size: None,
        trade_id: None,
        exchange_ts: i64_any(d, "update_time")
            .or_else(|| i64_any(d, "create_time"))
            .unwrap_or_else(now_ms),
        receipt_ts: now_ms(),
    })
}

/// Parse a v1 `user.trade` element — an individual fill — into an
/// [`OrderUpdate`] carrying the match price/size/trade-id.
fn parse_user_trade(d: &Value) -> Option<OrderUpdate> {
    let symbol = d.get("instrument_name")?.as_str()?.to_string();
    let qty = str_f64(d, "traded_quantity");
    let price = str_f64(d, "traded_price");
    Some(OrderUpdate {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        order_id: id_string(d, "order_id"),
        client_oid: nonempty(str_field(d, "client_oid")),
        side: side_of(d),
        // A trade event doesn't carry the order type; that's on `user.order`.
        order_type: String::new(),
        // A fill; whether it fully closed the order rides on `user.order`.
        status: "partialFilled".to_string(),
        price,
        size: qty,
        filled_size: qty,
        remaining_size: 0.0,
        fee: str_f64_any(d, &["fees", "fee"]),
        match_price: Some(price),
        match_size: Some(qty),
        trade_id: nonempty(id_string(d, "trade_id")),
        exchange_ts: i64_any(d, "create_time").unwrap_or_else(now_ms),
        receipt_ts: now_ms(),
    })
}

/// Parse a v1 `user.balance` account snapshot into one [`BalanceUpdate`] per
/// entry in its `position_balances` array.
fn parse_user_balance(d: &Value) -> Vec<BalanceUpdate> {
    // The v1 balance snapshot has no per-row timestamp — stamp with receipt.
    let ts = now_ms();
    let Some(positions) = d.get("position_balances").and_then(Value::as_array) else {
        return vec![];
    };
    positions
        .iter()
        .filter_map(|p| {
            let currency = nonempty(str_field(p, "instrument_name"))?;
            Some(BalanceUpdate {
                exchange: EXCHANGE_NAME.to_string(),
                currency,
                available_balance: str_f64_any(p, &["max_withdrawal_balance", "quantity"]),
                hold_balance: str_f64(p, "reserved_qty"),
                event: "user.balance".to_string(),
                exchange_ts: ts,
                receipt_ts: ts,
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

/// First non-empty string among `keys` (Crypto.com v1 spells some fields more
/// than one way across endpoints).
fn str_any(d: &Value, keys: &[&str]) -> String {
    for k in keys {
        let s = d.get(*k).and_then(Value::as_str).unwrap_or("");
        if !s.is_empty() {
            return s.to_string();
        }
    }
    String::new()
}

fn nonempty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

/// A field that may arrive as a JSON string or number, rendered as a `String`.
fn id_string(d: &Value, key: &str) -> String {
    match d.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Parse a Crypto.com string-decimal field to f64; `0.0` if absent.
fn str_f64(d: &Value, key: &str) -> f64 {
    d.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

/// First parseable string-decimal among `keys`.
fn str_f64_any(d: &Value, keys: &[&str]) -> f64 {
    for k in keys {
        if let Some(v) = d
            .get(*k)
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
        {
            return v;
        }
    }
    0.0
}

/// Read an i64 that may arrive as a JSON number or a numeric string. Crypto.com
/// stamps `update_time` / `create_time` as numbers.
fn i64_any(d: &Value, key: &str) -> Option<i64> {
    d.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

fn side_of(d: &Value) -> TradeSide {
    match d.get("side").and_then(Value::as_str).unwrap_or("BUY") {
        s if s.eq_ignore_ascii_case("sell") => TradeSide::Sell,
        _ => TradeSide::Buy,
    }
}

/// Map Crypto.com's `status` to the crate's vocabulary. v1 has no explicit
/// "partially filled" status — it reports `ACTIVE` with a non-zero
/// `cumulative_quantity`, so derive `partialFilled` from the fill count.
fn map_order_status(status: &str, filled_size: f64) -> String {
    match status {
        "FILLED" => "filled",
        "CANCELED" | "REJECTED" | "EXPIRED" => "canceled",
        // ACTIVE / NEW / PENDING with fills → partial; otherwise resting.
        _ if filled_size > 0.0 => "partialFilled",
        _ => "open",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector() -> CryptocomUserConnector {
        CryptocomUserConnector::new(CryptocomCredentials::new("test-key", "test-secret"))
    }

    #[test]
    fn auth_message_is_signed_public_auth_frame() {
        let v: Value = serde_json::from_str(&connector().auth_message().unwrap()).unwrap();
        assert_eq!(v["method"], "public/auth");
        assert_eq!(v["api_key"], "test-key");
        assert!(v["nonce"].as_i64().unwrap() > 0);
        // HMAC-SHA256 hex is 64 chars.
        assert_eq!(v["sig"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn subscription_lists_the_three_user_channels() {
        let v: Value =
            serde_json::from_str(&connector().subscription_message("").unwrap()).unwrap();
        assert_eq!(v["method"], "subscribe");
        let chans: Vec<&str> = v["params"]["channels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert_eq!(chans, vec!["user.order", "user.trade", "user.balance"]);
    }

    #[test]
    fn heartbeat_gets_a_respond_frame() {
        let c = connector();
        let resp = c
            .response_for(r#"{"id":42,"method":"public/heartbeat"}"#)
            .expect("heartbeat must be answered");
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["id"], 42);
        assert_eq!(v["method"], "public/respond-heartbeat");
        // Non-heartbeat frames get no response.
        assert!(c.response_for(r#"{"id":1,"method":"subscribe"}"#).is_none());
    }

    #[test]
    fn auth_and_subscribe_acks_yield_no_messages() {
        let c = connector();
        assert!(
            c.parse_message(r#"{"id":1,"method":"public/auth","code":0}"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            c.parse_message(r#"{"id":2,"method":"subscribe","code":0}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parses_user_order_into_order_update() {
        let raw = r#"{
            "result":{"channel":"user.order","instrument_name":"BTC_USDT","data":[{
                "instrument_name":"BTC_USDT","order_id":"19848525","client_oid":"my-oid",
                "side":"SELL","type":"LIMIT","status":"ACTIVE","limit_price":"30000.5",
                "quantity":"100","cumulative_quantity":"40","cumulative_fee":"0.12",
                "update_time":1700000000000
            }]}
        }"#;
        let msgs = connector().parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        let DataMessage::OrderUpdate(o) = &msgs[0] else {
            panic!("expected OrderUpdate, got {:?}", msgs[0]);
        };
        assert_eq!(o.symbol, "BTC_USDT");
        assert_eq!(o.exchange, "cryptocom");
        assert_eq!(o.order_id, "19848525");
        assert_eq!(o.client_oid.as_deref(), Some("my-oid"));
        assert_eq!(o.side, TradeSide::Sell);
        assert_eq!(o.order_type, "limit");
        // ACTIVE with cumulative_quantity > 0 → partialFilled.
        assert_eq!(o.status, "partialFilled");
        assert!((o.price - 30000.5).abs() < 1e-9);
        assert!((o.size - 100.0).abs() < 1e-9);
        assert!((o.filled_size - 40.0).abs() < 1e-9);
        assert!((o.remaining_size - 60.0).abs() < 1e-9);
        assert!((o.fee - 0.12).abs() < 1e-9);
        assert_eq!(o.match_price, None);
        assert_eq!(o.exchange_ts, 1_700_000_000_000);
    }

    #[test]
    fn numeric_order_id_and_filled_status() {
        // order_id as a JSON number; FILLED status maps directly.
        let raw = r#"{"result":{"channel":"user.order","data":[{
            "instrument_name":"ETH_USDT","order_id":42,"client_oid":"",
            "side":"BUY","order_type":"MARKET","status":"FILLED","price":"0",
            "quantity":"5","cumulative_quantity":"5","update_time":1700000000000
        }]}}"#;
        let DataMessage::OrderUpdate(o) = &connector().parse_message(raw).unwrap()[0] else {
            panic!("expected OrderUpdate");
        };
        assert_eq!(o.order_id, "42", "numeric id rendered as string");
        assert_eq!(o.client_oid, None, "empty client_oid → None");
        assert_eq!(o.status, "filled");
        assert!((o.remaining_size - 0.0).abs() < 1e-9);
    }

    #[test]
    fn parses_user_trade_with_match_details() {
        let raw = r#"{
            "result":{"channel":"user.trade","data":[{
                "instrument_name":"ETH_USDT","order_id":"ord-9","client_oid":"",
                "side":"BUY","traded_price":"2500.25","traded_quantity":"10",
                "fees":"0.05","trade_id":"exec-77","create_time":1700000005000
            }]}
        }"#;
        let DataMessage::OrderUpdate(o) = &connector().parse_message(raw).unwrap()[0] else {
            panic!("expected OrderUpdate");
        };
        assert_eq!(o.symbol, "ETH_USDT");
        assert_eq!(o.order_id, "ord-9");
        assert_eq!(o.side, TradeSide::Buy);
        assert_eq!(o.status, "partialFilled");
        assert!((o.match_price.unwrap() - 2500.25).abs() < 1e-9);
        assert_eq!(o.match_size, Some(10.0));
        assert!((o.filled_size - 10.0).abs() < 1e-9);
        assert_eq!(o.trade_id.as_deref(), Some("exec-77"));
        assert!((o.fee - 0.05).abs() < 1e-9);
        assert_eq!(o.exchange_ts, 1_700_000_005_000);
    }

    #[test]
    fn parses_user_balance_position_balances_fan_out() {
        let raw = r#"{
            "result":{"channel":"user.balance","data":[{
                "total_available_balance":"1000",
                "position_balances":[
                    {"instrument_name":"USDT","quantity":"1000.5","reserved_qty":"250.25","max_withdrawal_balance":"750.25"},
                    {"instrument_name":"BTC","quantity":"0.5","reserved_qty":"0","max_withdrawal_balance":"0.5"}
                ]
            }]}
        }"#;
        let msgs = connector().parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 2);
        let DataMessage::BalanceUpdate(usdt) = &msgs[0] else {
            panic!("expected BalanceUpdate, got {:?}", msgs[0]);
        };
        assert_eq!(usdt.exchange, "cryptocom");
        assert_eq!(usdt.currency, "USDT");
        assert!((usdt.available_balance - 750.25).abs() < 1e-9);
        assert!((usdt.hold_balance - 250.25).abs() < 1e-9);
        assert_eq!(usdt.event, "user.balance");
        let DataMessage::BalanceUpdate(btc) = &msgs[1] else {
            panic!("expected BalanceUpdate");
        };
        assert_eq!(btc.currency, "BTC");
        assert!((btc.available_balance - 0.5).abs() < 1e-9);
    }

    #[test]
    fn unknown_channel_is_ignored() {
        let raw = r#"{"result":{"channel":"user.margin","data":[{"x":1}]}}"#;
        assert!(connector().parse_message(raw).unwrap().is_empty());
    }
}
