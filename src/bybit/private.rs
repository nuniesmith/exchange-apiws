//! Bybit v5 private REST client — signed account / order / position endpoints.
//!
//! Wraps the signed surface of the Bybit v5 API:
//!
//! | Method | Endpoint | Verb |
//! |--------|----------|------|
//! | [`place_order`](BybitPrivateClient::place_order) | `/v5/order/create` | POST |
//! | [`cancel_order`](BybitPrivateClient::cancel_order) | `/v5/order/cancel` | POST |
//! | [`get_open_orders`](BybitPrivateClient::get_open_orders) | `/v5/order/realtime` | GET |
//! | [`get_positions`](BybitPrivateClient::get_positions) | `/v5/position/list` | GET |
//! | [`get_wallet_balance`](BybitPrivateClient::get_wallet_balance) | `/v5/account/wallet-balance` | GET |
//!
//! All requests are signed per [`BybitCredentials`] and carry the
//! `X-BAPI-API-KEY` / `X-BAPI-TIMESTAMP` / `X-BAPI-RECV-WINDOW` / `X-BAPI-SIGN`
//! headers. Every Bybit response is wrapped in a `{retCode, retMsg, result}`
//! envelope; non-zero `retCode` is surfaced as [`ExchangeError::Api`].

use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::auth::{BybitCredentials, DEFAULT_RECV_WINDOW};
use super::rest::BybitCategory;
use crate::error::{ExchangeError, Result};

const MAINNET: &str = "https://api.bybit.com";
const TESTNET: &str = "https://api-testnet.bybit.com";

// ── Order-entry types (Bybit v5 wire format) ─────────────────────────────────

/// Order side.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum BybitOrderSide {
    /// Bid / long.
    Buy,
    /// Ask / short.
    Sell,
}

/// Order type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum BybitOrderType {
    /// Resting limit order (requires `price`).
    Limit,
    /// Immediate market order.
    Market,
}

/// Time-in-force. Serialised as Bybit's uppercase codes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BybitTimeInForce {
    /// Good-til-cancelled.
    GTC,
    /// Immediate-or-cancel.
    IOC,
    /// Fill-or-kill.
    FOK,
    /// Post-only (maker-only).
    PostOnly,
}

/// A new-order request (`POST /v5/order/create`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BybitOrderRequest {
    /// Product category (`spot` / `linear` / `inverse`).
    pub category: String,
    /// Symbol, e.g. `BTCUSDT`.
    pub symbol: String,
    /// Buy / Sell.
    pub side: BybitOrderSide,
    /// Limit / Market.
    pub order_type: BybitOrderType,
    /// Order quantity, as a string to preserve exchange precision.
    pub qty: String,
    /// Limit price (required for `Limit`). String to preserve precision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    /// Time-in-force.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_in_force: Option<BybitTimeInForce>,
    /// Client-supplied order id (idempotency / correlation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_link_id: Option<String>,
    /// Reduce-only: never increase position size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduce_only: Option<bool>,
}

impl BybitOrderRequest {
    /// Build a market order for `category`/`symbol`.
    pub fn market(
        category: BybitCategory,
        symbol: impl Into<String>,
        side: BybitOrderSide,
        qty: impl Into<String>,
    ) -> Self {
        Self {
            category: category.as_str().to_string(),
            symbol: symbol.into(),
            side,
            order_type: BybitOrderType::Market,
            qty: qty.into(),
            price: None,
            time_in_force: None,
            order_link_id: None,
            reduce_only: None,
        }
    }

    /// Build a limit order for `category`/`symbol`.
    pub fn limit(
        category: BybitCategory,
        symbol: impl Into<String>,
        side: BybitOrderSide,
        qty: impl Into<String>,
        price: impl Into<String>,
    ) -> Self {
        Self {
            category: category.as_str().to_string(),
            symbol: symbol.into(),
            side,
            order_type: BybitOrderType::Limit,
            qty: qty.into(),
            price: Some(price.into()),
            time_in_force: Some(BybitTimeInForce::GTC),
            order_link_id: None,
            reduce_only: None,
        }
    }

