//! KuCoin Unified Trade Account (UTA) — account summary + margin endpoints.
//!
//! The UTA is KuCoin's combined spot + futures margin account. All
//! endpoints below use the **Spot** base URL (`api.kucoin.com`) and the
//! same HMAC-SHA256 signing as the rest of the crate; route via
//! [`KucoinEnv::Unified`](crate::KucoinEnv) (which shares the spot base
//! URL) for clarity in logs and downstream connectors.
//!
//! # Example
//!
//! ```no_run
//! use exchange_apiws::{Credentials, KuCoin};
//!
//! # async fn example() -> exchange_apiws::Result<()> {
//! let client = KuCoin::unified(Credentials::from_env()?).rest_client()?;
//! let summary = client.get_uta_account_summary().await?;
//! let cross   = client.get_cross_margin_accounts().await?;
//! println!("equity={}  cross-debt-ratio={}", summary.account_equity_total, cross.debt_ratio);
//! # Ok(())
//! # }
//! ```

use serde::Deserialize;
use tracing::info;

use crate::client::KuCoinClient;
use crate::error::Result;

// ── Response types ───────────────────────────────────────────────────────────

/// Account-wide summary returned by `GET /api/v3/account/summary`.
///
/// All fields are denominated in `total_currency` (typically `"USDT"`).
/// Numeric fields arrive as JSON strings on the wire and are parsed into
/// `f64` by serde.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UtaAccountSummary {
    /// Total account equity = balance + unrealised PnL.
    #[serde(with = "str_f64")]
    pub account_equity_total: f64,
    /// Total unrealised profit/loss across all open positions.
    // KuCoin uses uppercase "PNL" here, not the default camelCase
    // "unrealisedPnlTotal" — override the rename for this field only.
    #[serde(rename = "unrealisedPNLTotal", with = "str_f64")]
    pub unrealised_pnl_total: f64,
    /// Margin balance available across spot + futures.
    #[serde(with = "str_f64")]
    pub margin_balance_total: f64,
    /// Margin currently locked in open futures positions.
    #[serde(with = "str_f64")]
    pub position_margin_total: f64,
    /// Margin currently locked in open orders.
    #[serde(with = "str_f64")]
    pub order_margin_total: f64,
    /// Frozen funds (withdrawals in flight, sub-account holds, etc.).
    #[serde(with = "str_f64")]
    pub frozen_funds_total: f64,
    /// Balance available for new orders or withdrawal.
    #[serde(with = "str_f64")]
    pub available_balance_total: f64,
    /// Quote currency used for the totals above.
    pub total_currency: String,
}

/// Aggregate cross-margin account state returned by
/// `GET /api/v3/margin/accounts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossMarginAccount {
    /// Total assets in the quote currency (typically USDT).
    #[serde(with = "str_f64")]
    pub total_asset_of_quote_currency: f64,
    /// Total liabilities (borrowed + accrued interest) in quote currency.
    #[serde(with = "str_f64")]
    pub total_liability_of_quote_currency: f64,
    /// `liability / asset`. `0.0` means no borrows; rising values approach
    /// the forced-liquidation threshold.
    #[serde(with = "str_f64")]
    pub debt_ratio: f64,
    /// Account-level status — `"EFFECTIVE"`, `"LIQUIDATION"`, etc.
    pub status: String,
    /// Per-currency holdings.
    pub assets: Vec<CrossMarginAsset>,
}

/// One currency's slot inside a [`CrossMarginAccount`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossMarginAsset {
    /// Currency code (e.g. `"USDT"`).
    pub currency: String,
    /// `true` when borrowing is currently allowed on this asset.
    #[serde(default)]
    pub borrow_enabled: bool,
    /// `true` when repayments are currently allowed.
    #[serde(default)]
    pub repay_enabled: bool,
    /// `true` when transfers in/out are currently allowed.
    #[serde(default)]
    pub transfer_enabled: bool,
    /// Outstanding borrowed principal (excl. accrued interest).
    #[serde(with = "str_f64")]
    pub borrowed: f64,
    /// Total asset balance for this currency.
    #[serde(with = "str_f64")]
    pub total_asset: f64,
    /// Balance available for new borrows / orders / withdrawal.
    #[serde(with = "str_f64")]
    pub available: f64,
    /// Balance currently held against open orders or positions.
    #[serde(with = "str_f64")]
    pub hold: f64,
    /// Maximum additional borrow allowed given current asset state.
    #[serde(default, with = "opt_str_f64")]
    pub max_borrow_size: Option<f64>,
}

