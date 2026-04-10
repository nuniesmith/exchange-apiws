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
    pub symbol: String,
    pub best_bid_price: Option<f64>,
    pub best_bid_size: Option<f64>,
    pub best_ask_price: Option<f64>,
    pub best_ask_size: Option<f64>,
    pub ts: Option<i64>,
}

/// Minimal order book snapshot (top-N levels).
#[derive(Debug, Deserialize)]
pub struct OrderBookSnapshot {
    pub sequence: u64,
    pub asks: Vec<[f64; 2]>,
    pub bids: Vec<[f64; 2]>,
    pub ts: Option<i64>,
}

/// Current funding rate for a futures symbol.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FundingRate {
    pub symbol: String,
    pub granularity: Option<i64>,
    pub time_point: Option<i64>,
    pub value: f64,
}

/// Current mark price for a futures symbol.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkPrice {
    pub symbol: String,
    pub granularity: Option<i64>,
    pub time_point: Option<i64>,
    pub value: f64,
    pub index_price: Option<f64>,
}

/// Basic contract metadata returned by `/api/v1/contracts/active`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractInfo {
    pub symbol: String,
    pub root_symbol: Option<String>,
    pub contract_type: Option<String>,
    pub first_open_date: Option<i64>,
    pub expire_date: Option<i64>,
    pub settle_date: Option<i64>,
    pub base_currency: Option<String>,
    pub quote_currency: Option<String>,
    pub settle_currency: Option<String>,
    pub max_order_qty: Option<u64>,
    pub lot_size: Option<u64>,
    pub tick_size: Option<f64>,
    pub multiplier: Option<f64>,
    pub initial_margin: Option<f64>,
    pub maint_margin_rate: Option<f64>,
    pub status: Option<String>,
    pub funding_fee_rate: Option<f64>,
    pub predicted_funding_fee_rate: Option<f64>,
    pub open_interest: Option<String>,
    pub turnover_of24h: Option<f64>,
    pub volume_of24h: Option<f64>,
    pub mark_price: Option<f64>,
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
        let from_ms = now_ms - gran_i * 60_000 * limit as i64;
        let from_s = from_ms / 1000;
        let to_s = now_ms / 1000;

        let from = from_s.to_string();
        let to = to_s.to_string();

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

        let candles = raw.iter().filter_map(|r| Candle::from_raw(r)).collect();
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
            let window_ms = gran_i * 60_000 * batch as i64;
            let window_start_ms = window_end_ms - window_ms;

            let from = (window_start_ms / 1000).to_string();
            let to = (window_end_ms / 1000).to_string();

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
            let mut page: Vec<Candle> = raw.iter().filter_map(|r| Candle::from_raw(r)).collect();
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
    ///
    /// Returns the full book. For a lightweight top-N book, use
    /// [`orderbook_depth_subscription`][crate::ws::KucoinConnector::orderbook_depth_subscription]
    /// over WebSocket instead.
    ///
    /// Endpoint: `GET /api/v1/level2/snapshot`
    pub async fn get_orderbook_snapshot(&self, symbol: &str) -> Result<OrderBookSnapshot> {
        self.get("/api/v1/level2/snapshot", &[("symbol", symbol)])
            .await
    }

    /// Fetch the current funding rate for a futures `symbol`.
    ///
    /// Endpoint: `GET /api/v1/funding-rate/{symbol}/current`
    pub async fn get_funding_rate(&self, symbol: &str) -> Result<FundingRate> {
        self.get(&format!("/api/v1/funding-rate/{symbol}/current"), &[])
            .await
    }

    /// Fetch the current mark price for a futures `symbol`.
    ///
    /// Endpoint: `GET /api/v1/mark-price/{symbol}/current`
    pub async fn get_mark_price(&self, symbol: &str) -> Result<MarkPrice> {
        self.get(&format!("/api/v1/mark-price/{symbol}/current"), &[])
            .await
    }

    /// Fetch all active futures contracts.
    ///
    /// Useful for discovering available symbols and their tick/lot sizes.
    ///
    /// Endpoint: `GET /api/v1/contracts/active`
    pub async fn get_active_contracts(&self) -> Result<Vec<ContractInfo>> {
        self.get("/api/v1/contracts/active", &[]).await
    }

    /// Fetch metadata for a single contract by symbol.
    ///
    /// Endpoint: `GET /api/v1/contracts/{symbol}`
    pub async fn get_contract(&self, symbol: &str) -> Result<ContractInfo> {
        self.get(&format!("/api/v1/contracts/{symbol}"), &[]).await
    }
}
