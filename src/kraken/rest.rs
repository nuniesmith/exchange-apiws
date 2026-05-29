//! Kraken public REST endpoints (`https://api.kraken.com/0/public/...`).
//!
//! All endpoints exposed here are **unauthenticated**. The standard Kraken
//! envelope is `{"result": {...}, "error": []}`; a non-empty `error`
//! array surfaces as [`ExchangeError::Api`] via
//! [`unwrap_kraken_envelope`].
//!
//! # Endpoint coverage
//!
//! | Method | Endpoint | Returns |
//! |---|---|---|
//! | `get_system_status()` | `/0/public/SystemStatus` | `KrakenSystemStatus` |
//! | `get_assets()` | `/0/public/Assets` | `HashMap<String, KrakenAsset>` |
//! | `get_asset_pairs(pair?)` | `/0/public/AssetPairs` | `HashMap<String, KrakenAssetPair>` |
//! | `get_ticker(pair)` | `/0/public/Ticker` | `HashMap<String, KrakenTicker>` |
//! | `get_orderbook(pair, count)` | `/0/public/Depth` | `HashMap<String, KrakenOrderBook>` |
//! | `get_ohlc(pair, interval)` | `/0/public/OHLC` | `serde_json::Value` (mixed shape — pair entries + "last") |
//! | `get_recent_trades(pair)` | `/0/public/Trades` | `serde_json::Value` (same shape note) |
//! | `get_spread(pair)` | `/0/public/Spread` | `serde_json::Value` (same shape note) |
//!
//! OHLC, Trades, and Spread responses mix per-pair arrays with a `"last"`
//! cursor key; they're returned as `serde_json::Value` rather than forcing
//! a custom Deserialize. Use [`serde_json::from_value`] with your own
//! shape if you need typed access.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{ExchangeError, Result};
use crate::http::PublicRestClient;

const BASE_URL: &str = "https://api.kraken.com";

// ── Envelope unwrap ─────────────────────────────────────────────────────────

/// Unwrap the standard Kraken `{"result":...,"error":[]}` envelope.
///
/// Non-empty `error` arrays surface as [`ExchangeError::Api`] with all
/// messages joined by `"; "` for display. Otherwise `result` is
/// deserialized into the caller's `T`.
///
/// # Errors
///
/// Returns [`ExchangeError::Api`] when Kraken reports errors, or
/// [`ExchangeError::Json`] when the `result` field can't be decoded into `T`.
pub fn unwrap_kraken_envelope<T: serde::de::DeserializeOwned>(raw: Value) -> Result<T> {
    // Check `error` first — when the call failed, `result` is often `{}`
    // or absent and would just produce a confusing JSON decode error.
    let errors: Vec<String> = raw
        .get("error")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    if !errors.is_empty() {
        return Err(ExchangeError::Api {
            code: "kraken_error".into(),
            message: errors.join("; "),
        });
    }
    let result = raw.get("result").cloned().unwrap_or(Value::Null);
    serde_json::from_value(result).map_err(ExchangeError::Json)
}

// ── Response types ───────────────────────────────────────────────────────────

/// Response from `GET /0/public/SystemStatus`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenSystemStatus {
    /// `"online"`, `"maintenance"`, `"cancel_only"`, or `"post_only"`.
    pub status: String,
    /// ISO-8601 timestamp of the status sample.
    pub timestamp: String,
}

/// One asset entry from `GET /0/public/Assets`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenAsset {
    /// Asset class — `"currency"` for most cases.
    pub aclass: String,
    /// Alternate name (Kraken's user-friendly code, e.g. `"XBT"`).
    pub altname: String,
    /// Internal scaling decimals (precision used when storing balances).
    pub decimals: u32,
    /// Decimals to show in the UI.
    pub display_decimals: u32,
    /// Collateral value when used as margin (omitted on some assets).
    #[serde(default)]
    pub collateral_value: Option<f64>,
    /// `"enabled"`, `"deposit_only"`, `"withdrawal_only"`, …
    #[serde(default)]
    pub status: Option<String>,
}

