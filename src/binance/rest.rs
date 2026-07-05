//! Binance public REST endpoints.
//!
//! All endpoints exposed here are **unauthenticated** — no API key is
//! required for spot market data, futures klines, funding rates, mark
//! prices, or open interest. The client splits across two base URLs:
//!
//! - **Spot:**    `https://api.binance.com`
//! - **Futures:** `https://fapi.binance.com` (USDT-M perpetual)
//!
//! Both are wrapped by the same [`PublicRestClient`] retry/backoff logic.
//!
//! # Example
//!
//! ```no_run
//! use exchange_apiws::binance::BinanceRestClient;
//!
//! # async fn example() -> exchange_apiws::Result<()> {
//! let client = BinanceRestClient::new()?;
//! let klines = client.get_klines("BTCUSDT", "1m", 100).await?;
//! println!("latest close: {}", klines.last().unwrap().close);
//! # Ok(())
//! # }
//! ```

use serde::Deserialize;

use crate::actors::{CandleData, FundingData};
use crate::error::Result;
use crate::http::PublicRestClient;

const SPOT_BASE_URL: &str = "https://api.binance.com";
const FUTURES_BASE_URL: &str = "https://fapi.binance.com";
const EXCHANGE_NAME: &str = "binance";

// ── Helpers ───────────────────────────────────────────────────────────────────

/// serde adapter: deserialize a JSON string (or number) as `f64`.
///
/// Binance returns most prices/quantities as strings, but a few endpoints
/// switch between string and number depending on the field. This adapter
/// accepts either form so a single struct shape works for both.
pub(super) mod str_f64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrFloat {
            S(String),
            F(f64),
        }
        match StringOrFloat::deserialize(d)? {
            StringOrFloat::S(s) => s.parse().map_err(serde::de::Error::custom),
            StringOrFloat::F(f) => Ok(f),
        }
    }

    // serde's `with = "..."` attribute calls `serialize(&T, S)` — the
    // signature is fixed by the framework, so pass-by-reference here
    // isn't actually wasteful.
    #[allow(dead_code, clippy::trivially_copy_pass_by_ref)]
    pub(super) fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
}

fn limit_str(n: u32) -> String {
    n.to_string()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── Response types ────────────────────────────────────────────────────────────

/// A single Binance kline (candlestick) bar.
///
/// Returned by both `GET /api/v3/klines` and `GET /fapi/v1/klines`. The raw
/// Binance shape is a heterogeneous array; deserialization parses each
/// position into the named field below.
#[derive(Debug, Clone)]
pub struct BinanceKline {
    /// Open time (ms since epoch).
    pub open_time: i64,
    /// Open price.
    pub open: f64,
    /// High price.
    pub high: f64,
    /// Low price.
    pub low: f64,
    /// Close price.
    pub close: f64,
    /// Base asset volume.
    pub volume: f64,
    /// Close time (ms since epoch).
    pub close_time: i64,
    /// Quote asset volume.
    pub quote_volume: f64,
    /// Number of trades in the interval.
    pub trades: u64,
    /// Taker buy base asset volume.
    pub taker_buy_base_volume: f64,
    /// Taker buy quote asset volume.
    pub taker_buy_quote_volume: f64,
}

impl<'de> Deserialize<'de> for BinanceKline {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        // Binance returns 12 positional values; the 12th ("ignore") is unused.
        // Short names mirror the Binance docs (Open/High/Low/Close/Vol).
        type Raw = (
            i64,
            String,
            String,
            String,
            String,
            String,
            i64,
            String,
            u64,
            String,
            String,
            String,
        );
        let (
            open_time,
            open_s,
            high_s,
            low_s,
            close_s,
            vol_s,
            close_time,
            quote_vol_s,
            trades,
            taker_buy_base_s,
            taker_buy_quote_s,
            _ignore,
        ) = Raw::deserialize(d)?;
        let parse = |s: String| s.parse::<f64>().map_err(serde::de::Error::custom);
        Ok(Self {
            open_time,
            open: parse(open_s)?,
            high: parse(high_s)?,
            low: parse(low_s)?,
            close: parse(close_s)?,
            volume: parse(vol_s)?,
            close_time,
            quote_volume: parse(quote_vol_s)?,
            trades,
            taker_buy_base_volume: parse(taker_buy_base_s)?,
            taker_buy_quote_volume: parse(taker_buy_quote_s)?,
        })
    }
}

