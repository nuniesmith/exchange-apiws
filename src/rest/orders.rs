//! Order management — place, close, cancel, stop orders, fill history,
//! and contract sizing utility.

use serde::Deserialize;
use serde_json::json;
use tracing::info;
use uuid::Uuid;

use crate::client::KuCoinClient;
use crate::error::{ExchangeError, Result};
use crate::types::{OrderType, STP, Side, TimeInForce, contract_value};

// ── Response types ─────────────────────────────────────────────────────────────

/// Minimal order placement response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderResponse {
    pub order_id: String,
}

/// Full order detail returned by GET /api/v1/orders/{orderId}.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderDetail {
    pub id: String,
    pub symbol: String,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
    /// `"active"` or `"done"`.
    pub status: String,
    pub price: Option<f64>,
    pub size: u32,
    pub filled_size: Option<u32>,
    pub remaining_size: Option<u32>,
    pub leverage: Option<String>,
    pub reduce_only: Option<bool>,
    pub time_in_force: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

/// Single trade fill from GET /api/v1/recentFills.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fill {
    pub symbol: String,
    pub order_id: String,
    pub side: String,
    pub price: f64,
    pub size: u32,
    pub fee: f64,
    pub fee_currency: Option<String>,
    pub liquidity: Option<String>, // "maker" | "taker"
    pub trade_id: Option<String>,
    pub created_at: Option<i64>,
}

// ── Sizing utility ─────────────────────────────────────────────────────────────

/// Calculate the number of contracts to open given account parameters.
///
/// # Arguments
/// - `symbol`        — Futures symbol (e.g. `"XBTUSDTM"`).
/// - `price`         — Current market price.
/// - `balance`       — Available account balance in quote currency.
/// - `leverage`      — Desired leverage (e.g. `10`).
/// - `risk_fraction` — Fraction of `balance` to risk per trade (e.g. `0.02` = 2 %).
/// - `max_contracts` — Hard cap on position size.
///
/// Returns at least 1 contract.
pub fn calc_contracts(
    symbol: &str,
    price: f64,
    balance: f64,
    leverage: u32,
    risk_fraction: f64,
    max_contracts: u32,
) -> u32 {
    if price <= 0.0 {
        tracing::warn!(price, "calc_contracts: invalid price — defaulting to 1");
        return 1;
    }
    if leverage == 0 {
        tracing::warn!("calc_contracts: leverage is 0 — defaulting to 1");
        return 1;
    }
    let cv = contract_value(symbol);
    let notional_per_ct = price * cv;
    let margin_per_ct = notional_per_ct / f64::from(leverage);
    let raw = (balance * risk_fraction / margin_per_ct) as u32;
    raw.max(1).min(max_contracts)
}

// ── KuCoinClient methods ──────────────────────────────────────────────────────

impl KuCoinClient {
    /// Place a futures order.
    ///
    /// `leverage` is passed as a per-order field. `time_in_force` defaults to
    /// [`TimeInForce::GTC`] when `None`. Pass `stp` to enable Self-Trade Prevention.
    pub async fn place_order(
        &self,
        symbol: &str,
        side: Side,
        size: u32,
        leverage: u32,
        order_type: OrderType,
        time_in_force: Option<TimeInForce>,
        reduce_only: bool,
        stp: Option<STP>,
    ) -> Result<OrderResponse> {
        let tif = time_in_force.unwrap_or_default().as_str();
        let mut body = json!({
            "clientOid":   Uuid::new_v4().to_string(),
            "side":        side.as_str(),
            "symbol":      symbol,
            "type":        order_type.as_str(),
            "size":        size,
            "leverage":    leverage.to_string(),
            "timeInForce": tif,
            "reduceOnly":  reduce_only,
        });
        if let Some(s) = stp {
            body["stp"] = json!(s.as_str());
        }
        info!(
            symbol, side = ?side, size, leverage,
            order_type = order_type.as_str(), tif, reduce_only,
            "placing order"
        );
        self.post("/api/v1/orders", &body).await
    }

