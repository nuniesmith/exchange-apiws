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
//! | `get_ohlc(pair, interval)` | `/0/public/OHLC` | `KrakenOhlc` (candles + `last` cursor) |
//! | `get_recent_trades(pair)` | `/0/public/Trades` | `KrakenRecentTrades` (trades + `last` cursor) |
//! | `get_spread(pair)` | `/0/public/Spread` | `KrakenSpread` (spread ticks + `last` cursor) |
//!
//! OHLC, Trades, and Spread responses mix a single pair-keyed array with a
//! top-level `"last"` cursor key. A custom [`Deserialize`] splits the cursor
//! from the pair entry, so callers get the echoed pair name, the row `Vec`,
//! and the typed `last` cursor without touching raw JSON. Each row is a
//! positional wire array (`[time, open, high, ...]`) decoded into a named
//! struct with `*_f64()` accessors for the string-encoded numerics.

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

// ── OHLC / Trades / Spread (pair-keyed array + `last` cursor) ──────────────────

/// Split Kraken's `{ "<PAIR>": [rows...], "last": <cursor> }` object shape.
///
/// OHLC, Trades, and Spread all share this mixed layout: exactly one
/// pair-keyed row array plus a top-level `"last"` cursor. This walks the
/// map once, routing the `"last"` key to `Cursor` and the (single) pair
/// entry to `Vec<Row>`, returning the echoed pair name alongside them.
fn split_pair_keyed<'de, M, Row, Cursor>(
    mut map: M,
) -> std::result::Result<(String, Vec<Row>, Cursor), M::Error>
where
    M: serde::de::MapAccess<'de>,
    Row: Deserialize<'de>,
    Cursor: Deserialize<'de>,
{
    use serde::de::Error as _;
    let mut pair: Option<String> = None;
    let mut rows: Option<Vec<Row>> = None;
    let mut last: Option<Cursor> = None;
    while let Some(key) = map.next_key::<String>()? {
        if key == "last" {
            last = Some(map.next_value()?);
        } else {
            // The only non-`last` key is the pair Kraken echoes back.
            rows = Some(map.next_value()?);
            pair = Some(key);
        }
    }
    Ok((
        pair.ok_or_else(|| M::Error::custom("kraken response missing pair entry"))?,
        rows.unwrap_or_default(),
        last.ok_or_else(|| M::Error::missing_field("last"))?,
    ))
}

/// One OHLC candle from `GET /0/public/OHLC`.
///
/// Kraken's wire row is an 8-element mixed array
/// `[time, open, high, low, close, vwap, volume, count]`; the custom
/// [`Deserialize`] maps positional fields to the names below. Prices, vwap,
/// and volume keep Kraken's string wire shape — use the `*_f64()` accessors
/// to parse them.
#[derive(Debug, Clone)]
pub struct KrakenCandle {
    /// Bar open time (seconds since the Unix epoch).
    pub time: i64,
    /// Open price.
    pub open: String,
    /// High price.
    pub high: String,
    /// Low price.
    pub low: String,
    /// Close price.
    pub close: String,
    /// Volume-weighted average price over the bar.
    pub vwap: String,
    /// Base-asset volume traded during the bar.
    pub volume: String,
    /// Number of trades in the bar.
    pub count: u64,
}

impl<'de> Deserialize<'de> for KrakenCandle {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        // [time, open, high, low, close, vwap, volume, count]
        type Raw = (i64, String, String, String, String, String, String, u64);
        let (time, open, high, low, close, vwap, volume, count) = Raw::deserialize(d)?;
        Ok(Self {
            time,
            open,
            high,
            low,
            close,
            vwap,
            volume,
            count,
        })
    }
}

