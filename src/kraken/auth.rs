//! Kraken authentication — HMAC-SHA512 signing for private REST endpoints.
//!
//! Kraken's signing scheme (different from KuCoin's HMAC-SHA256 path used
//! by [`crate::auth`]):
//!
//! 1. Decode the base64-encoded API secret into raw key bytes.
//! 2. Hash the request: `SHA256(nonce_str || post_body_bytes)`.
//! 3. HMAC-SHA512 over `(uri_path_bytes || sha256_result)` with the
//!    decoded key.
//! 4. Base64-encode the HMAC bytes; that's the `API-Sign` header value.
//!
//! See [`sign_kraken_request`] for the canonical implementation and
//! [`form_encode`] for the form-body builder.

use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256, Sha512};
use zeroize::ZeroizeOnDrop;

use crate::error::{ExchangeError, Result};

type HmacSha512 = Hmac<Sha512>;

// ── Credentials ──────────────────────────────────────────────────────────────

/// Kraken API credentials. Implements [`ZeroizeOnDrop`] so the secret is
/// zeroed in memory on drop.
#[derive(Clone, ZeroizeOnDrop)]
pub struct KrakenCredentials {
    /// Kraken API key (sent as the `API-Key` header).
    pub api_key: String,
    /// Base64-encoded private key — Kraken decodes this server-side to
    /// HMAC-SHA512 your requests. Stored as the original base64 string so
    /// `from_env` can round-trip without intermediate decoding.
    pub api_secret_b64: String,
}

impl KrakenCredentials {
    /// Construct credentials directly.
    pub fn new(api_key: impl Into<String>, api_secret_b64: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_secret_b64: api_secret_b64.into(),
        }
    }

    /// Load from the `KRAKEN_API_KEY` and `KRAKEN_API_SECRET` environment
    /// variables.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeError::Config`] when either variable is unset.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            api_key: env("KRAKEN_API_KEY")?,
            api_secret_b64: env("KRAKEN_API_SECRET")?,
        })
    }
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| ExchangeError::Config(format!("{key} not set")))
}

// ── Signing ──────────────────────────────────────────────────────────────────

/// Sign a Kraken private request. Returns the base64 `API-Sign` value.
///
/// `post_body` must be the exact `application/x-www-form-urlencoded`
/// body that will be sent over the wire — Kraken validates the signature
/// against the byte-for-byte body, not the parsed parameters. Build it
/// with [`form_encode`] to keep the encoding consistent.
///
/// # Errors
///
/// Returns [`ExchangeError::Auth`] if the secret can't be base64-decoded
/// (malformed credentials) or if the decoded key is rejected by the HMAC
/// constructor (extremely rare — would mean a zero-length key).
pub fn sign_kraken_request(
    uri_path: &str,
    nonce: u64,
    post_body: &str,
    api_secret_b64: &str,
) -> Result<String> {
    let mut sha = Sha256::new();
    sha.update(nonce.to_string().as_bytes());
    sha.update(post_body.as_bytes());
    let sha_result = sha.finalize();

    let key = STANDARD
        .decode(api_secret_b64)
        .map_err(|e| ExchangeError::Auth(format!("Kraken secret is not valid base64: {e}")))?;

    let mut mac = HmacSha512::new_from_slice(&key)
        .map_err(|e| ExchangeError::Auth(format!("Kraken HMAC init failed: {e}")))?;
    mac.update(uri_path.as_bytes());
    mac.update(&sha_result);
    let result = mac.finalize().into_bytes();

    Ok(STANDARD.encode(result))
}

// ── Form encoding ────────────────────────────────────────────────────────────

/// Build an `application/x-www-form-urlencoded` body from key/value pairs.
///
/// Empty values are preserved (Kraken's API accepts `key=&...`). Returns
/// an empty string when `params` is empty.
#[must_use]
pub fn form_encode(params: &[(&str, &str)]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(params.len() * 16);
    for (i, (k, v)) in params.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(&form_url_encode(k));
        out.push('=');
        out.push_str(&form_url_encode(v));
    }
    out
}