/// Aggregate isolated-margin state returned by `GET /api/v1/isolated/accounts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolatedMarginAccount {
    /// Total balance across all isolated pairs, converted to the quote
    /// currency.
    #[serde(default, with = "opt_str_f64")]
    pub total_conversion_balance: Option<f64>,
    /// Total outstanding liability across all isolated pairs, converted.
    #[serde(default, with = "opt_str_f64")]
    pub liability_conversion_balance: Option<f64>,
    /// Per-pair isolated-margin slots.
    pub assets: Vec<IsolatedMarginPair>,
}

/// One trading-pair slot inside an [`IsolatedMarginAccount`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolatedMarginPair {
    /// Trading pair (e.g. `"BTC-USDT"`).
    pub symbol: String,
    /// Pair-level status — `"EFFECTIVE"`, `"LIQUIDATED"`, etc.
    #[serde(default)]
    pub status: String,
    /// Debt ratio for this pair only.
    #[serde(default, with = "opt_str_f64")]
    pub debt_ratio: Option<f64>,
    /// Base-asset slot (e.g. the BTC side of BTC-USDT).
    pub base_asset: IsolatedMarginAsset,
    /// Quote-asset slot.
    pub quote_asset: IsolatedMarginAsset,
}

/// Per-side balance/liability inside an [`IsolatedMarginPair`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolatedMarginAsset {
    /// Currency code.
    pub currency: String,
    /// Total balance for this side.
    #[serde(default, with = "opt_str_f64")]
    pub total_balance: Option<f64>,
    /// Balance currently held against open orders.
    #[serde(default, with = "opt_str_f64")]
    pub hold_balance: Option<f64>,
    /// Balance available for new orders.
    #[serde(default, with = "opt_str_f64")]
    pub available_balance: Option<f64>,
    /// Outstanding liability (borrowed principal).
    #[serde(default, with = "opt_str_f64")]
    pub liability: Option<f64>,
    /// Accrued interest on the outstanding liability.
    #[serde(default, with = "opt_str_f64")]
    pub interest: Option<f64>,
    /// Maximum additional borrow allowed on this side.
    #[serde(default, with = "opt_str_f64")]
    pub borrowable_amount: Option<f64>,
    /// `true` when borrowing is currently allowed.
    #[serde(default)]
    pub borrow_enabled: bool,
    /// `true` when transfers into this slot are currently allowed.
    #[serde(default)]
    pub transfer_in_enabled: bool,
    /// `true` when repayments are currently allowed.
    #[serde(default)]
    pub repay_enabled: bool,
}

// ── serde adapters ───────────────────────────────────────────────────────────

mod str_f64 {
    use serde::{Deserialize, Deserializer};
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum SF {
            S(String),
            F(f64),
        }
        match SF::deserialize(d)? {
            SF::S(s) if s.is_empty() => Ok(0.0),
            SF::S(s) => s.parse().map_err(serde::de::Error::custom),
            SF::F(f) => Ok(f),
        }
    }
}

mod opt_str_f64 {
    use serde::{Deserialize, Deserializer};
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum W {
            None,
            S(String),
            F(f64),
        }
        match Option::<W>::deserialize(d)? {
            None | Some(W::None) => Ok(None),
            Some(W::S(s)) if s.is_empty() => Ok(None),
            Some(W::S(s)) => s.parse().map(Some).map_err(serde::de::Error::custom),
            Some(W::F(f)) => Ok(Some(f)),
        }
    }
}

// ── KuCoinClient methods ─────────────────────────────────────────────────────

impl KuCoinClient {
    /// `GET /api/v3/account/summary` — account-wide summary across spot
    /// and futures (Unified Trade Account).
    ///
    /// Requires UTA to be enabled on the account; otherwise KuCoin returns
    /// `ExchangeError::Api`.
    pub async fn get_uta_account_summary(&self) -> Result<UtaAccountSummary> {
        info!("Fetching UTA account summary");
        self.get("/api/v3/account/summary", &[]).await
    }

    /// `GET /api/v3/margin/accounts` — aggregate cross-margin account
    /// state with per-currency breakdown.
    pub async fn get_cross_margin_accounts(&self) -> Result<CrossMarginAccount> {
        info!("Fetching cross-margin accounts");
        self.get("/api/v3/margin/accounts", &[]).await
    }