impl KrakenCandle {
    /// Open price parsed to `f64` (`0.0` if malformed).
    #[must_use]
    pub fn open_f64(&self) -> f64 {
        self.open.parse().unwrap_or(0.0)
    }
    /// High price parsed to `f64`.
    #[must_use]
    pub fn high_f64(&self) -> f64 {
        self.high.parse().unwrap_or(0.0)
    }
    /// Low price parsed to `f64`.
    #[must_use]
    pub fn low_f64(&self) -> f64 {
        self.low.parse().unwrap_or(0.0)
    }
    /// Close price parsed to `f64`.
    #[must_use]
    pub fn close_f64(&self) -> f64 {
        self.close.parse().unwrap_or(0.0)
    }
    /// Base-asset volume parsed to `f64`.
    #[must_use]
    pub fn volume_f64(&self) -> f64 {
        self.volume.parse().unwrap_or(0.0)
    }
}

/// Response from `GET /0/public/OHLC`.
///
/// Kraken mixes a single pair-keyed candle array with a top-level `"last"`
/// cursor. The [`Deserialize`] impl splits them, exposing the echoed pair
/// name, its candles (oldest first), and the cursor to pass as `since` on
/// the next request for incremental updates.
#[derive(Debug, Clone)]
pub struct KrakenOhlc {
    /// Pair key Kraken echoed back (e.g. `"XXBTZUSD"`).
    pub pair: String,
    /// Candle series, oldest first.
    pub candles: Vec<KrakenCandle>,
    /// Pagination cursor — pass as the `since` parameter on the next call.
    pub last: i64,
}

impl<'de> Deserialize<'de> for KrakenOhlc {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = KrakenOhlc;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a Kraken OHLC map (one pair candle array + `last` cursor)")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> std::result::Result<KrakenOhlc, M::Error> {
                let (pair, candles, last) = split_pair_keyed::<M, KrakenCandle, i64>(map)?;
                Ok(KrakenOhlc {
                    pair,
                    candles,
                    last,
                })
            }
        }
        d.deserialize_map(V)
    }
}

/// One trade from `GET /0/public/Trades`.
///
/// Kraken's wire row is a positional array
/// `[price, volume, time, side, order_type, misc, trade_id?]`. The 7th
/// element (`trade_id`) was added to the API later, so it's optional; any
/// further trailing fields Kraken appends are ignored for forward
/// compatibility.
#[derive(Debug, Clone)]
pub struct KrakenTrade {
    /// Trade price.
    pub price: String,
    /// Trade volume (base asset).
    pub volume: String,
    /// Execution time (seconds since the Unix epoch, fractional).
    pub time: f64,
    /// Aggressor side — `"b"` (buy) or `"s"` (sell).
    pub side: String,
    /// Order type — `"m"` (market) or `"l"` (limit).
    pub order_type: String,
    /// Miscellaneous flags (usually empty).
    pub misc: String,
    /// Trade id, when present (newer API responses include it).
    pub trade_id: Option<u64>,
}

impl<'de> Deserialize<'de> for KrakenTrade {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = KrakenTrade;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a Kraken trade array [price, volume, time, side, type, misc, id?]")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<KrakenTrade, A::Error> {
                use serde::de::Error as _;
                let price: String = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(0, &self))?;
                let volume: String = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(1, &self))?;
                let time: f64 = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(2, &self))?;
                let side: String = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(3, &self))?;
                let order_type: String = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(4, &self))?;
                let misc: String = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(5, &self))?;
                // 7th element (trade_id) is present only on newer responses.
                let trade_id: Option<u64> = seq.next_element()?;
                // Ignore any further trailing fields for forward compatibility.
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(KrakenTrade {
                    price,
                    volume,
                    time,
                    side,
                    order_type,
                    misc,
                    trade_id,
                })
            }
        }
        d.deserialize_seq(V)
    }
}

impl KrakenTrade {
    /// Price parsed to `f64` (`0.0` if malformed).
    #[must_use]
    pub fn price_f64(&self) -> f64 {
        self.price.parse().unwrap_or(0.0)
    }
    /// Volume parsed to `f64`.
    #[must_use]
    pub fn volume_f64(&self) -> f64 {
        self.volume.parse().unwrap_or(0.0)
    }
    /// `true` when the aggressor was a buyer (`side == "b"`).
    #[must_use]
    pub fn is_buy(&self) -> bool {
        self.side == "b"
    }
}