/// Application/x-www-form-urlencoded percent-encoding: space → `+`, every
/// reserved character → `%XX`. Matches RFC 1738 §2.2 for form bodies.
fn form_url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
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

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Test fixtures are computed at runtime from plain ASCII so the file
    // contains no high-entropy base64 string literals (which secret
    // scanners trip on). Decoded plaintexts are obviously synthetic:
    // "sim-secret" / "other-secret".
    fn sim_secret_b64() -> String {
        STANDARD.encode(b"sim-secret")
    }
    fn other_secret_b64() -> String {
        STANDARD.encode(b"other-secret")
    }

    /// HMAC-SHA512 → 64 bytes → base64 = 88 characters (with `=` padding).
    /// Any signature that round-trips through base64 must hit this length.
    #[test]
    fn sign_output_is_88_char_base64() {
        let secret = sim_secret_b64();
        let sig = sign_kraken_request(
            "/0/private/Balance",
            1_700_000_000_000,
            "nonce=1700000000000",
            &secret,
        )
        .expect("sign");
        assert_eq!(sig.len(), 88);
        // Confirm it parses as base64 — guards against accidental padding/charset bugs.
        STANDARD.decode(&sig).expect("output should be base64");
    }

    /// Same inputs must produce the same output (deterministic).
    #[test]
    fn sign_is_deterministic() {
        let secret = sim_secret_b64();
        let body = "nonce=1700000000000&pair=XBTUSD";
        let a =
            sign_kraken_request("/0/private/Balance", 1_700_000_000_000, body, &secret).unwrap();
        let b =
            sign_kraken_request("/0/private/Balance", 1_700_000_000_000, body, &secret).unwrap();
        assert_eq!(a, b);
    }

    /// Changing the nonce, the URI path, or the body must change the
    /// signature — proves all four signing inputs feed into the HMAC.
    #[test]
    fn sign_is_sensitive_to_every_input() {
        let secret = sim_secret_b64();
        let other_secret = other_secret_b64();
        let base = sign_kraken_request(
            "/0/private/Balance",
            1_700_000_000_000,
            "nonce=1700000000000",
            &secret,
        )
        .unwrap();

        let by_nonce = sign_kraken_request(
            "/0/private/Balance",
            1_700_000_000_001, // ← changed
            "nonce=1700000000000",
            &secret,
        )
        .unwrap();
        assert_ne!(by_nonce, base, "nonce change must alter signature");

        let by_path = sign_kraken_request(
            "/0/private/OpenOrders", // ← changed
            1_700_000_000_000,
            "nonce=1700000000000",
            &secret,
        )
        .unwrap();
        assert_ne!(by_path, base, "URI path change must alter signature");

        let by_body = sign_kraken_request(
            "/0/private/Balance",
            1_700_000_000_000,
            "nonce=1700000000000&extra=x", // ← changed
            &secret,
        )
        .unwrap();
        assert_ne!(by_body, base, "body change must alter signature");

        let by_secret = sign_kraken_request(
            "/0/private/Balance",
            1_700_000_000_000,
            "nonce=1700000000000",
            &other_secret, // ← changed
        )
        .unwrap();
        assert_ne!(by_secret, base, "secret change must alter signature");
    }

    #[test]
    fn sign_rejects_invalid_base64_secret() {
        let r = sign_kraken_request("/0/private/Balance", 1, "nonce=1", "not!valid!base64!");
        assert!(matches!(r, Err(ExchangeError::Auth(_))));
    }

    #[test]
    fn form_encode_empty_returns_empty_string() {
        assert_eq!(form_encode(&[]), "");
    }

    #[test]
    fn form_encode_joins_with_ampersand() {
        let body = form_encode(&[("nonce", "123"), ("pair", "XBTUSD")]);
        assert_eq!(body, "nonce=123&pair=XBTUSD");
    }

    #[test]
    fn form_encode_uses_plus_for_space() {
        let body = form_encode(&[("desc", "buy 1.0 XBTUSD")]);
        // Spaces map to '+' per application/x-www-form-urlencoded.
        assert_eq!(body, "desc=buy+1.0+XBTUSD");
    }

    #[test]
    fn form_encode_percent_encodes_reserved_chars() {
        // Includes '&' (separator) and '=' (kv separator) which MUST be encoded.
        let body = form_encode(&[("k", "a&b=c")]);
        assert_eq!(body, "k=a%26b%3Dc");
    }

    #[test]
    fn credentials_round_trip() {
        let c = KrakenCredentials::new("my-key", "my-base64-secret==");
        assert_eq!(c.api_key, "my-key");
        assert_eq!(c.api_secret_b64, "my-base64-secret==");
    }
}