impl BinanceKline {
    /// Convert to the unified [`CandleData`] type so callers can route Binance
    /// klines through the same downstream code paths as any other exchange.
    ///
    /// `is_closed` is set to `true` since REST klines are always finalised
    /// — only WS streams emit in-progress bars.
    #[must_use]
    pub fn into_candle_data(
        self,
        symbol: impl Into<String>,
        interval: impl Into<String>,
    ) -> CandleData {
        CandleData {
            symbol: symbol.into(),
            exchange: EXCHANGE_NAME.into(),
            interval: interval.into(),
            open_ts: self.open_time,
            open: self.open,
            high: self.high,
            low: self.low,
            close: self.close,
            volume: self.volume,
            is_closed: true,
            receipt_ts: now_ms(),
        }
    }
}

/// Order book snapshot returned by `GET /api/v3/depth`.
///
/// Levels are stored as raw `[price_str, qty_str]` arrays as Binance sends
/// them; use [`Self::bids_f64`] / [`Self::asks_f64`] to convert.
#[derive(Debug, Clone, Deserialize)]
pub struct BinanceOrderBook {
    /// Last update ID — use this as the sequence baseline when applying
    /// subsequent depth-diff updates over WS.
    #[serde(rename = "lastUpdateId")]
    pub last_update_id: i64,
    /// Bid levels, highest first. Each entry is `[price, quantity]` as
    /// strings (Binance's wire format).
    pub bids: Vec<[String; 2]>,
    /// Ask levels, lowest first. Same shape as `bids`.
    pub asks: Vec<[String; 2]>,
}

impl BinanceOrderBook {
    /// Parse `bids` into `[price, qty]` f64 pairs, skipping any malformed
    /// entries silently. Use the raw [`Self::bids`] field if you need
    /// strict error handling.
    #[must_use]
    pub fn bids_f64(&self) -> Vec<[f64; 2]> {
        Self::parse_levels(&self.bids)
    }

    /// Parse `asks` into `[price, qty]` f64 pairs.
    #[must_use]
    pub fn asks_f64(&self) -> Vec<[f64; 2]> {
        Self::parse_levels(&self.asks)
    }

    fn parse_levels(rows: &[[String; 2]]) -> Vec<[f64; 2]> {
        rows.iter()
            .filter_map(|[p, q]| Some([p.parse().ok()?, q.parse().ok()?]))
            .collect()
    }
}

/// A single trade returned by `GET /api/v3/trades`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceTrade {
    /// Exchange-assigned trade ID.
    pub id: u64,
    /// Trade price.
    #[serde(with = "str_f64")]
    pub price: f64,
    /// Trade quantity in base asset.
    #[serde(with = "str_f64")]
    pub qty: f64,
    /// Quote-asset volume of the trade (price × qty).
    #[serde(with = "str_f64")]
    pub quote_qty: f64,
    /// Trade timestamp (ms since epoch).
    pub time: i64,
    /// `true` when the buyer was the maker (i.e. an aggressive sell).
    pub is_buyer_maker: bool,
    /// `true` when this was the best match for the order book.
    pub is_best_match: bool,
}

/// Best bid / ask snapshot returned by `GET /api/v3/ticker/bookTicker`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceBookTicker {
    /// Instrument symbol.
    pub symbol: String,
    /// Current best bid price.
    #[serde(with = "str_f64")]
    pub bid_price: f64,
    /// Quantity at the best bid.
    #[serde(with = "str_f64")]
    pub bid_qty: f64,
    /// Current best ask price.
    #[serde(with = "str_f64")]
    pub ask_price: f64,
    /// Quantity at the best ask.
    #[serde(with = "str_f64")]
    pub ask_qty: f64,
}

