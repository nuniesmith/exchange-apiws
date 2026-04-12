//! Market data — klines, ticker, order book snapshot, funding rate, mark price,
//! and active contract metadata.

use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use crate::client::KuCoinClient;
use crate::error::Result;
use crate::types::Candle;

// ── Response types ─────────────────────────────────────────────────────────────

/// Single-level ticker returned by `/api/v1/ticker`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticker {
    /// Instrument symbol.
    pub symbol: String,
    /// Current best bid price.
    pub best_bid_price: Option<f64>,
    /// Quantity available at the best bid.
    pub best_bid_size: Option<f64>,
    /// Current best ask price.
    pub best_ask_price: Option<f64>,
    /// Quantity available at the best ask.
    pub best_ask_size: Option<f64>,
    /// Exchange timestamp in milliseconds.
    pub ts: Option<i64>,
}

/// Minimal order book snapshot (top-N levels).
#[derive(Debug, Deserialize)]
pub struct OrderBookSnapshot {
    /// Exchange sequence number for ordering incremental updates.
    pub sequence: u64,
    /// Ask price levels as `[price, qty]` pairs, ascending.
    pub asks: Vec<[f64; 2]>,
    /// Bid price levels as `[price, qty]` pairs, descending.
    pub bids: Vec<[f64; 2]>,
    /// Exchange timestamp in milliseconds.
    pub ts: Option<i64>,
}

/// Current funding rate for a futures symbol.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FundingRate {
    /// Instrument symbol.
    pub symbol: String,
    /// Funding interval in milliseconds (typically 28 800 000 = 8 h).
    pub granularity: Option<i64>,
    /// Unix timestamp of the next funding settlement (milliseconds).
    pub time_point: Option<i64>,
    /// Current funding rate (e.g. `0.0001` = 0.01 %).
    pub value: f64,
}

/// Current mark price for a futures symbol.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkPrice {
    /// Instrument symbol.
    pub symbol: String,
    /// Funding interval in milliseconds.
    pub granularity: Option<i64>,
    /// Unix timestamp of this mark price snapshot (milliseconds).
    pub time_point: Option<i64>,
    /// Current mark price.
    pub value: f64,
    /// Underlying spot index price used to compute the mark price.
    pub index_price: Option<f64>,
}

/// Basic contract metadata returned by `/api/v1/contracts/active`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractInfo {
    /// Futures contract symbol (e.g. `"XBTUSDTM"`).
    pub symbol: String,
    /// Underlying symbol (e.g. `"XBT"`).
    pub root_symbol: Option<String>,
    /// `"FFWCSX"` for perpetual, `"FFICSX"` for quarterly.
    pub contract_type: Option<String>,
    /// Unix timestamp when the contract first opened (milliseconds).
    pub first_open_date: Option<i64>,
    /// Unix timestamp when the contract expires (milliseconds); `None` for perpetuals.
    pub expire_date: Option<i64>,
    /// Unix timestamp of the settlement date (milliseconds).
    pub settle_date: Option<i64>,
    /// Base asset of the contract (e.g. `"XBT"`).
    pub base_currency: Option<String>,
    /// Quote currency (e.g. `"USDT"`).
    pub quote_currency: Option<String>,
    /// Settlement currency (e.g. `"USDT"` for linear, `"XBT"` for inverse).
    pub settle_currency: Option<String>,
    /// Maximum order quantity in contracts.
    pub max_order_qty: Option<u64>,
    /// Minimum order quantity increment (lot size).
    pub lot_size: Option<u64>,
    /// Minimum price movement (tick size).
    pub tick_size: Option<f64>,
    /// Notional value per contract in the settlement currency.
    pub multiplier: Option<f64>,
    /// Initial margin rate required to open a position.
    pub initial_margin: Option<f64>,
    /// Maintenance margin rate below which liquidation is triggered.
    pub maint_margin_rate: Option<f64>,
    /// Contract status — `"Open"` when trading is active.
    pub status: Option<String>,
    /// Current 8-hour funding rate.
    pub funding_fee_rate: Option<f64>,
    /// Predicted next 8-hour funding rate.
    pub predicted_funding_fee_rate: Option<f64>,
    /// Total open interest across all traders (contracts as a string).
    pub open_interest: Option<String>,
    /// 24-hour turnover in quote currency.
    pub turnover_of24h: Option<f64>,
    /// 24-hour volume in contracts.
    pub volume_of24h: Option<f64>,
    /// Current mark price.
    pub mark_price: Option<f64>,
    /// Underlying index price used for mark price calculation.
    pub index_price_value: Option<f64>,
}

// ── KuCoinClient methods ──────────────────────────────────────────────────────

