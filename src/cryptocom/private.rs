//! Crypto.com authenticated REST — account, orders, trades, withdrawals.
//!
//! Uses the HMAC-SHA256 + body-encoded `sig` scheme from
//! [`crate::cryptocom::auth`]. Every request POSTs a JSON body of the
//! shape:
//!
//! ```json
//! {
//!   "id": <i64>,
//!   "method": "private/...",
//!   "api_key": "...",
//!   "params": { ... },
//!   "nonce": <ms-since-epoch>,
//!   "sig": "<64-char hex>"
//! }
//! ```
//!
//! Responses share the same envelope as the public side
//! (`{"code": N, "result": {...}}`) so [`crate::cryptocom::rest::unwrap_cryptocom_envelope`]
//! handles the deserialise / Api-error split.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, info};

use crate::cryptocom::auth::{CryptocomCredentials, sign_cryptocom_request};
use crate::cryptocom::rest::unwrap_cryptocom_envelope;
use crate::error::{ExchangeError, Result};
use crate::http::send_with_retry;

const BASE_URL: &str = "https://api.crypto.com/exchange/v1";
const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 10;

// ── Wire-type helpers ─────────────────────────────────────────────────────────

/// Deserialise helpers for Crypto.com's mixed string/number wire types.
///
/// The trading endpoints send every price / quantity / fee as a JSON *string*,
/// but the wallet endpoints (deposit / withdrawal) send `amount`, `fee`, and
/// some IDs as JSON *numbers* — and `id` is a number from `create-withdrawal`
/// yet a string from `get-withdrawal-history`. These accept either wire form
/// and normalise to `String`, preserving the crate-wide "numbers stay strings
/// to keep wire precision" convention.
mod flex {
    use std::fmt;

    use serde::de::{self, Deserializer, Visitor};

    /// Visitor that accepts a JSON string or number and yields a `String`.
    struct StringOrNumber;

    impl Visitor<'_> for StringOrNumber {
        type Value = String;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string or number")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(v.to_owned())
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(v.to_string())
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(v.to_string())
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(v.to_string())
        }
    }

    /// Deserialize a required field that arrives as a JSON string or number.
    pub fn string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        d.deserialize_any(StringOrNumber)
    }

    /// Deserialize an optional field that may be a JSON string, number, or null.
    pub fn opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        struct OptStringOrNumber;

        impl<'de> Visitor<'de> for OptStringOrNumber {
            type Value = Option<String>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string, number, or null")
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_some<D2>(self, d: D2) -> Result<Self::Value, D2::Error>
            where
                D2: Deserializer<'de>,
            {
                d.deserialize_any(StringOrNumber).map(Some)
            }
        }

        d.deserialize_option(OptStringOrNumber)
    }
}

// ── Response types ────────────────────────────────────────────────────────────

/// One balance entry from `private/get-account-summary`.
///
/// Monetary fields are normalised to `String` (the endpoint sends them as JSON
/// numbers); parse with `.parse::<f64>()` where arithmetic is needed.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomBalance {
    /// Currency / token symbol (e.g. `"BTC"`).
    pub currency: String,
    /// Total balance (available + locked).
    #[serde(default, deserialize_with = "flex::opt_string")]
    pub balance: Option<String>,
    /// Balance free for new orders / withdrawals.
    #[serde(default, deserialize_with = "flex::opt_string")]
    pub available: Option<String>,
    /// Balance locked in open orders.
    #[serde(default, deserialize_with = "flex::opt_string")]
    pub order: Option<String>,
    /// Balance committed to staking.
    #[serde(default, deserialize_with = "flex::opt_string")]
    pub stake: Option<String>,
}

/// `result` wrapper for `private/get-account-summary` (`{"accounts": [...]}`).
#[derive(Debug, Clone, Deserialize)]
struct AccountSummary {
    #[serde(default)]
    accounts: Vec<CryptocomBalance>,
}

