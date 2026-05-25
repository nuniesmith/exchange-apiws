//! Generic HTTP plumbing for unauthenticated REST clients.
//!
//! [`PublicRestClient`] is the foundation for any exchange integration that
//! doesn't require signing — Binance public endpoints, Bybit public
//! endpoints, and the public-data side of Kraken and Crypto.com all build
//! on it. The authenticated [`KuCoinClient`](crate::client::KuCoinClient)
//! shares helper functions defined here ([`percent_encode`],
//! [`build_query_string`], [`jitter_secs`]) but adds its own signing layer.
//!
//! Responsibilities:
//! - reqwest HTTP client with rustls + configurable timeout
//! - jittered exponential-backoff retry on transient network errors
//! - HTTP 429 handling via the standard `Retry-After` header
//! - **No** envelope unwrapping — exchange-specific shapes are the caller's
//!   responsibility.

use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use tracing::warn;

use crate::error::{ExchangeError, Result};

// ── Shared helpers (also used by KuCoinClient) ────────────────────────────────

/// Percent-encode a single query parameter value (RFC 3986 §2.3).
///
/// Only unreserved characters (`A–Z`, `a–z`, `0–9`, `-`, `_`, `.`, `~`) are
/// left unencoded; everything else becomes `%XX`. Safe to use in URLs and
/// HMAC pre-hashes.
pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b => {
                out.push('%');
                out.push(
                    char::from_digit(u32::from(b) >> 4, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit(u32::from(b) & 0xF, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

/// Build a percent-encoded query string from key-value pairs.
///
/// Returns `""` when `params` is empty, otherwise
/// `"?key=value&key2=value2"` with all values percent-encoded.
pub(crate) fn build_query_string(params: &[(&str, &str)]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect();
    format!("?{}", pairs.join("&"))
}

/// Return a ±25 % jitter factor for `base`.
///
/// Uses sub-second system time as a cheap entropy source — no `rand`
/// dependency. The distribution isn't perfectly uniform but is sufficient
/// to spread out concurrent retry bursts.
pub(crate) fn jitter_secs(base: f64) -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let factor = (f64::from(nanos) / 1_000_000_000.0 - 0.5) * 0.5;
    base * factor
}

/// Default number of HTTP retry attempts for transient failures.
pub(crate) const DEFAULT_RETRIES: u32 = 3;

/// Default exponential backoff base (seconds).
pub(crate) const DEFAULT_BACKOFF: f64 = 1.5;

/// Cap on consecutive 429 sleeps per call. Prevents infinite loops if the
/// exchange keeps returning rate-limited.
pub(crate) const MAX_RATE_LIMIT_RETRIES: u32 = 5;

/// Default per-request timeout (seconds).
const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 10;

// ── PublicRestClient ──────────────────────────────────────────────────────────

/// Shared unauthenticated HTTP client for exchange public REST endpoints.
///
/// Create once and clone cheaply — the underlying `reqwest::Client` pools
/// connections across calls.
///
/// # Example
///
/// ```no_run
/// use exchange_apiws::http::PublicRestClient;
/// use serde::Deserialize;
///
/// # async fn example() -> exchange_apiws::Result<()> {
/// #[derive(Deserialize)]
/// struct ServerTime { serverTime: u64 }
///
/// let client = PublicRestClient::new("https://api.binance.com")?;
/// let ts: ServerTime = client.get("/api/v3/time", &[]).await?;
/// println!("Binance server time: {}", ts.serverTime);
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct PublicRestClient {
    http: Client,
    base_url: String,
}

impl PublicRestClient {
    /// Build a client pointed at `base_url` with the default 10 s timeout.
    ///
    /// # Errors
    /// Returns [`ExchangeError::Config`] if the underlying `reqwest` client
    /// cannot be built (e.g. TLS initialisation failure).
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        Self::with_timeout(base_url, Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS))
    }

    /// Build a client with a caller-specified per-request timeout.
    ///
    /// # Errors
    /// Returns [`ExchangeError::Config`] if the underlying `reqwest` client
    /// cannot be built.
    pub fn with_timeout(base_url: impl Into<String>, timeout: Duration) -> Result<Self> {
        let http = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| ExchangeError::Config(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            http,
            base_url: base_url.into(),
        })
    }

    /// The base URL the client was constructed with.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Public GET with jittered exponential-backoff retry.
    ///
    /// `params` are percent-encoded and appended as a query string. The
    /// response body is deserialized directly as `T` with no envelope
    /// unwrapping — the caller is responsible for handling exchange-specific
    /// response shapes (Binance bare JSON, Bybit `retCode`, etc.).
    ///
    /// Retry policy:
    /// - Network errors (connect, timeout, DNS) are retried up to
    ///   [`DEFAULT_RETRIES`] times with jittered exponential backoff.
    /// - HTTP 429 responses honour the `Retry-After` header (seconds form)
    ///   and are capped at [`MAX_RATE_LIMIT_RETRIES`] before giving up.
    /// - Other 4xx/5xx responses surface as
    ///   [`ExchangeError::Api`] without retry.
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let qs = build_query_string(params);
        let url = format!("{}{path}{qs}", self.base_url);

        let mut last_err: Option<ExchangeError> = None;
        let mut rate_limit_hits: u32 = 0;

        for attempt in 0..DEFAULT_RETRIES {
            let send_result = self.http.get(&url).send().await;
            let resp = match send_result {
                Ok(r) => r,
                Err(e) if attempt < DEFAULT_RETRIES - 1 => {
                    let base = DEFAULT_BACKOFF.powi(attempt.cast_signed() + 1);
                    let wait = (base + jitter_secs(base)).max(0.1);
                    warn!(
                        attempt,
                        path,
                        error = %e,
                        wait_secs = wait,
                        "public GET failed, retrying"
                    );
                    tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                    last_err = Some(ExchangeError::Http(e));
                    continue;
                }
                Err(e) => return Err(ExchangeError::Http(e)),
            };

            if resp.status() == StatusCode::TOO_MANY_REQUESTS {
                rate_limit_hits += 1;
                if rate_limit_hits > MAX_RATE_LIMIT_RETRIES {
                    return Err(ExchangeError::Api {
                        code: "429".into(),
                        message: format!(
                            "GET {path} was rate-limited \
                             {MAX_RATE_LIMIT_RETRIES} times; giving up"
                        ),
                    });
                }
                let wait = parse_retry_after(&resp).unwrap_or(Duration::from_secs(2));
                warn!(
                    attempt,
                    path,
                    wait_ms = wait.as_millis(),
                    rate_limit_hits,
                    "public GET rate-limited — waiting before retry"
                );
                tokio::time::sleep(wait).await;
                last_err = Some(ExchangeError::Api {
                    code: "429".into(),
                    message: "rate limited".into(),
                });
                continue;
            }

            if !resp.status().is_success() {
                let code = resp.status().as_u16().to_string();
                let message = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| String::from("no body"));
                return Err(ExchangeError::Api { code, message });
            }

            return Ok(resp.json::<T>().await?);
        }

        Err(last_err.unwrap_or_else(|| ExchangeError::Api {
            code: "retry_exhausted".into(),
            message: format!("GET {path} failed after {DEFAULT_RETRIES} attempts"),
        }))
    }
}

