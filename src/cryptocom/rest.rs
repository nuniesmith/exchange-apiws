//! Crypto.com public REST endpoints (`https://api.crypto.com/exchange/v1`).
//!
//! All endpoints exposed here are **unauthenticated**. Crypto.com wraps
//! every response in a `{"id": N, "method": "...", "code": N, "result": {...}}`
//! envelope; non-zero `code` surfaces as [`ExchangeError::Api`] via
//! [`unwrap_cryptocom_envelope`].
//!
//! # Endpoint coverage
//!
//! | Method | Endpoint | Returns |
//! |---|---|---|
//! | `get_instruments()` | `/public/get-instruments` | `Vec<CryptocomInstrument>` |
//! | `get_orderbook(instrument, depth)` | `/public/get-book` | `CryptocomOrderBook` |
//! | `get_candlestick(instrument, timeframe)` | `/public/get-candlestick` | `Vec<CryptocomCandle>` |
//! | `get_ticker(instrument)` | `/public/get-ticker` | `Vec<CryptocomTicker>` |
//! | `get_recent_trades(instrument)` | `/public/get-trades` | `Vec<CryptocomTrade>` |
//! | `get_valuations(instrument, valuation_type)` | `/public/get-valuations` | `Vec<CryptocomValuation>` |
//!
//! Crypto.com's wire format uses heavily abbreviated single-letter field
//! names (`i`, `b`, `k`, `vv`, …); the typed structs in this module
//! `#[serde(rename)]` them to readable names. Numeric fields stay as
//! `String` to preserve Crypto.com's wire precision; convert with
//! `.parse::<f64>()` where needed.

use serde::Deserialize;
use serde_json::Value;

use crate::error::{ExchangeError, Result};
use crate::http::PublicRestClient;

const BASE_URL: &str = "https://api.crypto.com/exchange/v1";

// ── Envelope unwrap ─────────────────────────────────────────────────────────

/// Unwrap the standard Crypto.com `{"code": N, "result": {...}}` envelope.
///
/// Non-zero `code` surfaces as [`ExchangeError::Api`] with the code as a
/// decimal string and the `message` field preserved when present.
///
/// # Errors
///
/// Returns [`ExchangeError::Api`] when Crypto.com reports an error code,
/// or [`ExchangeError::Json`] when the `result` field can't be decoded
/// into `T`.
pub fn unwrap_cryptocom_envelope<T: serde::de::DeserializeOwned>(raw: Value) -> Result<T> {
    let code = raw.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = raw
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("no message")
            .to_string();
        return Err(ExchangeError::Api {
            code: code.to_string(),
            message,
        });
    }
    let result = raw.get("result").cloned().unwrap_or(Value::Null);
    serde_json::from_value(result).map_err(ExchangeError::Json)
}

/// Inner `{"data": [...]}` wrapper that most Crypto.com endpoints use.
/// The list-of-T pattern is so consistent we unwrap it generically.
#[derive(Debug, Clone, Deserialize)]
struct DataList<T> {
    data: Vec<T>,
}

// ── Response types ──────────────────────────────────────────────────────────

/// Instrument metadata from `/public/get-instruments`.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomInstrument {
    /// Trading-pair symbol (e.g. `"BTC_USDT"`).
    pub symbol: String,
    /// `"CCY_PAIR"` (spot) or `"PERPETUAL_SWAP"`.
    #[serde(default)]
    pub inst_type: Option<String>,
    /// Human-friendly name (e.g. `"BTC/USDT"`).
    #[serde(default)]
    pub display_name: Option<String>,
    /// Base currency code.
    #[serde(default)]
    pub base_ccy: Option<String>,
    /// Quote currency code.
    #[serde(default)]
    pub quote_ccy: Option<String>,
    /// Decimal precision for quote-asset prices.
    #[serde(default)]
    pub quote_decimals: Option<u32>,
    /// Decimal precision for base-asset quantities.
    #[serde(default)]
    pub quantity_decimals: Option<u32>,
    /// Minimum price increment (string form).
    #[serde(default)]
    pub price_tick_size: Option<String>,
    /// Minimum size increment (string form).
    #[serde(default)]
    pub qty_tick_size: Option<String>,
    /// Maximum leverage for perpetuals; `"1"` for spot.
    #[serde(default)]
    pub max_leverage: Option<String>,
    /// `true` when the instrument is currently tradable.
    #[serde(default)]
    pub tradable: bool,
}

