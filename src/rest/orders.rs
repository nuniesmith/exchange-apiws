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
    /// Exchange-assigned order identifier.
    pub order_id: String,
}

/// Full order detail returned by GET /api/v1/orders/{orderId}.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderDetail {
    /// Exchange-assigned order identifier.
    pub id: String,
    /// Instrument symbol.
    pub symbol: String,
    /// Order side — `"buy"` or `"sell"`.
    pub side: String,
    #[serde(rename = "type")]
    /// Order type — `"market"` or `"limit"`.
    pub order_type: String,
    /// `"active"` or `"done"`.
    pub status: String,
    /// Limit price (absent for market orders).
    pub price: Option<f64>,
    /// Total order quantity in contracts.
    pub size: u32,
    /// Number of contracts filled.
    pub filled_size: Option<u32>,
    /// Number of contracts still open.
    pub remaining_size: Option<u32>,
    /// Leverage specified at order placement.
    pub leverage: Option<String>,
    /// `true` if this order can only reduce an open position.
    pub reduce_only: Option<bool>,
    /// Time-in-force policy (e.g. `"GTC"`).
    pub time_in_force: Option<String>,
    /// Unix timestamp when the order was created (milliseconds).
    pub created_at: Option<i64>,
    /// Unix timestamp of the last status update (milliseconds).
    pub updated_at: Option<i64>,
}

impl OrderDetail {
    /// Returns `true` if the order is still resting on the book.
    ///
    /// KuCoin sets `status` to `"done"` once an order is fully filled,
    /// cancelled, or expired. Any other value is treated as active.
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }

    /// Returns `true` if the order is fully filled.
    ///
    /// An order is considered filled when its `filled_size` equals `size`
    /// (all contracts matched) regardless of the `status` string.
    pub fn is_filled(&self) -> bool {
        self.filled_size.map_or(false, |f| f >= self.size)
    }
}

/// Single trade fill from GET /api/v1/recentFills.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fill {
    /// Instrument symbol.
    pub symbol: String,
    /// Order that generated this fill.
    pub order_id: String,
    /// Fill side — `"buy"` or `"sell"`.
    pub side: String,
    /// Execution price.
    pub price: f64,
    /// Quantity filled in contracts.
    pub size: u32,
    /// Fee charged for this fill.
    pub fee: f64,
    /// Currency the fee was charged in.
    pub fee_currency: Option<String>,
    /// `"maker"` or `"taker"` — determines fee tier.
    pub liquidity: Option<String>,
    /// Exchange-assigned trade identifier.
    pub trade_id: Option<String>,
    /// Unix timestamp when the fill occurred (milliseconds).
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
    ///
    /// For **limit orders** `price` must be `Some(limit_price)`.  
    /// For **market orders** `price` should be `None` (the field is omitted from the request body).
    #[allow(clippy::similar_names, clippy::too_many_arguments)] // `side` and `size` are the correct public API parameter names
    pub async fn place_order(
        &self,
        symbol: &str,
        side: Side,
        size: u32,
        leverage: u32,
        order_type: OrderType,
        price: Option<f64>,
        time_in_force: Option<TimeInForce>,
        reduce_only: bool,
        stp: Option<STP>,
    ) -> Result<OrderResponse> {
        // Validate: KuCoin will reject a limit order without a price.
        if order_type == OrderType::Limit && price.is_none() {
            return Err(ExchangeError::Order(
                "place_order: price is required for limit orders".into(),
            ));
        }

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
        if let Some(p) = price {
            body["price"] = json!(p.to_string());
        }
        if let Some(s) = stp {
            body["stp"] = json!(s.as_str());
        }
        info!(
            symbol, side = ?side, size, leverage,
            order_type = order_type.as_str(), tif, reduce_only,
            price = ?price,
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
        let abs_qty = qty.unsigned_abs();
        let body = json!({
            "clientOid":   Uuid::new_v4().to_string(),
            "side":        side.as_str(),
            "symbol":      symbol,
            "type":        "market",
            "size":        abs_qty,
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
        self.delete(&format!("/api/v1/orders?symbol={symbol}"))
            .await
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
            .get(
                "/api/v1/orders",
                &[("status", "active"), ("symbol", symbol)],
            )
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
    #[allow(clippy::similar_names)] // `side` and `size` are the correct public API parameter names
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
        self.delete(&format!("/api/v1/stopOrders?symbol={symbol}"))
            .await
    }
}
