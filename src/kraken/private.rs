//! Kraken authenticated REST — balance, orders, trades, ledger, withdrawals.
//!
//! Uses the HMAC-SHA512 signing scheme implemented in
//! [`crate::kraken::auth`]. The Kraken envelope is the same as the public
//! side (`{"result": ..., "error": []}`), so successful responses unwrap
//! through [`crate::kraken::rest::unwrap_kraken_envelope`].
//!
//! # Nonce strategy
//!
//! Kraken requires each authenticated request to carry a strictly
//! increasing nonce. [`KrakenPrivateClient`] seeds the nonce from the
//! current millisecond clock and uses an `AtomicU64` to guarantee
//! monotonicity across concurrent calls — even when two clones of the
//! same client race in different tasks. If the wall clock rewinds (NTP
//! step, suspend/resume), the atomic floor still grows so nonces remain
//! monotonic; the absolute value just stays ahead of the clock until
//! it catches up.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info};

use crate::error::{ExchangeError, Result};
use crate::http::send_with_retry;
use crate::kraken::auth::{KrakenCredentials, form_encode, sign_kraken_request};
use crate::kraken::rest::unwrap_kraken_envelope;

const BASE_URL: &str = "https://api.kraken.com";
const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 10;

// ── Response types ───────────────────────────────────────────────────────────

/// One open order from `POST /0/private/OpenOrders`. Models the fields
/// most callers need; pull additional fields from `raw` (via
/// [`serde_json::Value`]) if you need them.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenOrder {
    /// Order status: `"pending"`, `"open"`, `"closed"`, `"canceled"`,
    /// `"expired"`.
    pub status: String,
    /// Unix timestamp when the order was opened (seconds, fractional ok).
    #[serde(default)]
    pub opentm: Option<f64>,
    /// Order volume in base asset, as a string (preserve Kraken's precision).
    pub vol: String,
    /// Volume already executed.
    pub vol_exec: String,
    /// Cumulative cost in quote asset.
    pub cost: String,
    /// Cumulative fees.
    pub fee: String,
    /// Order description sub-object — pair, side, type, price etc.
    #[serde(default)]
    pub descr: Option<KrakenOrderDescr>,
}

/// Sub-object describing the order parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenOrderDescr {
    /// Trading pair (e.g. `"XBTUSD"`).
    pub pair: String,
    /// `"buy"` or `"sell"`.
    #[serde(rename = "type")]
    pub side: String,
    /// `"limit"`, `"market"`, `"stop-loss"`, `"take-profit"`, etc.
    pub ordertype: String,
    /// Primary price ("0" for market orders).
    #[serde(default)]
    pub price: String,
    /// Secondary price (used by stop/take-profit-limit orders).
    #[serde(default)]
    pub price2: String,
    /// Leverage as a string (`"none"` if unleveraged).
    #[serde(default)]
    pub leverage: String,
}

/// Response from `POST /0/private/OpenOrders`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenOpenOrders {
    /// Open orders keyed by Kraken transaction ID (e.g. `"O123-ABC-..."`).
    #[serde(default)]
    pub open: HashMap<String, KrakenOrder>,
}

/// Response from `POST /0/private/ClosedOrders`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenClosedOrders {
    /// Closed orders keyed by Kraken transaction ID.
    #[serde(default)]
    pub closed: HashMap<String, KrakenOrder>,
    /// Total count across all pages.
    #[serde(default)]
    pub count: u64,
}

/// Response from `POST /0/private/AddOrder`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenAddOrderResponse {
    /// Human-readable order description (e.g. `"buy 1.00 XBTUSD @ limit 30000"`).
    #[serde(default)]
    pub descr: Option<KrakenAddOrderDescr>,
    /// Transaction IDs created for the order. Usually one entry.
    #[serde(default)]
    pub txid: Vec<String>,
}

/// Inner `descr` of [`KrakenAddOrderResponse`].
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenAddOrderDescr {
    /// One-line plain-English summary returned by Kraken.
    pub order: String,
}

/// Response from `POST /0/private/CancelOrder` / `CancelAll`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenCancelResponse {
    /// Number of orders cancelled.
    #[serde(default)]
    pub count: u64,
}

/// Response from `POST /0/private/Withdraw`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenWithdrawResponse {
    /// Reference ID for the withdrawal — track with `WithdrawStatus`.
    pub refid: String,
}