/// Order book snapshot from `/public/get-book`.
///
/// Crypto.com's response wraps the book in an array of one entry; this
/// struct flattens it for ergonomics.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomOrderBook {
    /// Symbol the book is for.
    pub instrument_name: String,
    /// Returned depth (matches request).
    #[serde(default)]
    pub depth: Option<u32>,
    /// Bid levels — `[price_str, qty_str, num_orders_str]` per row.
    #[serde(default)]
    pub bids: Vec<[String; 3]>,
    /// Ask levels — same shape as bids.
    #[serde(default)]
    pub asks: Vec<[String; 3]>,
    /// Snapshot timestamp (ms since epoch).
    #[serde(default)]
    pub timestamp: i64,
    /// Sequence number for deltas.
    #[serde(default)]
    pub sequence: i64,
}

impl CryptocomOrderBook {
    /// Parse `bids` to `[price, qty]` `f64` pairs, dropping the
    /// num-orders column and skipping malformed entries.
    #[must_use]
    pub fn bids_f64(&self) -> Vec<[f64; 2]> {
        Self::parse_levels(&self.bids)
    }
    /// Parse `asks` to `[price, qty]` `f64` pairs.
    #[must_use]
    pub fn asks_f64(&self) -> Vec<[f64; 2]> {
        Self::parse_levels(&self.asks)
    }
    fn parse_levels(rows: &[[String; 3]]) -> Vec<[f64; 2]> {
        rows.iter()
            .filter_map(|[p, q, _n]| Some([p.parse().ok()?, q.parse().ok()?]))
            .collect()
    }
}

/// Single candlestick bar from `/public/get-candlestick`.
///
/// Crypto.com uses single-letter field names on the wire; renamed here
/// to readable names. Numeric fields stay as `String` to preserve
/// precision.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomCandle {
    /// Open price.
    #[serde(rename = "o")]
    pub open: String,
    /// High price.
    #[serde(rename = "h")]
    pub high: String,
    /// Low price.
    #[serde(rename = "l")]
    pub low: String,
    /// Close price.
    #[serde(rename = "c")]
    pub close: String,
    /// Volume in base asset.
    #[serde(rename = "v")]
    pub volume: String,
    /// Bar open timestamp (ms since epoch).
    #[serde(rename = "t")]
    pub open_ts: i64,
}

impl CryptocomCandle {
    /// Parse `open` as `f64`. Returns `0.0` on a malformed string.
    #[must_use]
    pub fn open_f64(&self) -> f64 {
        self.open.parse().unwrap_or(0.0)
    }
    /// Parse `close` as `f64`.
    #[must_use]
    pub fn close_f64(&self) -> f64 {
        self.close.parse().unwrap_or(0.0)
    }
    /// Parse `high` as `f64`.
    #[must_use]
    pub fn high_f64(&self) -> f64 {
        self.high.parse().unwrap_or(0.0)
    }
    /// Parse `low` as `f64`.
    #[must_use]
    pub fn low_f64(&self) -> f64 {
        self.low.parse().unwrap_or(0.0)
    }
    /// Parse `volume` as `f64`.
    #[must_use]
    pub fn volume_f64(&self) -> f64 {
        self.volume.parse().unwrap_or(0.0)
    }
}

/// 24-hour ticker snapshot from `/public/get-ticker`.
///
/// Crypto.com's wire shape uses single-letter field names that don't
/// follow any obvious convention — they're renamed here per the
/// official docs.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomTicker {
    /// Instrument symbol.
    #[serde(rename = "i")]
    pub instrument: String,
    /// Latest trade price.
    #[serde(rename = "a", default)]
    pub last_price: Option<String>,
    /// 24-hour high.
    #[serde(rename = "h", default)]
    pub high_24h: Option<String>,
    /// 24-hour low.
    #[serde(rename = "l", default)]
    pub low_24h: Option<String>,
    /// 24-hour base-asset volume.
    #[serde(rename = "v", default)]
    pub volume_24h: Option<String>,
    /// 24-hour quote-asset turnover.
    #[serde(rename = "vv", default)]
    pub value_24h: Option<String>,
    /// 24-hour price change percent (as a decimal — `0.05` = 5 %).
    #[serde(rename = "c", default)]
    pub change_pct_24h: Option<String>,
    /// Best bid price.
    #[serde(rename = "b", default)]
    pub best_bid: Option<String>,
    /// Best ask price. (Crypto.com uses `"k"` for ask, unintuitively.)
    #[serde(rename = "k", default)]
    pub best_ask: Option<String>,
    /// Snapshot timestamp (ms since epoch).
    #[serde(rename = "t", default)]
    pub timestamp: i64,
}

/// Single recent trade from `/public/get-trades`.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomTrade {
    /// `"buy"` or `"sell"` (direction of the taker).
    #[serde(rename = "s")]
    pub side: String,
    /// Trade price.
    #[serde(rename = "p")]
    pub price: String,
    /// Trade quantity in base asset.
    #[serde(rename = "q")]
    pub qty: String,
    /// Trade timestamp (ms since epoch).
    #[serde(rename = "t")]
    pub timestamp: i64,
    /// Trade ID.
    #[serde(rename = "d", default)]
    pub trade_id: Option<String>,
    /// Instrument symbol.
    #[serde(rename = "i", default)]
    pub instrument: Option<String>,
}