/// 24-hour rolling window statistics returned by `GET /api/v3/ticker/24hr`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceTicker24h {
    /// Instrument symbol.
    pub symbol: String,
    /// Absolute price change over the last 24 h.
    #[serde(with = "str_f64")]
    pub price_change: f64,
    /// Percent price change over the last 24 h (e.g. `1.234` = +1.234 %).
    #[serde(with = "str_f64")]
    pub price_change_percent: f64,
    /// Volume-weighted average price.
    #[serde(with = "str_f64")]
    pub weighted_avg_price: f64,
    /// Last traded price.
    #[serde(with = "str_f64")]
    pub last_price: f64,
    /// Last trade quantity.
    #[serde(with = "str_f64")]
    pub last_qty: f64,
    /// Current best bid price.
    #[serde(with = "str_f64")]
    pub bid_price: f64,
    /// Current best ask price.
    #[serde(with = "str_f64")]
    pub ask_price: f64,
    /// Open price 24 h ago.
    #[serde(with = "str_f64")]
    pub open_price: f64,
    /// 24 h high.
    #[serde(with = "str_f64")]
    pub high_price: f64,
    /// 24 h low.
    #[serde(with = "str_f64")]
    pub low_price: f64,
    /// 24 h base-asset volume.
    #[serde(with = "str_f64")]
    pub volume: f64,
    /// 24 h quote-asset volume.
    #[serde(with = "str_f64")]
    pub quote_volume: f64,
    /// Window open time (ms since epoch).
    pub open_time: i64,
    /// Window close time (ms since epoch).
    pub close_time: i64,
    /// Number of trades in the window.
    pub count: u64,
}

/// A single historical funding rate returned by `GET /fapi/v1/fundingRate`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceFundingRate {
    /// Instrument symbol.
    pub symbol: String,
    /// Realised funding rate for the interval ending at `funding_time`.
    #[serde(with = "str_f64")]
    pub funding_rate: f64,
    /// Settlement timestamp for this rate (ms since epoch).
    pub funding_time: i64,
    /// Mark price snapshot at the time of settlement, if returned.
    #[serde(default, with = "opt_str_f64")]
    pub mark_price: Option<f64>,
}

/// serde adapter for `Option<f64>` carried as an optional string field.
pub(super) mod opt_str_f64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wrapper {
            None,
            S(String),
            F(f64),
        }
        match Option::<Wrapper>::deserialize(d)? {
            None | Some(Wrapper::None) => Ok(None),
            Some(Wrapper::S(s)) if s.is_empty() => Ok(None),
            Some(Wrapper::S(s)) => s.parse().map(Some).map_err(serde::de::Error::custom),
            Some(Wrapper::F(f)) => Ok(Some(f)),
        }
    }

    // serde's `with = "..."` mandates `serialize(&T, S)`; can't take by value.
    #[allow(dead_code, clippy::ref_option)]
    pub(super) fn serialize<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(f) => s.serialize_str(&f.to_string()),
            None => s.serialize_none(),
        }
    }
}

impl BinanceFundingRate {
    /// Convert to the unified [`FundingData`] type. Uses `funding_time` as
    /// `next_funding_time` for parity with WS events; treat as "settlement
    /// time of this row" (REST is historical so there is no live next-funding).
    #[must_use]
    pub fn into_funding_data(self) -> FundingData {
        FundingData {
            symbol: self.symbol,
            exchange: EXCHANGE_NAME.into(),
            funding_rate: self.funding_rate,
            next_funding_time: self.funding_time,
            mark_price: self.mark_price,
            index_price: None,
            exchange_ts: self.funding_time,
            receipt_ts: now_ms(),
        }
    }
}

/// Mark-price snapshot returned by `GET /fapi/v1/premiumIndex`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceMarkPrice {
    /// Instrument symbol.
    pub symbol: String,
    /// Current mark price.
    #[serde(with = "str_f64")]
    pub mark_price: f64,
    /// Underlying spot index price.
    #[serde(with = "str_f64")]
    pub index_price: f64,
    /// Estimated price at the next funding settlement.
    #[serde(default, with = "opt_str_f64")]
    pub estimated_settle_price: Option<f64>,
    /// Most recent realised funding rate.
    #[serde(with = "str_f64")]
    pub last_funding_rate: f64,
    /// Interest rate component used in the funding-rate formula.
    #[serde(default, with = "opt_str_f64")]
    pub interest_rate: Option<f64>,
    /// Timestamp of the next funding settlement (ms since epoch).
    pub next_funding_time: i64,
    /// Snapshot timestamp (ms since epoch).
    pub time: i64,
}

/// Open-interest snapshot returned by `GET /fapi/v1/openInterest`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceOpenInterest {
    /// Instrument symbol.
    pub symbol: String,
    /// Aggregate open interest in base-asset units.
    #[serde(with = "str_f64")]
    pub open_interest: f64,
    /// Snapshot timestamp (ms since epoch).
    pub time: i64,
}