/// One asset-pair entry from `GET /0/public/AssetPairs`.
///
/// Models the most commonly-used fields; the full Kraken shape includes
/// fee schedules, margin tiers, etc. Pull those from the raw response
/// (`serde_json::Value`) on demand via [`serde_json::from_value`] if needed.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenAssetPair {
    /// Alternate pair name (e.g. `"XBTUSD"`).
    pub altname: String,
    /// WebSocket-channel name (e.g. `"XBT/USD"`); absent on some pairs.
    #[serde(default)]
    pub wsname: Option<String>,
    /// Base currency code (e.g. `"XXBT"`).
    pub base: String,
    /// Quote currency code (e.g. `"ZUSD"`).
    pub quote: String,
    /// Decimal precision for prices.
    pub pair_decimals: u32,
    /// Decimal precision for lot sizes.
    pub lot_decimals: u32,
    /// Lot multiplier applied to size.
    pub lot_multiplier: u32,
    /// Pair status — `"online"`, `"cancel_only"`, `"post_only"`, …
    #[serde(default)]
    pub status: Option<String>,
}

/// Ticker for a single pair returned by `GET /0/public/Ticker`.
///
/// Each `[String; 2]` / `[String; 3]` field is Kraken's tuple wire shape
/// — see field comments for the index meanings. Use `*_f64()` helpers
/// to convert to `f64`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenTicker {
    /// Ask: `[price, whole_lot_volume, lot_volume]`.
    pub a: [String; 3],
    /// Bid: `[price, whole_lot_volume, lot_volume]`.
    pub b: [String; 3],
    /// Last trade closed: `[price, lot_volume]`.
    pub c: [String; 2],
    /// Volume: `[today, last_24h]`.
    pub v: [String; 2],
    /// Volume-weighted average price: `[today, last_24h]`.
    pub p: [String; 2],
    /// Number of trades: `[today, last_24h]`.
    pub t: [u64; 2],
    /// Low price: `[today, last_24h]`.
    pub l: [String; 2],
    /// High price: `[today, last_24h]`.
    pub h: [String; 2],
    /// Opening price today.
    pub o: String,
}

impl KrakenTicker {
    /// Best ask price (first element of `a`).
    #[must_use]
    pub fn ask_price(&self) -> f64 {
        self.a[0].parse().unwrap_or(0.0)
    }
    /// Best bid price (first element of `b`).
    #[must_use]
    pub fn bid_price(&self) -> f64 {
        self.b[0].parse().unwrap_or(0.0)
    }
    /// Last trade price (first element of `c`).
    #[must_use]
    pub fn last_price(&self) -> f64 {
        self.c[0].parse().unwrap_or(0.0)
    }
    /// 24 h volume (second element of `v`).
    #[must_use]
    pub fn volume_24h(&self) -> f64 {
        self.v[1].parse().unwrap_or(0.0)
    }
    /// 24 h high (second element of `h`).
    #[must_use]
    pub fn high_24h(&self) -> f64 {
        self.h[1].parse().unwrap_or(0.0)
    }
    /// 24 h low (second element of `l`).
    #[must_use]
    pub fn low_24h(&self) -> f64 {
        self.l[1].parse().unwrap_or(0.0)
    }
}

/// Order book snapshot for a single pair from `GET /0/public/Depth`.
///
/// Each level is `(price_str, volume_str, timestamp_secs)` — Kraken sends
/// price/volume as JSON strings and the timestamp as a JSON number
/// (seconds since the Unix epoch, with millisecond precision via the
/// fractional part on some pairs). Use [`Self::bids_f64`] /
/// [`Self::asks_f64`] for parsed `[price, volume]` pairs.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenOrderBook {
    /// Ask levels, lowest price first.
    pub asks: Vec<(String, String, f64)>,
    /// Bid levels, highest price first.
    pub bids: Vec<(String, String, f64)>,
}