/// One trade from `POST /0/private/TradesHistory`.
///
/// Numeric fields are kept as the raw Kraken string shape to preserve
/// precision (matching [`KrakenOrder`]); parse with `.parse::<f64>()` at
/// the call site if you need arithmetic.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenTradeHistoryEntry {
    /// Order transaction ID that produced this trade.
    pub ordertxid: String,
    /// Position transaction ID (empty when not part of a margin position).
    #[serde(default)]
    pub postxid: String,
    /// Trading pair (e.g. `"XXBTZUSD"`).
    pub pair: String,
    /// Unix timestamp of the trade (seconds, fractional).
    pub time: f64,
    /// `"buy"` or `"sell"`.
    #[serde(rename = "type")]
    pub side: String,
    /// `"limit"`, `"market"`, `"stop-loss"`, …
    pub ordertype: String,
    /// Execution price (quote currency).
    pub price: String,
    /// Total cost (price × volume, quote currency).
    pub cost: String,
    /// Fee charged for the trade.
    pub fee: String,
    /// Volume executed in base asset.
    pub vol: String,
    /// Initial margin posted (empty for non-margin trades).
    #[serde(default)]
    pub margin: String,
    /// Miscellaneous comma-separated flags.
    #[serde(default)]
    pub misc: String,
}

/// Response from `POST /0/private/TradesHistory`.
///
/// Kraken keys each trade by its trade ID inside a `trades` map and
/// reports the total across all pages in `count`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenTradesHistory {
    /// Trades keyed by trade ID (e.g. `"TZ5X4A-..."`).
    #[serde(default)]
    pub trades: HashMap<String, KrakenTradeHistoryEntry>,
    /// Total trade count across all pages.
    #[serde(default)]
    pub count: u64,
}

/// One ledger entry from `POST /0/private/Ledgers`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenLedgerEntry {
    /// Reference ID linking related ledger entries (e.g. both sides of a trade).
    pub refid: String,
    /// Unix timestamp of the entry (seconds, fractional).
    pub time: f64,
    /// Entry type — `"trade"`, `"deposit"`, `"withdrawal"`, `"transfer"`, …
    #[serde(rename = "type")]
    pub entry_type: String,
    /// Entry subtype (often empty).
    #[serde(default)]
    pub subtype: String,
    /// Asset class — typically `"currency"`.
    pub aclass: String,
    /// Asset code (e.g. `"ZUSD"`, `"XXBT"`).
    pub asset: String,
    /// Signed amount (negative = debit).
    pub amount: String,
    /// Fee charged for the operation.
    pub fee: String,
    /// Resulting running balance for the asset.
    pub balance: String,
}

/// Response from `POST /0/private/Ledgers`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenLedgers {
    /// Ledger entries keyed by ledger ID (e.g. `"LXXX..."`).
    #[serde(default)]
    pub ledger: HashMap<String, KrakenLedgerEntry>,
    /// Total ledger-entry count across all pages.
    #[serde(default)]
    pub count: u64,
}

/// One withdrawal record from `POST /0/private/WithdrawStatus`.
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenWithdrawalRecord {
    /// Withdrawal method (e.g. `"Bitcoin"`).
    #[serde(default)]
    pub method: String,
    /// Asset class — typically `"currency"`.
    #[serde(default)]
    pub aclass: String,
    /// Asset code being withdrawn.
    pub asset: String,
    /// Reference ID (matches the one returned by [`KrakenWithdrawResponse`]).
    pub refid: String,
    /// On-chain transaction ID once broadcast (absent while pending).
    #[serde(default)]
    pub txid: Option<String>,
    /// Free-form info field (often the destination address).
    #[serde(default)]
    pub info: String,
    /// Withdrawal amount (asset units).
    pub amount: String,
    /// Fee charged for the withdrawal.
    #[serde(default)]
    pub fee: String,
    /// Unix timestamp the withdrawal was requested (seconds).
    pub time: f64,
    /// Status — `"Initial"`, `"Pending"`, `"Settled"`, `"Success"`,
    /// `"Failure"`, …
    pub status: String,
}

/// Response of `POST /0/private/GetWebSocketsToken` — a short-lived token for
/// authenticating to the private WebSocket channels (`executions`/`balances`).
#[derive(Debug, Clone, Deserialize)]
pub struct KrakenWsToken {
    /// The token to place in `params.token` on a private subscribe frame
    /// (see [`KrakenConnector::executions_subscription`](crate::kraken::KrakenConnector::executions_subscription)).
    pub token: String,
    /// Seconds the token stays valid for the *initial* WS connection (Kraken
    /// returns `900`). Once subscribed, the connection persists past expiry.
    pub expires: i64,
}

// ── Client ───────────────────────────────────────────────────────────────────