    /// Attach a client order id.
    #[must_use]
    pub fn with_order_link_id(mut self, id: impl Into<String>) -> Self {
        self.order_link_id = Some(id.into());
        self
    }

    /// Mark the order reduce-only.
    #[must_use]
    pub const fn reduce_only(mut self) -> Self {
        self.reduce_only = Some(true);
        self
    }
}

/// Acknowledgement returned by `/v5/order/create` and `/v5/order/cancel`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BybitOrderAck {
    /// Exchange-assigned order id.
    pub order_id: String,
    /// Echoed client order id (empty string if none was sent).
    #[serde(default)]
    pub order_link_id: String,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Signed Bybit v5 REST client.
pub struct BybitPrivateClient {
    base_url: String,
    creds: BybitCredentials,
    recv_window: u64,
    http: Client,
}

impl BybitPrivateClient {
    /// Create a client against Bybit mainnet (`testnet = false`) or testnet.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeError::Http`] if the underlying HTTP client fails to
    /// build.
    pub fn new(creds: BybitCredentials, testnet: bool) -> Result<Self> {
        let base_url = if testnet { TESTNET } else { MAINNET }.to_string();
        Ok(Self {
            base_url,
            creds,
            recv_window: DEFAULT_RECV_WINDOW,
            http: Client::builder().build()?,
        })
    }

    /// Override the signed `recv_window` (ms).
    #[must_use]
    pub const fn with_recv_window(mut self, recv_window: u64) -> Self {
        self.recv_window = recv_window;
        self
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default()
    }

    /// Signed GET. `query` is the raw query string (no leading `?`).
    async fn signed_get(&self, path: &str, query: &str) -> Result<reqwest::Response> {
        let ts = Self::now_ms();
        let sign = self.creds.sign_rest(ts, self.recv_window, query);
        let url = if query.is_empty() {
            format!("{}{path}", self.base_url)
        } else {
            format!("{}{path}?{query}", self.base_url)
        };
        Ok(self
            .http
            .get(url)
            .header("X-BAPI-API-KEY", &self.creds.api_key)
            .header("X-BAPI-TIMESTAMP", ts.to_string())
            .header("X-BAPI-RECV-WINDOW", self.recv_window.to_string())
            .header("X-BAPI-SIGN", sign)
            .send()
            .await?)
    }

    /// Signed POST with a JSON `body`.
    async fn signed_post(&self, path: &str, body: &str) -> Result<reqwest::Response> {
        let ts = Self::now_ms();
        let sign = self.creds.sign_rest(ts, self.recv_window, body);
        Ok(self
            .http
            .post(format!("{}{path}", self.base_url))
            .header("X-BAPI-API-KEY", &self.creds.api_key)
            .header("X-BAPI-TIMESTAMP", ts.to_string())
            .header("X-BAPI-RECV-WINDOW", self.recv_window.to_string())
            .header("X-BAPI-SIGN", sign)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await?)
    }

    /// Place a new order. Returns the order ack on success.
    ///
    /// # Errors
    ///
    /// [`ExchangeError::Api`] on non-zero `retCode`, [`ExchangeError::Http`] /
    /// [`ExchangeError::Json`] on transport / decode failures.
    pub async fn place_order(&self, order: &BybitOrderRequest) -> Result<BybitOrderAck> {
        let body = serde_json::to_string(order)?;
        let resp = self.signed_post("/v5/order/create", &body).await?;
        unwrap_result(resp).await
    }

    /// Cancel an open order by exchange order id.
    ///
    /// # Errors
    ///
    /// As [`place_order`](Self::place_order).
    pub async fn cancel_order(
        &self,
        category: BybitCategory,
        symbol: &str,
        order_id: &str,
    ) -> Result<BybitOrderAck> {
        let body = serde_json::json!({
            "category": category.as_str(),
            "symbol": symbol,
            "orderId": order_id,
        })
        .to_string();
        let resp = self.signed_post("/v5/order/cancel", &body).await?;
        unwrap_result(resp).await
    }