/// Response from `GET /0/public/Trades`.
///
/// Same mixed shape as OHLC, but the `"last"` cursor is a **nanosecond**
/// timestamp Kraken returns as a string, so it's kept as `String`.
#[derive(Debug, Clone)]
pub struct KrakenRecentTrades {
    /// Pair key Kraken echoed back (e.g. `"XXBTZUSD"`).
    pub pair: String,
    /// Trades, oldest first.
    pub trades: Vec<KrakenTrade>,
    /// Nanosecond cursor — pass as the `since` parameter on the next call.
    pub last: String,
}

impl<'de> Deserialize<'de> for KrakenRecentTrades {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = KrakenRecentTrades;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a Kraken Trades map (one pair trade array + `last` cursor)")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> std::result::Result<KrakenRecentTrades, M::Error> {
                let (pair, trades, last) = split_pair_keyed::<M, KrakenTrade, String>(map)?;
                Ok(KrakenRecentTrades { pair, trades, last })
            }
        }
        d.deserialize_map(V)
    }
}

/// One spread tick from `GET /0/public/Spread`.
///
/// Kraken's wire row is a 3-element array `[time, bid, ask]`. Bid/ask keep
/// the string wire shape — use `bid_f64()` / `ask_f64()` to parse.
#[derive(Debug, Clone)]
pub struct KrakenSpreadTick {
    /// Sample time (seconds since the Unix epoch).
    pub time: i64,
    /// Best bid price.
    pub bid: String,
    /// Best ask price.
    pub ask: String,
}

impl<'de> Deserialize<'de> for KrakenSpreadTick {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        // [time, bid, ask]
        type Raw = (i64, String, String);
        let (time, bid, ask) = Raw::deserialize(d)?;
        Ok(Self { time, bid, ask })
    }
}

impl KrakenSpreadTick {
    /// Best bid parsed to `f64` (`0.0` if malformed).
    #[must_use]
    pub fn bid_f64(&self) -> f64 {
        self.bid.parse().unwrap_or(0.0)
    }
    /// Best ask parsed to `f64`.
    #[must_use]
    pub fn ask_f64(&self) -> f64 {
        self.ask.parse().unwrap_or(0.0)
    }
}

/// Response from `GET /0/public/Spread`.
#[derive(Debug, Clone)]
pub struct KrakenSpread {
    /// Pair key Kraken echoed back (e.g. `"XXBTZUSD"`).
    pub pair: String,
    /// Spread ticks, oldest first.
    pub spreads: Vec<KrakenSpreadTick>,
    /// Pagination cursor — pass as the `since` parameter on the next call.
    pub last: i64,
}