/// Spot trading rules — `GET /api/v3/exchangeInfo`.
///
/// Only the fields needed for order validation are typed; the rest of the
/// (large) response is ignored. Per-symbol tick/lot/min-notional come from the
/// `filters` array — use the [`BinanceSymbolInfo`] accessors.
#[derive(Debug, Clone, Deserialize)]
pub struct BinanceExchangeInfo {
    /// Per-symbol trading rules.
    pub symbols: Vec<BinanceSymbolInfo>,
}

impl BinanceExchangeInfo {
    /// Rules for `symbol` (exact match, e.g. `"BTCUSDT"`), if listed.
    pub fn symbol(&self, symbol: &str) -> Option<&BinanceSymbolInfo> {
        self.symbols.iter().find(|s| s.symbol == symbol)
    }
}

/// Trading rules for one spot symbol.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceSymbolInfo {
    /// Instrument symbol, e.g. `"BTCUSDT"`.
    pub symbol: String,
    /// Trading status, e.g. `"TRADING"`.
    pub status: String,
    /// Base asset, e.g. `"BTC"`.
    pub base_asset: String,
    /// Base-asset decimal precision.
    pub base_asset_precision: u32,
    /// Quote asset, e.g. `"USDT"`.
    pub quote_asset: String,
    /// Quote-asset decimal precision.
    pub quote_asset_precision: u32,
    /// Order filters (price / lot / notional rules).
    pub filters: Vec<BinanceSymbolFilter>,
}

impl BinanceSymbolInfo {
    /// Price increment (tick size) from the `PRICE_FILTER`.
    pub fn tick_size(&self) -> Option<f64> {
        self.filters.iter().find_map(|f| match f {
            BinanceSymbolFilter::PriceFilter { tick_size, .. } => Some(*tick_size),
            _ => None,
        })
    }

    /// Quantity increment (step size) from the `LOT_SIZE` filter.
    pub fn lot_size(&self) -> Option<f64> {
        self.filters.iter().find_map(|f| match f {
            BinanceSymbolFilter::LotSize { step_size, .. } => Some(*step_size),
            _ => None,
        })
    }

    /// Minimum order notional from `NOTIONAL` (or legacy `MIN_NOTIONAL`).
    pub fn min_notional(&self) -> Option<f64> {
        self.filters.iter().find_map(|f| match f {
            BinanceSymbolFilter::MinNotional { min_notional }
            | BinanceSymbolFilter::Notional { min_notional, .. } => Some(*min_notional),
            _ => None,
        })
    }
}

/// The subset of Binance symbol filters relevant to order validation.
///
/// Unknown filter types deserialize to [`BinanceSymbolFilter::Other`], so a new
/// Binance filter never breaks parsing.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "filterType")]
pub enum BinanceSymbolFilter {
    /// Price tick size + bounds.
    #[serde(rename = "PRICE_FILTER", rename_all = "camelCase")]
    PriceFilter {
        /// Price increment.
        #[serde(with = "str_f64")]
        tick_size: f64,
        /// Minimum price.
        #[serde(with = "str_f64")]
        min_price: f64,
        /// Maximum price.
        #[serde(with = "str_f64")]
        max_price: f64,
    },
    /// Quantity step size + bounds.
    #[serde(rename = "LOT_SIZE", rename_all = "camelCase")]
    LotSize {
        /// Quantity increment.
        #[serde(with = "str_f64")]
        step_size: f64,
        /// Minimum order quantity.
        #[serde(with = "str_f64")]
        min_qty: f64,
        /// Maximum order quantity.
        #[serde(with = "str_f64")]
        max_qty: f64,
    },
    /// Legacy minimum-notional filter.
    #[serde(rename = "MIN_NOTIONAL", rename_all = "camelCase")]
    MinNotional {
        /// Minimum order notional (price × qty).
        #[serde(with = "str_f64")]
        min_notional: f64,
    },
    /// Current minimum-notional filter (replaces `MIN_NOTIONAL`).
    #[serde(rename = "NOTIONAL", rename_all = "camelCase")]
    Notional {
        /// Minimum order notional (price × qty).
        #[serde(with = "str_f64")]
        min_notional: f64,
    },
    /// Any other filter type (ignored for order validation).
    #[serde(other)]
    Other,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Binance public REST client — spot + USDT-M futures endpoints.
///
/// Construct once and clone cheaply (the underlying HTTP client pools
/// connections). All methods are `&self`; no mutable state.
#[derive(Clone)]
pub struct BinanceRestClient {
    spot: PublicRestClient,
    futures: PublicRestClient,
}

impl BinanceRestClient {
    /// Build a client pointed at Binance's live spot and USDT-M futures
    /// base URLs.
    pub fn new() -> Result<Self> {
        Self::with_base_urls(SPOT_BASE_URL, FUTURES_BASE_URL)
    }