/// Single valuation entry from `/public/get-valuations`.
///
/// `valuation_type` selects what `value` carries: `"mark_price"`,
/// `"index_price"`, `"funding_rate"`, `"estimated_funding_rate"`.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomValuation {
    /// Numeric value of the requested metric.
    #[serde(rename = "v")]
    pub value: String,
    /// Sample timestamp (ms since epoch).
    #[serde(rename = "t")]
    pub timestamp: i64,
}

// ── Client ──────────────────────────────────────────────────────────────────

/// Crypto.com public REST client.
///
/// Construct once and clone cheaply — the underlying HTTP client pools
/// connections. All methods are `&self` and async.
#[derive(Clone)]
pub struct CryptocomRestClient {
    http: PublicRestClient,
}

impl CryptocomRestClient {
    /// Build a client pointed at Crypto.com's live exchange API.
    pub fn new() -> Result<Self> {
        Self::with_base_url(BASE_URL)
    }

    /// Build a client with a caller-supplied base URL (tests, proxies).
    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            http: PublicRestClient::new(base_url)?,
        })
    }

    /// `GET /public/get-instruments` — every tradable instrument.
    pub async fn get_instruments(&self) -> Result<Vec<CryptocomInstrument>> {
        let raw: Value = self.http.get("/public/get-instruments", &[]).await?;
        let list: DataList<CryptocomInstrument> = unwrap_cryptocom_envelope(raw)?;
        Ok(list.data)
    }

    /// `GET /public/get-book` — order book snapshot for `instrument`.
    ///
    /// `depth` is clamped server-side to a supported value (e.g. 10, 50).
    pub async fn get_orderbook(&self, instrument: &str, depth: u32) -> Result<CryptocomOrderBook> {
        let d = depth.to_string();
        let raw: Value = self
            .http
            .get(
                "/public/get-book",
                &[("instrument_name", instrument), ("depth", &d)],
            )
            .await?;
        // The response wraps the book in result.data[0]; unwrap to a single struct.
        let list: DataList<CryptocomOrderBook> = unwrap_cryptocom_envelope(raw)?;
        list.data
            .into_iter()
            .next()
            .ok_or_else(|| ExchangeError::Api {
                code: "empty_data".into(),
                message: "Crypto.com returned an empty book array".into(),
            })
    }

    /// `GET /public/get-candlestick` — OHLC bars for `instrument`.
    ///
    /// `timeframe` follows Crypto.com's wire values — `"1m"`, `"5m"`,
    /// `"15m"`, `"30m"`, `"1h"`, `"4h"`, `"6h"`, `"12h"`, `"1D"`,
    /// `"7D"`, `"14D"`, `"1M"`.
    pub async fn get_candlestick(
        &self,
        instrument: &str,
        timeframe: &str,
    ) -> Result<Vec<CryptocomCandle>> {
        let raw: Value = self
            .http
            .get(
                "/public/get-candlestick",
                &[("instrument_name", instrument), ("timeframe", timeframe)],
            )
            .await?;
        let list: DataList<CryptocomCandle> = unwrap_cryptocom_envelope(raw)?;
        Ok(list.data)
    }

    /// `GET /public/get-tickers` — 24-hour rolling ticker for one or all
    /// instruments. Pass `None` to fetch every instrument.
    pub async fn get_ticker(&self, instrument: Option<&str>) -> Result<Vec<CryptocomTicker>> {
        let mut params: Vec<(&str, &str)> = Vec::new();
        if let Some(i) = instrument {
            params.push(("instrument_name", i));
        }
        let raw: Value = self.http.get("/public/get-tickers", &params).await?;
        let list: DataList<CryptocomTicker> = unwrap_cryptocom_envelope(raw)?;
        Ok(list.data)
    }

    /// `GET /public/get-trades` — recent trades for `instrument`.
    pub async fn get_recent_trades(&self, instrument: &str) -> Result<Vec<CryptocomTrade>> {
        let raw: Value = self
            .http
            .get("/public/get-trades", &[("instrument_name", instrument)])
            .await?;
        let list: DataList<CryptocomTrade> = unwrap_cryptocom_envelope(raw)?;
        Ok(list.data)
    }

    /// `GET /public/get-valuations` — mark/index price or funding rate
    /// time series for a perpetual.
    ///
    /// `valuation_type` is one of `"index_price"`, `"mark_price"`,
    /// `"funding_rate"`, `"estimated_funding_rate"`. Returns a time
    /// series; the latest value is the last element.
    pub async fn get_valuations(
        &self,
        instrument: &str,
        valuation_type: &str,
    ) -> Result<Vec<CryptocomValuation>> {
        let raw: Value = self
            .http
            .get(
                "/public/get-valuations",
                &[
                    ("instrument_name", instrument),
                    ("valuation_type", valuation_type),
                ],
            )
            .await?;
        let list: DataList<CryptocomValuation> = unwrap_cryptocom_envelope(raw)?;
        Ok(list.data)
    }
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_unwraps_success() {
        let raw = serde_json::json!({"code": 0, "result": {"x": 1}});
        let v: Value = unwrap_cryptocom_envelope(raw).expect("unwrap");
        assert_eq!(v["x"], 1);
    }

    #[test]
    fn envelope_surfaces_nonzero_code_as_api_error() {
        let raw = serde_json::json!({
            "code": 30009,
            "message": "Invalid instrument_name",
            "result": {}
        });
        let r: Result<Value> = unwrap_cryptocom_envelope(raw);
        match r {
            Err(ExchangeError::Api { code, message }) => {
                assert_eq!(code, "30009");
                assert!(message.contains("Invalid instrument_name"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[test]
    fn orderbook_helpers_drop_num_orders_column() {
        let raw = r#"{
            "instrument_name": "BTC_USDT",
            "depth": 5,
            "bids": [["96000.0", "1.5", "2"], ["95999.0", "2.0", "3"]],
            "asks": [["96001.0", "0.5", "1"]],
            "timestamp": 1700000000000,
            "sequence": 42
        }"#;
        let book: CryptocomOrderBook = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(book.instrument_name, "BTC_USDT");
        assert_eq!(book.sequence, 42);
        let bids = book.bids_f64();
        let asks = book.asks_f64();
        assert!((bids[0][0] - 96_000.0).abs() < 1e-9);
        assert!((bids[1][1] - 2.0).abs() < 1e-9);
        assert!((asks[0][0] - 96_001.0).abs() < 1e-9);
    }

    #[test]
    fn candle_renames_letters_and_helpers_parse() {
        let raw = r#"{"o":"96000.0","h":"96100.0","l":"95900.0","c":"96050.0","v":"10.5","t":1700000000000}"#;
        let c: CryptocomCandle = serde_json::from_str(raw).expect("deserialize");
        assert!((c.open_f64() - 96_000.0).abs() < 1e-9);
        assert!((c.close_f64() - 96_050.0).abs() < 1e-9);
        assert!((c.high_f64() - 96_100.0).abs() < 1e-9);
        assert!((c.low_f64() - 95_900.0).abs() < 1e-9);
        assert!((c.volume_f64() - 10.5).abs() < 1e-9);
        assert_eq!(c.open_ts, 1_700_000_000_000);
    }

    #[test]
    fn ticker_renames_cryptic_letters() {
        let raw = r#"{
            "i":"BTC_USDT","a":"96000.0","h":"96500.0","l":"95500.0",
            "v":"100.5","vv":"9650000","c":"0.005",
            "b":"95999.0","k":"96001.0","t":1700000000000
        }"#;
        let t: CryptocomTicker = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(t.instrument, "BTC_USDT");
        assert_eq!(t.last_price.as_deref(), Some("96000.0"));
        assert_eq!(t.best_bid.as_deref(), Some("95999.0"));
        assert_eq!(t.best_ask.as_deref(), Some("96001.0"));
        assert_eq!(t.value_24h.as_deref(), Some("9650000"));
        assert_eq!(t.timestamp, 1_700_000_000_000);
    }

    #[test]
    fn trade_renames_letters() {
        let raw =
            r#"{"s":"buy","p":"96000.0","q":"0.05","t":1700000000000,"d":"abc","i":"BTC_USDT"}"#;
        let t: CryptocomTrade = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(t.side, "buy");
        assert_eq!(t.price, "96000.0");
        assert_eq!(t.trade_id.as_deref(), Some("abc"));
    }

    #[test]
    fn valuation_renames_letters() {
        let raw = r#"{"v":"96010.5","t":1700000000000}"#;
        let val: CryptocomValuation = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(val.value, "96010.5");
        assert_eq!(val.timestamp, 1_700_000_000_000);
    }

    #[test]
    fn instrument_handles_missing_optional_fields() {
        // A minimal instrument response — only the required `symbol` field.
        let raw = r#"{"symbol":"BTC_USDT","tradable":true}"#;
        let i: CryptocomInstrument = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(i.symbol, "BTC_USDT");
        assert!(i.tradable);
        assert!(i.inst_type.is_none());
        assert!(i.max_leverage.is_none());
    }
}
