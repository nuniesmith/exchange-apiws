//! Exchange connector definitions.
//!
//! This is the single place where exchange-specific configuration lives.
//! The generic HTTP machinery ([`crate::client::KuCoinClient`]), WS runner
//! ([`crate::ws::runner`]), and data types ([`crate::actors`]) are all
//! exchange-agnostic; connectors wire them to a specific exchange.
//!
//! # Adding a new exchange
//!
//! 1. Add an `<Exchange>Env` enum listing the environment variants (live,
//!    testnet, …) and implement `rest_base()` / `ws_base()` on it.
//! 2. Add an `<Exchange>` config struct that holds the credentials and
//!    environment, and implement [`ExchangeConfig`] on it.
//! 3. Provide a `rest_client()` method on your struct that returns a
//!    [`KuCoinClient`]-equivalent (or the shared [`KuCoinClient`] pointing at
//!    the new base URL if the HTTP envelope is compatible).
//! 4. Implement [`crate::actors::ExchangeConnector`] for your WS connector
//!    struct in a new `ws/` submodule or alongside it in this file.
//! 5. Re-export the new types from `lib.rs`.
//!
//! # Example — KuCoin Futures
//!
//! ```no_run
//! use exchange_apiws::client::Credentials;
//! use exchange_apiws::connectors::{KuCoin, KucoinEnv};
//! use exchange_apiws::ws::KucoinConnector;
//! use std::sync::Arc;
//!
//! # async fn example() -> exchange_apiws::Result<()> {
//! let kucoin = KuCoin::futures(Credentials::from_env()?);
//! let client = kucoin.rest_client();
//!
//! let bal  = client.get_balance("USDT").await?;
//! let token = client.get_ws_token_public().await?;
//! let ws = Arc::new(KucoinConnector::new(&token, kucoin.env())?);
//! # Ok(())
//! # }
//! ```

use crate::client::{Credentials, KuCoinClient};

// ── Exchange config trait ─────────────────────────────────────────────────────

/// Minimal configuration that every exchange connector must provide.
///
/// Implement this trait to make a new exchange usable with the shared runner
/// and logging infrastructure. Exchange-specific concerns (auth signing,
/// response envelope format) stay inside the connector's own methods and are
/// not part of this trait — they vary too much to unify at this level.
pub trait ExchangeConfig {
    /// Short, lowercase exchange identifier used in logs and
    /// [`DataMessage`][crate::actors::DataMessage] `exchange` fields.
    fn name(&self) -> &'static str;

    /// Base URL for the exchange's REST API, without a trailing slash.
    ///
    /// e.g. `"https://api-futures.kucoin.com"`
    fn rest_base_url(&self) -> &str;
}

// ── KuCoin ────────────────────────────────────────────────────────────────────

/// KuCoin API environment — selects between Spot, Futures, and Unified.
///
/// Pass this to [`KuCoin::new`], [`KuCoinClient::new`], or
/// [`KucoinConnector::new`][crate::ws::KucoinConnector::new] to route requests
/// to the correct base URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KucoinEnv {
    /// KuCoin Spot exchange (`api.kucoin.com`).
    LiveSpot,
    /// KuCoin Futures exchange (`api-futures.kucoin.com`).
    LiveFutures,
    /// KuCoin Unified Trade Account — routes to the Spot base URL.
    Unified,
}

impl KucoinEnv {
    /// Base REST URL for this environment.
    pub const fn rest_base(self) -> &'static str {
        match self {
            Self::LiveFutures => "https://api-futures.kucoin.com",
            Self::LiveSpot | Self::Unified => "https://api.kucoin.com",
        }
    }

    /// Base WebSocket URL for this environment.
    ///
    /// In practice the actual WS endpoint is negotiated at runtime via
    /// [`KuCoinClient::get_ws_token_public`][crate::client::KuCoinClient::get_ws_token_public]
    /// or `get_ws_token_private`. This constant is provided for documentation
    /// and diagnostic purposes.
    pub const fn ws_base(self) -> &'static str {
        match self {
            Self::LiveFutures => "wss://ws-api-futures.kucoin.com",
            Self::LiveSpot | Self::Unified => "wss://ws-api.kucoin.com",
        }
    }
}