/// Inner `{"data": [...]}` wrapper used by the list endpoints (open-orders,
/// trades). Kept local since the public side's equivalent is private to
/// [`crate::cryptocom::rest`].
#[derive(Debug, Clone, Deserialize)]
struct DataList<T> {
    #[serde(default = "Vec::new")]
    data: Vec<T>,
}

/// Acknowledgement from `private/create-order` and `private/cancel-order`.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomOrderAck {
    /// Server-assigned order ID (a 19-digit snowflake returned as a string).
    /// Empty when the endpoint returns no body (some cancel responses).
    #[serde(default)]
    pub order_id: String,
    /// Client-supplied order ID, echoed back when one was provided.
    #[serde(default)]
    pub client_oid: Option<String>,
}

/// A single order from `private/get-open-orders` (one element of `data`) or the
/// full order returned by `private/get-order-detail`.
///
/// Price / quantity / fee values are Crypto.com's raw strings (parse with the
/// `_f64` helpers). Fields absent from one variant default rather than fail the
/// deserialise, so the one type serves both call sites.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomOrder {
    /// Server-assigned order ID (string snowflake).
    pub order_id: String,
    /// Instrument symbol (e.g. `"BTC_USD"`, `"BTCUSD-PERP"`).
    pub instrument_name: String,
    /// Order status: `"ACTIVE"`, `"FILLED"`, `"CANCELED"`, `"REJECTED"`,
    /// `"EXPIRED"`, `"NEW"`, `"PENDING"`.
    pub status: String,
    /// `"BUY"` or `"SELL"`.
    pub side: String,
    /// Order type: `"LIMIT"`, `"MARKET"`, `"STOP_LOSS"`, `"STOP_LIMIT"`,
    /// `"TAKE_PROFIT"`, `"TAKE_PROFIT_LIMIT"`.
    pub order_type: String,
    /// Order quantity in base asset.
    pub quantity: String,
    /// Client-supplied order ID, when present.
    #[serde(default)]
    pub client_oid: Option<String>,
    /// Limit price (absent / `"0"` for market orders).
    #[serde(default)]
    pub limit_price: Option<String>,
    /// Notional order value (price × quantity).
    #[serde(default)]
    pub order_value: Option<String>,
    /// Average fill price (`"0"` until partially filled).
    #[serde(default)]
    pub avg_price: Option<String>,
    /// Trigger price for stop / take-profit orders (order-detail).
    #[serde(default)]
    pub trigger_price: Option<String>,
    /// Reference price for trigger orders (order-detail).
    #[serde(default)]
    pub ref_price: Option<String>,
    /// Quantity filled so far.
    #[serde(default)]
    pub cumulative_quantity: Option<String>,
    /// Notional value filled so far.
    #[serde(default)]
    pub cumulative_value: Option<String>,
    /// Cumulative fee accrued.
    #[serde(default)]
    pub cumulative_fee: Option<String>,
    /// Maker fee rate (present on `get-open-orders`).
    #[serde(default)]
    pub maker_fee_rate: Option<String>,
    /// Taker fee rate (present on `get-open-orders`).
    #[serde(default)]
    pub taker_fee_rate: Option<String>,
    /// Time-in-force: `"GOOD_TILL_CANCEL"`, `"IMMEDIATE_OR_CANCEL"`,
    /// `"FILL_OR_KILL"`.
    #[serde(default)]
    pub time_in_force: Option<String>,
    /// Execution instructions, e.g. `["POST_ONLY"]`.
    #[serde(default)]
    pub exec_inst: Vec<String>,
    /// Currency fees are charged in.
    #[serde(default)]
    pub fee_instrument_name: Option<String>,
    /// Account UUID that owns the order.
    #[serde(default)]
    pub account_id: Option<String>,
    /// Order creation time (ms since epoch).
    #[serde(default)]
    pub create_time: i64,
    /// Order creation time (ns since epoch, kept as a string to avoid overflow).
    #[serde(default)]
    pub create_time_ns: Option<String>,
    /// Last-update time (ms since epoch).
    #[serde(default)]
    pub update_time: i64,
}