    /// Build a client with caller-supplied base URLs. Used by integration
    /// tests pointing at `wiremock` and by callers proxying through a
    /// custom domain.
    pub fn with_base_urls(
        spot_url: impl Into<String>,
        futures_url: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            spot: PublicRestClient::new(spot_url)?,
            futures: PublicRestClient::new(futures_url)?,
        })
    }

    // ── Spot endpoints ───────────────────────────────────────────────────────

    /// `GET /api/v3/klines` — historical candlesticks for `symbol`.
    ///
    /// `interval` is a Binance interval label (`"1m"`, `"5m"`, `"1h"`,
    /// `"1d"`, …). `limit` is clamped to 1000 by the server.
    pub async fn get_klines(
        &self,
        symbol: &str,
        interval: &str,
        limit: u32,
    ) -> Result<Vec<BinanceKline>> {
        let limit = limit_str(limit);
        self.spot
            .get(
                "/api/v3/klines",
                &[
                    ("symbol", symbol),
                    ("interval", interval),
                    ("limit", &limit),
                ],
            )
            .await
    }

    /// `GET /api/v3/depth` — order book snapshot for `symbol`.
    ///
    /// `limit` is clamped server-side to a valid value (5, 10, 20, 50, 100,
    /// 500, 1000, or 5000). Pass 100 for general-purpose use.
    pub async fn get_orderbook(&self, symbol: &str, limit: u32) -> Result<BinanceOrderBook> {
        let limit = limit_str(limit);
        self.spot
            .get("/api/v3/depth", &[("symbol", symbol), ("limit", &limit)])
            .await
    }

    /// `GET /api/v3/trades` — most-recent trades for `symbol`.
    pub async fn get_recent_trades(&self, symbol: &str, limit: u32) -> Result<Vec<BinanceTrade>> {
        let limit = limit_str(limit);
        self.spot
            .get("/api/v3/trades", &[("symbol", symbol), ("limit", &limit)])
            .await
    }

    /// `GET /api/v3/ticker/bookTicker` — best bid/ask for `symbol`.
    pub async fn get_ticker(&self, symbol: &str) -> Result<BinanceBookTicker> {
        self.spot
            .get("/api/v3/ticker/bookTicker", &[("symbol", symbol)])
            .await
    }

    /// `GET /api/v3/ticker/24hr` — 24-hour rolling window stats.
    pub async fn get_ticker_24h(&self, symbol: &str) -> Result<BinanceTicker24h> {
        self.spot
            .get("/api/v3/ticker/24hr", &[("symbol", symbol)])
            .await
    }

    /// `GET /api/v3/exchangeInfo` — full exchange metadata.
    ///
    /// Returns the raw JSON. The full schema includes per-symbol filters
    /// (price step, lot size, market lot size, …) that vary by symbol and
    /// are easier to extract on demand than to model statically; use
    /// [`serde_json::from_value`] with your own shape if you need typed
    /// access to specific fields.
    pub async fn get_exchange_info(&self) -> Result<BinanceExchangeInfo> {
        self.spot.get("/api/v3/exchangeInfo", &[]).await
    }

    // ── Futures endpoints (USDT-M) ───────────────────────────────────────────

    /// `GET /fapi/v1/klines` — futures kline bars.
    ///
    /// Same shape as [`Self::get_klines`]; uses the USDT-M base URL.
    pub async fn get_futures_klines(
        &self,
        symbol: &str,
        interval: &str,
        limit: u32,
    ) -> Result<Vec<BinanceKline>> {
        let limit = limit_str(limit);
        self.futures
            .get(
                "/fapi/v1/klines",
                &[
                    ("symbol", symbol),
                    ("interval", interval),
                    ("limit", &limit),
                ],
            )
            .await
    }

    /// `GET /fapi/v1/fundingRate` — historical funding rate for a symbol.
    ///
    /// Returns the most-recent settlements (newest last). Server caps
    /// at ~1000 entries.
    pub async fn get_futures_funding_rate(
        &self,
        symbol: &str,
        limit: u32,
    ) -> Result<Vec<BinanceFundingRate>> {
        let limit = limit_str(limit);
        self.futures
            .get(
                "/fapi/v1/fundingRate",
                &[("symbol", symbol), ("limit", &limit)],
            )
            .await
    }

    /// `GET /fapi/v1/premiumIndex` — mark / index / funding snapshot.
    pub async fn get_futures_mark_price(&self, symbol: &str) -> Result<BinanceMarkPrice> {
        self.futures
            .get("/fapi/v1/premiumIndex", &[("symbol", symbol)])
            .await
    }

    /// `GET /fapi/v1/openInterest` — total open interest for `symbol`.
    pub async fn get_futures_open_interest(&self, symbol: &str) -> Result<BinanceOpenInterest> {
        self.futures
            .get("/fapi/v1/openInterest", &[("symbol", symbol)])
            .await
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Binance's kline shape is an array of 12 heterogeneous values; the
    /// custom Deserialize impl is the most error-prone part of this module
    /// and warrants a direct deserialization test.
    #[test]
    fn kline_deserializes_from_binance_array_shape() {
        let raw = r#"[
            1499040000000,
            "0.01634790",
            "0.80000000",
            "0.01575800",
            "0.01577100",
            "148976.11427815",
            1499644799999,
            "2434.19055334",
            308,
            "1756.87402397",
            "28.46694368",
            "0"
        ]"#;

        let k: BinanceKline = serde_json::from_str(raw).expect("kline deserialize");
        assert_eq!(k.open_time, 1_499_040_000_000);
        assert!((k.open - 0.016_347_9).abs() < 1e-9);
        assert!((k.high - 0.8).abs() < 1e-9);
        assert!((k.low - 0.015_758).abs() < 1e-9);
        assert!((k.close - 0.015_771).abs() < 1e-9);
        assert_eq!(k.trades, 308);
    }

    #[test]
    fn kline_into_candle_data_marks_rest_bars_closed() {
        let k = BinanceKline {
            open_time: 100,
            open: 1.0,
            high: 2.0,
            low: 0.5,
            close: 1.5,
            volume: 10.0,
            close_time: 200,
            quote_volume: 15.0,
            trades: 5,
            taker_buy_base_volume: 6.0,
            taker_buy_quote_volume: 9.0,
        };
        let c = k.into_candle_data("BTCUSDT", "1m");
        assert_eq!(c.symbol, "BTCUSDT");
        assert_eq!(c.exchange, "binance");
        assert_eq!(c.interval, "1m");
        assert!(c.is_closed, "REST klines are always finalised");
        assert!((c.close - 1.5).abs() < 1e-9);
    }

    #[test]
    fn orderbook_parses_string_levels_to_f64() {
        let raw = r#"{
            "lastUpdateId": 42,
            "bids": [["100.5", "1.0"], ["100.4", "2.5"]],
            "asks": [["100.6", "0.5"], ["100.7", "3.0"]]
        }"#;
        let book: BinanceOrderBook = serde_json::from_str(raw).expect("orderbook deserialize");
        assert_eq!(book.last_update_id, 42);
        let bids = book.bids_f64();
        assert_eq!(bids.len(), 2);
        assert!((bids[0][0] - 100.5).abs() < 1e-9);
        let asks = book.asks_f64();
        assert!((asks[1][1] - 3.0).abs() < 1e-9);
    }

    #[test]
    fn book_ticker_deserialises_string_fields() {
        let raw = r#"{
            "symbol": "BTCUSDT",
            "bidPrice": "96000.50",
            "bidQty": "0.5",
            "askPrice": "96001.00",
            "askQty": "1.0"
        }"#;
        let t: BinanceBookTicker = serde_json::from_str(raw).expect("ticker deserialize");
        assert_eq!(t.symbol, "BTCUSDT");
        assert!((t.bid_price - 96_000.5).abs() < 1e-9);
        assert!((t.ask_qty - 1.0).abs() < 1e-9);
    }

    #[test]
    fn funding_rate_into_funding_data() {
        let r = BinanceFundingRate {
            symbol: "BTCUSDT".into(),
            funding_rate: 0.000_1,
            funding_time: 1_700_028_800_000,
            mark_price: Some(96_010.0),
        };
        let f = r.into_funding_data();
        assert_eq!(f.symbol, "BTCUSDT");
        assert_eq!(f.exchange, "binance");
        assert_eq!(f.next_funding_time, 1_700_028_800_000);
        assert_eq!(f.mark_price, Some(96_010.0));
    }
}
