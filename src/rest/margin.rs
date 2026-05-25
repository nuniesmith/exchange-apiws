//! KuCoin spot-margin orders.
//!
//! Spot-margin trading reuses the standard KuCoin Spot base URL
//! (`api.kucoin.com`) and HMAC-SHA256 signing. The API surface is parallel
//! to the futures order endpoints in [`crate::rest::orders`] but the wire
//! shapes differ:
//!
//! - Sizes / prices are **string-encoded base-asset amounts** (e.g.
//!   `"0.01"` BTC) rather than the integer contract counts futures use.
//! - The order body carries `marginModel` (`"cross"` or `"isolated"`) and
//!   optional `autoBorrow` semantics for leverage-via-borrow.
//!
//! For account-state queries prefer the v3 endpoints exposed in
//! [`crate::rest::uta`] — [`KuCoinClient::get_cross_margin_accounts`] and
//! [`KuCoinClient::get_isolated_margin_accounts`]; the v1
//! `get_margin_balance` exposed here is kept for callers that still
//! depend on the older response shape.

use serde::Deserialize;
use serde_json::json;
use tracing::info;
use uuid::Uuid;

use crate::client::KuCoinClient;
use crate::error::Result;
use crate::types::{OrderType, Side, TimeInForce};

// ── Response types ───────────────────────────────────────────────────────────

/// Response from `POST /api/v1/margin/order`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarginOrderResponse {
    /// Exchange-assigned order identifier.
    pub order_id: String,
    /// Amount that was auto-borrowed to fund this order, if any
    /// (`"0"` when `auto_borrow` was false or no borrow was needed).
    #[serde(default)]
    pub borrow_size: Option<String>,
    /// ID of the borrow record created by `auto_borrow`, when applicable.
    #[serde(default)]
    pub loan_apply_id: Option<String>,
}

/// Full margin-order detail returned by `GET /api/v1/margin/orders/{id}`.
///
/// Sizes and prices are kept as the raw KuCoin string shape (base-asset
/// amounts like `"0.01"`); convert to `f64` at the call site if needed.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarginOrderDetail {
    /// Exchange-assigned order identifier.
    pub id: String,
    /// Trading pair (e.g. `"BTC-USDT"`).
    pub symbol: String,
    /// Order side — `"buy"` or `"sell"`.
    pub side: String,
    /// `"limit"` or `"market"`.
    #[serde(rename = "type")]
    pub order_type: String,
    /// Order size in base asset (e.g. `"0.01"` BTC).
    pub size: String,
    /// Limit price (`"0"` for market orders).
    pub price: String,
    /// Quote funds spent / received (alternative to `size` for market orders).
    #[serde(default)]
    pub funds: Option<String>,
    /// Cumulative quantity filled in base asset.
    #[serde(default)]
    pub deal_size: Option<String>,
    /// Cumulative funds matched (quote asset).
    #[serde(default)]
    pub deal_funds: Option<String>,
    /// `"cross"` or `"isolated"`.
    #[serde(default)]
    pub margin_model: Option<String>,
    /// Time-in-force policy (`"GTC"`, `"IOC"`, …).
    #[serde(default)]
    pub time_in_force: Option<String>,
    /// `true` while the order is open on the book.
    #[serde(default)]
    pub is_active: bool,
    /// `true` after the order has been cancelled.
    #[serde(default)]
    pub cancel_exist: bool,
    /// Creation timestamp (ms since epoch).
    #[serde(default)]
    pub created_at: Option<i64>,
}

impl MarginOrderDetail {
    /// Convenience: parse `size` as `f64`. Returns `0.0` on a malformed
    /// string (callers wanting strict handling should read `self.size`
    /// directly).
    #[must_use]
    pub fn size_f64(&self) -> f64 {
        self.size.parse().unwrap_or(0.0)
    }

    /// Convenience: parse `price` as `f64`.
    #[must_use]
    pub fn price_f64(&self) -> f64 {
        self.price.parse().unwrap_or(0.0)
    }
}

/// Response from `DELETE /api/v1/margin/orders/{id}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelMarginOrderResponse {
    /// Order IDs that were actually cancelled (one entry for a single-order
    /// cancel; empty if the order had already completed).
    #[serde(default)]
    pub cancelled_order_ids: Vec<String>,
}