impl CryptocomOrder {
    /// Parse `quantity` as `f64` (0.0 on a malformed value).
    #[must_use]
    pub fn quantity_f64(&self) -> f64 {
        self.quantity.parse().unwrap_or(0.0)
    }
    /// Parse `cumulative_quantity` as `f64` (0.0 when absent / malformed).
    #[must_use]
    pub fn cumulative_quantity_f64(&self) -> f64 {
        self.cumulative_quantity
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0)
    }
    /// Parse `avg_price` as `f64` (0.0 when absent / malformed).
    #[must_use]
    pub fn avg_price_f64(&self) -> f64 {
        self.avg_price
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0)
    }
    /// Parse `limit_price` as `f64` (0.0 when absent / malformed).
    #[must_use]
    pub fn limit_price_f64(&self) -> f64 {
        self.limit_price
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0)
    }
}

/// A single trade (fill) from `private/get-trades`.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomPrivateTrade {
    /// Trade ID (string snowflake).
    pub trade_id: String,
    /// Originating order ID.
    pub order_id: String,
    /// Instrument symbol.
    pub instrument_name: String,
    /// `"BUY"` or `"SELL"`.
    pub side: String,
    /// Execution price.
    pub traded_price: String,
    /// Executed quantity in base asset.
    pub traded_quantity: String,
    /// Fee charged (negative = rebate). Crypto.com's wire field is `fees`.
    #[serde(default)]
    pub fees: Option<String>,
    /// Currency the fee is charged in.
    #[serde(default)]
    pub fee_instrument_name: Option<String>,
    /// Whether this fill was the taker or maker side (`"TAKER"` / `"MAKER"`).
    #[serde(default)]
    pub taker_side: Option<String>,
    /// Match ID linking the two sides of the trade.
    #[serde(default)]
    pub trade_match_id: Option<String>,
    /// Client-supplied order ID of the originating order.
    #[serde(default)]
    pub client_oid: Option<String>,
    /// Account UUID.
    #[serde(default)]
    pub account_id: Option<String>,
    /// Trade date (`YYYY-MM-DD`).
    #[serde(default)]
    pub event_date: Option<String>,
    /// Journal type, e.g. `"TRADING"`.
    #[serde(default)]
    pub journal_type: Option<String>,
    /// Fill time (ms since epoch).
    #[serde(default)]
    pub create_time: i64,
    /// Fill time (ns since epoch, kept as a string to avoid overflow).
    #[serde(default)]
    pub create_time_ns: Option<String>,
}

impl CryptocomPrivateTrade {
    /// Parse `traded_price` as `f64` (0.0 on a malformed value).
    #[must_use]
    pub fn traded_price_f64(&self) -> f64 {
        self.traded_price.parse().unwrap_or(0.0)
    }
    /// Parse `traded_quantity` as `f64` (0.0 on a malformed value).
    #[must_use]
    pub fn traded_quantity_f64(&self) -> f64 {
        self.traded_quantity.parse().unwrap_or(0.0)
    }
}

/// A saved deposit address from `private/get-deposit-address`.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomDepositAddress {
    /// Currency the address is for.
    pub currency: String,
    /// Address record ID (returned as a string).
    pub id: String,
    /// The deposit address (may carry a `?tag` / memo suffix on some chains).
    pub address: String,
    /// Status code as a string: `"0"` = inactive, `"1"` = active.
    pub status: String,
    /// Network / chain, e.g. `"BTC"`, `"ETH"`, `"CRO"`.
    #[serde(default)]
    pub network: Option<String>,
    /// Creation time (ms since epoch).
    #[serde(default)]
    pub create_time: i64,
}

/// `result` wrapper for `private/get-deposit-address`.
#[derive(Debug, Clone, Deserialize)]
struct DepositAddressList {
    #[serde(default = "Vec::new")]
    deposit_address_list: Vec<CryptocomDepositAddress>,
}