impl<'de> Deserialize<'de> for KrakenSpread {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = KrakenSpread;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a Kraken Spread map (one pair spread array + `last` cursor)")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> std::result::Result<KrakenSpread, M::Error> {
                let (pair, spreads, last) = split_pair_keyed::<M, KrakenSpreadTick, i64>(map)?;
                Ok(KrakenSpread {
                    pair,
                    spreads,
                    last,
                })
            }
        }
        d.deserialize_map(V)
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
    /// The response mixes a single pair-keyed candle array with a `"last"`
    /// cursor; [`KrakenOhlc`] splits them into the echoed pair name, the
    /// candle `Vec` (oldest first), and the cursor (pass `last` as `since`
    /// to page forward).
    pub async fn get_ohlc(&self, pair: &str, interval_mins: u32) -> Result<KrakenOhlc> {
        let i = interval_mins.to_string();
        let raw: Value = self
            .http
            .get("/0/public/OHLC", &[("pair", pair), ("interval", &i)])
            .await?;
        unwrap_kraken_envelope(raw)
    }

    /// `GET /0/public/Trades` — recent trade history.
    ///
    /// The response mixes a single pair-keyed trade array with a `"last"`
    /// **nanosecond** cursor (a string); see [`KrakenRecentTrades`].
    pub async fn get_recent_trades(&self, pair: &str) -> Result<KrakenRecentTrades> {
        let raw: Value = self.http.get("/0/public/Trades", &[("pair", pair)]).await?;
        unwrap_kraken_envelope(raw)
    }

    /// `GET /0/public/Spread` — recent spread history (bid/ask).
    ///
    /// See [`KrakenSpread`] for the split pair-array + `last` cursor shape.
    pub async fn get_spread(&self, pair: &str) -> Result<KrakenSpread> {
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

    #[test]
    fn ohlc_splits_candles_and_last_cursor() {
        // One pair-keyed 8-element candle array + a numeric `last` cursor.
        let raw = r#"{
            "XXBTZUSD": [
                [1700000000, "96000.0", "96100.0", "95900.0", "96050.0", "96025.0", "10.5", 100],
                [1700000060, "96050.0", "96200.0", "96000.0", "96150.0", "96100.0", "5.25", 42]
            ],
            "last": 1700000060
        }"#;
        let ohlc: KrakenOhlc = serde_json::from_str(raw).expect("deserialize ohlc");
        assert_eq!(ohlc.pair, "XXBTZUSD");
        assert_eq!(ohlc.last, 1_700_000_060);
        assert_eq!(ohlc.candles.len(), 2);
        assert_eq!(ohlc.candles[0].time, 1_700_000_000);
        assert_eq!(ohlc.candles[0].count, 100);
        assert!((ohlc.candles[0].close_f64() - 96_050.0).abs() < 1e-9);
        assert!((ohlc.candles[1].volume_f64() - 5.25).abs() < 1e-9);
    }

    #[test]
    fn trades_split_and_tolerate_optional_trade_id() {
        // First row is the legacy 6-element shape (no trade_id); the second
        // is the newer 7-element shape. `last` is a nanosecond string cursor.
        let raw = r#"{
            "XXBTZUSD": [
                ["96000.0", "0.001", 1700000000.123, "b", "l", ""],
                ["96010.5", "0.250", 1700000001.987, "s", "m", "", 987654]
            ],
            "last": "1700000060123456789"
        }"#;
        let trades: KrakenRecentTrades = serde_json::from_str(raw).expect("deserialize trades");
        assert_eq!(trades.pair, "XXBTZUSD");
        assert_eq!(trades.last, "1700000060123456789");
        assert_eq!(trades.trades.len(), 2);
        assert!(trades.trades[0].is_buy());
        assert!(trades.trades[0].trade_id.is_none());
        assert!((trades.trades[0].price_f64() - 96_000.0).abs() < 1e-9);
        assert!(!trades.trades[1].is_buy());
        assert_eq!(trades.trades[1].trade_id, Some(987_654));
        assert!((trades.trades[1].time - 1_700_000_001.987).abs() < 1e-6);
    }

    #[test]
    fn spread_splits_ticks_and_last_cursor() {
        let raw = r#"{
            "XXBTZUSD": [
                [1700000000, "95999.0", "96001.0"],
                [1700000030, "95998.5", "96002.5"]
            ],
            "last": 1700000060
        }"#;
        let spread: KrakenSpread = serde_json::from_str(raw).expect("deserialize spread");
        assert_eq!(spread.pair, "XXBTZUSD");
        assert_eq!(spread.last, 1_700_000_060);
        assert_eq!(spread.spreads.len(), 2);
        assert_eq!(spread.spreads[0].time, 1_700_000_000);
        assert!((spread.spreads[0].bid_f64() - 95_999.0).abs() < 1e-9);
        assert!((spread.spreads[1].ask_f64() - 96_002.5).abs() < 1e-9);
    }
}
