//! `KuCoinClient` — authenticated reqwest wrapper with exponential-backoff retry.
//!
//! - Signs every request via `auth::build_headers`
//! - Retries on transient failures with configurable backoff
//! - Auto-pauses on HTTP 429 (Rate Limit) using KuCoin's reset headers
//! - Unwraps KuCoin's `{"code":"200000","data":{...}}` envelope

use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::warn;

use crate::auth::build_headers;
use crate::error::{ExchangeError, Result};

// ── Environment Routing ───────────────────────────────────────────────────────

/// KuCoin API Environment. Allows routing to Spot, Futures, or UTA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KucoinEnv {
    /// KuCoin Spot exchange (`api.kucoin.com`).
    LiveSpot,
    /// KuCoin Futures exchange (`api-futures.kucoin.com`).
    LiveFutures,
    /// KuCoin Unified Trade Account — routes to the Spot base URL.
    Unified, // Unified Trade Account
}

impl KucoinEnv {
    /// Base REST URL for this environment.
    pub const fn rest_base(&self) -> &'static str {
        match self {
            Self::LiveFutures => "https://api-futures.kucoin.com",
            Self::LiveSpot | Self::Unified => "https://api.kucoin.com",
        }
    }
}

// ── Credentials ───────────────────────────────────────────────────────────────

/// API credentials loaded from environment or passed directly.
#[derive(Clone)]
pub struct Credentials {
    /// KuCoin API key.
    pub key: String,
    /// KuCoin API secret used for HMAC-SHA256 signing.
    pub secret: String,
    /// KuCoin API passphrase set at key creation time.
    pub passphrase: String,
}

impl Credentials {
    /// Construct credentials directly from strings.
    pub fn new(
        key: impl Into<String>,
        secret: impl Into<String>,
        passphrase: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            secret: secret.into(),
            passphrase: passphrase.into(),
        }
    }

    /// Load from `KC_KEY`, `KC_SECRET`, `KC_PASSPHRASE` env vars.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            key: env("KC_KEY")?,
            secret: env("KC_SECRET")?,
            passphrase: env("KC_PASSPHRASE")?,
        })
    }

    /// Sim-mode placeholder — never reaches the exchange.
    pub fn sim() -> Self {
        Self::new("sim_key", "sim_secret", "sim_pass")
    }
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| ExchangeError::Config(format!("{key} not set")))
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Shared HTTP client — create once, clone cheaply.
#[derive(Clone)]
pub struct KuCoinClient {
    pub(crate) http: Client,
    pub(crate) creds: Credentials,
    pub(crate) base_url: String,
}

impl KuCoinClient {
    /// Create a new client targeting a specific KuCoin environment (e.g., Futures)
    pub fn new(creds: Credentials, env: KucoinEnv) -> Self {
        Self::with_base_url(creds, env.rest_base())
    }

