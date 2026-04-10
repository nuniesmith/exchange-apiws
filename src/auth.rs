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
use hmac::{Hmac, Mac};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Compute `base64(HMAC-SHA256(key, message))`.
pub fn hmac_b64(key: &str, message: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

/// Build the full signed header map for one KuCoin Futures request.
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
) -> HeaderMap {
    let ts = chrono::Utc::now().timestamp_millis().to_string();
    let prehash = format!("{}{}{}{}", ts, method.to_uppercase(), endpoint, body);

    let sig = hmac_b64(secret, &prehash);
    let pp_sig = hmac_b64(secret, passphrase);

    let mut h = HeaderMap::new();
    h.insert("KC-API-KEY", hv(key));
    h.insert("KC-API-SIGN", hv(&sig));
    h.insert("KC-API-TIMESTAMP", hv(&ts));
    h.insert("KC-API-PASSPHRASE", hv(&pp_sig));
    h.insert("KC-API-KEY-VERSION", HeaderValue::from_static("2"));
    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h
}

fn hv(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).expect("header value contains non-ASCII")
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
        let h = build_headers("key", "secret", "pass", "POST", "/api/v1/orders", "{}");
        assert!(h.contains_key("KC-API-KEY"));
        assert!(h.contains_key("KC-API-SIGN"));
        assert!(h.contains_key("KC-API-TIMESTAMP"));
        assert!(h.contains_key("KC-API-PASSPHRASE"));
        assert_eq!(h.get("KC-API-KEY-VERSION").unwrap(), "2");
    }
}