impl KrakenOrderBook {
    /// Parse `bids` to `[price, volume]` `f64` pairs, dropping the timestamp
    /// column and skipping any malformed entry.
    #[must_use]
    pub fn bids_f64(&self) -> Vec<[f64; 2]> {
        Self::parse_levels(&self.bids)
    }
    /// Parse `asks` to `[price, volume]` `f64` pairs.
    #[must_use]
    pub fn asks_f64(&self) -> Vec<[f64; 2]> {
        Self::parse_levels(&self.asks)
    }
    fn parse_levels(rows: &[(String, String, f64)]) -> Vec<[f64; 2]> {
        rows.iter()
            .filter_map(|(p, v, _ts)| Some([p.parse().ok()?, v.parse().ok()?]))
            .collect()
    }
}

// ── Client ───────────────────────────────────────────────────────────────────

/// Kraken public REST client.
///
/// Construct once and clone cheaply — the underlying HTTP client pools
/// connections. All methods are `&self` and async.
#[derive(Clone)]
pub struct KrakenRestClient {
    http: PublicRestClient,
}

impl KrakenRestClient {
    /// Build a client pointed at Kraken's live API base URL.
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

    /// `GET /0/public/SystemStatus` — Kraken system health.
    pub async fn get_system_status(&self) -> Result<KrakenSystemStatus> {
        let raw: Value = self.http.get("/0/public/SystemStatus", &[]).await?;
        unwrap_kraken_envelope(raw)
    }

    /// `GET /0/public/Assets` — every tradable asset.
    pub async fn get_assets(&self) -> Result<HashMap<String, KrakenAsset>> {
        let raw: Value = self.http.get("/0/public/Assets", &[]).await?;
        unwrap_kraken_envelope(raw)
    }

    /// `GET /0/public/AssetPairs` — pair metadata (decimals, base/quote, …).
    ///
    /// When `pair` is `Some`, requests the specified pair (e.g. `"XBTUSD"`)
    /// only; when `None`, returns every pair (large response).
    pub async fn get_asset_pairs(
        &self,
        pair: Option<&str>,
    ) -> Result<HashMap<String, KrakenAssetPair>> {
        let raw: Value = if let Some(p) = pair {
            self.http
                .get("/0/public/AssetPairs", &[("pair", p)])
                .await?
        } else {
            self.http.get("/0/public/AssetPairs", &[]).await?
        };
        unwrap_kraken_envelope(raw)
    }

    /// `GET /0/public/Ticker` — ticker data for one or more pairs.
    ///
    /// `pair` is a comma-separated list (e.g. `"XBTUSD,ETHUSD"`).
    pub async fn get_ticker(&self, pair: &str) -> Result<HashMap<String, KrakenTicker>> {
        let raw: Value = self.http.get("/0/public/Ticker", &[("pair", pair)]).await?;
        unwrap_kraken_envelope(raw)
    }

    /// `GET /0/public/Depth` — order book snapshot.
    ///
    /// `count` is clamped server-side to 1..=500.
    pub async fn get_orderbook(
        &self,
        pair: &str,
        count: u32,
    ) -> Result<HashMap<String, KrakenOrderBook>> {
        let c = count.to_string();
        let raw: Value = self
            .http
            .get("/0/public/Depth", &[("pair", pair), ("count", &c)])
            .await?;
        unwrap_kraken_envelope(raw)
    }

    /// `GET /0/public/OHLC` — OHLC candle history.
    ///
    /// `interval` is in minutes — `1`, `5`, `15`, `30`, `60`, `240`,
    /// `1440`, `10080`, `21600`.
    ///
    /// Returned shape mixes pair-keyed arrays with a `"last"` cursor key:
    ///
    /// ```text
    /// {
    ///   "XXBTZUSD": [[time, open, high, low, close, vwap, volume, count], ...],
    ///   "last":      1700000060
    /// }
    /// ```
    ///
    /// Returned as `serde_json::Value` so callers pick whichever shape suits
    /// them (extract `last` for pagination, iterate the pair array, etc.).
    pub async fn get_ohlc(&self, pair: &str, interval_mins: u32) -> Result<Value> {
        let i = interval_mins.to_string();
        let raw: Value = self
            .http
            .get("/0/public/OHLC", &[("pair", pair), ("interval", &i)])
            .await?;
        unwrap_kraken_envelope(raw)
    }