    /// `GET /api/v1/isolated/accounts` — aggregate isolated-margin
    /// account state with per-pair breakdown.
    pub async fn get_isolated_margin_accounts(&self) -> Result<IsolatedMarginAccount> {
        info!("Fetching isolated-margin accounts");
        self.get("/api/v1/isolated/accounts", &[]).await
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_summary_deserializes_string_fields() {
        let raw = r#"{
            "accountEquityTotal": "10000.00",
            "unrealisedPNLTotal": "12.50",
            "marginBalanceTotal": "9987.50",
            "positionMarginTotal": "100.00",
            "orderMarginTotal": "50.00",
            "frozenFundsTotal": "0.00",
            "availableBalanceTotal": "9837.50",
            "totalCurrency": "USDT"
        }"#;
        let s: UtaAccountSummary = serde_json::from_str(raw).expect("deserialize");
        assert!((s.account_equity_total - 10_000.0).abs() < 1e-6);
        assert!((s.unrealised_pnl_total - 12.5).abs() < 1e-9);
        assert_eq!(s.total_currency, "USDT");
    }

    #[test]
    fn cross_margin_with_assets() {
        let raw = r#"{
            "totalAssetOfQuoteCurrency": "10000.0",
            "totalLiabilityOfQuoteCurrency": "0.0",
            "debtRatio": "0.0",
            "status": "EFFECTIVE",
            "assets": [
                {
                    "currency": "USDT",
                    "borrowEnabled": true,
                    "repayEnabled": true,
                    "transferEnabled": true,
                    "borrowed": "0.0",
                    "totalAsset": "10000.0",
                    "available": "10000.0",
                    "hold": "0.0",
                    "maxBorrowSize": "5000.0"
                }
            ]
        }"#;
        let m: CrossMarginAccount = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(m.status, "EFFECTIVE");
        assert!((m.debt_ratio - 0.0).abs() < 1e-12);
        assert_eq!(m.assets.len(), 1);
        let a = &m.assets[0];
        assert_eq!(a.currency, "USDT");
        assert!(a.borrow_enabled);
        assert_eq!(a.max_borrow_size, Some(5000.0));
    }

    #[test]
    fn cross_margin_handles_missing_max_borrow() {
        // Some account states omit maxBorrowSize entirely — it must come
        // through as None, not a parse error.
        let raw = r#"{
            "totalAssetOfQuoteCurrency": "0.0",
            "totalLiabilityOfQuoteCurrency": "0.0",
            "debtRatio": "0.0",
            "status": "EFFECTIVE",
            "assets": [{
                "currency": "USDT",
                "borrowed": "0.0",
                "totalAsset": "0.0",
                "available": "0.0",
                "hold": "0.0"
            }]
        }"#;
        let m: CrossMarginAccount = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(m.assets[0].max_borrow_size, None);
        // bool fields default to false when absent
        assert!(!m.assets[0].borrow_enabled);
    }

    #[test]
    fn isolated_margin_with_pair() {
        let raw = r#"{
            "totalConversionBalance": "1000.0",
            "liabilityConversionBalance": "0.0",
            "assets": [{
                "symbol": "BTC-USDT",
                "status": "EFFECTIVE",
                "debtRatio": "0.0",
                "baseAsset": {
                    "currency": "BTC",
                    "totalBalance": "0.01",
                    "holdBalance": "0.0",
                    "availableBalance": "0.01",
                    "liability": "0.0",
                    "interest": "0.0",
                    "borrowableAmount": "0.005",
                    "borrowEnabled": true,
                    "transferInEnabled": true,
                    "repayEnabled": true
                },
                "quoteAsset": {
                    "currency": "USDT",
                    "totalBalance": "500.0",
                    "holdBalance": "0.0",
                    "availableBalance": "500.0",
                    "liability": "0.0",
                    "interest": "0.0",
                    "borrowableAmount": "250.0",
                    "borrowEnabled": true,
                    "transferInEnabled": true,
                    "repayEnabled": true
                }
            }]
        }"#;
        let m: IsolatedMarginAccount = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(m.total_conversion_balance, Some(1000.0));
        assert_eq!(m.assets.len(), 1);
        let p = &m.assets[0];
        assert_eq!(p.symbol, "BTC-USDT");
        assert_eq!(p.base_asset.currency, "BTC");
        assert_eq!(p.base_asset.available_balance, Some(0.01));
        assert_eq!(p.quote_asset.borrowable_amount, Some(250.0));
    }
}
