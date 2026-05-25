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
    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let nonce = self.next_nonce();
        let nonce_str = nonce.to_string();

        // Nonce must come first so the post-body string the server hashes
        // matches what we sign. Kraken docs show nonce as the first field.
        let mut all_params: Vec<(&str, &str)> = Vec::with_capacity(params.len() + 1);
        all_params.push(("nonce", &nonce_str));
        all_params.extend_from_slice(params);
        let body = form_encode(&all_params);

        let sig = sign_kraken_request(path, nonce, &body, &self.credentials.api_secret_b64)?;

        debug!(path, "Kraken private POST");
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("API-Key", &self.credentials.api_key)
            .header("API-Sign", &sig)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
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

    /// `POST /0/private/TradesHistory` — trade history (paginated).
    ///
    /// Returns raw [`serde_json::Value`] — the response wraps a
    /// `trades` map plus a `count`, and the per-trade shape has many
    /// fields. Deserialize a tailored type with
    /// [`serde_json::from_value`] if you need typed access.
    pub async fn get_trades_history(&self) -> Result<Value> {
        self.post("/0/private/TradesHistory", &[]).await
    }

    /// `POST /0/private/Ledgers` — ledger entries for an asset.
    ///
    /// Returns raw [`serde_json::Value`]. Per-entry shape includes
    /// `refid`, `time`, `type`, `aclass`, `asset`, `amount`, `fee`,
    /// `balance`.
    pub async fn get_ledger(&self, asset: &str) -> Result<Value> {
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
    /// Returns raw [`serde_json::Value`] (array of withdrawal records).
    pub async fn get_withdrawal_status(&self, asset: &str) -> Result<Value> {
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
        KrakenPrivateClient::with_base_url(
            KrakenCredentials::new("sim-key", secret),
            base_url,
        )
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
}