// ── KuCoin connector config ───────────────────────────────────────────────────

/// KuCoin connector configuration.
///
/// Bundles [`Credentials`] and [`KucoinEnv`] into a single struct that acts
/// as the entry point for all KuCoin interaction. Use the named constructors
/// ([`KuCoin::futures`], [`KuCoin::spot`], [`KuCoin::unified`]) rather than
/// building a client and env separately.
///
/// ```no_run
/// # use exchange_apiws::client::Credentials;
/// # use exchange_apiws::connectors::KuCoin;
/// let kucoin  = KuCoin::futures(Credentials::from_env().unwrap());
/// let client  = kucoin.rest_client();
/// ```
pub struct KuCoin {
    creds: Credentials,
    env: KucoinEnv,
}

impl KuCoin {
    /// Create a connector targeting the KuCoin **Futures** exchange.
    pub fn futures(creds: Credentials) -> Self {
        Self {
            creds,
            env: KucoinEnv::LiveFutures,
        }
    }

    /// Create a connector targeting the KuCoin **Spot** exchange.
    pub fn spot(creds: Credentials) -> Self {
        Self {
            creds,
            env: KucoinEnv::LiveSpot,
        }
    }

    /// Create a connector targeting the KuCoin **Unified Trade Account**.
    pub fn unified(creds: Credentials) -> Self {
        Self {
            creds,
            env: KucoinEnv::Unified,
        }
    }

    /// Create a connector from explicit credentials and environment.
    pub fn new(creds: Credentials, env: KucoinEnv) -> Self {
        Self { creds, env }
    }

    /// The environment this connector targets.
    ///
    /// Pass this to [`KucoinConnector::new`][crate::ws::KucoinConnector::new]
    /// when building the WebSocket side:
    ///
    /// ```no_run
    /// # use exchange_apiws::client::Credentials;
    /// # use exchange_apiws::connectors::KuCoin;
    /// # use exchange_apiws::ws::KucoinConnector;
    /// # use std::sync::Arc;
    /// # async fn example() -> exchange_apiws::Result<()> {
    /// let kucoin = KuCoin::futures(Credentials::from_env()?);
    /// let client = kucoin.rest_client();
    /// let token  = client.get_ws_token_public().await?;
    /// let ws     = Arc::new(KucoinConnector::new(&token, kucoin.env())?);
    /// # Ok(())
    /// # }
    /// ```
    pub fn env(&self) -> KucoinEnv {
        self.env
    }

    /// Build an authenticated REST client for this exchange and environment.
    ///
    /// The returned [`KuCoinClient`] is `Clone` — create it once and clone
    /// cheaply into tasks that need independent lifetimes.
    pub fn rest_client(&self) -> KuCoinClient {
        KuCoinClient::with_base_url(self.creds.clone(), self.env.rest_base())
    }
}

impl ExchangeConfig for KuCoin {
    fn name(&self) -> &'static str {
        "kucoin"
    }

    fn rest_base_url(&self) -> &str {
        self.env.rest_base()
    }
}

// ── KuCoinClient::new lives here ──────────────────────────────────────────────
//
// `client.rs` contains only exchange-agnostic HTTP plumbing and knows nothing
// about KuCoin environments. The named constructor `KuCoinClient::new` is
// implemented here (Rust allows impl blocks in any file of the same crate) so
// callers that want a one-liner can still use it without importing `KuCoin`.

impl KuCoinClient {
    /// Create a client targeting the given KuCoin environment.
    ///
    /// Prefer [`KuCoin::rest_client`] when you are already working with a
    /// [`KuCoin`] config struct, as it avoids importing `KucoinEnv` separately.
    pub fn new(creds: Credentials, env: KucoinEnv) -> Self {
        Self::with_base_url(creds, env.rest_base())
    }
}
