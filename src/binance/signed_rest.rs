//! Binance spot **signed** REST — the HMAC-SHA256 `SIGNED` surface.
//!
//! Distinct from [`crate::binance::BinanceUserDataRest`] (the API-key-only
//! `listenKey` lifecycle): every request here is HMAC-SHA256 signed per
//! [`crate::binance::auth`] and carries the `X-MBX-APIKEY` header.
//!
//! | Method | Endpoint | Verb |
//! |--------|----------|------|
//! | [`get_account`](BinanceSignedRest::get_account) | `/api/v3/account` | GET |
//! | [`place_order`](BinanceSignedRest::place_order) | `/api/v3/order` | POST |
//!
//! Each request appends `recvWindow` + `timestamp`, signs the exact query
//! string, and appends `&signature=…`. Binance error bodies
//! (`{"code":-2015,"msg":"…"}`) surface as [`ExchangeError::Api`].
//!
//! **Security:** the API secret and the request signature are never logged —
//! only the endpoint path appears in trace output.
//!
//! ```no_run
//! # use exchange_apiws::binance::{BinanceCredentials, BinanceSignedRest};
//! # async fn example() -> exchange_apiws::Result<()> {
//! let creds = BinanceCredentials::from_env()?;
//! let rest = BinanceSignedRest::new(creds)?;
//! let account = rest.get_account().await?;
//! for bal in account.non_zero_balances() {
//!     println!("{}: free={} locked={}", bal.asset, bal.free_f64(), bal.locked_f64());
//! }
//! # Ok(())
//! # }
//! ```

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::debug;

use super::auth::{API_KEY_HEADER, BinanceCredentials, DEFAULT_RECV_WINDOW};
use crate::error::{ExchangeError, Result};
use crate::http::{percent_encode, send_with_retry};

/// Binance spot REST base URL.
const SPOT_BASE_URL: &str = "https://api.binance.com";

// ── Wire-type helpers ─────────────────────────────────────────────────────────

/// Deserialise helpers for Binance's mixed string/number wire types.
///
/// Balances arrive as JSON *strings* (`"free": "0.01"`), preserving exchange
/// precision, but the helper also tolerates JSON numbers for
/// forward-compatibility — keeping the crate-wide "numbers stay strings to keep
/// wire precision" convention.
mod flex {
    use std::fmt;

    use serde::de::{self, Deserializer, Visitor};

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
}

/// Parse a wire-decimal string to `f64`; `0.0` when malformed.
fn parse_f64(s: &str) -> f64 {
    s.parse().unwrap_or(0.0)
}

// ── Response types ────────────────────────────────────────────────────────────

/// One asset balance from `GET /api/v3/account`.
///
/// `free` / `locked` are Binance's raw wire strings (parse with the `_f64`
/// accessors where arithmetic is needed) so full exchange precision is
/// preserved.
#[derive(Debug, Clone, Deserialize)]
pub struct BinanceBalance {
    /// Asset symbol (e.g. `"BTC"`, `"USDT"`).
    pub asset: String,
    /// Balance free for new orders / withdrawals.
    #[serde(deserialize_with = "flex::string")]
    pub free: String,
    /// Balance locked in open orders.
    #[serde(deserialize_with = "flex::string")]
    pub locked: String,
}

impl BinanceBalance {
    /// Parse `free` as `f64` (`0.0` on a malformed value).
    #[must_use]
    pub fn free_f64(&self) -> f64 {
        parse_f64(&self.free)
    }

    /// Parse `locked` as `f64` (`0.0` on a malformed value).
    #[must_use]
    pub fn locked_f64(&self) -> f64 {
        parse_f64(&self.locked)
    }

    /// `free + locked` as `f64` (`0.0` on malformed values).
    #[must_use]
    pub fn total_f64(&self) -> f64 {
        self.free_f64() + self.locked_f64()
    }
}

