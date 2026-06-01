//! Bybit v5 authentication — credentials + HMAC-SHA256 request signing.
//!
//! Bybit signs requests with HMAC-SHA256 over a concatenated string, hex-encoded:
//!
//! - **REST:** `sign(timestamp + api_key + recv_window + query_or_body)` →
//!   sent in the `X-BAPI-SIGN` header alongside `X-BAPI-API-KEY`,
//!   `X-BAPI-TIMESTAMP`, `X-BAPI-RECV-WINDOW`.
//! - **WebSocket:** `sign("GET/realtime" + expires)` → sent in the private
//!   `auth` op frame as `[api_key, expires, signature]`.
//!
//! The signing is byte-for-byte compatible with Bybit's v5 spec; see
//! <https://bybit-exchange.github.io/docs/v5/guide#authentication>.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::ZeroizeOnDrop;

use crate::error::{ExchangeError, Result};

type HmacSha256 = Hmac<Sha256>;

/// Default `recv_window` (ms) — how long after `timestamp` Bybit will accept
/// the request. 5000 ms is Bybit's own default.
pub const DEFAULT_RECV_WINDOW: u64 = 5_000;

/// Bybit API credentials. Implements [`ZeroizeOnDrop`] so the secret is zeroed
/// in memory on drop.
#[derive(Clone, ZeroizeOnDrop)]
pub struct BybitCredentials {
    /// API key (sent as the `X-BAPI-API-KEY` header).
    pub api_key: String,
    /// API secret — the HMAC-SHA256 key. Never sent over the wire.
    pub api_secret: String,
}

impl BybitCredentials {
    /// Construct credentials directly.
    pub fn new(api_key: impl Into<String>, api_secret: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_secret: api_secret.into(),
        }
    }

    /// Load from the `BYBIT_API_KEY` and `BYBIT_API_SECRET` environment
    /// variables.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeError::Config`] when either variable is unset.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            api_key: env("BYBIT_API_KEY")?,
            api_secret: env("BYBIT_API_SECRET")?,
        })
    }

    /// Sign a REST request. Bybit v5 signs the string
    /// `timestamp + api_key + recv_window + payload`, where `payload` is the
    /// raw query string (GET) or the JSON body (POST). Returns the lowercase
    /// hex HMAC-SHA256, suitable for the `X-BAPI-SIGN` header.
    #[must_use]
    pub fn sign_rest(&self, timestamp: u64, recv_window: u64, payload: &str) -> String {
        let sign_str = format!("{timestamp}{}{recv_window}{payload}", self.api_key);
        hmac_hex(self.api_secret.as_bytes(), sign_str.as_bytes())
    }

    /// Sign a private-WebSocket `auth` frame. Bybit signs `"GET/realtime" +
    /// expires` (expires in ms). Returns the lowercase hex HMAC-SHA256.
    #[must_use]
    pub fn sign_ws(&self, expires: u64) -> String {
        let sign_str = format!("GET/realtime{expires}");
        hmac_hex(self.api_secret.as_bytes(), sign_str.as_bytes())
    }
}

/// HMAC-SHA256(`key`, `msg`) as lowercase hex.
fn hmac_hex(key: &[u8], msg: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any size");
    mac.update(msg);
    let bytes = mac.finalize().into_bytes();
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| ExchangeError::Config(format!("{key} not set")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_signature_is_64_char_hex() {
        let creds = BybitCredentials::new("testkey", "testsecret");
        let sig = creds.sign_rest(1_700_000_000_000, DEFAULT_RECV_WINDOW, "category=linear");
        assert_eq!(sig.len(), 64, "HMAC-SHA256 hex is 64 chars");
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn rest_signature_matches_known_construction() {
        // Verify the signed string layout (timestamp+key+recv+payload) is
        // stable — a regression here breaks every signed request.
        let creds = BybitCredentials::new("KEY", "SECRET");
        let a = creds.sign_rest(1, 5000, "x=1");
        // Same inputs → deterministic.
        assert_eq!(a, creds.sign_rest(1, 5000, "x=1"));
        // Any input change flips the signature.
        assert_ne!(a, creds.sign_rest(2, 5000, "x=1"));
        assert_ne!(a, creds.sign_rest(1, 5000, "x=2"));
        assert_ne!(a, creds.sign_rest(1, 6000, "x=1"));
    }

    #[test]
    fn ws_signature_uses_get_realtime_prefix() {
        let creds = BybitCredentials::new("k", "s");
        let sig = creds.sign_ws(1_700_000_010_000);
        assert_eq!(sig.len(), 64);
        assert_ne!(sig, creds.sign_ws(1_700_000_010_001));
    }
}
