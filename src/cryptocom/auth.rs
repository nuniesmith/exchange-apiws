//! Crypto.com authentication — HMAC-SHA256 signing with body-encoded `sig`.
//!
//! Crypto.com's scheme (distinct from KuCoin's HMAC-SHA256 in
//! [`crate::auth`] and Kraken's HMAC-SHA512 in
//! [`crate::kraken::auth`]):
//!
//! 1. Compute the **params string** by visiting every key in the request
//!    `params` object in **alphabetical order** and appending
//!    `key + value` for each. Nested objects recurse; arrays index into
//!    their elements.
//! 2. Compute the **signature payload**:
//!    `method || id || api_key || params_string || nonce`
//!    (the four scalar fields with the params string in the middle).
//! 3. HMAC-SHA256 with the API secret as key, hex-encode the bytes —
//!    that's the `sig` field placed in the JSON body alongside the
//!    other request fields (NOT a header).
//!
//! See [`sign_cryptocom_request`] for the canonical implementation and
//! [`build_params_string`] for the deterministic serialiser.

use std::fmt::Write;

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use zeroize::ZeroizeOnDrop;

use crate::error::{ExchangeError, Result};

type HmacSha256 = Hmac<Sha256>;

// ── Credentials ─────────────────────────────────────────────────────────────

/// Crypto.com API credentials. `ZeroizeOnDrop` to clear the secret in
/// memory on drop.
#[derive(Clone, ZeroizeOnDrop)]
pub struct CryptocomCredentials {
    /// API key (sent as the `api_key` JSON field, not a header).
    pub api_key: String,
    /// API secret used for HMAC-SHA256.
    pub api_secret: String,
}

impl CryptocomCredentials {
    /// Construct credentials directly.
    pub fn new(api_key: impl Into<String>, api_secret: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_secret: api_secret.into(),
        }
    }

    /// Load from `CRYPTOCOM_API_KEY` / `CRYPTOCOM_API_SECRET` env vars.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeError::Config`] when either variable is unset.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            api_key: env("CRYPTOCOM_API_KEY")?,
            api_secret: env("CRYPTOCOM_API_SECRET")?,
        })
    }
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| ExchangeError::Config(format!("{key} not set")))
}

// ── Signing ─────────────────────────────────────────────────────────────────