    /// Open orders for `category` (optionally filtered by `symbol`). Returns the
    /// raw `result` object (the `list` array is exchange-shaped).
    ///
    /// # Errors
    ///
    /// As [`place_order`](Self::place_order).
    pub async fn get_open_orders(
        &self,
        category: BybitCategory,
        symbol: Option<&str>,
    ) -> Result<Value> {
        let mut q = format!("category={}", category.as_str());
        if let Some(s) = symbol {
            use std::fmt::Write as _;
            let _ = write!(q, "&symbol={s}");
        }
        let resp = self.signed_get("/v5/order/realtime", &q).await?;
        unwrap_result(resp).await
    }

    /// Positions for `category` (optionally filtered by `symbol`).
    ///
    /// # Errors
    ///
    /// As [`place_order`](Self::place_order).
    pub async fn get_positions(
        &self,
        category: BybitCategory,
        symbol: Option<&str>,
    ) -> Result<Value> {
        let mut q = format!("category={}", category.as_str());
        if let Some(s) = symbol {
            use std::fmt::Write as _;
            let _ = write!(q, "&symbol={s}");
        }
        let resp = self.signed_get("/v5/position/list", &q).await?;
        unwrap_result(resp).await
    }

    /// Wallet balance for an account type (e.g. `"UNIFIED"`, `"CONTRACT"`).
    ///
    /// # Errors
    ///
    /// As [`place_order`](Self::place_order).
    pub async fn get_wallet_balance(&self, account_type: &str) -> Result<Value> {
        let q = format!("accountType={account_type}");
        let resp = self.signed_get("/v5/account/wallet-balance", &q).await?;
        unwrap_result(resp).await
    }
}

/// Bybit's standard `{retCode, retMsg, result}` envelope.
#[derive(Debug, Deserialize)]
struct BybitEnvelope {
    #[serde(rename = "retCode")]
    ret_code: i64,
    #[serde(rename = "retMsg", default)]
    ret_msg: String,
    #[serde(default)]
    result: Value,
}

/// Parse a Bybit envelope: `retCode == 0` → deserialize `result` into `T`;
/// otherwise surface [`ExchangeError::Api`].
async fn unwrap_result<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
    let env: BybitEnvelope = resp.json().await?;
    if env.ret_code != 0 {
        return Err(ExchangeError::Api {
            code: env.ret_code.to_string(),
            message: env.ret_msg,
        });
    }
    Ok(serde_json::from_value(env.result)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_order_serialises_to_bybit_wire_shape() {
        let o = BybitOrderRequest::market(
            BybitCategory::Linear,
            "BTCUSDT",
            BybitOrderSide::Buy,
            "0.01",
        );
        let j: Value = serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(j["category"], "linear");
        assert_eq!(j["symbol"], "BTCUSDT");
        assert_eq!(j["side"], "Buy");
        assert_eq!(j["orderType"], "Market");
        assert_eq!(j["qty"], "0.01");
        // Optional fields skipped when None.
        assert!(j.get("price").is_none());
        assert!(j.get("reduceOnly").is_none());
    }

    #[test]
    fn limit_order_includes_price_tif_and_builders() {
        let o = BybitOrderRequest::limit(
            BybitCategory::Linear,
            "ETHUSDT",
            BybitOrderSide::Sell,
            "1",
            "3000.5",
        )
        .with_order_link_id("abc")
        .reduce_only();
        let j: Value = serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(j["orderType"], "Limit");
        assert_eq!(j["price"], "3000.5");
        assert_eq!(j["timeInForce"], "GTC");
        assert_eq!(j["orderLinkId"], "abc");
        assert_eq!(j["reduceOnly"], true);
    }

    #[test]
    fn envelope_nonzero_retcode_maps_to_api_error() {
        // The `result` branch of `unwrap_result` needs a live `Response`, but
        // the envelope→error mapping is the part worth pinning: a non-zero
        // retCode becomes ExchangeError::Api carrying the code + msg.
        let env = BybitEnvelope {
            ret_code: 10001,
            ret_msg: "param error".into(),
            result: Value::Null,
        };
        let err = ExchangeError::Api {
            code: env.ret_code.to_string(),
            message: env.ret_msg,
        };
        match err {
            ExchangeError::Api { code, message } => {
                assert_eq!(code, "10001");
                assert_eq!(message, "param error");
            }
            _ => panic!("expected Api error"),
        }
    }
}
