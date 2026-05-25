//! Bybit public REST endpoints (v5 API).
//!
//! All endpoints exposed here are **unauthenticated** — no API key is
//! required for market data, funding rates, or open interest. The v5 API
//! unifies spot, linear-perpetual, and inverse contracts under a single
//! base URL (`https://api.bybit.com`); the `category` query parameter
//! routes to the right product class.
//!
//! Bybit wraps every response in the envelope
//! `{"retCode": N, "retMsg": "...", "result": {...}, "time": <ms>}`.
//! [`unwrap_bybit_envelope`] handles this; non-zero `retCode` surfaces as
//! [`ExchangeError::Api`].
//!
//! # Example
//!
//! ```no_run
//! use exchange_apiws::bybit::{BybitCategory, BybitRestClient};
//!
//! # async fn example() -> exchange_apiws::Result<()> {
//! let client = BybitRestClient::new()?;
//! let klines = client
//!     .get_klines(BybitCategory::Linear, "BTCUSDT", "1", 100)
//!     .await?;
//! println!("latest close: {}", klines.list.first().unwrap().close);
//! # Ok(())
//! # }
//! ```

use serde::Deserialize;
use serde_json::Value;

use crate::actors::{CandleData, FundingData};
use crate::error::{ExchangeError, Result};
use crate::http::PublicRestClient;

const BASE_URL: &str = "https://api.bybit.com";
const EXCHANGE_NAME: &str = "bybit";

// ── Helpers ───────────────────────────────────────────────────────────────────

/// serde adapter: accept a JSON string OR number as `f64`.
///
/// Bybit's v5 API is mostly string-encoded but a few fields slip back into
/// raw numbers; the union form handles both with one struct.
pub(super) mod str_f64 {
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

/// serde adapter: accept a JSON string OR number as `i64`.
///
/// Bybit returns most timestamps as strings (e.g. `"time": "1700000000000"`)
/// even though they're integers conceptually. This adapter normalises both.
pub(super) mod str_i64 {
    use serde::{Deserialize, Deserializer};
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum SI {
            S(String),
            I(i64),
        }
        match SI::deserialize(d)? {
            SI::S(s) => s.parse().map_err(serde::de::Error::custom),
            SI::I(i) => Ok(i),
        }
    }
}

/// serde adapter: optional `f64` carried as an optional/empty string.
pub(super) mod opt_str_f64 {
    use serde::{Deserialize, Deserializer};
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<f64>, D::Error> {
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

/// Unwrap the standard Bybit `{"retCode":N,"result":...}` envelope.
///
/// Non-zero `retCode` surfaces as [`ExchangeError::Api`] with the code and
/// `retMsg` preserved so the caller can decide how to react. `result` is
/// then deserialised into the caller's `T`.
///
/// # Errors
///
/// Returns [`ExchangeError::Api`] when Bybit reports an error code, or
/// [`ExchangeError::Json`] when the `result` field can't be decoded into `T`.
pub fn unwrap_bybit_envelope<T: serde::de::DeserializeOwned>(raw: Value) -> Result<T> {
    let ret_code = raw.get("retCode").and_then(Value::as_i64).unwrap_or(-1);
    if ret_code != 0 {
        let message = raw
            .get("retMsg")
            .and_then(Value::as_str)
            .unwrap_or("no message")
            .to_string();
        return Err(ExchangeError::Api {
            code: ret_code.to_string(),
            message,
        });
    }
    let result = raw.get("result").cloned().unwrap_or(Value::Null);
    serde_json::from_value(result).map_err(ExchangeError::Json)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── Category ──────────────────────────────────────────────────────────────────

/// Product class routing for v5 endpoints.
///
/// Most market-data endpoints accept all three; futures-specific ones
/// (funding rate, open interest, long/short ratio) require `Linear` or
/// `Inverse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BybitCategory {
    /// Spot trading.
    Spot,
    /// USDT-margined / USDC-margined perpetuals.
    Linear,
    /// Coin-margined (inverse) perpetuals and futures.
    Inverse,
}

impl BybitCategory {
    /// Wire-format string passed to the Bybit `category` query parameter.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spot => "spot",
            Self::Linear => "linear",
            Self::Inverse => "inverse",
        }
    }
}

// ── Response types ────────────────────────────────────────────────────────────

