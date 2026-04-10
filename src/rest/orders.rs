//! Order management — place, close, cancel, and contract sizing.
//!
//! Mirrors Python `place_order`, `close_position`, `calc_contracts`.

use serde::Deserialize;
use serde_json::json;
use tracing::info;
use uuid::Uuid;

use crate::client::KuCoinClient;
use crate::error::Result;
use crate::types::{OrderType, Side, contract_value};

// ── Response types ─────────────────────────────────────────────────────────────

/// Minimal order placement response — only `orderId` is guaranteed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderResponse {
    pub order_id: String,
}

// ── Sizing utility ─────────────────────────────────────────────────────────────

/// Calculate the number of contracts to open given account parameters.
///
/// # Arguments
/// - `symbol`        — KuCoin futures symbol (e.g. `"XBTUSDTM"`).
/// - `price`         — Current market price.
/// - `balance`       — Available account balance in quote currency (USDT or XBT).
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
    /// `leverage` and `time_in_force` are passed through to KuCoin as-is.
    /// Defaults to `"GTC"` if `time_in_force` is `None`.
    pub async fn place_order(
        &self,
        symbol: &str,
        side: Side,
        size: u32,
        leverage: u32,
        order_type: OrderType,
        time_in_force: Option<&str>,
        reduce_only: bool,
    ) -> Result<OrderResponse> {
        let tif = time_in_force.unwrap_or("GTC");

        let body = json!({
            "clientOid":   Uuid::new_v4().to_string(),
            "side":        side.as_str(),
            "symbol":      symbol,
            "type":        order_type.as_str(),
            "size":        size,
            "leverage":    leverage.to_string(),
            "timeInForce": tif,
            "reduceOnly":  reduce_only,
        });

        info!(
            symbol, side = ?side, size, leverage,
            order_type = order_type.as_str(), tif, reduce_only,
            "placing order"
        );
        self.post("/api/v1/orders", &body).await
    }

    /// Close an existing position with a market order.
    ///
    /// `qty` is the current position size: positive = long, negative = short.
    /// Pass the value from [`crate::rest::account::PositionInfo::current_qty`].
    pub async fn close_position(
        &self,
        symbol: &str,
        qty: i32,
        leverage: u32,
    ) -> Result<OrderResponse> {
        if qty == 0 {
            return Err(crate::error::BotError::Order(
                "qty is 0 — nothing to close".into(),
            ));
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
        self.delete(&format!("/api/v1/orders?symbol={symbol}"))
            .await
    }
}