/// Typed account snapshot from `GET /api/v3/account`.
///
/// Trade / withdraw / deposit permission flags plus the per-asset
/// [`balances`](Self::balances) list. Fields absent from a given account
/// state default rather than fail the deserialise.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceAccountInfo {
    /// Maker commission rate in basis points (e.g. `15` = 0.15%).
    #[serde(default)]
    pub maker_commission: i64,
    /// Taker commission rate in basis points.
    #[serde(default)]
    pub taker_commission: i64,
    /// Buyer commission rate in basis points.
    #[serde(default)]
    pub buyer_commission: i64,
    /// Seller commission rate in basis points.
    #[serde(default)]
    pub seller_commission: i64,
    /// Whether the account may place trades.
    #[serde(default)]
    pub can_trade: bool,
    /// Whether the account may withdraw.
    #[serde(default)]
    pub can_withdraw: bool,
    /// Whether the account may deposit.
    #[serde(default)]
    pub can_deposit: bool,
    /// Account type, e.g. `"SPOT"`.
    #[serde(default)]
    pub account_type: Option<String>,
    /// Last account-update time (ms since epoch).
    #[serde(default)]
    pub update_time: i64,
    /// Numeric account/user id.
    #[serde(default)]
    pub uid: i64,
    /// Enabled trading permissions, e.g. `["SPOT"]`.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Per-asset balances.
    #[serde(default)]
    pub balances: Vec<BinanceBalance>,
}

impl BinanceAccountInfo {
    /// Iterate the balances whose `free + locked` is greater than zero — the
    /// common case where the caller only cares about held assets.
    pub fn non_zero_balances(&self) -> impl Iterator<Item = &BinanceBalance> {
        self.balances.iter().filter(|b| b.total_f64() > 0.0)
    }

    /// Look up the balance for a single `asset` (case-sensitive, matching
    /// Binance's uppercase symbols).
    #[must_use]
    pub fn balance(&self, asset: &str) -> Option<&BinanceBalance> {
        self.balances.iter().find(|b| b.asset == asset)
    }
}

/// Acknowledgement from `POST /api/v3/order` (the default `ACK`/`RESULT`
/// response fields common to a spot new-order reply).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceOrderAck {
    /// Symbol the order was placed on.
    pub symbol: String,
    /// Server-assigned order id.
    pub order_id: i64,
    /// Client-supplied order id, echoed back.
    #[serde(default)]
    pub client_order_id: Option<String>,
    /// Order status (e.g. `"NEW"`, `"FILLED"`), present on `RESULT`/`FULL`.
    #[serde(default)]
    pub status: Option<String>,
    /// Transaction time (ms since epoch).
    #[serde(default)]
    pub transact_time: i64,
}

/// Binance's error envelope (`{"code":-2015,"msg":"…"}`).
#[derive(Debug, Deserialize)]
struct BinanceApiError {
    code: i64,
    msg: String,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Authenticated Binance spot REST client for the signed surface.
///
/// Cheap to clone — shares the HTTP connection pool and credentials. All
/// methods are `&self`.
#[derive(Clone)]
pub struct BinanceSignedRest {
    http: Client,
    base_url: String,
    creds: BinanceCredentials,
    recv_window: u64,
}

impl BinanceSignedRest {
    /// Build a client pointed at Binance's live spot base URL.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeError::Http`] if the HTTP client cannot be built.
    pub fn new(creds: BinanceCredentials) -> Result<Self> {
        Self::with_base_url(creds, SPOT_BASE_URL)
    }

    /// Build a client with a caller-supplied base URL — used by integration
    /// tests pointing at `wiremock`, and by callers proxying through a custom
    /// domain or the testnet (`https://testnet.binance.vision`).
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_base_url(creds: BinanceCredentials, base_url: impl Into<String>) -> Result<Self> {
        crate::tls::ensure_crypto_provider();
        Ok(Self {
            http: Client::builder().build()?,
            base_url: base_url.into(),
            creds,
            recv_window: DEFAULT_RECV_WINDOW,
        })
    }

    /// Override the signed `recvWindow` (ms, capped by Binance at 60000).
    #[must_use]
    pub const fn with_recv_window(mut self, recv_window: u64) -> Self {
        self.recv_window = recv_window;
        self
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default()
    }

    /// Build the fully-signed query string for `params`.
    ///
    /// Appends `recvWindow` + a fresh `timestamp`, HMAC-signs the exact bytes,
    /// and appends `&signature=…`. Every value is percent-encoded, and the same
    /// string is both signed and transmitted so the signature always covers the
    /// literal request.
    fn signed_query(&self, params: &[(&str, &str)]) -> String {
        let mut q = String::new();
        for (k, v) in params {
            if !q.is_empty() {
                q.push('&');
            }
            q.push_str(&percent_encode(k));
            q.push('=');
            q.push_str(&percent_encode(v));
        }
        if !q.is_empty() {
            q.push('&');
        }
        let _ = write!(
            q,
            "recvWindow={}&timestamp={}",
            self.recv_window,
            Self::now_ms()
        );
        let sig = self.creds.sign_query(&q);
        q.push_str("&signature=");
        q.push_str(&sig);
        q
    }

