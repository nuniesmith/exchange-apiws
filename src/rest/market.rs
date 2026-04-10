//! Market data — kline/candle fetching, ticker, and order book.
//!
//! Mirrors Python `fetch_klines`, `fetch_klines_extended`, `fetch_ticker`, `fetch_order_book`.

use crate::client::KuCoinClient;
use crate::error::Result;
use crate::types::Candle;
use serde::Deserialize;
use tracing::info;

// ── Response types ─────────────────────────────────────────────────────────────

/// Response from `/api/v1/ticker`.
/// KuCoin returns numbers as strings in the REST API to prevent precision loss.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TickerResponse {
    pub sequence: u64,
    pub price: String,
    pub size: String,
    pub best_bid_price: String,
    pub best_bid_size: String,
    pub best_ask_price: String,
    pub best_ask_size: String,
}

/// Response from `/api/v1/level2/snapshot`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderBookResponse {
    pub sequence: u64,
    /// Arrays of [price, size] as strings
    pub asks: Vec<[String; 2]>,
    pub bids: Vec<[String; 2]>,
}

// ── KuCoinClient methods ──────────────────────────────────────────────────────

impl KuCoinClient {
    /// Fetch the current ticker for a symbol (best bid/ask and last trade price).
    pub async fn get_ticker(&self, symbol: &str) -> Result<TickerResponse> {
        self.get("/api/v1/ticker", &[("symbol", symbol)]).await
    }

    /// Fetch a Level 2 Order Book snapshot.
    pub async fn get_orderbook_snapshot(&self, symbol: &str) -> Result<OrderBookResponse> {
        self.get("/api/v1/level2/snapshot", &[("symbol", symbol)])
            .await
    }

    /// Fetch up to 200 recent candles.  Mirrors Python `fetch_klines`.
    ///
    /// `/api/v1/kline/query?symbol=XBTUSDTM&granularity=1&from=...&to=...`
    pub async fn fetch_klines(
        &self,
        symbol: &str,
        limit: usize,
        granularity: &str,
    ) -> Result<Vec<Candle>> {
        let limit = limit.min(200);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let gran_ms = granularity.parse::<i64>().unwrap_or(1) * 60 * 1_000;
        let from_ms = now_ms - (limit as i64) * gran_ms;

        let from = from_ms.to_string();
        let to = now_ms.to_string();

        let raw: serde_json::Value = self
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

        let mut candles = parse_candle_array(&raw);
        candles.sort_by_key(|c| c.time);

        info!(count = candles.len(), symbol, "fetched klines");
        Ok(candles)
    }

    /// Paginated fetch for large histories.  Mirrors Python `fetch_klines_extended`.
    ///
    /// Pages backward from `now`, deduplicates by timestamp, returns sorted result.
    pub async fn fetch_klines_extended(
        &self,
        symbol: &str,
        target: usize,
        granularity: &str,
    ) -> Result<Vec<Candle>> {
        let gran_min = granularity.parse::<i64>().unwrap_or(1);
        let window_ms = 200 * gran_min * 60 * 1_000;
        let pages = ((target + 199) / 200).max(1);

        let mut all: std::collections::HashMap<i64, Candle> = std::collections::HashMap::new();
        let mut end_ms = chrono::Utc::now().timestamp_millis();

        for page in 0..pages {
            let start_ms = end_ms - window_ms;
            let from = start_ms.to_string();
            let to = end_ms.to_string();

            match self
                .get::<serde_json::Value>(
                    "/api/v1/kline/query",
                    &[
                        ("symbol", symbol),
                        ("granularity", granularity),
                        ("from", &from),
                        ("to", &to),
                    ],
                )
                .await
            {
                Ok(raw) => {
                    let page_candles = parse_candle_array(&raw);
                    if page_candles.is_empty() {
                        break;
                    }
                    for c in page_candles {
                        all.insert(c.time, c);
                    }
                }
                Err(e) => {
                    tracing::warn!(page, symbol, error = %e, "klines page failed — stopping");
                    break;
                }
            }

            end_ms = start_ms;
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }

        let mut candles: Vec<Candle> = all.into_values().collect();
        candles.sort_by_key(|c| c.time);
        info!(count = candles.len(), symbol, "fetch_klines_extended done");
        Ok(candles)
    }
}

/// Parse KuCoin kline array `[[ts, o, h, l, c, v], ...]`.
fn parse_candle_array(val: &serde_json::Value) -> Vec<Candle> {
    val.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let arr = row.as_array()?;
                    Candle::from_raw(arr)
                })
                .collect()
        })
        .unwrap_or_default()
}
