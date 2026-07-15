//! Binance authentication — credentials + HMAC-SHA256 request signing.
//!
//! Binance signs `SIGNED` (TRADE / USER_DATA / MARGIN) REST requests with
//! HMAC-SHA256 over the **exact serialized query string** (or form body):
//!
//! - Build the `totalParams` string from the request parameters plus
//!   `recvWindow` and `timestamp` (ms since epoch).
//! - `signature = hex(HMAC_SHA256(secret, totalParams))`, appended as a
//!   trailing `&signature=…` parameter.
//! - The API key travels in the `X-MBX-APIKEY` header; the secret is **never**
//!   sent over the wire.
//!
//! The signature covers the *literal bytes* that are transmitted, so the caller
//! must sign the identical string it sends — [`BinanceCredentials::sign_query`]
//! takes that pre-built string verbatim and never re-serialises it. The signing
//! is byte-for-byte compatible with Binance's published example (verified by
//! the known-answer test below); see
//! <https://developers.binance.com/docs/binance-spot-api-docs/rest-api/endpoint-security-type>.

use std::fmt::Write as _;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeroize::ZeroizeOnDrop;

use crate::error::{ExchangeError, Result};

type HmacSha256 = Hmac<Sha256>;

/// Header carrying the API key on every signed (and API-key-only) request.
pub const API_KEY_HEADER: &str = "X-MBX-APIKEY";

/// Default `recvWindow` (ms) — how long after `timestamp` Binance will still
/// accept the request. 5000 ms is Binance's own default; the max is 60000.
pub const DEFAULT_RECV_WINDOW: u64 = 5_000;

/// Binance API credentials. Implements [`ZeroizeOnDrop`] so the secret is
/// zeroed in memory on drop.
#[derive(Clone, ZeroizeOnDrop)]
pub struct BinanceCredentials {
    /// API key (sent as the `X-MBX-APIKEY` header).
    pub api_key: String,
    /// API secret — the HMAC-SHA256 key. Never sent over the wire.
    pub api_secret: String,
}

impl BinanceCredentials {
    /// Construct credentials directly.
    pub fn new(api_key: impl Into<String>, api_secret: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_secret: api_secret.into(),
        }
    }

    /// Load from the `BINANCE_API_KEY` and `BINANCE_API_SECRET` environment
    /// variables.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeError::Config`] when either variable is unset.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            api_key: env("BINANCE_API_KEY")?,
            api_secret: env("BINANCE_API_SECRET")?,
        })
    }

    /// Sign a Binance `SIGNED` request. `query` is the **exact** query string
    /// (or form body) that will be transmitted — everything after `?`, with no
    /// leading `?` and no trailing `&`. Returns the lowercase-hex
    /// HMAC-SHA256, to be appended as `&signature=<value>`.
    ///
    /// The caller must sign the identical bytes it sends; Binance validates the
    /// signature against the literal request string, not the parsed params.
    #[must_use]
    pub fn sign_query(&self, query: &str) -> String {
        hmac_hex(self.api_secret.as_bytes(), query.as_bytes())
    }
}

/// HMAC-SHA256(`key`, `msg`) as lowercase hex.
fn hmac_hex(key: &[u8], msg: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any size");
    mac.update(msg);
    let bytes = mac.finalize().into_bytes();
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn env(key: &str) -> std::result::Result<String, ExchangeError> {
    std::env::var(key).map_err(|_| ExchangeError::Config(format!("{key} not set")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Binance's own published HMAC-SHA256 worked example (public documentation,
    // NOT a live credential). Signing this exact query string with this exact
    // secret MUST reproduce the documented signature — this is the ground truth
    // that proves the signing is byte-for-byte correct.
    // https://developers.binance.com/docs/binance-spot-api-docs/rest-api/endpoint-security-type
    const DOC_SECRET: &str = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
    const DOC_QUERY: &str = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
    const DOC_SIGNATURE: &str = "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71";

    #[test]
    fn matches_binance_documented_vector() {
        let creds = BinanceCredentials::new("doc-key", DOC_SECRET);
        assert_eq!(creds.sign_query(DOC_QUERY), DOC_SIGNATURE);
    }

    #[test]
    fn signature_is_64_char_lowercase_hex() {
        let creds = BinanceCredentials::new("k", "s");
        let sig = creds.sign_query("timestamp=1&recvWindow=5000");
        assert_eq!(sig.len(), 64, "HMAC-SHA256 hex is 64 chars");
        assert!(
            sig.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn signature_is_deterministic_and_sensitive_to_every_input() {
        let creds = BinanceCredentials::new("k", "secret");
        let base = creds.sign_query("timestamp=1&recvWindow=5000");
        // Deterministic.
        assert_eq!(base, creds.sign_query("timestamp=1&recvWindow=5000"));
        // Any change to the signed payload flips the signature.
        assert_ne!(base, creds.sign_query("timestamp=2&recvWindow=5000"));
        assert_ne!(base, creds.sign_query("timestamp=1&recvWindow=6000"));
        // A change to the secret flips the signature.
        assert_ne!(
            base,
            BinanceCredentials::new("k", "other").sign_query("timestamp=1&recvWindow=5000")
        );
    }

    #[test]
    fn credentials_round_trip() {
        let c = BinanceCredentials::new("my-key", "my-secret");
        assert_eq!(c.api_key, "my-key");
        assert_eq!(c.api_secret, "my-secret");
    }
}
