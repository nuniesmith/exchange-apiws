//! Binance spot **user-data** WebSocket — order & balance events.
//!
//! Unlike the public market streams, the user-data stream is authenticated by a
//! `listenKey` baked into the URL (`/ws/<listenKey>`, obtained via
//! [`BinanceUserDataRest`]) rather than a subscription or auth frame — so this
//! connector sends no subscription and events flow automatically once
//! connected. It normalises the two spot events into the unified feed:
//!
//! | `e` (event) | → Variant | Notes |
//! |-------------|-----------|-------|
//! | `executionReport` | [`DataMessage::OrderUpdate`] | one per order state change / fill |
//! | `outboundAccountPosition` | [`DataMessage::BalanceUpdate`] | one per asset in the snapshot |
//!
//! On a fill (`x == "TRADE"`) the report carries the last-fill price/qty/trade-id,
//! surfaced via [`OrderUpdate::match_price`] / [`OrderUpdate::match_size`] /
//! [`OrderUpdate::trade_id`] (`None` on non-trade events) — so even a market
//! order (whose `price` is `0`) reports the price it actually filled at.
//!
//! Like the other connectors, [`OrderUpdate`]'s `size` family is `u32`;
//! fractional spot quantities are truncated while the fill price stays exact via
//! `match_price`. The single-asset `balanceUpdate` *delta* event is intentionally
//! not mapped (it isn't an available/hold snapshot).
//!
//! [`BinanceUserDataRest`]: crate::binance::BinanceUserDataRest
//!
//! ```no_run
//! # use exchange_apiws::binance::{BinanceUserDataRest, BinanceUserDataConnector};
//! # async fn example() -> exchange_apiws::Result<()> {
//! let rest = BinanceUserDataRest::new(std::env::var("BINANCE_API_KEY").unwrap())?;
//! let listen_key = rest.create_listen_key().await?;
//! let connector = BinanceUserDataConnector::new(&listen_key);
//! // hand `connector` to the WS runner; PUT-keepalive the listenKey every ~30 min.
//! # Ok(())
//! # }
//! ```

use serde_json::Value;

use crate::actors::{
    BalanceUpdate, DataMessage, ExchangeConnector, OrderUpdate, TradeSide, WebSocketConfig,
};
use crate::error::Result;

const EXCHANGE_NAME: &str = "binance";
const SPOT_WS_BASE: &str = "wss://stream.binance.com:9443";
/// Binance sends WS-level pings ~every 3 min and expects a pong within 10 min;
/// that's handled at the transport layer, so no app-level ping frame is needed.
/// This interval just bounds the runner's idle keepalive.
const PING_INTERVAL_SECS: u64 = 180;

/// Binance spot user-data connector: `executionReport` → `OrderUpdate`,
/// `outboundAccountPosition` → `BalanceUpdate`.
pub struct BinanceUserDataConnector {
    url: String,
}

impl BinanceUserDataConnector {
    /// Build a connector for the given `listenKey` (mainnet spot user-data).
    pub fn new(listen_key: &str) -> Self {
        Self {
            url: format!("{SPOT_WS_BASE}/ws/{listen_key}"),
        }
    }

    /// Build against a caller-supplied base (testnet / a mock server); the
    /// `/ws/<listenKey>` path is appended.
    pub fn with_base(listen_key: &str, base: &str) -> Self {
        Self {
            url: format!("{base}/ws/{listen_key}"),
        }
    }
}

impl ExchangeConnector for BinanceUserDataConnector {
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
            ping_interval_secs: PING_INTERVAL_SECS,
            reconnect_delay_secs: 5,
            max_reconnect_attempts: 5,
        }
    }

    /// Binance user-data needs no subscription — events stream automatically
    /// once connected to the `listenKey` URL.
    fn subscription_message(&self, _symbol: &str) -> Option<String> {
        None
    }

    fn parse_message(&self, raw: &str) -> Result<Vec<DataMessage>> {
        let json: Value = serde_json::from_str(raw)?;
        let out = match json.get("e").and_then(Value::as_str) {
            Some("executionReport") => parse_execution_report(&json)
                .map(DataMessage::OrderUpdate)
                .into_iter()
                .collect(),
            Some("outboundAccountPosition") => parse_account_position(&json),
            _ => vec![],
        };
        Ok(out)
    }
}