/// Acknowledgement from `private/create-withdrawal`.
///
/// Crypto.com's wallet endpoint sends `id`, `amount`, and `fee` as JSON
/// *numbers* here — unlike the string-typed trading endpoints, and unlike
/// `get-withdrawal-history`, where `id` comes back as a string. All three are
/// normalised to `String`.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomWithdrawalAck {
    /// Withdrawal ID (a JSON number on this endpoint; normalised to a string).
    #[serde(deserialize_with = "flex::string")]
    pub id: String,
    /// Withdrawal amount.
    #[serde(default, deserialize_with = "flex::opt_string")]
    pub amount: Option<String>,
    /// Network fee charged.
    #[serde(default, deserialize_with = "flex::opt_string")]
    pub fee: Option<String>,
    /// Currency symbol.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Destination address.
    #[serde(default)]
    pub address: Option<String>,
    /// Client-supplied withdrawal ID, echoed back.
    #[serde(default)]
    pub client_wid: Option<String>,
    /// Network ID (nullable).
    #[serde(default)]
    pub network_id: Option<String>,
    /// Creation time (ms since epoch).
    #[serde(default)]
    pub create_time: i64,
}

/// A withdrawal record from `private/get-withdrawal-history`.
///
/// `id` arrives as a JSON *string* here (contrast [`CryptocomWithdrawalAck`],
/// where the same field is a number); `amount` and `fee` are JSON numbers and
/// are normalised to `String`.
#[derive(Debug, Clone, Deserialize)]
pub struct CryptocomWithdrawal {
    /// Withdrawal ID (a string on this endpoint).
    #[serde(default, deserialize_with = "flex::string")]
    pub id: String,
    /// Currency symbol.
    pub currency: String,
    /// Withdrawal amount.
    #[serde(default, deserialize_with = "flex::opt_string")]
    pub amount: Option<String>,
    /// Network fee charged.
    #[serde(default, deserialize_with = "flex::opt_string")]
    pub fee: Option<String>,
    /// Destination address.
    #[serde(default)]
    pub address: Option<String>,
    /// Status code as a string (`"0"`..`"6"`); see Crypto.com's status table.
    pub status: String,
    /// On-chain transaction hash (empty until broadcast).
    #[serde(default)]
    pub txid: Option<String>,
    /// Client-supplied withdrawal ID.
    #[serde(default)]
    pub client_wid: Option<String>,
    /// Network ID (nullable).
    #[serde(default)]
    pub network_id: Option<String>,
    /// Creation time (ms since epoch).
    #[serde(default)]
    pub create_time: i64,
    /// Last-update time (ms since epoch).
    #[serde(default)]
    pub update_time: i64,
}

/// `result` wrapper for `private/get-withdrawal-history`.
#[derive(Debug, Clone, Deserialize)]
struct WithdrawalList {
    #[serde(default = "Vec::new")]
    withdrawal_list: Vec<CryptocomWithdrawal>,
}

// ── Client ──────────────────────────────────────────────────────────────────

/// Authenticated Crypto.com REST client.
///
/// Cheap to clone — shares the HTTP connection pool, credentials, and
/// both the `id` and `nonce` counters across handles. All methods are
/// `&self`.
#[derive(Clone)]
pub struct CryptocomPrivateClient {
    http: Client,
    base_url: String,
    credentials: CryptocomCredentials,
    /// Monotonic request ID. Crypto.com echoes it in the response so
    /// the caller can correlate; per-process uniqueness is enough.
    next_id: Arc<AtomicI64>,
    /// Monotonic nonce floor (ms since epoch). Strictly increases.
    nonce_state: Arc<AtomicU64>,
}

impl CryptocomPrivateClient {
    /// Build a client pointed at Crypto.com's live exchange API.
    pub fn new(credentials: CryptocomCredentials) -> Result<Self> {
        Self::with_base_url(credentials, BASE_URL)
    }