/// Parse the HTTP `Retry-After` header (RFC 7231 §7.1.3).
///
/// Supports the integer-seconds form used by Binance, Bybit, and most other
/// exchange APIs. HTTP-date form falls back to the caller's default since
/// it's vanishingly rare in practice for rate-limit responses.
fn parse_retry_after(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get("retry-after")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_leaves_unreserved_chars_unchanged() {
        assert_eq!(percent_encode("XBTUSDTM"), "XBTUSDTM");
        assert_eq!(percent_encode("abc-123_def.ghi~"), "abc-123_def.ghi~");
    }

    #[test]
    fn percent_encode_encodes_special_chars() {
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("a=b&c=d"), "a%3Db%26c%3Dd");
        assert_eq!(percent_encode("a+b"), "a%2Bb");
    }

    #[test]
    fn build_query_string_empty() {
        assert_eq!(build_query_string(&[]), "");
    }

    #[test]
    fn build_query_string_encodes_values() {
        let qs = build_query_string(&[("symbol", "XBT USDT"), ("side", "buy&sell")]);
        assert_eq!(qs, "?symbol=XBT%20USDT&side=buy%26sell");
    }

    #[test]
    fn jitter_stays_within_25_percent() {
        let base = 4.0_f64;
        for _ in 0..100 {
            let j = jitter_secs(base);
            assert!(
                j.abs() <= base.mul_add(0.25, 1e-9),
                "jitter {j} exceeded ±25% of {base}"
            );
        }
    }
}
