//! Error types — [`ExchangeError`] and the [`Result`] alias used throughout the crate.

use thiserror::Error;

/// All errors that can be returned by `exchange-apiws`.
///
/// Marked `#[non_exhaustive]` so downstream `match` arms must include a
/// catch-all (`_`). This allows new variants to be added in minor releases
/// without breaking callers.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum ExchangeError {
    /// HTTP transport error from `reqwest`.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// WebSocket transport error from `tungstenite` (boxed to reduce enum size).
    #[error("WebSocket error: {0}")]
    WebSocket(Box<tokio_tungstenite::tungstenite::Error>),

    /// JSON serialization or deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The exchange returned a non-success response code.
    #[error("Exchange API error — code: {code}, msg: {message}")]
    Api {
        /// KuCoin error code string (e.g. `"400100"`).
        code: String,
        /// Human-readable error message from the exchange.
        message: String,
    },

    /// HMAC signing or credential validation failed.
    #[error("Authentication error: {0}")]
    Auth(String),

    /// A required configuration value is missing or invalid.
    #[error("Config error: {0}")]
    Config(String),

    /// An order-level error (e.g. trying to close a flat position).
    #[error("Order error: {0}")]
    Order(String),

    /// WebSocket feed gave up after exhausting all reconnect attempts.
    ///
    /// Carries the WS URL and the number of attempts made so callers can log
    /// which feed died and how hard it tried.
    #[error("WebSocket disconnected after {attempts} reconnect attempts on {url}")]
    WsDisconnected {
        /// The WSS URL that failed.
        url: String,
        /// Number of consecutive reconnect attempts before giving up.
        attempts: u32,
    },

    /// Not enough historical data to complete the requested operation.
    #[error("Insufficient data: {0}")]
    InsufficientData(String),

    /// Catch-all for errors from third-party libraries via `anyhow`.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<tokio_tungstenite::tungstenite::Error> for ExchangeError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(Box::new(e))
    }
}

/// Shorthand `Result` type used throughout the crate.
pub type Result<T> = std::result::Result<T, ExchangeError>;

/// How a caller should react to an [`ExchangeError`].
///
/// A coarse, actionable triage of the crate's stringly-typed exchange errors.
/// Obtain one with [`ExchangeError::classify`] (for read/query calls) or
/// [`ExchangeError::classify_submit`] (for order-submit calls, which can leave
/// the venue in an *ambiguous* state on a transport timeout).
///
/// `#[non_exhaustive]` so future variants don't break `match` arms.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorClass {
    /// Transient — safe to retry the same request after a backoff (429s,
    /// 5xx/gateway errors, transient network failures on idempotent reads,
    /// clock-skew timestamp rejections after a resync).
    Retriable,
    /// Permanent for this request — retrying unchanged will not help (bad
    /// params, insufficient balance, permission denied, invalid signature,
    /// malformed response). Surface to the operator / abort the action.
    Fatal,
    /// **Unknown outcome** — a submit may or may not have reached the matching
    /// engine (transport timeout, or a duplicate-`clientOid` rejection proving
    /// a *prior* attempt landed). The caller MUST reconcile before acting:
    /// re-query by `client_oid` (when present) via
    /// [`KuCoinClient::get_order_by_client_oid`][crate::KuCoinClient::get_order_by_client_oid]
    /// and adopt the real state rather than blindly re-placing.
    Ambiguous {
        /// The `clientOid` to reconcile with, when the caller supplied one.
        client_oid: Option<String>,
    },
}

impl ExchangeError {
    /// The KuCoin error code when this is an [`ExchangeError::Api`], else `None`.
    pub const fn kucoin_code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } => Some(code.as_str()),
            _ => None,
        }
    }

    /// `true` if the exchange rate-limited this request (HTTP 429 / KuCoin
    /// `429000`). Always also [`is_retriable`](Self::is_retriable).
    pub fn is_rate_limited(&self) -> bool {
        matches!(self.kucoin_code(), Some("429" | "429000"))
    }

    /// `true` for an authentication / signing / permission failure — a
    /// credential or clock problem, not an order problem. Covers KuCoin
    /// `400002` (bad `KC-API-TIMESTAMP`), `400004`/`400005` (bad signature /
    /// passphrase), `400006` (invalid key), `400007` (require more permission),
    /// and the crate's own [`ExchangeError::Auth`].
    pub fn is_auth(&self) -> bool {
        matches!(self, Self::Auth(_))
            || matches!(
                self.kucoin_code(),
                Some("400002" | "400003" | "400004" | "400005" | "400006" | "400007")
            )
    }

    /// `true` if a timestamp/clock-skew rejection — retry after a server-time
    /// resync ([`KuCoinClient::sync_server_time`][crate::KuCoinClient::sync_server_time]).
    pub fn is_clock_skew(&self) -> bool {
        matches!(self.kucoin_code(), Some("400002" | "400003"))
    }

    /// `true` if retrying the identical request could plausibly succeed.
    ///
    /// Transient network errors, 429s, 5xx/gateway codes, and clock-skew
    /// timestamp rejections (after a resync) are retriable; parameter,
    /// balance, permission, and signature errors are not.
    pub fn is_retriable(&self) -> bool {
        match self {
            // Transport-level failures on their own are transient. (For an
            // *order submit* prefer `classify_submit`, which treats these as
            // Ambiguous instead of blindly retriable.)
            Self::Http(_) | Self::WsDisconnected { .. } => true,
            Self::Api { code, .. } => {
                is_retriable_code(code) || self.is_rate_limited() || self.is_clock_skew()
            }
            _ => false,
        }
    }

    /// `true` for a KuCoin duplicate-`clientOid` rejection — proof that an
    /// earlier submit of the *same* `clientOid` already reached the engine.
    /// The order exists; reconcile by `clientOid` rather than re-placing.
    pub fn is_duplicate_client_oid(&self) -> bool {
        // KuCoin surfaces a duplicate clientOid as 400100 with a
        // "clientOid"/"duplicate" message, and (historically) as 300005 /
        // 100004 on some venues. Match on code plus message to avoid
        // misclassifying unrelated 400100 param errors.
        match self {
            Self::Api { code, message } => {
                let m = message.to_ascii_lowercase();
                (code == "400100" || code == "300005" || code == "100004")
                    && m.contains("clientoid")
                    && (m.contains("duplicate") || m.contains("exist"))
            }
            _ => false,
        }
    }

    /// Triage a **non-submit** error (reads, queries, cancels). Never returns
    /// [`ErrorClass::Ambiguous`] with a `client_oid` — use
    /// [`classify_submit`](Self::classify_submit) for order placement.
    pub fn classify(&self) -> ErrorClass {
        if self.is_retriable() {
            ErrorClass::Retriable
        } else {
            ErrorClass::Fatal
        }
    }

    /// Triage an **order-submit** error, given the `clientOid` the submit used.
    ///
    /// The key difference from [`classify`](Self::classify): a transport
    /// failure (`Http`) on a submit is [`ErrorClass::Ambiguous`], not
    /// `Retriable` — the request may have reached the engine before the
    /// connection dropped, so a naive retry risks a double-entry. A
    /// duplicate-`clientOid` rejection is likewise `Ambiguous` (a prior
    /// attempt landed). Everything else falls through to
    /// [`classify`](Self::classify).
    pub fn classify_submit(&self, client_oid: &str) -> ErrorClass {
        let ambiguous = || ErrorClass::Ambiguous {
            client_oid: Some(client_oid.to_string()),
        };
        match self {
            Self::Http(_) => ambiguous(),
            Self::Api { .. } if self.is_duplicate_client_oid() => ambiguous(),
            other => other.classify(),
        }
    }
}

