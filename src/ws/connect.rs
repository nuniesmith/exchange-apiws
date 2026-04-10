//! WebSocket token negotiation.
//!
//! Mirrors KuCoin's `bullet-private` and `bullet-public` endpoints.

use crate::client::KuCoinClient;
use crate::error::{BotError, Result};
use crate::ws::types::WsToken;
use tracing::info;

impl KuCoinClient {
    /// Fetch private WebSocket token and server list.
    ///
    /// Requires authentication. Allows subscription to private user events
    /// (order fills, balance changes) as well as public market data.
    pub async fn get_ws_token_private(&self) -> Result<WsToken> {
        info!("Fetching private WebSocket token");
        self.post("/api/v1/bullet-private", &serde_json::json!({}))
            .await
    }

    /// Fetch public WebSocket token and server list.
    ///
    /// Unauthenticated. Can only subscribe to public market data (tickers, order books, trades).
    pub async fn get_ws_token_public(&self) -> Result<WsToken> {
        info!("Fetching public WebSocket token");
        self.post("/api/v1/bullet-public", &serde_json::json!({}))
            .await
    }
}
