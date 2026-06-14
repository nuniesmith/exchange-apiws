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
use serde_json::{Value, json};
use tracing::{debug, info};

use crate::cryptocom::auth::{CryptocomCredentials, sign_cryptocom_request};
use crate::cryptocom::rest::unwrap_cryptocom_envelope;
use crate::error::{ExchangeError, Result};
use crate::http::send_with_retry;

const BASE_URL: &str = "https://api.crypto.com/exchange/v1";
const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 10;

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
    pub async fn get_account_summary(&self, currency: Option<&str>) -> Result<Value> {
        let mut params = serde_json::Map::new();
        if let Some(c) = currency {
            params.insert("currency".into(), Value::String(c.to_string()));
        }
        self.post("private/get-account-summary", Value::Object(params))
            .await
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
    ) -> Result<Value> {
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
    pub async fn cancel_order(&self, instrument: &str, order_id: &str) -> Result<Value> {
        info!(instrument, order_id, "Crypto.com cancel order");
        self.post(
            "private/cancel-order",
            json!({"instrument_name": instrument, "order_id": order_id}),
        )
        .await
    }

    /// `POST /private/cancel-all-orders` — cancel every open order on
    /// `instrument`.
    pub async fn cancel_all_orders(&self, instrument: &str) -> Result<Value> {
        info!(instrument, "Crypto.com cancel ALL open orders");
        self.post(
            "private/cancel-all-orders",
            json!({"instrument_name": instrument}),
        )
        .await
    }

    /// `POST /private/get-open-orders` — currently-open orders for
    /// `instrument` (or all instruments when `None`).
    pub async fn get_open_orders(&self, instrument: Option<&str>) -> Result<Value> {
        let mut params = serde_json::Map::new();
        if let Some(i) = instrument {
            params.insert("instrument_name".into(), Value::String(i.to_string()));
        }
        self.post("private/get-open-orders", Value::Object(params))
            .await
    }

    /// `POST /private/get-order-detail` — full detail for one order ID.
    pub async fn get_order_detail(&self, order_id: &str) -> Result<Value> {
        self.post("private/get-order-detail", json!({"order_id": order_id}))
            .await
    }

    /// `POST /private/get-trades` — trade history for an instrument (or
    /// all instruments when `None`).
    pub async fn get_trades(&self, instrument: Option<&str>) -> Result<Value> {
        let mut params = serde_json::Map::new();
        if let Some(i) = instrument {
            params.insert("instrument_name".into(), Value::String(i.to_string()));
        }
        self.post("private/get-trades", Value::Object(params)).await
    }

    /// `POST /private/get-deposit-address` — saved deposit addresses for
    /// a currency.
    pub async fn get_deposit_address(&self, currency: &str) -> Result<Value> {
        self.post("private/get-deposit-address", json!({"currency": currency}))
            .await
    }

    /// `POST /private/create-withdrawal` — initiate a withdrawal.
    ///
    /// `address` must be on the account's pre-approved withdrawal list.
    pub async fn create_withdrawal(
        &self,
        currency: &str,
        amount: &str,
        address: &str,
    ) -> Result<Value> {
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
    pub async fn get_withdrawal_history(&self, currency: &str) -> Result<Value> {
        self.post(
            "private/get-withdrawal-history",
            json!({"currency": currency}),
        )
        .await
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
}