/// Retriable KuCoin / gateway codes independent of rate-limit and clock-skew
/// (which have dedicated predicates). Covers KuCoin's own transient/system
/// codes and any surfaced 5xx-style gateway code.
fn is_retriable_code(code: &str) -> bool {
    matches!(
        code,
        // KuCoin: system busy / service unavailable / operation failed-retry.
        "100001" | "100002" | "200002" | "300001"
        // Gateway / upstream transient (when a 5xx bubbles up as a code).
        | "500000" | "502" | "503" | "504"
    ) || code.starts_with('5')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(code: &str, msg: &str) -> ExchangeError {
        ExchangeError::Api {
            code: code.to_string(),
            message: msg.to_string(),
        }
    }

    #[test]
    fn rate_limit_is_retriable() {
        assert!(api("429000", "too many").is_rate_limited());
        assert!(api("429000", "too many").is_retriable());
        assert_eq!(api("429000", "x").classify(), ErrorClass::Retriable);
    }

    #[test]
    fn server_5xx_is_retriable() {
        assert!(api("500000", "system busy").is_retriable());
        assert!(api("100001", "system busy").is_retriable());
    }

    #[test]
    fn clock_skew_is_retriable_and_auth() {
        let e = api("400002", "invalid KC-API-TIMESTAMP");
        assert!(e.is_clock_skew());
        assert!(e.is_retriable(), "resync-then-retry");
        assert!(e.is_auth());
    }

    #[test]
    fn signature_and_permission_are_fatal_auth() {
        for code in ["400004", "400005", "400006", "400007"] {
            let e = api(code, "nope");
            assert!(e.is_auth(), "{code} should be auth");
            assert!(!e.is_retriable(), "{code} must not be retriable");
            assert_eq!(e.classify(), ErrorClass::Fatal, "{code} fatal");
        }
    }

    #[test]
    fn insufficient_balance_and_bad_params_are_fatal() {
        assert_eq!(
            api("300000", "Balance insufficient").classify(),
            ErrorClass::Fatal
        );
        assert_eq!(
            api("400100", "invalid parameter").classify(),
            ErrorClass::Fatal
        );
        assert!(!api("400100", "invalid parameter").is_retriable());
    }

    #[test]
    fn duplicate_client_oid_detected_by_code_and_message() {
        assert!(api("400100", "clientOid duplicate").is_duplicate_client_oid());
        assert!(api("400100", "The clientOid already exists").is_duplicate_client_oid());
        // A plain 400100 param error must NOT be mistaken for a duplicate.
        assert!(!api("400100", "invalid leverage").is_duplicate_client_oid());
    }

    #[test]
    fn classify_submit_transport_is_ambiguous_with_oid() {
        // A duplicate-oid rejection (constructible without a reqwest error) is
        // also Ambiguous and must carry the oid to reconcile with.
        let dup = api("400100", "clientOid duplicate");
        match dup.classify_submit("oid-9") {
            ErrorClass::Ambiguous { client_oid } => {
                assert_eq!(client_oid.as_deref(), Some("oid-9"));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn classify_submit_fatal_passes_through() {
        assert_eq!(
            api("300000", "Balance insufficient").classify_submit("oid"),
            ErrorClass::Fatal
        );
    }

    #[test]
    fn ws_disconnected_is_retriable() {
        let e = ExchangeError::WsDisconnected {
            url: "wss://x".into(),
            attempts: 5,
        };
        assert!(e.is_retriable());
    }

    #[test]
    fn kucoin_code_only_for_api() {
        assert_eq!(api("400100", "x").kucoin_code(), Some("400100"));
        assert_eq!(ExchangeError::Order("x".into()).kucoin_code(), None);
    }
}
