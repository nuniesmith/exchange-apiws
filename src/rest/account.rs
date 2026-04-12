//! Account management — balance, positions, auto-deposit, risk limit, and funding history.
//!
//! # Leverage in KuCoin Futures
//!
//! KuCoin Futures does **not** have a standalone "set leverage" endpoint like
//! Binance. Leverage is specified per-order via the `leverage` field in the
//! order body. The methods here control the two account-level knobs that affect
//! how margin is managed:
//!
//! - [`KuCoinClient::set_auto_deposit`] — enable or disable automatic margin top-up when a
//!   position approaches liquidation.
//! - [`KuCoinClient::set_risk_limit_level`] — change the risk limit tier (1–N), which controls
//!   the maximum position size and the minimum maintenance margin rate. A higher
//!   level allows larger positions but requires proportionally more margin.

use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info};

use crate::client::KuCoinClient;
use crate::error::Result;

// ── Response types ─────────────────────────────────────────────────────────────

/// Response from `GET /api/v1/account-overview`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountOverview {
    /// Balance available for trading or withdrawal.
    pub available_balance: f64,
    /// Margin currently locked in open orders.
    pub order_margin: Option<f64>,
    /// Margin currently locked in open positions.
    pub position_margin: Option<f64>,
    /// Total unrealised profit/loss across all open positions.
    pub unrealised_pnl: Option<f64>,
}

/// Response from `GET /api/v1/position`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionInfo {
    /// Positive = long, negative = short, 0 = flat.
    pub current_qty: i32,
    /// Instrument symbol.
    pub symbol: String,
    /// Volume-weighted average entry price, if a position is open.
    pub avg_entry_price: Option<f64>,
    /// Current unrealised profit/loss in quote currency.
    pub unrealised_pnl: Option<f64>,
    /// Cumulative realised profit/loss in quote currency.
    pub realised_pnl: Option<f64>,
    /// Effective leverage of the current position.
    pub leverage: Option<f64>,
    /// `true` when a position is open (non-zero quantity).
    pub is_open: Option<bool>,
    /// Current mark price used for unrealised PnL and liquidation.
    pub mark_price: Option<f64>,
    /// Notional value of the position at the current mark price.
    pub mark_value: Option<f64>,
    /// Maintenance margin required to avoid liquidation.
    pub maintenance_margin: Option<f64>,
}

impl PositionInfo {
    /// Returns `true` when `current_qty` is zero (no open position).
    pub const fn is_flat(&self) -> bool {
        self.current_qty == 0
    }

    /// Returns `true` when `current_qty` is positive (long position).
    pub const fn is_long(&self) -> bool {
        self.current_qty > 0
    }

    /// Returns `true` when `current_qty` is negative (short position).
    pub const fn is_short(&self) -> bool {
        self.current_qty < 0
    }
}

/// A single funding payment record from `GET /api/v1/funding-history`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FundingRecord {
    /// Exchange-assigned record identifier.
    pub id: Option<String>,
    /// Instrument symbol this payment relates to.
    pub symbol: String,
    /// Unix timestamp of the funding settlement (milliseconds).
    pub time_point: Option<i64>,
    /// Funding rate applied at this settlement.
    pub funding_rate: Option<f64>,
    /// Mark price at the time of settlement.
    pub mark_price: Option<f64>,
    /// Position size (contracts) at the time of settlement.
    pub position_qty: Option<i32>,
    /// Notional position cost at the time of settlement.
    pub position_cost: Option<f64>,
    /// Funding payment amount (positive = received, negative = paid).
    pub funding: Option<f64>,
    /// Settlement currency (e.g. `"USDT"`).
    pub settlement: Option<String>,
}

