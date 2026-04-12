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