/// Authenticated Kraken REST client.
///
/// Cheap to clone — shares the reqwest connection pool, credentials, and
/// the nonce counter across handles. All methods are `&self`.
#[derive(Clone)]
pub struct KrakenPrivateClient {
    http: Client,
    base_url: String,
    credentials: KrakenCredentials,
    nonce_state: Arc<AtomicU64>,
}

impl KrakenPrivateClient {
    /// Build a client pointed at Kraken's live API base URL.
    pub fn new(credentials: KrakenCredentials) -> Result<Self> {
        Self::with_base_url(credentials, BASE_URL)
    }

    /// Build a client with a caller-supplied base URL (tests, proxies).
    pub fn with_base_url(
        credentials: KrakenCredentials,
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
            nonce_state: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Strictly increasing nonce derived from the millisecond wall clock.
    fn next_nonce(&self) -> u64 {
        // Bump the floor to `now` (no-op if state is already ahead — e.g.
        // when many requests happen inside one ms). Then atomically
        // increment & return the previous value, guaranteeing uniqueness
        // across concurrent calls.
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        self.nonce_state.fetch_max(now_ms, Ordering::SeqCst);
        self.nonce_state.fetch_add(1, Ordering::SeqCst)
    }

    /// Sign and POST to a Kraken private endpoint.
    ///
    /// `params` is the form-body content WITHOUT the nonce — the client
    /// injects a fresh monotonic nonce at the front of the body.
    ///
    /// Wrapped in [`send_with_retry`] for transient-network + HTTP 429
    /// (`Retry-After`) backoff. Each attempt mints a **fresh nonce** and
    /// re-signs: Kraken rejects a reused nonce as a replay, so a retry must
    /// not resend the previous attempt's signed body.
    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        // Sign once up front so a malformed-secret error surfaces immediately
        // (and not buried inside the retry closure). The secret is fixed, so
        // signing is deterministic — once this succeeds it cannot fail on a
        // later attempt.
        let _ = self.sign_body(path, params)?;

        debug!(path, "Kraken private POST");
        let url = format!("{}{path}", self.base_url);
        let label = format!("Kraken POST {path}");
        let resp = send_with_retry(&label, || {
            // Fresh nonce + signature per attempt (nonce is replay-protected).
            let (body, sig) = self
                .sign_body(path, params)
                .expect("Kraken signing is deterministic and was validated above");
            self.http
                .post(&url)
                .header("API-Key", &self.credentials.api_key)
                .header("API-Sign", sig)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(body)
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
        unwrap_kraken_envelope(raw)
    }

    /// Build a fresh nonce-prefixed form body and its Kraken signature.
    ///
    /// Returns `(post_body, api_sign)`. Each call mints a new monotonic nonce,
    /// so successive calls produce distinct bodies/signatures — required for
    /// retries, since Kraken rejects a reused nonce.
    fn sign_body(&self, path: &str, params: &[(&str, &str)]) -> Result<(String, String)> {
        let nonce = self.next_nonce();
        let nonce_str = nonce.to_string();

        // Nonce must come first so the post-body string the server hashes
        // matches what we sign. Kraken docs show nonce as the first field.
        let mut all_params: Vec<(&str, &str)> = Vec::with_capacity(params.len() + 1);
        all_params.push(("nonce", &nonce_str));
        all_params.extend_from_slice(params);
        let body = form_encode(&all_params);

        let sig = sign_kraken_request(path, nonce, &body, &self.credentials.api_secret_b64)?;
        Ok((body, sig))
    }

    // ── Endpoints ───────────────────────────────────────────────────────────

    /// `POST /0/private/Balance` — every asset balance.
    ///
    /// Returns a map from asset code (e.g. `"XXBT"`) to a string amount.
    pub async fn get_balance(&self) -> Result<HashMap<String, String>> {
        info!("Fetching Kraken balance");
        self.post("/0/private/Balance", &[]).await
    }

    /// `POST /0/private/OpenOrders` — currently-open orders.
    pub async fn get_open_orders(&self) -> Result<KrakenOpenOrders> {
        self.post("/0/private/OpenOrders", &[]).await
    }

    /// `POST /0/private/GetWebSocketsToken` — a short-lived token for the
    /// private WS channels (`executions` / `balances`).
    ///
    /// Pass [`KrakenWsToken::token`] to
    /// [`KrakenConnector::executions_subscription`](crate::kraken::KrakenConnector::executions_subscription)
    /// / [`balances_subscription`](crate::kraken::KrakenConnector::balances_subscription)
    /// and subscribe on the [`private`](crate::kraken::KrakenConnector::private)
    /// endpoint.
    pub async fn get_websockets_token(&self) -> Result<KrakenWsToken> {
        info!("Fetching Kraken WebSockets token");
        self.post("/0/private/GetWebSocketsToken", &[]).await
    }

    /// `POST /0/private/ClosedOrders` — historical orders (paginated
    /// server-side; this surfaces the first page).
    pub async fn get_closed_orders(&self) -> Result<KrakenClosedOrders> {
        self.post("/0/private/ClosedOrders", &[]).await
    }

    /// `POST /0/private/AddOrder` — place a new order.
    ///
    /// `volume` is a string in base-asset units (Kraken accepts decimal
    /// strings up to the pair's `lot_decimals` precision). `price` is
    /// required for limit orders and ignored for market orders.
    #[allow(clippy::too_many_arguments)]
    pub async fn place_order(
        &self,
        pair: &str,
        side: &str,       // "buy" or "sell"
        order_type: &str, // "limit", "market", "stop-loss", ...
        volume: &str,
        price: Option<&str>,
    ) -> Result<KrakenAddOrderResponse> {
        info!(pair, side, order_type, volume, ?price, "Kraken place order");
        let mut params: Vec<(&str, &str)> = vec![
            ("pair", pair),
            ("type", side),
            ("ordertype", order_type),
            ("volume", volume),
        ];
        if let Some(p) = price {
            params.push(("price", p));
        }
        self.post("/0/private/AddOrder", &params).await
    }

    /// `POST /0/private/CancelOrder` — cancel a single order by txid.
    pub async fn cancel_order(&self, txid: &str) -> Result<KrakenCancelResponse> {
        info!(txid, "Kraken cancel order");
        self.post("/0/private/CancelOrder", &[("txid", txid)]).await
    }

    /// `POST /0/private/CancelAll` — cancel every open order.
    pub async fn cancel_all_orders(&self) -> Result<KrakenCancelResponse> {
        info!("Kraken cancel ALL open orders");
        self.post("/0/private/CancelAll", &[]).await
    }

    /// `POST /0/private/TradesHistory` — trade history (paginated;
    /// surfaces the first page).
    pub async fn get_trades_history(&self) -> Result<KrakenTradesHistory> {
        self.post("/0/private/TradesHistory", &[]).await
    }

    /// `POST /0/private/Ledgers` — ledger entries for an asset
    /// (paginated; surfaces the first page).
    pub async fn get_ledger(&self, asset: &str) -> Result<KrakenLedgers> {
        self.post("/0/private/Ledgers", &[("asset", asset)]).await
    }

    /// `POST /0/private/Withdraw` — withdraw funds to a pre-registered
    /// withdrawal `key`.
    ///
    /// `key` is the label of a withdrawal address you've previously
    /// authorised on the Kraken account. Returns the `refid` to track
    /// the withdrawal via [`Self::get_withdrawal_status`].
    pub async fn withdraw(
        &self,
        asset: &str,
        key: &str,
        amount: &str,
    ) -> Result<KrakenWithdrawResponse> {
        info!(asset, key, amount, "Kraken withdraw");
        self.post(
            "/0/private/Withdraw",
            &[("asset", asset), ("key", key), ("amount", amount)],
        )
        .await
    }

    /// `POST /0/private/WithdrawStatus` — recent withdrawals for an asset.
    ///
    /// Kraken returns the records as a JSON array; this deserialises into a
    /// `Vec<KrakenWithdrawalRecord>`.
    pub async fn get_withdrawal_status(&self, asset: &str) -> Result<Vec<KrakenWithdrawalRecord>> {
        self.post("/0/private/WithdrawStatus", &[("asset", asset)])
            .await
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sim_client(base_url: &str) -> KrakenPrivateClient {
        // Test secret computed at runtime so the file contains no
        // high-entropy base64 literal for secret scanners to trip on.
        use base64::Engine;
        let secret = base64::engine::general_purpose::STANDARD.encode(b"sim-secret");
        KrakenPrivateClient::with_base_url(KrakenCredentials::new("sim-key", secret), base_url)
            .expect("client build")
    }

    #[test]
    fn nonce_is_strictly_increasing_across_calls() {
        let c = sim_client("http://example.invalid");
        let mut prev = 0_u64;
        for _ in 0..1000 {
            let n = c.next_nonce();
            assert!(n > prev, "nonce did not increase: {prev} -> {n}");
            prev = n;
        }
    }

    #[test]
    fn ws_token_deserializes() {
        // Shape of the `result` object after `unwrap_kraken_envelope`.
        let raw = r#"{"token":"abc-123-token","expires":900}"#;
        let t: KrakenWsToken = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(t.token, "abc-123-token");
        assert_eq!(t.expires, 900);
    }

    #[test]
    fn order_deserializes_minimum_fields() {
        let raw = r#"{
            "status": "open",
            "opentm": 1700000000.5,
            "vol": "1.00000000",
            "vol_exec": "0.50000000",
            "cost": "30000.0",
            "fee": "5.0",
            "descr": {
                "pair": "XBTUSD",
                "type": "buy",
                "ordertype": "limit",
                "price": "30000",
                "price2": "0",
                "leverage": "none"
            }
        }"#;
        let o: KrakenOrder = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(o.status, "open");
        assert_eq!(o.opentm, Some(1_700_000_000.5));
        assert_eq!(o.descr.unwrap().pair, "XBTUSD");
    }

    #[test]
    fn add_order_response_with_txid() {
        let raw = r#"{
            "descr": {"order": "buy 1.00 XBTUSD @ limit 30000"},
            "txid": ["OQCLML-BW3P3-BUCMWZ"]
        }"#;
        let r: KrakenAddOrderResponse = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(r.txid.len(), 1);
        assert!(r.descr.unwrap().order.contains("buy 1.00"));
    }

    #[test]
    fn cancel_response_count() {
        let raw = r#"{"count": 3}"#;
        let r: KrakenCancelResponse = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(r.count, 3);
    }

    #[test]
    fn trades_history_deserializes_keyed_map() {
        let raw = r#"{
            "trades": {
                "TZ5X4A-ABCDE-FGHIJK": {
                    "ordertxid": "OQCLML-BW3P3-BUCMWZ",
                    "postxid": "",
                    "pair": "XXBTZUSD",
                    "time": 1700000000.1234,
                    "type": "buy",
                    "ordertype": "limit",
                    "price": "30000.0",
                    "cost": "30000.0",
                    "fee": "48.0",
                    "vol": "1.0",
                    "margin": "0.0",
                    "misc": ""
                }
            },
            "count": 1
        }"#;
        let h: KrakenTradesHistory = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(h.count, 1);
        let t = &h.trades["TZ5X4A-ABCDE-FGHIJK"];
        assert_eq!(t.pair, "XXBTZUSD");
        assert_eq!(t.side, "buy");
        assert_eq!(t.ordertxid, "OQCLML-BW3P3-BUCMWZ");
        assert!((t.time - 1_700_000_000.123_4).abs() < 1e-6);
    }