/// A single fill returned by `GET /api/v1/margin/fills`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarginFill {
    /// Trading pair.
    pub symbol: String,
    /// Order that generated this fill.
    pub order_id: String,
    /// `"buy"` or `"sell"`.
    pub side: String,
    /// Execution price (base asset).
    pub price: String,
    /// Filled quantity (base asset).
    pub size: String,
    /// Matched funds (quote asset = price × size).
    #[serde(default)]
    pub funds: Option<String>,
    /// Fee charged for this fill.
    pub fee: String,
    /// Currency the fee was charged in.
    #[serde(default)]
    pub fee_currency: Option<String>,
    /// `"maker"` or `"taker"`.
    #[serde(default)]
    pub liquidity: Option<String>,
    /// Exchange-assigned trade identifier.
    #[serde(default)]
    pub trade_id: Option<String>,
    /// Fill timestamp (ms since epoch).
    #[serde(default)]
    pub created_at: Option<i64>,
}

/// v1 margin account balance returned by `GET /api/v1/margin/account`.
///
/// Prefer [`KuCoinClient::get_cross_margin_accounts`] (v3) for new code;
/// this is kept for callers that still depend on the v1 wire shape.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarginAccountV1 {
    /// `liability / asset` — `"0"` when no borrows are outstanding.
    #[serde(default)]
    pub debt_ratio: Option<String>,
    /// Account-level status (e.g. `"EFFECTIVE"`).
    #[serde(default)]
    pub status: Option<String>,
    /// Per-currency slots.
    #[serde(default)]
    pub accounts: Vec<MarginAccountAssetV1>,
}

/// One currency's slot inside a [`MarginAccountV1`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarginAccountAssetV1 {
    /// Currency code (e.g. `"USDT"`).
    pub currency: String,
    /// Total balance for this currency.
    #[serde(default)]
    pub total_balance: Option<String>,
    /// Balance available for new orders / withdrawal / repayment.
    #[serde(default)]
    pub available_balance: Option<String>,
    /// Balance currently held against open orders.
    #[serde(default)]
    pub hold_balance: Option<String>,
    /// Outstanding borrowed principal.
    #[serde(default)]
    pub liability: Option<String>,
    /// Maximum additional borrow allowed.
    #[serde(default)]
    pub max_borrow_size: Option<String>,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Margin-trading mode — passed as the `marginModel` body field on
/// [`KuCoinClient::place_margin_order`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarginModel {
    /// Shared margin pool across all margin pairs.
    Cross,
    /// Per-pair isolated margin (borrows scoped to one symbol).
    Isolated,
}

impl MarginModel {
    /// Wire-format string passed to KuCoin's `marginModel` field.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cross => "cross",
            Self::Isolated => "isolated",
        }
    }
}

// ── KuCoinClient methods ─────────────────────────────────────────────────────

impl KuCoinClient {
    /// `POST /api/v1/margin/order` — place a spot-margin order.
    ///
    /// `size` is in base-asset units (e.g. `0.01` for 0.01 BTC). For
    /// market orders pass `price = None`. When `auto_borrow` is `true`,
    /// KuCoin auto-borrows up to your max-borrow limit to fund the order
    /// — the `MarginOrderResponse.borrow_size` field reports what was
    /// actually borrowed.
    // `side` and `size` are conventional names lifted from KuCoin's API
    // docs; renaming for the similar-names lint would hurt readability.
    #[allow(clippy::too_many_arguments, clippy::similar_names)]
    pub async fn place_margin_order(
        &self,
        symbol: &str,
        side: Side,
        order_type: OrderType,
        size: f64,
        price: Option<f64>,
        margin_model: MarginModel,
        auto_borrow: bool,
        time_in_force: Option<TimeInForce>,
    ) -> Result<MarginOrderResponse> {
        let tif = time_in_force.unwrap_or_default().as_str();
        let mut body = json!({
            "clientOid":   Uuid::new_v4().to_string(),
            "side":        side.as_str(),
            "symbol":      symbol,
            "type":        order_type.as_str(),
            "size":        size.to_string(),
            "marginModel": margin_model.as_str(),
            "autoBorrow":  auto_borrow,
            "timeInForce": tif,
        });
        if let Some(p) = price {
            body["price"] = json!(p.to_string());
        }
        info!(
            symbol, side = ?side, size, auto_borrow,
            margin_model = margin_model.as_str(),
            order_type = order_type.as_str(),
            price = ?price,
            "placing margin order"
        );
        self.post("/api/v1/margin/order", &body).await
    }

    /// `GET /api/v1/margin/orders/{order_id}` — fetch a single margin order.
    pub async fn get_margin_order(&self, order_id: &str) -> Result<MarginOrderDetail> {
        self.get(&format!("/api/v1/margin/orders/{order_id}"), &[])
            .await
    }