    /// Create a client with an explicit base URL (useful for testing/proxies).
    pub fn with_base_url(creds: Credentials, base_url: impl Into<String>) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client");
        Self {
            http,
            creds,
            base_url: base_url.into(),
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Authenticated GET with retry.
    ///
    /// `params` are appended as a query string.
    pub async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, &str)]) -> Result<T> {
        self.get_with_retries(path, params, 3, 1.5).await
    }

    /// Authenticated POST with retry.
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        self.post_with_retries(path, body, 3, 1.5).await
    }

    /// Authenticated DELETE with retry (used for cancellations).
    ///
    /// The `endpoint` should include any necessary query strings (e.g., `?symbol=XBTUSDTM`).
    pub async fn delete<T: DeserializeOwned>(&self, endpoint: &str) -> Result<T> {
        self.delete_with_retries(endpoint, 3, 1.5).await
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    async fn get_with_retries<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
        retries: u32,
        backoff: f64,
    ) -> Result<T> {
        let qs = if params.is_empty() {
            String::new()
        } else {
            let pairs: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!("?{}", pairs.join("&"))
        };
        let endpoint = format!("{path}{qs}");
        let url = format!("{}{}", self.base_url, endpoint);

        for attempt in 0..retries {
            let headers = build_headers(
                &self.creds.key,
                &self.creds.secret,
                &self.creds.passphrase,
                "GET",
                &endpoint,
                "",
            );

            match self.http.get(&url).headers(headers).send().await {
                Ok(resp) => {
                    if let Some(wait) = Self::check_rate_limit(&resp) {
                        tokio::time::sleep(wait).await;
                        continue; // Retry after sleeping
                    }
                    let raw: Value = resp.json().await?;
                    return Self::unwrap_envelope(raw);
                }
                Err(e) if attempt < retries - 1 => {
                    let wait = backoff.powi(attempt.cast_signed() + 1);
                    warn!(attempt, path, error = %e, wait_secs = wait, "GET failed, retrying");
                    tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                }
                Err(e) => return Err(ExchangeError::Http(e)),
            }
        }
        unreachable!()
    }

    async fn post_with_retries<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &Value,
        retries: u32,
        backoff: f64,
    ) -> Result<T> {
        let body_str = serde_json::to_string(body)?;

        for attempt in 0..retries {
            let headers = build_headers(
                &self.creds.key,
                &self.creds.secret,
                &self.creds.passphrase,
                "POST",
                path,
                &body_str,
            );

            match self
                .http
                .post(format!("{}{path}", self.base_url))
                .headers(headers)
                .body(body_str.clone())
                .send()
                .await
            {
                Ok(resp) => {
                    if let Some(wait) = Self::check_rate_limit(&resp) {
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    let raw: Value = resp.json().await?;
                    return Self::unwrap_envelope(raw);
                }
                Err(e) if attempt < retries - 1 => {
                    let wait = backoff.powi(attempt.cast_signed() + 1);
                    warn!(attempt, path, error = %e, wait_secs = wait, "POST failed, retrying");
                    tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                }
                Err(e) => return Err(ExchangeError::Http(e)),
            }
        }
        unreachable!()
    }

    async fn delete_with_retries<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        retries: u32,
        backoff: f64,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, endpoint);

        for attempt in 0..retries {
            let headers = build_headers(
                &self.creds.key,
                &self.creds.secret,
                &self.creds.passphrase,
                "DELETE",
                endpoint,
                "",
            );

            match self.http.delete(&url).headers(headers).send().await {
                Ok(resp) => {
                    if let Some(wait) = Self::check_rate_limit(&resp) {
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    let raw: Value = resp.json().await?;
                    return Self::unwrap_envelope(raw);
                }
                Err(e) if attempt < retries - 1 => {
                    let wait = backoff.powi(attempt.cast_signed() + 1);
                    warn!(attempt, endpoint, error = %e, wait_secs = wait, "DELETE failed, retrying");
                    tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                }
                Err(e) => return Err(ExchangeError::Http(e)),
            }
        }
        unreachable!()
    }

    /// Checks for a 429 Too Many Requests response and reads the reset timer header.
    fn check_rate_limit(resp: &reqwest::Response) -> Option<Duration> {
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            let reset_ms = resp
                .headers()
                .get("gw-ratelimit-reset")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(2000); // Default to 2 seconds if header is missing

            warn!(reset_ms, "Rate limited (HTTP 429). Pausing request.");
            return Some(Duration::from_millis(reset_ms));
        }
        None
    }

    /// Unwrap KuCoin's `{"code":"200000","data":{...}}` envelope.
    fn unwrap_envelope<T: DeserializeOwned>(raw: Value) -> Result<T> {
        let code = raw
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        if code != "200000" {
            let msg = raw
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("no message")
                .to_string();
            return Err(ExchangeError::Api { code, message: msg });
        }

        let data = raw.get("data").cloned().unwrap_or(Value::Null);

        serde_json::from_value(data).map_err(ExchangeError::Json)
    }
}