    #[test]
    fn ledgers_deserialize_keyed_map() {
        let raw = r#"{
            "ledger": {
                "L4UESK-KG3EQ-UFO4T5": {
                    "refid": "TY5BYV-WLD5M-ABCDEF",
                    "time": 1700000000.0,
                    "type": "trade",
                    "subtype": "",
                    "aclass": "currency",
                    "asset": "ZUSD",
                    "amount": "-30000.0",
                    "fee": "48.0",
                    "balance": "12345.6"
                }
            },
            "count": 1
        }"#;
        let l: KrakenLedgers = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(l.count, 1);
        let e = &l.ledger["L4UESK-KG3EQ-UFO4T5"];
        assert_eq!(e.entry_type, "trade");
        assert_eq!(e.asset, "ZUSD");
        assert_eq!(e.amount, "-30000.0");
    }

    #[test]
    fn withdrawal_record_handles_pending_without_txid() {
        // A pending withdrawal has no on-chain txid yet.
        let raw = r#"{
            "method": "Bitcoin",
            "aclass": "currency",
            "asset": "XXBT",
            "refid": "FTQcuak-V6Za8qrPnhsw47JfVff",
            "info": "bc1qexample",
            "amount": "0.05",
            "fee": "0.00015",
            "time": 1700000000.0,
            "status": "Pending"
        }"#;
        let w: KrakenWithdrawalRecord = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(w.asset, "XXBT");
        assert_eq!(w.status, "Pending");
        assert!(w.txid.is_none());
        assert_eq!(w.amount, "0.05");
    }

    #[test]
    fn withdrawal_record_with_settled_txid() {
        let raw = r#"{
            "method": "Bitcoin",
            "aclass": "currency",
            "asset": "XXBT",
            "refid": "FTQcuak-V6Za8qrPnhsw47JfVff",
            "txid": "deadbeef...",
            "info": "bc1qexample",
            "amount": "0.05",
            "fee": "0.00015",
            "time": 1700000000.0,
            "status": "Success"
        }"#;
        let w: KrakenWithdrawalRecord = serde_json::from_str(raw).expect("deserialize");
        assert_eq!(w.txid.as_deref(), Some("deadbeef..."));
        assert_eq!(w.status, "Success");
    }
}