    /// Build a client with a caller-supplied base URL (tests, proxies).
    pub fn with_base_url(
        credentials: CryptocomCredentials,
        base_url: impl Into<String>,
    ) -> Result<Self> {
        crate::tls::ensure_crypto_provider();
        let http = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| ExchangeError::Config(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            http,
            base_url: base_url.into(),
            credentials,
            next_id: Arc::new(AtomicI64::new(1)),
            nonce_state: Arc::new(AtomicU64::new(0)),
        })
    }

    fn next_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    fn next_nonce(&self) -> i64 {
        // Same monotonic-with-floor pattern as KrakenPrivateClient.
        // The .max(0) before the cast guarantees the i64 is non-negative,
        // so the i64↔u64 round-trip preserves value.
        let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis().max(0)).unwrap_or(0);
        self.nonce_state.fetch_max(now_ms, Ordering::SeqCst);
        let next = self.nonce_state.fetch_add(1, Ordering::SeqCst);
        i64::try_from(next).unwrap_or(i64::MAX)
    }

    /// Sign and POST a private-API request. `method` is the wire method
    /// name (e.g. `"private/get-account-summary"`); the request `id` and
    /// `nonce` are injected by the client.
    ///
    /// Wrapped in [`send_with_retry`] for transient-network + HTTP 429
    /// (`Retry-After`) backoff. Each attempt mints a **fresh id + nonce** and
    /// re-signs, since Crypto.com replay-protects the nonce — a retry must not
    /// resend the previous attempt's signed envelope.
    async fn post<T: serde::de::DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        // Sign once up front so a signing (config) error surfaces immediately
        // rather than from inside the retry closure. The secret is fixed, so
        // signing is deterministic — once this succeeds it cannot fail later.
        let _ = self.sign_body(method, &params)?;

        debug!(method, "Crypto.com private POST");
        let url = format!("{}/{method}", self.base_url);
        let label = format!("Crypto.com POST {method}");
        let resp = send_with_retry(&label, || {
            // Fresh id + nonce + signature per attempt (nonce is replay-protected).
            let body = self
                .sign_body(method, &params)
                .expect("Crypto.com signing is deterministic and was validated above");
            self.http.post(&url).json(&body)
        })
        .await?;

        if !resp.status().is_success() {
            let code = resp.status().as_u16().to_string();
            let message = resp
                .text()
                .await
                .unwrap_or_else(|_| String::from("no body"));
            return Err(ExchangeError::Api { code, message });
        }

        let raw: Value = resp.json().await?;
        unwrap_cryptocom_envelope(raw)
    }

    /// Build a fully-signed request body (`id`/`method`/`api_key`/`params`/
    /// `nonce`/`sig`) for `method`.
    ///
    /// Each call mints a new `id` and monotonic `nonce`, so successive calls
    /// produce distinct, independently-signed envelopes — required for retries,
    /// since Crypto.com rejects a reused nonce.
    fn sign_body(&self, method: &str, params: &Value) -> Result<Value> {
        let id = self.next_id();
        let nonce = self.next_nonce();
        let sig = sign_cryptocom_request(
            method,
            id,
            &self.credentials.api_key,
            params,
            nonce,
            &self.credentials.api_secret,
        )?;

        Ok(json!({
            "id":      id,
            "method":  method,
            "api_key": self.credentials.api_key,
            "params":  params,
            "nonce":   nonce,
            "sig":     sig,
        }))
    }

    // ── Endpoints ────────────────────────────────────────────────────────────

    /// `POST /private/get-account-summary` — account balances; pass a
    /// currency to filter, or `None` for all.
    pub async fn get_account_summary(
        &self,
        currency: Option<&str>,
    ) -> Result<Vec<CryptocomBalance>> {
        let mut params = serde_json::Map::new();
        if let Some(c) = currency {
            params.insert("currency".into(), Value::String(c.to_string()));
        }
        let summary: AccountSummary = self
            .post("private/get-account-summary", Value::Object(params))
            .await?;
        Ok(summary.accounts)
    }

    /// `POST /private/create-order` — place a new order.
    #[allow(clippy::too_many_arguments, clippy::similar_names)]
    pub async fn place_order(
        &self,
        instrument: &str,
        side: &str,       // "BUY" or "SELL"
        order_type: &str, // "LIMIT", "MARKET", "STOP_LIMIT", ...
        quantity: &str,
        price: Option<&str>,
    ) -> Result<CryptocomOrderAck> {
        info!(
            instrument,
            side,
            order_type,
            quantity,
            ?price,
            "Crypto.com place order"
        );
        let mut params = json!({
            "instrument_name": instrument,
            "side": side,
            "type": order_type,
            "quantity": quantity,
        });
        if let Some(p) = price {
            params["price"] = Value::String(p.to_string());
        }
        self.post("private/create-order", params).await
    }

    /// `POST /private/cancel-order` — cancel a single order by its ID.
    pub async fn cancel_order(
        &self,
        instrument: &str,
        order_id: &str,
    ) -> Result<CryptocomOrderAck> {
        info!(instrument, order_id, "Crypto.com cancel order");
        self.post(
            "private/cancel-order",
            json!({"instrument_name": instrument, "order_id": order_id}),
        )
        .await
    }

    /// `POST /private/cancel-all-orders` — cancel every open order on
    /// `instrument`.
    pub async fn cancel_all_orders(&self, instrument: &str) -> Result<()> {
        info!(instrument, "Crypto.com cancel ALL open orders");
        // Crypto.com signals success with `code: 0` and an empty result body;
        // accept any shape and discard it.
        let _: serde::de::IgnoredAny = self
            .post(
                "private/cancel-all-orders",
                json!({"instrument_name": instrument}),
            )
            .await?;
        Ok(())
    }

    /// `POST /private/get-open-orders` — currently-open orders for
    /// `instrument` (or all instruments when `None`).
    pub async fn get_open_orders(&self, instrument: Option<&str>) -> Result<Vec<CryptocomOrder>> {
        let mut params = serde_json::Map::new();
        if let Some(i) = instrument {
            params.insert("instrument_name".into(), Value::String(i.to_string()));
        }
        let list: DataList<CryptocomOrder> = self
            .post("private/get-open-orders", Value::Object(params))
            .await?;
        Ok(list.data)
    }

    /// `POST /private/get-order-detail` — full detail for one order ID.
    pub async fn get_order_detail(&self, order_id: &str) -> Result<CryptocomOrder> {
        self.post("private/get-order-detail", json!({"order_id": order_id}))
            .await
    }

    /// `POST /private/get-trades` — trade history for an instrument (or
    /// all instruments when `None`).
    pub async fn get_trades(&self, instrument: Option<&str>) -> Result<Vec<CryptocomPrivateTrade>> {
        let mut params = serde_json::Map::new();
        if let Some(i) = instrument {
            params.insert("instrument_name".into(), Value::String(i.to_string()));
        }
        let list: DataList<CryptocomPrivateTrade> = self
            .post("private/get-trades", Value::Object(params))
            .await?;
        Ok(list.data)
    }

    /// `POST /private/get-deposit-address` — saved deposit addresses for
    /// a currency.
    pub async fn get_deposit_address(
        &self,
        currency: &str,
    ) -> Result<Vec<CryptocomDepositAddress>> {
        let list: DepositAddressList = self
            .post("private/get-deposit-address", json!({"currency": currency}))
            .await?;
        Ok(list.deposit_address_list)
    }

    /// `POST /private/create-withdrawal` — initiate a withdrawal.
    ///
    /// `address` must be on the account's pre-approved withdrawal list.
    pub async fn create_withdrawal(
        &self,
        currency: &str,
        amount: &str,
        address: &str,
    ) -> Result<CryptocomWithdrawalAck> {
        info!(currency, amount, address, "Crypto.com create withdrawal");
        self.post(
            "private/create-withdrawal",
            json!({
                "currency": currency,
                "amount": amount,
                "address": address,
            }),
        )
        .await
    }

    /// `POST /private/get-withdrawal-history` — withdrawal records for a
    /// currency.
    pub async fn get_withdrawal_history(&self, currency: &str) -> Result<Vec<CryptocomWithdrawal>> {
        let list: WithdrawalList = self
            .post(
                "private/get-withdrawal-history",
                json!({"currency": currency}),
            )
            .await?;
        Ok(list.withdrawal_list)
    }
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sim_client(base_url: &str) -> CryptocomPrivateClient {
        CryptocomPrivateClient::with_base_url(
            CryptocomCredentials::new("sim-key", "sim-secret"),
            base_url,
        )
        .expect("client build")
    }

    #[test]
    fn nonce_is_strictly_increasing_across_calls() {
        let c = sim_client("http://example.invalid");
        let mut prev = 0_i64;
        for _ in 0..1000 {
            let n = c.next_nonce();
            assert!(n > prev, "nonce did not increase: {prev} -> {n}");
            prev = n;
        }
    }

    #[test]
    fn id_is_strictly_increasing_across_calls() {
        let c = sim_client("http://example.invalid");
        let a = c.next_id();
        let b = c.next_id();
        let c2 = c.next_id();
        assert!(a < b && b < c2);
    }

    #[test]
    fn withdrawal_ack_coerces_numeric_id_and_amounts() {
        // `private/create-withdrawal` sends `id`/`amount`/`fee` as JSON numbers;
        // the typed model normalises them to strings.
        let ack: CryptocomWithdrawalAck =
            serde_json::from_str(r#"{"id": 2220, "amount": 1, "fee": 0.0004, "symbol": "BTC"}"#)
                .expect("deserialize withdrawal ack");
        assert_eq!(ack.id, "2220");
        assert_eq!(ack.amount.as_deref(), Some("1"));
        assert_eq!(ack.fee.as_deref(), Some("0.0004"));
        assert!(ack.network_id.is_none());
    }

    #[test]
    fn withdrawal_record_accepts_string_id() {
        // `private/get-withdrawal-history` sends the same `id` as a JSON string.
        let wd: CryptocomWithdrawal = serde_json::from_str(
            r#"{"id": "5275977", "currency": "BTC", "amount": 0.0005, "status": "5"}"#,
        )
        .expect("deserialize withdrawal record");
        assert_eq!(wd.id, "5275977");
        assert_eq!(wd.currency, "BTC");
        assert_eq!(wd.amount.as_deref(), Some("0.0005"));
    }

    #[test]
    fn order_deserializes_from_minimal_payload() {
        // Only the always-present fields are supplied; the rest must default
        // rather than fail the deserialise (order-detail omits the fee rates,
        // open-orders omits the trigger prices).
        let order: CryptocomOrder = serde_json::from_str(
            r#"{"order_id":"1","instrument_name":"BTC_USD","status":"ACTIVE","side":"BUY","order_type":"LIMIT","quantity":"0.5"}"#,
        )
        .expect("deserialize minimal order");
        assert_eq!(order.order_id, "1");
        assert!(order.limit_price.is_none());
        assert!(order.exec_inst.is_empty());
        assert_eq!(order.create_time, 0);
        assert!((order.quantity_f64() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn balance_coerces_numeric_and_string_amounts() {
        let bal: CryptocomBalance =
            serde_json::from_str(r#"{"currency":"BTC","balance":0.5,"available":"0.4"}"#)
                .expect("deserialize balance");
        assert_eq!(bal.balance.as_deref(), Some("0.5"));
        assert_eq!(bal.available.as_deref(), Some("0.4"));
        assert!(bal.stake.is_none());
    }
}