/// Sign a Crypto.com private-API request and return the hex `sig` field.
///
/// `params` is the JSON `params` object that will be POSTed; an empty
/// `params` (e.g. `Value::Null` or an empty object) sends no inner
/// keys but still contributes nothing to the params string.
///
/// # Errors
///
/// Returns [`ExchangeError::Auth`] if the HMAC constructor rejects the
/// key (impossible in practice — HMAC accepts any byte length).
pub fn sign_cryptocom_request(
    method: &str,
    id: i64,
    api_key: &str,
    params: &Value,
    nonce: i64,
    api_secret: &str,
) -> Result<String> {
    let params_string = build_params_string(params);
    let payload = format!("{method}{id}{api_key}{params_string}{nonce}");

    let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
        .map_err(|e| ExchangeError::Auth(format!("Crypto.com HMAC init failed: {e}")))?;
    mac.update(payload.as_bytes());
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

/// Build the deterministic params string for the signature payload.
///
/// Visits object keys in **alphabetical** order, concatenating
/// `key + serialized_value`. Recursively serialises nested objects and
/// arrays so order-of-insertion in the JSON map can't change the
/// signature.
#[must_use]
pub fn build_params_string(v: &Value) -> String {
    let mut out = String::new();
    append_value(&mut out, v, /* include_self_as_label */ None);
    out
}

/// Internal recursive helper. `label` is the key under which `v` is
/// being serialised (when present, the label is emitted before the
/// value). Top-level objects don't have a label — the keys inside them
/// are emitted alphabetically.
fn append_value(out: &mut String, v: &Value, label: Option<&str>) {
    if let Some(l) = label {
        out.push_str(l);
    }
    match v {
        Value::Null => {} // emit nothing
        Value::Bool(b) => write!(out, "{b}").unwrap(),
        Value::Number(n) => write!(out, "{n}").unwrap(),
        Value::String(s) => out.push_str(s),
        Value::Array(arr) => {
            for item in arr {
                append_value(out, item, None);
            }
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                append_value(out, &map[k], Some(k));
            }
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn params_string_sorts_keys_alphabetically() {
        // Insertion order is side / instrument / price, but the
        // signature payload must sort to instrument / price / side.
        let p = json!({
            "side": "BUY",
            "instrument_name": "ETH_USDT",
            "price": "0.025",
        });
        let s = build_params_string(&p);
        assert_eq!(s, "instrument_nameETH_USDTprice0.025sideBUY");
    }

    #[test]
    fn params_string_empty_object() {
        assert_eq!(build_params_string(&json!({})), "");
    }

    #[test]
    fn params_string_null_emits_nothing() {
        assert_eq!(build_params_string(&Value::Null), "");
    }

    #[test]
    fn params_string_serialises_numbers_and_bools() {
        let p = json!({"a": 1, "b": true, "c": 1.5});
        // Numbers come through Display — 1, true, 1.5 (no quotes).
        assert_eq!(build_params_string(&p), "a1btruec1.5");
    }

    #[test]
    fn params_string_nested_object_recurses_with_label() {
        // Crypto.com signs nested objects by emitting the inner key+value
        // alongside the outer label.
        let p = json!({
            "outer": {"inner_b": "y", "inner_a": "x"},
            "z": "last"
        });
        // Outer emits "outer" + (sorted-inner serialised): "outerinner_axinner_byzlast"
        assert_eq!(build_params_string(&p), "outerinner_axinner_byzlast");
    }

    #[test]
    fn signature_is_deterministic() {
        let a = sign_cryptocom_request(
            "private/get-account-summary",
            11,
            "key1",
            &json!({"currency": "BTC"}),
            1_700_000_000_000,
            "secret",
        )
        .unwrap();
        let b = sign_cryptocom_request(
            "private/get-account-summary",
            11,
            "key1",
            &json!({"currency": "BTC"}),
            1_700_000_000_000,
            "secret",
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn signature_is_sensitive_to_every_input() {
        let secret = "secret";
        let base = sign_cryptocom_request(
            "private/m",
            1,
            "k",
            &json!({"a": "x"}),
            1_700_000_000_000,
            secret,
        )
        .unwrap();

        let by_method = sign_cryptocom_request(
            "private/other",
            1,
            "k",
            &json!({"a": "x"}),
            1_700_000_000_000,
            secret,
        )
        .unwrap();
        assert_ne!(base, by_method);

        let by_id = sign_cryptocom_request(
            "private/m",
            2,
            "k",
            &json!({"a": "x"}),
            1_700_000_000_000,
            secret,
        )
        .unwrap();
        assert_ne!(base, by_id);

        let by_key = sign_cryptocom_request(
            "private/m",
            1,
            "k2",
            &json!({"a": "x"}),
            1_700_000_000_000,
            secret,
        )
        .unwrap();
        assert_ne!(base, by_key);

        let by_params = sign_cryptocom_request(
            "private/m",
            1,
            "k",
            &json!({"a": "y"}),
            1_700_000_000_000,
            secret,
        )
        .unwrap();
        assert_ne!(base, by_params);

        let by_nonce = sign_cryptocom_request(
            "private/m",
            1,
            "k",
            &json!({"a": "x"}),
            1_700_000_000_001,
            secret,
        )
        .unwrap();
        assert_ne!(base, by_nonce);

        let by_secret = sign_cryptocom_request(
            "private/m",
            1,
            "k",
            &json!({"a": "x"}),
            1_700_000_000_000,
            "other_secret",
        )
        .unwrap();
        assert_ne!(base, by_secret);
    }

    #[test]
    fn signature_is_64_char_hex() {
        // HMAC-SHA256 → 32 bytes → hex = 64 chars.
        let sig = sign_cryptocom_request(
            "private/get-account-summary",
            1,
            "k",
            &json!({}),
            1_700_000_000_000,
            "secret",
        )
        .unwrap();
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn credentials_round_trip() {
        let c = CryptocomCredentials::new("my-key", "my-secret");
        assert_eq!(c.api_key, "my-key");
        assert_eq!(c.api_secret, "my-secret");
    }
}