/// Standard Bybit v5 list result — `{"category":"...","list":[...]}`.
///
/// Most market-data endpoints return this shape so callers handle data via
/// `result.list` regardless of which product class they're querying.
#[derive(Debug, Clone, Deserialize)]
pub struct BybitListResult<T> {
    /// Product class the response was filtered for (mirrors the request).
    pub category: String,
    /// Result rows; ordering varies by endpoint (newest-first for klines).
    pub list: Vec<T>,
}

/// A single Bybit kline returned by `GET /v5/market/kline`.
///
/// Bybit's wire format is a 7-element string array per row; the custom
/// [`Deserialize`] impl maps positional fields to the names below.
#[derive(Debug, Clone)]
pub struct BybitKline {
    /// Bar start time (ms since epoch).
    pub start_time: i64,
    /// Open price.
    pub open: f64,
    /// High price.
    pub high: f64,
    /// Low price.
    pub low: f64,
    /// Close price.
    pub close: f64,
    /// Base-asset volume.
    pub volume: f64,
    /// Quote-asset turnover (price × size).
    pub turnover: f64,
}

impl<'de> Deserialize<'de> for BybitKline {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        // Bybit returns 7 positional strings: [start, open, high, low, close, volume, turnover].
        type Raw = (String, String, String, String, String, String, String);
        let (start_s, open_s, high_s, low_s, close_s, vol_s, turn_s) = Raw::deserialize(d)?;
        let parse_f = |s: String| s.parse::<f64>().map_err(serde::de::Error::custom);
        let parse_i = |s: String| s.parse::<i64>().map_err(serde::de::Error::custom);
        Ok(Self {
            start_time: parse_i(start_s)?,
            open: parse_f(open_s)?,
            high: parse_f(high_s)?,
            low: parse_f(low_s)?,
            close: parse_f(close_s)?,
            volume: parse_f(vol_s)?,
            turnover: parse_f(turn_s)?,
        })
    }
}

impl BybitKline {
    /// Convert to the unified [`CandleData`] type. REST klines are always
    /// finalised, so `is_closed` is `true`.
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
            open_ts: self.start_time,
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

/// Order book snapshot returned by `GET /v5/market/orderbook`.
///
/// Levels are stored as raw `[price, qty]` string pairs from the wire;
/// helper methods convert to `f64` on demand.
#[derive(Debug, Clone, Deserialize)]
pub struct BybitOrderBook {
    /// Instrument symbol.
    #[serde(rename = "s")]
    pub symbol: String,
    /// Bid levels, highest first. `[price, qty]` as strings.
    #[serde(rename = "b")]
    pub bids: Vec<[String; 2]>,
    /// Ask levels, lowest first.
    #[serde(rename = "a")]
    pub asks: Vec<[String; 2]>,
    /// Snapshot timestamp (ms since epoch).
    pub ts: i64,
    /// Sequence number; use as a baseline for delta updates over WS.
    #[serde(rename = "u")]
    pub update_id: i64,
}

impl BybitOrderBook {
    /// Parse `bids` to `[price, qty]` `f64` pairs, silently skipping any
    /// malformed entry.
    #[must_use]
    pub fn bids_f64(&self) -> Vec<[f64; 2]> {
        Self::parse_levels(&self.bids)
    }