/// Parse a spot `executionReport` event into an [`OrderUpdate`].
fn parse_execution_report(d: &Value) -> Option<OrderUpdate> {
    let symbol = d.get("s")?.as_str()?.to_string();
    let size = str_f64(d, "q") as u32;
    let filled_size = str_f64(d, "z") as u32;
    let is_trade = d.get("x").and_then(Value::as_str) == Some("TRADE");
    Some(OrderUpdate {
        symbol,
        exchange: EXCHANGE_NAME.to_string(),
        order_id: i64_num(d, "i").map(|i| i.to_string()).unwrap_or_default(),
        client_oid: nonempty(str_field(d, "c")),
        side: side_of(d),
        order_type: d
            .get("o")
            .and_then(Value::as_str)
            .unwrap_or("MARKET")
            .to_lowercase(),
        status: map_status(d.get("X").and_then(Value::as_str).unwrap_or("")),
        price: str_f64(d, "p"),
        size,
        filled_size,
        remaining_size: size.saturating_sub(filled_size),
        fee: str_f64(d, "n"),
        // Fill details ride only on `x == "TRADE"` events.
        match_price: is_trade.then(|| str_f64(d, "L")),
        match_size: is_trade.then(|| str_f64(d, "l") as u32),
        // `t` is -1 when the report isn't a trade.
        trade_id: i64_num(d, "t").filter(|&t| t >= 0).map(|t| t.to_string()),
        exchange_ts: i64_num(d, "T")
            .or_else(|| i64_num(d, "E"))
            .unwrap_or_else(now_ms),
        receipt_ts: now_ms(),
    })
}