/// One risk limit tier returned by `GET /api/v1/contracts/risk-limit/{symbol}`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskLimitLevel {
    /// Instrument symbol this tier applies to.
    pub symbol: String,
    /// Tier number (1 = lowest risk/smallest position, N = highest).
    pub level: u32,
    /// Maximum notional value allowed at this tier.
    pub max_risk_limit: Option<f64>,
    /// Minimum notional value required to use this tier.
    pub min_risk_limit: Option<f64>,
    /// Maximum leverage permitted at this tier.
    pub max_leverage: Option<u32>,
    /// Initial margin rate required at this tier.
    pub initial_margin: Option<f64>,
    /// Maintenance margin rate — drop below this to trigger liquidation.
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
    /// Call [`KuCoinClient::get_risk_limit_levels`] to see the available tiers and their margin
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
    /// maintenance margin rate. Use this before calling [`KuCoinClient::set_risk_limit_level`].
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

    // ── Fund transfers ────────────────────────────────────────────────────────

    /// Transfer funds from the **Futures** account to the **Main** account.
    ///
    /// Both the sending and receiving business accounts are on the same UID;
    /// the funds move between the two sub-ledgers internally.
    ///
    /// `currency` — e.g. `"USDT"` or `"XBT"`.
    /// `amount`   — exact decimal amount to transfer.
    ///
    /// Endpoint: `POST /api/v1/transfer-out`
    pub async fn transfer_to_main(&self, currency: &str, amount: f64) -> Result<TransferResponse> {
        let resp: TransferResponse = self
            .post(
                "/api/v1/transfer-out",
                &json!({
                    "currency": currency,
                    "amount":   amount,
                    "recAccountType": "MAIN",
                }),
            )
            .await?;
        info!(currency, amount, apply_id = %resp.apply_id, "transfer Futures→Main initiated");
        Ok(resp)
    }

    /// Transfer funds from the **Main** account to the **Futures** account.
    ///
    /// `currency` — e.g. `"USDT"` or `"XBT"`.
    /// `amount`   — exact decimal amount to transfer.
    ///
    /// Endpoint: `POST /api/v1/transfer-in`
    pub async fn transfer_to_futures(
        &self,
        currency: &str,
        amount: f64,
    ) -> Result<TransferResponse> {
        let resp: TransferResponse = self
            .post(
                "/api/v1/transfer-in",
                &json!({
                    "currency":      currency,
                    "amount":        amount,
                    "payAccountType": "MAIN",
                }),
            )
            .await?;
        info!(currency, amount, apply_id = %resp.apply_id, "transfer Main→Futures initiated");
        Ok(resp)
    }

    /// Fetch paginated fund transfer history.
    ///
    /// `transfer_type` — `None` for all, `Some("TRANSFER_IN")`, or `Some("TRANSFER_OUT")`.
    /// `currency`      — e.g. `"USDT"`. Pass `None` for all currencies.
    /// `max_count`     — records per page, capped at 50.
    ///
    /// Endpoint: `GET /api/v1/transfer-list`
    pub async fn get_transfer_list(
        &self,
        currency: Option<&str>,
        transfer_type: Option<&str>,
        max_count: u32,
    ) -> Result<Vec<TransferRecord>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Page {
            items: Vec<TransferRecord>,
        }
        let limit = max_count.min(50).to_string();
        let mut params: Vec<(&str, &str)> = vec![("maxCount", &limit)];
        if let Some(c) = currency {
            params.push(("currency", c));
        }
        if let Some(t) = transfer_type {
            params.push(("type", t));
        }
        let page: Page = self.get("/api/v1/transfer-list", &params).await?;
        Ok(page.items)
    }

    // ── Margin management ─────────────────────────────────────────────────────

    /// Manually add (positive `margin`) or remove (negative `margin`) isolated
    /// margin for `symbol`.
    ///
    /// KuCoin requires the Futures account to be in **isolated** margin mode for
    /// this to be effective.  The `direction` field must be `"IN"` (add) or
    /// `"OUT"` (remove); the sign of `margin` is always positive — direction
    /// is encoded separately.
    ///
    /// Endpoint: `POST /api/v1/position/changeMargin`
    pub async fn add_position_margin(
        &self,
        symbol: &str,
        margin: f64,
        direction: &str, // "IN" or "OUT"
    ) -> Result<()> {
        let resp: serde_json::Value = self
            .post(
                "/api/v1/position/changeMargin",
                &json!({
                    "symbol":    symbol,
                    "margin":    margin,
                    "direction": direction,
                }),
            )
            .await?;
        info!(symbol, margin, direction, resp = %resp, "position margin updated");
        Ok(())
    }

    // ── Account overview — all currencies ────────────────────────────────────

    /// Fetch account balances for **all** currencies at once.
    ///
    /// Equivalent to calling `get_account_overview` for every currency the
    /// account holds, but in a single round-trip.
    ///
    /// Endpoint: `GET /api/v2/account-overview-all`
    pub async fn get_account_overview_all(&self) -> Result<Vec<AccountOverview>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wrapper {
            summary: Vec<AccountOverview>,
        }
        let w: Wrapper = self.get("/api/v2/account-overview-all", &[]).await?;
        debug!(count = w.summary.len(), "fetched all account overviews");
        Ok(w.summary)
    }
}

// ── Transfer types ────────────────────────────────────────────────────────────

/// Response from `POST /api/v1/transfer-out` and `POST /api/v1/transfer-in`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferResponse {
    /// Exchange-assigned transfer application ID for status tracking.
    pub apply_id: String,
}

/// A single transfer record from `GET /api/v1/transfer-list`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferRecord {
    /// Exchange-assigned application ID.
    pub apply_id: Option<String>,
    /// Currency transferred (e.g. `"USDT"`).
    pub currency: String,
    /// Transfer status: `"PROCESSING"`, `"SUCCESS"`, or `"FAILURE"`.
    pub status: Option<String>,
    /// `"TRANSFER_OUT"` (Futures→Main) or `"TRANSFER_IN"` (Main→Futures).
    #[serde(rename = "type")]
    pub transfer_type: Option<String>,
    /// Amount transferred as a decimal string.
    pub amount: Option<f64>,
    /// Unix timestamp when the transfer was created (milliseconds).
    pub created_at: Option<i64>,
}