    /// Parse `asks` to `[price, qty]` `f64` pairs.
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

/// Ticker data returned by `GET /v5/market/tickers`.
///
/// The Bybit v5 ticker shape varies by category (Spot returns a subset of
/// the Linear/Inverse fields). Fields that aren't present on every product
/// class are typed as `Option<f64>` so a single struct handles all three.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BybitTicker {
    /// Instrument symbol.
    pub symbol: String,
    /// Last traded price.
    #[serde(with = "str_f64")]
    pub last_price: f64,
    /// Top-of-book bid price.
    #[serde(default, with = "opt_str_f64", rename = "bid1Price")]
    pub bid1_price: Option<f64>,
    /// Top-of-book bid size.
    #[serde(default, with = "opt_str_f64", rename = "bid1Size")]
    pub bid1_size: Option<f64>,
    /// Top-of-book ask price.
    #[serde(default, with = "opt_str_f64", rename = "ask1Price")]
    pub ask1_price: Option<f64>,
    /// Top-of-book ask size.
    #[serde(default, with = "opt_str_f64", rename = "ask1Size")]
    pub ask1_size: Option<f64>,
    /// 24 h high.
    #[serde(default, with = "opt_str_f64")]
    pub high_price_24h: Option<f64>,
    /// 24 h low.
    #[serde(default, with = "opt_str_f64")]
    pub low_price_24h: Option<f64>,
    /// 24 h base-asset volume.
    #[serde(default, with = "opt_str_f64")]
    pub volume_24h: Option<f64>,
    /// 24 h quote-asset turnover.
    #[serde(default, with = "opt_str_f64")]
    pub turnover_24h: Option<f64>,
    /// Mark price (futures only).
    #[serde(default, with = "opt_str_f64")]
    pub mark_price: Option<f64>,
    /// Index price (futures only).
    #[serde(default, with = "opt_str_f64")]
    pub index_price: Option<f64>,
    /// Most recent funding rate (futures only).
    #[serde(default, with = "opt_str_f64")]
    pub funding_rate: Option<f64>,
    /// Next funding settlement timestamp, ms (futures only).
    #[serde(default, rename = "nextFundingTime")]
    pub next_funding_time: Option<String>,
}

/// A single recent trade returned by `GET /v5/market/recent-trade`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BybitTrade {
    /// Exchange-assigned execution ID.
    pub exec_id: String,
    /// Instrument symbol.
    pub symbol: String,
    /// Trade price.
    #[serde(with = "str_f64")]
    pub price: f64,
    /// Trade quantity in base asset.
    #[serde(with = "str_f64")]
    pub size: f64,
    /// `"Buy"` or `"Sell"` — direction of the aggressor.
    pub side: String,
    /// Trade timestamp (ms since epoch; Bybit sends this as a string).
    #[serde(with = "str_i64")]
    pub time: i64,
    /// `true` when this trade was reported as a block trade.
    #[serde(default)]
    pub is_block_trade: bool,
}

/// Historical funding rate returned by `GET /v5/market/funding/history`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BybitFundingRate {
    /// Instrument symbol.
    pub symbol: String,
    /// Realised funding rate.
    #[serde(with = "str_f64")]
    pub funding_rate: f64,
    /// Settlement timestamp (ms since epoch; sent as a string).
    #[serde(with = "str_i64")]
    pub funding_rate_timestamp: i64,
}

impl BybitFundingRate {
    /// Convert to the unified [`FundingData`] type. Bybit doesn't bundle
    /// mark / index price on this endpoint; both fields stay `None`.
    #[must_use]
    pub fn into_funding_data(self) -> FundingData {
        FundingData {
            symbol: self.symbol,
            exchange: EXCHANGE_NAME.into(),
            funding_rate: self.funding_rate,
            next_funding_time: self.funding_rate_timestamp,
            mark_price: None,
            index_price: None,
            exchange_ts: self.funding_rate_timestamp,
            receipt_ts: now_ms(),
        }
    }
}

/// Open-interest snapshot returned by `GET /v5/market/open-interest`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BybitOpenInterest {
    /// Aggregate open interest (base-asset units for Linear, USD for Inverse).
    #[serde(with = "str_f64")]
    pub open_interest: f64,
    /// Snapshot timestamp (ms since epoch; sent as a string).
    #[serde(with = "str_i64")]
    pub timestamp: i64,
}

/// Buy/sell account ratio returned by `GET /v5/market/account-ratio`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BybitLongShortRatio {
    /// Instrument symbol.
    pub symbol: String,
    /// Fraction of accounts holding long positions (0.0–1.0).
    #[serde(with = "str_f64")]
    pub buy_ratio: f64,
    /// Fraction of accounts holding short positions (0.0–1.0).
    #[serde(with = "str_f64")]
    pub sell_ratio: f64,
    /// Sampling timestamp (ms since epoch; sent as a string).
    #[serde(with = "str_i64")]
    pub timestamp: i64,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Bybit public REST client (v5 API).
///
/// Construct once and clone cheaply; the underlying HTTP client pools
/// connections. All methods are `&self` and async.
#[derive(Clone)]
pub struct BybitRestClient {
    http: PublicRestClient,
}

impl BybitRestClient {
    /// Build a client pointed at Bybit's live v5 endpoint.
    pub fn new() -> Result<Self> {
        Self::with_base_url(BASE_URL)
    }