/// Parse an `outboundAccountPosition` snapshot into one [`BalanceUpdate`] per
/// asset in the `B` array.
fn parse_account_position(d: &Value) -> Vec<DataMessage> {
    let exchange_ts = i64_num(d, "E").unwrap_or_else(now_ms);
    let receipt_ts = now_ms();
    let Some(balances) = d.get("B").and_then(Value::as_array) else {
        return vec![];
    };
    balances
        .iter()
        .filter_map(|b| {
            let currency = nonempty(str_field(b, "a"))?;
            Some(DataMessage::BalanceUpdate(BalanceUpdate {
                exchange: EXCHANGE_NAME.to_string(),
                currency,
                available_balance: str_f64(b, "f"),
                hold_balance: str_f64(b, "l"),
                event: "outboundAccountPosition".to_string(),
                exchange_ts,
                receipt_ts,
            }))
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

/// Parse a Binance string-decimal field (`"123.45"`) to f64; `0.0` if absent.
fn str_f64(d: &Value, key: &str) -> f64 {
    d.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

/// Read a JSON-number i64 field — Binance sends ids and timestamps as numbers,
/// unlike its string-typed decimal amounts.
fn i64_num(d: &Value, key: &str) -> Option<i64> {
    d.get(key).and_then(Value::as_i64)
}

fn side_of(d: &Value) -> TradeSide {
    match d.get("S").and_then(Value::as_str).unwrap_or("BUY") {
        s if s.eq_ignore_ascii_case("sell") => TradeSide::Sell,
        _ => TradeSide::Buy,
    }
}

/// Map Binance's `X` (current order status) to the crate's `open` /
/// `partialFilled` / `filled` / `canceled` vocabulary.
fn map_status(status: &str) -> String {
    match status {
        "PARTIALLY_FILLED" => "partialFilled",
        "FILLED" => "filled",
        "CANCELED" | "REJECTED" | "EXPIRED" | "PENDING_CANCEL" => "canceled",
        // NEW / anything else → resting.
        _ => "open",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector() -> BinanceUserDataConnector {
        BinanceUserDataConnector::new("test-listen-key")
    }

    #[test]
    fn ws_url_embeds_listen_key() {
        let c = BinanceUserDataConnector::new("abc123");
        assert_eq!(c.ws_url(), "wss://stream.binance.com:9443/ws/abc123");
        let c2 = BinanceUserDataConnector::with_base("k", "ws://127.0.0.1:9001");
        assert_eq!(c2.ws_url(), "ws://127.0.0.1:9001/ws/k");
    }

    #[test]
    fn no_subscription_message() {
        assert!(connector().subscription_message("BTCUSDT").is_none());
    }

    #[test]
    fn execution_report_new_order_maps_to_open_order_update() {
        let c = connector();
        let raw = r#"{
            "e":"executionReport","E":1700000000000,"s":"BTCUSDT","c":"my-oid",
            "S":"BUY","o":"LIMIT","f":"GTC","q":"2.000","p":"30000.50",
            "X":"NEW","x":"NEW","i":123456,"l":"0","z":"0","L":"0","n":"0",
            "N":null,"T":1700000000001,"t":-1
        }"#;
        let msgs = c.parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        let DataMessage::OrderUpdate(o) = &msgs[0] else {
            panic!("expected OrderUpdate, got {:?}", msgs[0]);
        };
        assert_eq!(o.symbol, "BTCUSDT");
        assert_eq!(o.exchange, "binance");
        assert_eq!(o.order_id, "123456");
        assert_eq!(o.client_oid.as_deref(), Some("my-oid"));
        assert_eq!(o.side, TradeSide::Buy);
        assert_eq!(o.order_type, "limit");
        assert_eq!(o.status, "open");
        assert!((o.price - 30000.50).abs() < 1e-9);
        assert_eq!(o.size, 2);
        assert_eq!(o.filled_size, 0);
        assert_eq!(o.remaining_size, 2);
        assert_eq!(o.match_price, None, "NEW is not a TRADE");
        assert_eq!(o.trade_id, None, "t=-1 → no trade id");
        assert_eq!(o.exchange_ts, 1_700_000_000_001, "T (transaction time)");
    }

    #[test]
    fn execution_report_trade_carries_match_details() {
        let c = connector();
        let raw = r#"{
            "e":"executionReport","E":1700000005000,"s":"ETHUSDT","c":"",
            "S":"SELL","o":"MARKET","q":"10","p":"0","X":"PARTIALLY_FILLED",
            "x":"TRADE","i":999,"l":"4","z":"4","L":"2500.25","n":"0.01",
            "N":"USDT","T":1700000005002,"t":77
        }"#;
        let DataMessage::OrderUpdate(o) = &c.parse_message(raw).unwrap()[0] else {
            panic!("expected OrderUpdate");
        };
        assert_eq!(o.symbol, "ETHUSDT");
        assert_eq!(o.client_oid, None, "empty c → None");
        assert_eq!(o.side, TradeSide::Sell);
        assert_eq!(o.status, "partialFilled");
        // The true fill price/size/id ride on the match_* fields.
        assert!((o.match_price.unwrap() - 2500.25).abs() < 1e-9);
        assert_eq!(o.match_size, Some(4));
        assert_eq!(o.filled_size, 4);
        assert_eq!(o.remaining_size, 6, "q=10 − z=4");
        assert_eq!(o.trade_id.as_deref(), Some("77"));
        assert!((o.fee - 0.01).abs() < 1e-9);
        assert_eq!(o.exchange_ts, 1_700_000_005_002);
    }

    #[test]
    fn account_position_fans_out_one_balance_per_asset() {
        let c = connector();
        let raw = r#"{
            "e":"outboundAccountPosition","E":1700000010000,"u":1700000010000,
            "B":[
                {"a":"USDT","f":"1000.5","l":"250.25"},
                {"a":"BTC","f":"0.5","l":"0"}
            ]
        }"#;
        let msgs = c.parse_message(raw).unwrap();
        assert_eq!(msgs.len(), 2);
        let DataMessage::BalanceUpdate(usdt) = &msgs[0] else {
            panic!("expected BalanceUpdate, got {:?}", msgs[0]);
        };
        assert_eq!(usdt.exchange, "binance");
        assert_eq!(usdt.currency, "USDT");
        assert!((usdt.available_balance - 1000.5).abs() < 1e-9);
        assert!((usdt.hold_balance - 250.25).abs() < 1e-9);
        assert_eq!(usdt.event, "outboundAccountPosition");
        assert_eq!(usdt.exchange_ts, 1_700_000_010_000);
        let DataMessage::BalanceUpdate(btc) = &msgs[1] else {
            panic!("expected BalanceUpdate");
        };
        assert_eq!(btc.currency, "BTC");
        assert!((btc.available_balance - 0.5).abs() < 1e-9);
    }

    #[test]
    fn unknown_event_is_ignored() {
        let c = connector();
        // `balanceUpdate` is a single-asset delta — intentionally not mapped.
        let raw =
            r#"{"e":"balanceUpdate","E":1700000000000,"a":"BTC","d":"0.1","T":1700000000000}"#;
        assert!(c.parse_message(raw).unwrap().is_empty());
    }
}
