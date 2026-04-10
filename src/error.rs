use thiserror::Error;

/// All errors that can be returned by `exchange-apiws`.
#[derive(Debug, Error)]
pub enum ExchangeError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Exchange API error — code: {code}, msg: {message}")]
    Api { code: String, message: String },

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Order error: {0}")]
    Order(String),

    #[error("WebSocket disconnected after max reconnect attempts")]
    WsDisconnected,

    #[error("Insufficient data: {0}")]
    InsufficientData(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, ExchangeError>;
