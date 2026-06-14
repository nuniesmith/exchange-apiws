//! Binance spot **user-data stream** — `listenKey` lifecycle.
//!
//! The user-data WebSocket ([`BinanceUserDataConnector`]) is keyed by a
//! `listenKey` obtained from this authenticated REST endpoint. Unlike Binance's
//! signed account/order endpoints, the listenKey endpoints require **only** the
//! `X-MBX-APIKEY` header — no HMAC signature — so this client needs just the
//! API key (full signed REST is a separate, out-of-scope concern).
//!
//! Lifecycle (all three share `/api/v3/userDataStream`):
//! - [`create_listen_key`](BinanceUserDataRest::create_listen_key) — `POST` → a fresh key.
//! - [`keepalive_listen_key`](BinanceUserDataRest::keepalive_listen_key) — `PUT` to extend
//!   validity; the key expires 60 min after the last keepalive, so call every ~30 min.
//! - [`close_listen_key`](BinanceUserDataRest::close_listen_key) — `DELETE` to end the stream.
//!
//! [`BinanceUserDataConnector`]: crate::binance::BinanceUserDataConnector
//!
//! ```no_run
//! # use exchange_apiws::binance::BinanceUserDataRest;
//! # async fn example() -> exchange_apiws::Result<()> {
//! let rest = BinanceUserDataRest::new(std::env::var("BINANCE_API_KEY").unwrap())?;
//! let listen_key = rest.create_listen_key().await?;
//! // open BinanceUserDataConnector::new(&listen_key); PUT-keepalive every ~30 min.
//! # Ok(())
//! # }
//! ```

use reqwest::Client;
use serde::Deserialize;

use crate::error::Result;

/// Binance spot REST base URL.
const SPOT_BASE_URL: &str = "https://api.binance.com";
/// The user-data stream listenKey endpoint (same path for POST / PUT / DELETE).
const LISTEN_KEY_PATH: &str = "/api/v3/userDataStream";
/// Header carrying the API key on listenKey requests.
const API_KEY_HEADER: &str = "X-MBX-APIKEY";

#[derive(Deserialize)]
struct ListenKeyResponse {
    #[serde(rename = "listenKey")]
    listen_key: String,
}

/// Authenticated client for the Binance spot user-data stream `listenKey`
/// lifecycle.
///
/// Holds only the API key (these endpoints need no HMAC signature). Construct
/// once and clone cheaply — the underlying HTTP client pools connections.
#[derive(Clone)]
pub struct BinanceUserDataRest {
    api_key: String,
    base_url: String,
    http: Client,
}

impl BinanceUserDataRest {
    /// Build a client pointed at Binance's live spot base URL.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ExchangeError::Http`] if the HTTP client cannot
    /// be constructed.
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        Self::with_base_url(api_key, SPOT_BASE_URL)
    }

    /// Build a client with a caller-supplied base URL — used by integration
    /// tests pointing at `wiremock` and by callers proxying through a custom
    /// domain or the testnet.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Result<Self> {
        crate::tls::ensure_crypto_provider();
        Ok(Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            http: Client::builder().build()?,
        })
    }

    /// `POST /api/v3/userDataStream` — create a new `listenKey`.
    ///
    /// # Errors
    ///
    /// [`crate::error::ExchangeError::Http`] on transport failure or a non-2xx
    /// status (e.g. an invalid API key), [`crate::error::ExchangeError::Json`]
    /// if the response body can't be decoded.
    pub async fn create_listen_key(&self) -> Result<String> {
        let resp = self
            .http
            .post(format!("{}{LISTEN_KEY_PATH}", self.base_url))
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await?
            .error_for_status()?;
        let body: ListenKeyResponse = resp.json().await?;
        Ok(body.listen_key)
    }

    /// `PUT /api/v3/userDataStream?listenKey=…` — extend the key's validity.
    /// Call every ~30 min; the key expires 60 min after the last keepalive.
    ///
    /// # Errors
    ///
    /// [`crate::error::ExchangeError::Http`] on transport failure or a non-2xx
    /// status (e.g. the key has already expired).
    pub async fn keepalive_listen_key(&self, listen_key: &str) -> Result<()> {
        self.http
            .put(format!(
                "{}{LISTEN_KEY_PATH}?listenKey={listen_key}",
                self.base_url
            ))
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// `DELETE /api/v3/userDataStream?listenKey=…` — close the stream early.
    ///
    /// # Errors
    ///
    /// [`crate::error::ExchangeError::Http`] on transport failure or a non-2xx
    /// status.
    pub async fn close_listen_key(&self, listen_key: &str) -> Result<()> {
        self.http
            .delete(format!(
                "{}{LISTEN_KEY_PATH}?listenKey={listen_key}",
                self.base_url
            ))
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}