    /// Build a client with a caller-supplied base URL. Used by integration
    /// tests pointing at `wiremock` and by callers proxying through a
    /// custom domain.
    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            http: PublicRestClient::new(base_url)?,
        })
    }

    /// `GET /v5/market/kline` — historical klines.
    pub async fn get_klines(
        &self,
        category: BybitCategory,
        symbol: &str,
        interval: &str,
        limit: u32,
    ) -> Result<BybitListResult<BybitKline>> {
        let limit_s = limit.to_string();
        let raw: Value = self
            .http
            .get(
                "/v5/market/kline",
                &[
                    ("category", category.as_str()),
                    ("symbol", symbol),
                    ("interval", interval),
                    ("limit", &limit_s),
                ],
            )
            .await?;
        unwrap_bybit_envelope(raw)
    }

    /// `GET /v5/market/orderbook` — order book snapshot.
    pub async fn get_orderbook(
        &self,
        category: BybitCategory,
        symbol: &str,
        limit: u32,
    ) -> Result<BybitOrderBook> {
        let limit_s = limit.to_string();
        let raw: Value = self
            .http
            .get(
                "/v5/market/orderbook",
                &[
                    ("category", category.as_str()),
                    ("symbol", symbol),
                    ("limit", &limit_s),
                ],
            )
            .await?;
        unwrap_bybit_envelope(raw)
    }

    /// `GET /v5/market/tickers` — single ticker (when `symbol` is provided)
    /// or all tickers in the category (when `symbol` is `None`).
    pub async fn get_tickers(
        &self,
        category: BybitCategory,
        symbol: Option<&str>,
    ) -> Result<BybitListResult<BybitTicker>> {
        let mut params: Vec<(&str, &str)> = vec![("category", category.as_str())];
        if let Some(s) = symbol {
            params.push(("symbol", s));
        }
        let raw: Value = self.http.get("/v5/market/tickers", &params).await?;
        unwrap_bybit_envelope(raw)
    }

    /// `GET /v5/market/recent-trade` — most-recent public trades.
    pub async fn get_recent_trades(
        &self,
        category: BybitCategory,
        symbol: &str,
        limit: u32,
    ) -> Result<BybitListResult<BybitTrade>> {
        let limit_s = limit.to_string();
        let raw: Value = self
            .http
            .get(
                "/v5/market/recent-trade",
                &[
                    ("category", category.as_str()),
                    ("symbol", symbol),
                    ("limit", &limit_s),
                ],
            )
            .await?;
        unwrap_bybit_envelope(raw)
    }

    /// `GET /v5/market/instruments-info` — instrument metadata for `category`.
    ///
    /// Returns raw JSON because the filter shape (lot-size, tick-size, …)
    /// is per-product-class and easier to deserialize on demand than to
    /// model statically. Callers run [`serde_json::from_value`] with their
    /// own shape if they need typed access.
    pub async fn get_instruments(&self, category: BybitCategory) -> Result<Value> {
        let raw: Value = self
            .http
            .get(
                "/v5/market/instruments-info",
                &[("category", category.as_str())],
            )
            .await?;
        unwrap_bybit_envelope(raw)
    }

    /// `GET /v5/market/funding/history` — historical funding rates
    /// (Linear / Inverse only; Spot will return an Api error).
    pub async fn get_funding_rate(
        &self,
        category: BybitCategory,
        symbol: &str,
        limit: u32,
    ) -> Result<BybitListResult<BybitFundingRate>> {
        let limit_s = limit.to_string();
        let raw: Value = self
            .http
            .get(
                "/v5/market/funding/history",
                &[
                    ("category", category.as_str()),
                    ("symbol", symbol),
                    ("limit", &limit_s),
                ],
            )
            .await?;
        unwrap_bybit_envelope(raw)
    }

    /// `GET /v5/market/open-interest` — open-interest time series.
    ///
    /// `interval_time` follows Bybit's discrete values: `"5min"`, `"15min"`,
    /// `"30min"`, `"1h"`, `"4h"`, `"1d"`.
    pub async fn get_open_interest(
        &self,
        category: BybitCategory,
        symbol: &str,
        interval_time: &str,
        limit: u32,
    ) -> Result<BybitListResult<BybitOpenInterest>> {
        let limit_s = limit.to_string();
        let raw: Value = self
            .http
            .get(
                "/v5/market/open-interest",
                &[
                    ("category", category.as_str()),
                    ("symbol", symbol),
                    ("intervalTime", interval_time),
                    ("limit", &limit_s),
                ],
            )
            .await?;
        unwrap_bybit_envelope(raw)
    }

    /// `GET /v5/market/account-ratio` — buy/sell account ratio time series.
    ///
    /// `period` follows Bybit's values: `"5min"`, `"15min"`, `"30min"`,
    /// `"1h"`, `"4h"`, `"1d"`.
    pub async fn get_long_short_ratio(
        &self,
        category: BybitCategory,
        symbol: &str,
        period: &str,
        limit: u32,
    ) -> Result<BybitListResult<BybitLongShortRatio>> {
        let limit_s = limit.to_string();
        let raw: Value = self
            .http
            .get(
                "/v5/market/account-ratio",
                &[
                    ("category", category.as_str()),
                    ("symbol", symbol),
                    ("period", period),
                    ("limit", &limit_s),
                ],
            )
            .await?;
        unwrap_bybit_envelope(raw)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_unwraps_success() {
        let raw = serde_json::json!({
            "retCode": 0,
            "retMsg": "OK",
            "result": {"hello": "world"},
            "time": 1_700_000_000_000_u64,
        });
        let v: Value = unwrap_bybit_envelope(raw).expect("unwrap success");
        assert_eq!(v["hello"], "world");
    }

    #[test]
    fn envelope_surfaces_nonzero_as_api_error() {
        let raw = serde_json::json!({
            "retCode": 10001,
            "retMsg": "invalid symbol",
            "result": {},
            "time": 1_700_000_000_000_u64,
        });
        let r: Result<Value> = unwrap_bybit_envelope(raw);
        match r {
            Err(ExchangeError::Api { code, message }) => {
                assert_eq!(code, "10001");
                assert!(message.contains("invalid symbol"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[test]
    fn kline_deserializes_from_array_shape() {
        let raw =
            r#"["1672041600000", "16513.00", "16550.00", "16500.00", "16540.00", "0.000076", "1.255988"]"#;
        let k: BybitKline = serde_json::from_str(raw).expect("kline deserialize");
        assert_eq!(k.start_time, 1_672_041_600_000);
        assert!((k.open - 16_513.0).abs() < 1e-6);
        assert!((k.high - 16_550.0).abs() < 1e-6);
        assert!((k.close - 16_540.0).abs() < 1e-6);
        assert!((k.turnover - 1.255_988).abs() < 1e-6);
    }

    #[test]
    fn kline_into_candle_data_marks_rest_bars_closed() {
        let k = BybitKline {
            start_time: 100,
            open: 1.0,
            high: 2.0,
            low: 0.5,
            close: 1.5,
            volume: 10.0,
            turnover: 15.0,
        };
        let c = k.into_candle_data("BTCUSDT", "1");
        assert_eq!(c.exchange, "bybit");
        assert!(c.is_closed, "REST klines are always finalised");
    }

    #[test]
    fn orderbook_parses_short_keys() {
        let raw = r#"{
            "s": "BTCUSDT",
            "b": [["96000.0", "1.5"]],
            "a": [["96001.0", "0.8"]],
            "ts": 1700000000000,
            "u": 42
        }"#;
        let book: BybitOrderBook = serde_json::from_str(raw).expect("orderbook deserialize");
        assert_eq!(book.symbol, "BTCUSDT");
        assert_eq!(book.update_id, 42);
        assert!((book.bids_f64()[0][0] - 96_000.0).abs() < 1e-9);
    }

    #[test]
    fn category_wire_format() {
        assert_eq!(BybitCategory::Spot.as_str(), "spot");
        assert_eq!(BybitCategory::Linear.as_str(), "linear");
        assert_eq!(BybitCategory::Inverse.as_str(), "inverse");
    }

    #[test]
    fn funding_rate_into_funding_data() {
        let r = BybitFundingRate {
            symbol: "BTCUSDT".into(),
            funding_rate: 0.000_1,
            funding_rate_timestamp: 1_700_028_800_000,
        };
        let f = r.into_funding_data();
        assert_eq!(f.exchange, "bybit");
        assert!(f.mark_price.is_none()); // Bybit's funding history doesn't bundle mark
        assert_eq!(f.next_funding_time, 1_700_028_800_000);
    }
}
