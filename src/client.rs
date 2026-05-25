//! Generic HTTP client — authenticated reqwest wrapper with exponential-backoff retry.
//!
//! This module is **exchange-agnostic**. It knows how to sign requests, retry
//! on transient failures, respect HTTP 429 rate-limit headers, and unwrap
//! KuCoin's JSON envelope — but it has no opinion about which environment
//! or base URL to use. Environment routing lives in [`crate::connectors`].
//!
//! - Signs every request via [`crate::auth::build_headers`]
//! - Retries on transient failures with jittered exponential backoff
//! - Auto-pauses on HTTP 429 (Rate Limit) using KuCoin's reset headers,
//!   with a cap of [`MAX_RATE_LIMIT_RETRIES`] to prevent infinite loops
//! - Unwraps KuCoin's `{"code":"200000","data":{...}}` envelope
//! - Percent-encodes all query parameter values before signing
//!
//! Shared helpers (`percent_encode`, `build_query_string`, `jitter_secs`)
//! and the retry tuning constants live in [`crate::http`] so the
//! authenticated [`KuCoinClient`] and the public [`PublicRestClient`] stay
//! in sync.

use std::time::Duration;

use reqwest::{Client, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::warn;
use zeroize::ZeroizeOnDrop;

use crate::auth::build_headers;
use crate::error::{ExchangeError, Result};
use crate::http::{
    DEFAULT_BACKOFF, DEFAULT_RETRIES, MAX_RATE_LIMIT_RETRIES, build_query_string, jitter_secs,
};

// ── Credentials ───────────────────────────────────────────────────────────────

/// API credentials loaded from environment or passed directly.
///
/// Implements [`ZeroizeOnDrop`]: the key material is zeroed out in memory
/// when this struct is dropped, preventing secrets from lingering in heap
/// dumps or core files.
#[derive(Clone, ZeroizeOnDrop)]
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
    ///
    /// # ⚠️ Development only
    ///
    /// These credentials are hardcoded and will be rejected by any live
    /// exchange endpoint. Use [`Credentials::from_env`] or
    /// [`Credentials::new`] for real trading. Gate sim-mode behind a
    /// runtime flag or feature flag; never ship this to production.
    #[cfg(any(test, feature = "testutils"))]
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
    /// Create a client with an explicit base URL (useful for testing/proxies).
    ///
    /// # Errors
    /// Returns [`ExchangeError::Config`] if the underlying `reqwest` HTTP
    /// client cannot be built (e.g. TLS initialisation failure).
    pub fn with_base_url(creds: Credentials, base_url: impl Into<String>) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| ExchangeError::Config(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            http,
            creds,
            base_url: base_url.into(),
        })
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Authenticated GET with jittered exponential-backoff retry.
    ///
    /// `params` are percent-encoded and appended as a query string. The
    /// encoded query string is included in the HMAC pre-hash so the signature
    /// matches what the server receives.
    pub async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, &str)]) -> Result<T> {
        let qs = build_query_string(params);
        let endpoint = format!("{path}{qs}");
        let url = format!("{}{endpoint}", self.base_url);
        self.execute_with_retries("GET", &endpoint, &url, None, DEFAULT_RETRIES, DEFAULT_BACKOFF)
            .await
    }

    /// Authenticated POST with jittered exponential-backoff retry.
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        let body_str = serde_json::to_string(body)?;
        let url = format!("{}{path}", self.base_url);
        self.execute_with_retries(
            "POST",
            path,
            &url,
            Some(&body_str),
            DEFAULT_RETRIES,
            DEFAULT_BACKOFF,
        )
        .await
    }

    /// Authenticated DELETE with jittered exponential-backoff retry.
    ///
    /// The `endpoint` should include any necessary query strings (e.g., `?symbol=XBTUSDTM`).
    pub async fn delete<T: DeserializeOwned>(&self, endpoint: &str) -> Result<T> {
        let url = format!("{}{endpoint}", self.base_url);
        self.execute_with_retries(
            "DELETE",
            endpoint,
            &url,
            None,
            DEFAULT_RETRIES,
            DEFAULT_BACKOFF,
        )
        .await
    }

    /// Authenticated PUT with jittered exponential-backoff retry.
    pub async fn put<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        let body_str = serde_json::to_string(body)?;
        let url = format!("{}{path}", self.base_url);
        self.execute_with_retries(
            "PUT",
            path,
            &url,
            Some(&body_str),
            DEFAULT_RETRIES,
            DEFAULT_BACKOFF,
        )
        .await
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    /// Unified retry loop for all HTTP verbs.
    ///
    /// - `verb`     — `"GET"`, `"POST"`, `"DELETE"`, or `"PUT"`.
    /// - `endpoint` — path + query string (used for HMAC signing and logging).
    /// - `url`      — full URL sent to `reqwest`.
    /// - `body`     — `Some(json_str)` for POST/PUT, `None` for GET/DELETE.
    ///
    /// Transient network errors are retried up to `retries` times with
    /// jittered exponential backoff. HTTP 429 responses trigger a sleep based
    /// on the `gw-ratelimit-reset` header and do **not** consume a retry slot,
    /// but are capped at [`MAX_RATE_LIMIT_RETRIES`] to avoid infinite loops.
    async fn execute_with_retries<T: DeserializeOwned>(
        &self,
        verb: &str,
        endpoint: &str,
        url: &str,
        body: Option<&str>,
        retries: u32,
        backoff: f64,
    ) -> Result<T> {
        let body_str = body.unwrap_or("");
        let mut last_err: Option<ExchangeError> = None;
        let mut rate_limit_hits: u32 = 0;

        for attempt in 0..retries {
            let headers = build_headers(
                &self.creds.key,
                &self.creds.secret,
                &self.creds.passphrase,
                verb,
                endpoint,
                body_str,
            )?;

            // Build the request for this verb. `RequestBuilder` is consumed by
            // `.send()`, so we reconstruct it on each retry.
            let mut req: RequestBuilder = match verb {
                "GET" => self.http.get(url),
                "POST" => self.http.post(url),
                "DELETE" => self.http.delete(url),
                "PUT" => self.http.put(url),
                other => {
                    return Err(ExchangeError::Config(format!(
                        "unsupported HTTP verb: {other}"
                    )))
                }
            };
            req = req.headers(headers);
            if !body_str.is_empty() {
                req = req.body(body_str.to_owned());
            }

            match req.send().await {
                Ok(resp) => {
                    if let Some(wait) = Self::check_rate_limit(&resp) {
                        rate_limit_hits += 1;
                        if rate_limit_hits > MAX_RATE_LIMIT_RETRIES {
                            return Err(ExchangeError::Api {
                                code: "429".into(),
                                message: format!(
                                    "{verb} {endpoint} was rate-limited \
                                     {MAX_RATE_LIMIT_RETRIES} times; giving up"
                                ),
                            });
                        }
                        warn!(
                            attempt,
                            endpoint,
                            wait_ms = wait.as_millis(),
                            rate_limit_hits,
                            "{verb} rate-limited — waiting before retry"
                        );
                        tokio::time::sleep(wait).await;
                        last_err = Some(ExchangeError::Api {
                            code: "429".into(),
                            message: "rate limited".into(),
                        });
                        // Rate-limit sleeps do not consume the retry budget.
                        continue;
                    }
                    let raw: Value = resp.json().await?;
                    return Self::unwrap_envelope(raw);
                }
                Err(e) if attempt < retries - 1 => {
                    let base = backoff.powi(attempt.cast_signed() + 1);
                    let wait = (base + jitter_secs(base)).max(0.1);
                    warn!(
                        attempt,
                        endpoint,
                        error = %e,
                        wait_secs = wait,
                        "{verb} failed, retrying"
                    );
                    tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                    last_err = Some(ExchangeError::Http(e));
                }
                Err(e) => return Err(ExchangeError::Http(e)),
            }
        }

        Err(last_err.unwrap_or_else(|| ExchangeError::Api {
            code: "retry_exhausted".into(),
            message: format!("{verb} {endpoint} failed after {retries} attempts"),
        }))
    }

    /// Checks for a 429 Too Many Requests response and reads the reset timer header.
    fn check_rate_limit(resp: &reqwest::Response) -> Option<Duration> {
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            let reset_ms = resp
                .headers()
                .get("gw-ratelimit-reset")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(2_000); // Default to 2 seconds if header is missing

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

