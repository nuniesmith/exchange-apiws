//! KuCoin API authentication — HMAC-SHA256 signing, key version 2.
//!
//! Version 2 differs from v1 in that the passphrase is also HMAC-signed
//! (not sent raw).  This matches the Python `_sign()` function exactly:
//!
//! ```python
//! prehash = ts + method.upper() + endpoint + body
//! sig     = base64(hmac_sha256(secret, prehash))
//! pp_sig  = base64(hmac_sha256(secret, passphrase))
//! ```

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use sha2::Sha256;

use crate::error::{ExchangeError, Result};

type HmacSha256 = Hmac<Sha256>;

/// Compute `base64(HMAC-SHA256(key, message))`.
pub fn hmac_b64(key: &str, message: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

/// Build the full signed header map for one KuCoin Futures request.
///
/// Returns `Err(ExchangeError::Auth)` if any header value cannot be encoded
/// (e.g. contains non-ASCII bytes). In practice this can only happen if a
/// credential or HMAC digest is malformed.
///
/// # Arguments
/// - `endpoint` — path **plus** query string if present, e.g.
///   `"/api/v1/kline/query?symbol=XBTUSDTM&granularity=1"`.
/// - `method`   — HTTP verb, case-insensitive (`"GET"`, `"POST"`, …).
/// - `body`     — serialised request body; empty string `""` for GET.
pub fn build_headers(
    key: &str,
    secret: &str,
    passphrase: &str,
    method: &str,
    endpoint: &str,
    body: &str,
) -> Result<HeaderMap> {
    build_headers_with_offset(key, secret, passphrase, method, endpoint, body, 0)
}

/// Local wall-clock in milliseconds, shifted by a signed `offset_ms`.
///
/// `offset_ms` is `server_time - local_time`, so adding it yields the
/// exchange's clock. Kept as a tiny pure function so the skew math is unit
/// testable without a clock. Saturates rather than overflowing on absurd
/// offsets.
pub const fn skewed_timestamp_ms(local_ms: i64, offset_ms: i64) -> i64 {
    local_ms.saturating_add(offset_ms)
}

/// As [`build_headers`], but applies a signed server-time `offset_ms` (in
/// milliseconds) to the `KC-API-TIMESTAMP` used for signing.
///
/// Pass the cached offset from
/// [`KuCoinClient::time_offset_ms`][crate::KuCoinClient::time_offset_ms]
/// (obtained via [`KuCoinClient::sync_server_time`][crate::KuCoinClient::sync_server_time])
/// so signed timestamps track KuCoin's clock and survive local NTP drift
/// beyond the venue's ±5 s tolerance. An offset of `0` is identical to
/// [`build_headers`].
pub fn build_headers_with_offset(
    key: &str,
    secret: &str,
    passphrase: &str,
    method: &str,
    endpoint: &str,
    body: &str,
    offset_ms: i64,
) -> Result<HeaderMap> {
    let local_ms = chrono::Utc::now().timestamp_millis();
    let ts = skewed_timestamp_ms(local_ms, offset_ms).to_string();
    let prehash = format!("{}{}{}{}", ts, method.to_uppercase(), endpoint, body);

    let sig = hmac_b64(secret, &prehash);
    let pp_sig = hmac_b64(secret, passphrase);

    let mut h = HeaderMap::new();
    h.insert("KC-API-KEY", hv(key)?);
    h.insert("KC-API-SIGN", hv(&sig)?);
    h.insert("KC-API-TIMESTAMP", hv(&ts)?);
    h.insert("KC-API-PASSPHRASE", hv(&pp_sig)?);
    h.insert("KC-API-KEY-VERSION", HeaderValue::from_static("2"));
    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(h)
}

/// Convert a string to a `HeaderValue`, returning `Err(Auth)` on failure.
///
/// `HeaderValue::from_str` rejects strings containing bytes outside the
/// visible ASCII range (32–127, excluding DEL). API keys and HMAC-SHA256
/// base64 digests only use printable ASCII, so this should never fail in
/// practice — but we propagate the error rather than panicking.
fn hv(s: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(s)
        .map_err(|_| ExchangeError::Auth(format!("header value contains invalid bytes: {s:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test — verify the signature changes when the message changes.
    #[test]
    fn hmac_differs_for_different_inputs() {
        let a = hmac_b64("secret", "message_a");
        let b = hmac_b64("secret", "message_b");
        assert_ne!(a, b);
    }

    /// Verify output is valid base64.
    #[test]
    fn hmac_is_valid_base64() {
        let sig = hmac_b64("my-secret", "payload");
        B64.decode(&sig).expect("should be valid base64");
    }

    #[test]
    fn build_headers_has_required_keys() {
        let h = build_headers("key", "secret", "pass", "POST", "/api/v1/orders", "{}")
            .expect("valid ASCII credentials should never fail");
        assert!(h.contains_key("KC-API-KEY"));
        assert!(h.contains_key("KC-API-SIGN"));
        assert!(h.contains_key("KC-API-TIMESTAMP"));
        assert!(h.contains_key("KC-API-PASSPHRASE"));
        assert_eq!(h.get("KC-API-KEY-VERSION").unwrap(), "2");
    }

    #[test]
    fn build_headers_returns_err_on_invalid_key() {
        // A NUL byte is not valid in a header value.
        let result = build_headers("key\0bad", "secret", "pass", "GET", "/api/v1/test", "");
        assert!(result.is_err());
    }

    /// Known-answer test — pins the exact KuCoin key-v2 signing recipe
    /// (`base64(HMAC-SHA256(secret, ts+METHOD+endpoint+body))` and the
    /// separately-signed passphrase) against a vector computed independently
    /// with Python's `hmac`/`base64`. Guards against silent drift in the
    /// pre-hash construction. Mirrors Binance's signing KAT.
    #[test]
    fn kucoin_signing_known_answer() {
        let secret = "test-secret-key";
        let ts = "1700000000000";
        let prehash = format!("{ts}POST/api/v1/orders{{\"symbol\":\"XBTUSDTM\"}}");
        assert_eq!(
            hmac_b64(secret, &prehash),
            "ryn3lauCKysTv31+K11M0amC+pXlovaOmWA+b6zlOlI="
        );
        assert_eq!(
            hmac_b64(secret, "passphrase123"),
            "izTy93wNW0Z4fyogazVQ1Ix0SgSFLXX/UCS8qeO8ebs="
        );
    }

    #[test]
    fn skewed_timestamp_applies_offset() {
        assert_eq!(skewed_timestamp_ms(1_000, 250), 1_250);
        assert_eq!(skewed_timestamp_ms(1_000, -250), 750);
        assert_eq!(skewed_timestamp_ms(1_000, 0), 1_000);
    }

    #[test]
    fn skewed_timestamp_saturates_instead_of_overflowing() {
        assert_eq!(skewed_timestamp_ms(i64::MAX, 1), i64::MAX);
        assert_eq!(skewed_timestamp_ms(i64::MIN, -1), i64::MIN);
    }

    #[test]
    fn zero_offset_headers_match_default_builder() {
        // Both builders sign with `Utc::now()`; a zero offset must not change
        // the signing recipe. We can't compare signatures (timestamps differ
        // by microseconds) but the header *set* must be identical.
        let a = build_headers("k", "s", "p", "GET", "/api/v1/x", "").unwrap();
        let b = build_headers_with_offset("k", "s", "p", "GET", "/api/v1/x", "", 0).unwrap();
        assert_eq!(a.len(), b.len());
        assert_eq!(a.get("KC-API-KEY-VERSION"), b.get("KC-API-KEY-VERSION"));
    }
}
