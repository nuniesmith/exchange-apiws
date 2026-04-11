//! Account management — balance, positions, auto-deposit, risk limit, and funding history.
//!
//! # Leverage in KuCoin Futures
//!
//! KuCoin Futures does **not** have a standalone "set leverage" endpoint like
//! Binance. Leverage is specified per-order via the `leverage` field in the
//! order body. The methods here control the two account-level knobs that affect
//! how margin is managed:
//!
//! - [`set_auto_deposit`] — enable or disable automatic margin top-up when a
//!   position approaches liquidation.
//! - [`set_risk_limit_level`] — change the risk limit tier (1–N), which controls
//!   the maximum position size and the minimum maintenance margin rate. A higher
//!   level allows larger positions but requires proportionally more margin.

use serde::Deserialize;
use serde_json::json;
use tracing::{debug, info};

use crate::client::KuCoinClient;
use crate::error::Result;

// ── Response types ─────────────────────────────────────────────────────────────

/// Response from `GET /api/v1/account-overview`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountOverview {
    pub available_balance: f64,
    pub order_margin: Option<f64>,
    pub position_margin: Option<f64>,
    pub unrealised_pnl: Option<f64>,
}

/// Response from `GET /api/v1/position`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionInfo {
    /// Positive = long, negative = short, 0 = flat.
    pub current_qty: i32,
    pub symbol: String,
    pub avg_entry_price: Option<f64>,
    pub unrealised_pnl: Option<f64>,
    pub realised_pnl: Option<f64>,
    pub leverage: Option<f64>,
    pub is_open: Option<bool>,
    pub mark_price: Option<f64>,
    pub mark_value: Option<f64>,
    pub maintenance_margin: Option<f64>,
}

impl PositionInfo {
    pub fn is_flat(&self) -> bool {
        self.current_qty == 0
    }
    pub fn is_long(&self) -> bool {
        self.current_qty > 0
    }
    pub fn is_short(&self) -> bool {
        self.current_qty < 0
    }
}

/// A single funding payment record from `GET /api/v1/funding-history`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FundingRecord {
    pub id: Option<String>,
    pub symbol: String,
    pub time_point: Option<i64>,
    pub funding_rate: Option<f64>,
    pub mark_price: Option<f64>,
    pub position_qty: Option<i32>,
    pub position_cost: Option<f64>,
    pub funding: Option<f64>,
    pub settlement: Option<String>,
}

/// One risk limit tier returned by `GET /api/v1/contracts/risk-limit/{symbol}`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskLimitLevel {
    pub symbol: String,
    pub level: u32,
    pub max_risk_limit: Option<f64>,
    pub min_risk_limit: Option<f64>,
    pub max_leverage: Option<u32>,
    pub initial_margin: Option<f64>,
    pub maint_margin_rate: Option<f64>,
}

// ── KuCoinClient methods ──────────────────────────────────────────────────────

impl KuCoinClient {
    /// Get available futures balance for `currency` (e.g. `"USDT"` or `"XBT"`).
    pub async fn get_balance(&self, currency: &str) -> Result<f64> {
        let overview: AccountOverview = self
            .get("/api/v1/account-overview", &[("currency", currency)])
            .await?;
        debug!(
            balance = overview.available_balance,
            currency, "got balance"
        );
        Ok(overview.available_balance)
    }

    /// Get the full account overview for `currency`.
    pub async fn get_account_overview(&self, currency: &str) -> Result<AccountOverview> {
        self.get("/api/v1/account-overview", &[("currency", currency)])
            .await
    }

    /// Get the current open position for `symbol`.
    ///
    /// Endpoint: `GET /api/v1/position`
    pub async fn get_position(&self, symbol: &str) -> Result<PositionInfo> {
        self.get("/api/v1/position", &[("symbol", symbol)]).await
    }

    /// Get all open positions across all symbols.
    ///
    /// Endpoint: `GET /api/v1/positions`
    pub async fn get_all_positions(&self) -> Result<Vec<PositionInfo>> {
        self.get("/api/v1/positions", &[]).await
    }

    /// Enable or disable automatic margin top-up for `symbol`.
    ///
    /// When `auto_deposit` is `true`, KuCoin will automatically add margin from
    /// your available balance to prevent liquidation. When `false`, a position
    /// will liquidate once the maintenance margin is breached.
    ///
    /// Endpoint: `POST /api/v1/position/changeAutoDeposit`
    pub async fn set_auto_deposit(&self, symbol: &str, auto_deposit: bool) -> Result<()> {
        self.post::<serde_json::Value>(
            "/api/v1/position/changeAutoDeposit",
            &json!({ "symbol": symbol, "autoDeposit": auto_deposit }),
        )
        .await?;
        info!(symbol, auto_deposit, "auto-deposit updated");
        Ok(())
    }

    /// Change the risk limit level for `symbol`.
    ///
    /// KuCoin defines risk limit tiers (level 1, 2, 3 …) per symbol. Each higher
    /// level raises the maximum position size but also increases the maintenance
    /// margin rate. Use level 1 unless you're running large positions.
    ///
    /// Call [`get_risk_limit_levels`] to see the available tiers and their margin
    /// requirements for your symbol before changing.
    ///
    /// Endpoint: `POST /api/v1/position/risk-limit-level/change`
    pub async fn set_risk_limit_level(&self, symbol: &str, level: u32) -> Result<()> {
        let resp: serde_json::Value = self
            .post(
                "/api/v1/position/risk-limit-level/change",
                &json!({ "symbol": symbol, "level": level }),
            )
            .await?;
        info!(symbol, level, resp = %resp, "risk limit level updated");
        Ok(())
    }

    /// Fetch all risk limit tiers available for `symbol`.
    ///
    /// Each tier specifies the max leverage, required initial margin, and
    /// maintenance margin rate. Use this before calling [`set_risk_limit_level`].
    ///
    /// Endpoint: `GET /api/v1/contracts/risk-limit/{symbol}`
    pub async fn get_risk_limit_levels(&self, symbol: &str) -> Result<Vec<RiskLimitLevel>> {
        self.get(&format!("/api/v1/contracts/risk-limit/{symbol}"), &[])
            .await
    }

    /// Fetch funding payment history for `symbol`.
    ///
    /// Results are ordered most-recent-first. Use `max_count` to limit the
    /// number of records returned (KuCoin's max per page is 100).
    ///
    /// Endpoint: `GET /api/v1/funding-history`
    pub async fn get_funding_history(
        &self,
        symbol: &str,
        max_count: u32,
    ) -> Result<Vec<FundingRecord>> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Page {
            data_list: Vec<FundingRecord>,
        }
        let limit = max_count.min(100).to_string();
        let page: Page = self
            .get(
                "/api/v1/funding-history",
                &[("symbol", symbol), ("maxCount", &limit)],
            )
            .await?;
        Ok(page.data_list)
    }
}