    /// `DELETE /api/v1/margin/orders/{order_id}` — cancel a margin order.
    pub async fn cancel_margin_order(&self, order_id: &str) -> Result<CancelMarginOrderResponse> {
        info!(order_id, "cancelling margin order");
        self.delete(&format!("/api/v1/margin/orders/{order_id}"))
            .await
    }

    /// `GET /api/v1/margin/fills?symbol={symbol}` — recent margin fills
    /// for `symbol`.
    pub async fn get_margin_fills(&self, symbol: &str) -> Result<Vec<MarginFill>> {
        // KuCoin paginates this endpoint; for simplicity we surface the
        // first page directly. The full shape is `{"items":[...], ...}`.
        #[derive(Deserialize)]
        struct Page {
            items: Vec<MarginFill>,
        }
        let page: Page = self
            .get("/api/v1/margin/fills", &[("symbol", symbol)])
            .await?;
        Ok(page.items)
    }

    /// `GET /api/v1/margin/account` — legacy v1 margin-account state.
    ///
    /// Prefer [`Self::get_cross_margin_accounts`] (v3) for new code; this
    /// is provided for callers still depending on the v1 wire shape.
    pub async fn get_margin_balance(&self) -> Result<MarginAccountV1> {
        self.get("/api/v1/margin/account", &[]).await
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margin_order_response_with_borrow() {
        let raw = r#"{
            "orderId": "abc123",
            "borrowSize": "0.05",
            "loanApplyId": "loan-1"
        }"#;
        let r: MarginOrderResponse = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(r.order_id, "abc123");
        assert_eq!(r.borrow_size.as_deref(), Some("0.05"));
        assert_eq!(r.loan_apply_id.as_deref(), Some("loan-1"));
    }

    #[test]
    fn margin_order_response_without_borrow() {
        // auto_borrow=false → borrowSize "0", loanApplyId null
        let raw = r#"{"orderId": "abc", "borrowSize": "0", "loanApplyId": null}"#;
        let r: MarginOrderResponse = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(r.borrow_size.as_deref(), Some("0"));
        assert!(r.loan_apply_id.is_none());
    }

    #[test]
    fn margin_order_detail_parses_strings_and_helpers() {
        let raw = r#"{
            "id":"o-1","symbol":"BTC-USDT","side":"buy","type":"limit",
            "size":"0.01","price":"30000","dealSize":"0.005","dealFunds":"150",
            "marginModel":"cross","timeInForce":"GTC","isActive":true,
            "cancelExist":false,"createdAt":1700000000000
        }"#;
        let d: MarginOrderDetail = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(d.symbol, "BTC-USDT");
        assert!((d.size_f64() - 0.01).abs() < 1e-9);
        assert!((d.price_f64() - 30_000.0).abs() < 1e-9);
        assert!(d.is_active);
        assert!(!d.cancel_exist);
        assert_eq!(d.margin_model.as_deref(), Some("cross"));
    }

    #[test]
    fn cancel_response_extracts_ids() {
        let raw = r#"{"cancelledOrderIds":["o-1","o-2"]}"#;
        let r: CancelMarginOrderResponse = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(r.cancelled_order_ids, vec!["o-1", "o-2"]);
    }

    #[test]
    fn cancel_response_empty_when_already_done() {
        // KuCoin returns an empty list if the order had already filled or
        // been cancelled by another client — must round-trip cleanly.
        let raw = r#"{"cancelledOrderIds":[]}"#;
        let r: CancelMarginOrderResponse = serde_json::from_str(raw).expect("deserialize");
        assert!(r.cancelled_order_ids.is_empty());
    }

    #[test]
    fn margin_account_v1_with_assets() {
        let raw = r#"{
            "debtRatio":"0",
            "status":"EFFECTIVE",
            "accounts":[{
                "currency":"USDT",
                "totalBalance":"100",
                "availableBalance":"100",
                "holdBalance":"0",
                "liability":"0",
                "maxBorrowSize":"50"
            }]
        }"#;
        let a: MarginAccountV1 = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(a.status.as_deref(), Some("EFFECTIVE"));
        assert_eq!(a.accounts.len(), 1);
        assert_eq!(a.accounts[0].currency, "USDT");
        assert_eq!(a.accounts[0].available_balance.as_deref(), Some("100"));
    }

    #[test]
    fn margin_model_wire_format() {
        assert_eq!(MarginModel::Cross.as_str(), "cross");
        assert_eq!(MarginModel::Isolated.as_str(), "isolated");
    }
}