    /// `GET /0/public/Trades` — recent trade history.
    ///
    /// Returned shape mixes pair-keyed trade arrays with a `"last"`
    /// nanosecond cursor; surfaced as `serde_json::Value`.
    pub async fn get_recent_trades(&self, pair: &str) -> Result<Value> {
        let raw: Value = self.http.get("/0/public/Trades", &[("pair", pair)]).await?;
        unwrap_kraken_envelope(raw)
    }

    /// `GET /0/public/Spread` — recent spread history (bid/ask).
    pub async fn get_spread(&self, pair: &str) -> Result<Value> {
        let raw: Value = self.http.get("/0/public/Spread", &[("pair", pair)]).await?;
        unwrap_kraken_envelope(raw)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_unwraps_success() {
        let raw = serde_json::json!({"result": {"x": 1}, "error": []});
        let v: Value = unwrap_kraken_envelope(raw).expect("unwrap");
        assert_eq!(v["x"], 1);
    }

    #[test]
    fn envelope_surfaces_error_array_as_api_error() {
        let raw = serde_json::json!({
            "result": {},
            "error": ["EAPI:Invalid key", "EGeneral:Permission denied"]
        });
        let r: Result<Value> = unwrap_kraken_envelope(raw);
        match r {
            Err(ExchangeError::Api { code, message }) => {
                assert_eq!(code, "kraken_error");
                assert!(message.contains("Invalid key"));
                assert!(message.contains("Permission denied"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[test]
    fn ticker_helpers_parse_tuple_fields() {
        let raw = r#"{
            "a": ["96001.0", "1", "1.000"],
            "b": ["95999.0", "1", "1.000"],
            "c": ["96000.0", "0.01"],
            "v": ["10.5", "100.5"],
            "p": ["95950.0", "95800.0"],
            "t": [100, 1000],
            "l": ["95500.0", "95000.0"],
            "h": ["96500.0", "97000.0"],
            "o": "95750.0"
        }"#;
        let t: KrakenTicker = serde_json::from_str(raw).expect("deserialize");
        assert!((t.ask_price() - 96_001.0).abs() < 1e-9);
        assert!((t.bid_price() - 95_999.0).abs() < 1e-9);
        assert!((t.last_price() - 96_000.0).abs() < 1e-9);
        assert!((t.volume_24h() - 100.5).abs() < 1e-9);
        assert!((t.high_24h() - 97_000.0).abs() < 1e-9);
        assert!((t.low_24h() - 95_000.0).abs() < 1e-9);
    }

    #[test]
    fn orderbook_helpers_drop_timestamp() {
        let raw = r#"{
            "asks": [["96000.0", "1.5", 1700000000]],
            "bids": [["95999.0", "2.0", 1700000000]]
        }"#;
        let book: KrakenOrderBook = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(book.asks_f64().len(), 1);
        assert!((book.asks_f64()[0][0] - 96_000.0).abs() < 1e-9);
        assert!((book.bids_f64()[0][1] - 2.0).abs() < 1e-9);
    }

    #[test]
    fn asset_deserialize_handles_missing_optionals() {
        // collateral_value and status are sometimes omitted.
        let raw = r#"{
            "aclass": "currency",
            "altname": "XBT",
            "decimals": 10,
            "display_decimals": 5
        }"#;
        let a: KrakenAsset = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(a.altname, "XBT");
        assert_eq!(a.decimals, 10);
        assert!(a.collateral_value.is_none());
        assert!(a.status.is_none());
    }

    #[test]
    fn asset_pair_handles_missing_wsname_and_status() {
        // wsname / status are absent on some pairs (e.g. inactive ones).
        let raw = r#"{
            "altname": "XBTUSD",
            "base": "XXBT",
            "quote": "ZUSD",
            "pair_decimals": 1,
            "lot_decimals": 8,
            "lot_multiplier": 1
        }"#;
        let p: KrakenAssetPair = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(p.altname, "XBTUSD");
        assert!(p.wsname.is_none());
        assert!(p.status.is_none());
    }
}