    /// Close an existing position with a market order.
    ///
    /// `qty` is the current position size — positive = long, negative = short.
    /// Use [`crate::rest::account::PositionInfo::current_qty`] as the source.
    pub async fn close_position(
        &self,
        symbol: &str,
        qty: i32,
        leverage: u32,
    ) -> Result<OrderResponse> {
        if qty == 0 {
            return Err(ExchangeError::Order("qty is 0 — nothing to close".into()));
        }
        let side = if qty > 0 { Side::Sell } else { Side::Buy };
        let size = qty.unsigned_abs();
        let body = json!({
            "clientOid":   Uuid::new_v4().to_string(),
            "side":        side.as_str(),
            "symbol":      symbol,
            "type":        "market",
            "size":        size,
            "leverage":    leverage.to_string(),
            "closeOrder":  true,
            "timeInForce": "GTC",
        });
        info!(symbol, qty, side = ?side, "closing position");
        self.post("/api/v1/orders", &body).await
    }

    /// Cancel a specific order by its KuCoin order ID.
    pub async fn cancel_order(&self, order_id: &str) -> Result<serde_json::Value> {
        info!(order_id, "cancelling order");
        self.delete(&format!("/api/v1/orders/{order_id}")).await
    }

    /// Cancel all open orders for a symbol.
    pub async fn cancel_all_orders(&self, symbol: &str) -> Result<serde_json::Value> {
        info!(symbol, "cancelling all open orders");
        self.delete(&format!("/api/v1/orders?symbol={symbol}")).await
    }

    /// Fetch all active (open) orders for a symbol.
    ///
    /// Endpoint: `GET /api/v1/orders?status=active&symbol={symbol}`
    pub async fn get_open_orders(&self, symbol: &str) -> Result<Vec<OrderDetail>> {
        #[derive(Deserialize)]
        struct Page {
            items: Vec<OrderDetail>,
        }
        let page: Page = self
            .get("/api/v1/orders", &[("status", "active"), ("symbol", symbol)])
            .await?;
        Ok(page.items)
    }

    /// Fetch a single order by its KuCoin order ID.
    ///
    /// Endpoint: `GET /api/v1/orders/{orderId}`
    pub async fn get_order(&self, order_id: &str) -> Result<OrderDetail> {
        self.get(&format!("/api/v1/orders/{order_id}"), &[]).await
    }

    /// Fetch recent trade fills (last 1,000 by default) for a symbol.
    ///
    /// Endpoint: `GET /api/v1/recentFills?symbol={symbol}`
    pub async fn get_recent_fills(&self, symbol: &str) -> Result<Vec<Fill>> {
        self.get("/api/v1/recentFills", &[("symbol", symbol)]).await
    }

    /// Place a stop order (stop-limit or stop-market).
    ///
    /// `stop_price` is the trigger price. `stop_type` is `"up"` (triggers when
    /// price rises to `stop_price`) or `"down"` (triggers when price falls to
    /// `stop_price`).
    ///
    /// Pass `price = None` for a stop-market order, or `Some(limit_price)` for a
    /// stop-limit.
    ///
    /// Endpoint: `POST /api/v1/stopOrders`
    pub async fn place_stop_order(
        &self,
        symbol: &str,
        side: Side,
        size: u32,
        leverage: u32,
        stop_price: f64,
        stop_type: &str,
        price: Option<f64>,
        reduce_only: bool,
    ) -> Result<OrderResponse> {
        let order_type = if price.is_some() { "limit" } else { "market" };
        let mut body = json!({
            "clientOid":  Uuid::new_v4().to_string(),
            "side":       side.as_str(),
            "symbol":     symbol,
            "type":       order_type,
            "size":       size,
            "leverage":   leverage.to_string(),
            "stop":       stop_type,
            "stopPrice":  stop_price.to_string(),
            "reduceOnly": reduce_only,
        });
        if let Some(lp) = price {
            body["price"] = json!(lp.to_string());
        }
        info!(symbol, side = ?side, size, stop_price, stop_type, "placing stop order");
        self.post("/api/v1/stopOrders", &body).await
    }

    /// Cancel a stop order by its order ID.
    ///
    /// Endpoint: `DELETE /api/v1/stopOrders/{orderId}`
    pub async fn cancel_stop_order(&self, order_id: &str) -> Result<serde_json::Value> {
        info!(order_id, "cancelling stop order");
        self.delete(&format!("/api/v1/stopOrders/{order_id}")).await
    }

    /// Cancel all stop orders for a symbol.
    ///
    /// Endpoint: `DELETE /api/v1/stopOrders?symbol={symbol}`
    pub async fn cancel_all_stop_orders(&self, symbol: &str) -> Result<serde_json::Value> {
        info!(symbol, "cancelling all stop orders");
        self.delete(&format!("/api/v1/stopOrders?symbol={symbol}")).await
    }
}