impl KuCoinClient {
    /// Fetch the most recent `limit` klines for `symbol` at timeframe `granularity`
    /// (minutes as a string — KuCoin uses `"1"`, `"5"`, `"15"`, `"30"`, `"60"`,
    /// `"120"`, `"240"`, `"480"`, `"720"`, `"1440"`, `"10080"`).
    pub async fn fetch_klines(
        &self,
        symbol: &str,
        limit: usize,
        granularity: &str,
    ) -> Result<Vec<Candle>> {
        let gran_i = granularity.parse::<i64>().unwrap_or(1);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let from_ms = now_ms - gran_i * 60_000 * limit_i64;

        // KuCoin /api/v1/kline/query requires `from` and `to` in milliseconds.
        let from = from_ms.to_string();
        let to = now_ms.to_string();

        let raw: Vec<Vec<Value>> = self
            .get(
                "/api/v1/kline/query",
                &[
                    ("symbol", symbol),
                    ("granularity", granularity),
                    ("from", &from),
                    ("to", &to),
                ],
            )
            .await?;

        let candles = raw
            .iter()
            .filter_map(|r| match Candle::from_raw(r) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!(error = %e, "skipping malformed candle");
                    None
                }
            })
            .collect();
        debug!(symbol, granularity, count = raw.len(), "fetched klines");
        Ok(candles)
    }

    /// Paginated klines fetch — calls the API in multiple windows to return
    /// more bars than a single API response would allow.
    ///
    /// Each page window spans at most `page_size` bars.
    pub async fn fetch_klines_extended(
        &self,
        symbol: &str,
        total: usize,
        granularity: &str,
        page_size: usize,
    ) -> Result<Vec<Candle>> {
        let gran_i = granularity.parse::<i64>().unwrap_or(1);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut all: Vec<Candle> = Vec::with_capacity(total);
        let mut window_end_ms = now_ms;

        while all.len() < total {
            let remaining = total - all.len();
            let batch = remaining.min(page_size);
            let batch_i64 = i64::try_from(batch).unwrap_or(i64::MAX);
            let window_ms = gran_i * 60_000 * batch_i64;
            let window_start_ms = window_end_ms - window_ms;

            // KuCoin /api/v1/kline/query requires `from` and `to` in milliseconds.
            let from = window_start_ms.to_string();
            let to = window_end_ms.to_string();

            let raw: Vec<Vec<Value>> = self
                .get(
                    "/api/v1/kline/query",
                    &[
                        ("symbol", symbol),
                        ("granularity", granularity),
                        ("from", &from),
                        ("to", &to),
                    ],
                )
                .await?;

            let n = raw.len();
            let mut page: Vec<Candle> = raw
                .iter()
                .filter_map(|r| match Candle::from_raw(r) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        tracing::warn!(error = %e, "skipping malformed candle in page");
                        None
                    }
                })
                .collect();
            page.sort_by_key(|c| c.time);
            all.extend(page);

            if n == 0 {
                break;
            }

            window_end_ms = window_start_ms - gran_i * 60_000;
        }

        all.sort_by_key(|c| c.time);
        all.truncate(total);
        Ok(all)
    }

    /// Fetch the Level 2 order book snapshot for `symbol`.
    pub async fn get_orderbook_snapshot(&self, symbol: &str) -> Result<OrderBookSnapshot> {
        self.get("/api/v1/level2/snapshot", &[("symbol", symbol)])
            .await
    }

    /// Fetch the current funding rate for a futures `symbol`.
    pub async fn get_funding_rate(&self, symbol: &str) -> Result<FundingRate> {
        self.get(&format!("/api/v1/funding-rate/{symbol}/current"), &[])
            .await
    }

    /// Fetch the current mark price for a futures `symbol`.
    pub async fn get_mark_price(&self, symbol: &str) -> Result<MarkPrice> {
        self.get(&format!("/api/v1/mark-price/{symbol}/current"), &[])
            .await
    }

    /// Fetch all active futures contracts.
    pub async fn get_active_contracts(&self) -> Result<Vec<ContractInfo>> {
        self.get("/api/v1/contracts/active", &[]).await
    }

    /// Fetch metadata for a single contract by symbol.
    pub async fn get_contract(&self, symbol: &str) -> Result<ContractInfo> {
        self.get(&format!("/api/v1/contracts/{symbol}"), &[]).await
    }

    /// Fetch the best bid/ask ticker for a futures `symbol`.
    pub async fn get_ticker(&self, symbol: &str) -> Result<Ticker> {
        self.get("/api/v1/ticker", &[("symbol", symbol)]).await
    }
}