    /// Signed GET. Wrapped in [`send_with_retry`]; the query — including its
    /// fresh timestamp + signature — is rebuilt per attempt so a stale
    /// signature is never resent. Only the path is logged (never the signature).
    async fn signed_get<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        debug!(path, "Binance signed GET");
        let label = format!("Binance GET {path}");
        let resp = send_with_retry(&label, || {
            let query = self.signed_query(params);
            self.http
                .get(format!("{}{path}?{query}", self.base_url))
                .header(API_KEY_HEADER, &self.creds.api_key)
        })
        .await?;
        unwrap_binance(resp).await
    }

    /// Signed POST. Binance accepts signed params in the query string for POST
    /// endpoints; the body is empty. Retried like [`signed_get`](Self::signed_get).
    async fn signed_post<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        debug!(path, "Binance signed POST");
        let label = format!("Binance POST {path}");
        let resp = send_with_retry(&label, || {
            let query = self.signed_query(params);
            self.http
                .post(format!("{}{path}?{query}", self.base_url))
                .header(API_KEY_HEADER, &self.creds.api_key)
        })
        .await?;
        unwrap_binance(resp).await
    }

    // ── Endpoints ─────────────────────────────────────────────────────────────

    /// `GET /api/v3/account` — current account info and per-asset balances.
    ///
    /// # Errors
    ///
    /// [`ExchangeError::Api`] on a Binance error envelope (e.g. an invalid
    /// API key or signature), [`ExchangeError::Http`] on transport failure, or
    /// [`ExchangeError::Json`] if the success body can't be decoded.
    pub async fn get_account(&self) -> Result<BinanceAccountInfo> {
        self.signed_get("/api/v3/account", &[]).await
    }

    /// `POST /api/v3/order` — place a new spot order.
    ///
    /// `side` is `"BUY"`/`"SELL"`, `order_type` a Binance type (`"LIMIT"`,
    /// `"MARKET"`, …). `LIMIT` orders require `time_in_force` (e.g. `"GTC"`) and
    /// `price`; `MARKET` orders omit both. `quantity` is the base-asset amount.
    ///
    /// # Errors
    ///
    /// As [`get_account`](Self::get_account) — a rejected order surfaces as
    /// [`ExchangeError::Api`] carrying Binance's `{code,msg}`.
    #[allow(clippy::too_many_arguments)]
    pub async fn place_order(
        &self,
        symbol: &str,
        side: &str,
        order_type: &str,
        quantity: &str,
        price: Option<&str>,
        time_in_force: Option<&str>,
    ) -> Result<BinanceOrderAck> {
        let mut params: Vec<(&str, &str)> = vec![
            ("symbol", symbol),
            ("side", side),
            ("type", order_type),
            ("quantity", quantity),
        ];
        if let Some(p) = price {
            params.push(("price", p));
        }
        if let Some(tif) = time_in_force {
            params.push(("timeInForce", tif));
        }
        self.signed_post("/api/v3/order", &params).await
    }
}

/// Decode a Binance signed-REST response. Non-2xx bodies are parsed as the
/// `{code,msg}` error envelope → [`ExchangeError::Api`]; a 2xx body is
/// deserialised into `T`.
async fn unwrap_binance<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() {
        if let Ok(err) = serde_json::from_slice::<BinanceApiError>(&bytes) {
            return Err(ExchangeError::Api {
                code: err.code.to_string(),
                message: err.msg,
            });
        }
        return Err(ExchangeError::Api {
            code: status.as_u16().to_string(),
            message: String::from_utf8_lossy(&bytes).into_owned(),
        });
    }
    Ok(serde_json::from_slice(&bytes)?)
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_info_deserializes_with_balances() {
        // Representative `GET /api/v3/account` body (trimmed).
        let raw = r#"{
            "makerCommission": 15,
            "takerCommission": 15,
            "buyerCommission": 0,
            "sellerCommission": 0,
            "canTrade": true,
            "canWithdraw": true,
            "canDeposit": true,
            "updateTime": 123456789,
            "accountType": "SPOT",
            "balances": [
                {"asset": "BTC", "free": "4723846.89208129", "locked": "0.00000000"},
                {"asset": "LTC", "free": "0.00000000", "locked": "0.00000000"},
                {"asset": "USDT", "free": "100.50", "locked": "10.00"}
            ],
            "permissions": ["SPOT"],
            "uid": 354937868
        }"#;
        let acct: BinanceAccountInfo = serde_json::from_str(raw).expect("deserialize account info");
        assert!(acct.can_trade);
        assert_eq!(acct.account_type.as_deref(), Some("SPOT"));
        assert_eq!(acct.maker_commission, 15);
        assert_eq!(acct.uid, 354_937_868);
        assert_eq!(acct.balances.len(), 3);
        // Wire strings preserved exactly.
        assert_eq!(acct.balance("BTC").unwrap().free, "4723846.89208129");
        assert!((acct.balance("USDT").unwrap().total_f64() - 110.50).abs() < 1e-9);
        // Only BTC and USDT carry a non-zero balance.
        let held: Vec<&str> = acct.non_zero_balances().map(|b| b.asset.as_str()).collect();
        assert_eq!(held, vec!["BTC", "USDT"]);
    }

    #[test]
    fn balance_tolerates_numeric_wire_form() {
        // Strings are the norm, but the `flex` helper also accepts JSON numbers.
        let bal: BinanceBalance = serde_json::from_str(r#"{"asset":"ETH","free":1.5,"locked":0}"#)
            .expect("deserialize numeric balance");
        assert_eq!(bal.free, "1.5");
        assert_eq!(bal.locked, "0");
        assert!((bal.free_f64() - 1.5).abs() < 1e-9);
    }

    #[test]
    fn account_info_malformed_errs_not_panics() {
        // `balances` as a string instead of an array must Err, never panic.
        let malformed = r#"{"accountType":"SPOT","balances":"not-an-array"}"#;
        let res: std::result::Result<BinanceAccountInfo, _> = serde_json::from_str(malformed);
        assert!(res.is_err(), "malformed balances must Err");

        // A balance missing the required `free` field must also Err.
        let bad_balance = r#"{"balances":[{"asset":"BTC","locked":"0"}]}"#;
        let res2: std::result::Result<BinanceAccountInfo, _> = serde_json::from_str(bad_balance);
        assert!(res2.is_err(), "balance without `free` must Err");

        // A wrongly-typed scalar (bool given a string) must Err.
        let bad_bool = r#"{"canTrade":"yes","balances":[]}"#;
        let res3: std::result::Result<BinanceAccountInfo, _> = serde_json::from_str(bad_bool);
        assert!(res3.is_err(), "non-bool canTrade must Err");
    }

    #[test]
    fn order_ack_deserializes() {
        let raw = r#"{
            "symbol": "BTCUSDT",
            "orderId": 28,
            "clientOrderId": "6gCrw2kRUAF9CvJDGP16IP",
            "transactTime": 1507725176595,
            "status": "NEW"
        }"#;
        let ack: BinanceOrderAck = serde_json::from_str(raw).expect("deserialize order ack");
        assert_eq!(ack.symbol, "BTCUSDT");
        assert_eq!(ack.order_id, 28);
        assert_eq!(ack.status.as_deref(), Some("NEW"));
    }

    #[test]
    fn error_envelope_maps_to_api_error() {
        // The `{code,msg}` shape Binance returns on a rejected signed request.
        let err: BinanceApiError = serde_json::from_str(
            r#"{"code":-2015,"msg":"Invalid API-key, IP, or permissions for action."}"#,
        )
        .expect("deserialize error envelope");
        let mapped = ExchangeError::Api {
            code: err.code.to_string(),
            message: err.msg,
        };
        match mapped {
            ExchangeError::Api { code, message } => {
                assert_eq!(code, "-2015");
                assert!(message.contains("Invalid API-key"));
            }
            _ => panic!("expected Api error"),
        }
    }

    #[test]
    fn signed_query_covers_sent_bytes_and_appends_signature() {
        let client = BinanceSignedRest::with_base_url(
            BinanceCredentials::new("k", "secret"),
            "http://example.invalid",
        )
        .expect("build client");
        let q = client.signed_query(&[("symbol", "BTCUSDT")]);
        // The transmitted query carries the params, recvWindow, timestamp, and a
        // trailing signature.
        assert!(q.starts_with("symbol=BTCUSDT&recvWindow=5000&timestamp="));
        let (payload, sig_part) = q.rsplit_once("&signature=").expect("has signature");
        // The signature must be the HMAC of the exact preceding bytes — proving
        // the signed string equals the sent string.
        assert_eq!(
            sig_part,
            BinanceCredentials::new("k", "secret").sign_query(payload)
        );
        assert_eq!(sig_part.len(), 64);
    }
}
