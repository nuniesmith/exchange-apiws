//! `exchange-apiws` — Exchange REST and WebSocket clients.
//!
//! Supports **KuCoin** (Spot, Futures, Unified), **Binance**, **Bybit**,
//! **Kraken**, and **Crypto.com**. The crate is designed to be exchange-
//! agnostic: new exchanges implement the [`actors::ExchangeConnector`]
//! trait and the shared runner drives their feeds.
//!
//! # Cargo features
//!
//! KuCoin is the default implementation and is always on. The four other
//! exchanges are opt-out via Cargo features:
//!
//! ```toml
//! [dependencies]
//! # All exchanges (default — same as 0.2.18 behaviour):
//! exchange-apiws = "0.2"
//!
//! # KuCoin-only — smaller compile, no Kraken/Crypto.com signing code:
//! exchange-apiws = { version = "0.2", default-features = false }
//!
//! # Just KuCoin + Binance:
//! exchange-apiws = { version = "0.2", default-features = false, features = ["binance"] }
//! ```
//!
//! Available features: `binance`, `bybit`, `kraken`, `cryptocom`.
//!
//! # Quick start
//!
//! ```no_run
//! use std::sync::Arc;
//! use tokio::sync::{mpsc, watch};
//! use exchange_apiws::client::Credentials;
//! use exchange_apiws::connectors::KuCoin;
//! use exchange_apiws::actors::{DataMessage, ExchangeConnector};
//! use exchange_apiws::ws::{KucoinConnector, WsRunnerConfig, run_feed};
//!
//! #[tokio::main]
//! async fn main() -> exchange_apiws::Result<()> {
//!     // ── Connect ───────────────────────────────────────────────────────────
//!     let kucoin = KuCoin::futures(Credentials::from_env()?);
//!     let client = kucoin.rest_client()?;
//!
//!     // ── REST ──────────────────────────────────────────────────────────────
//!     let bal  = client.get_balance("USDT").await?;
//!     let pos  = client.get_position("XBTUSDTM").await?;
//!     let bars = client.fetch_klines("XBTUSDTM", 200, "1").await?;
//!
//!     // ── WebSocket (public) ────────────────────────────────────────────────
//!     let token = client.get_ws_token_public().await?;
//!     let conn  = Arc::new(KucoinConnector::new(&token, kucoin.env())?);
//!
//!     let mut subs = vec![];
//!     if let Some(s) = conn.trade_subscription("XBTUSDTM")  { subs.push(s); }
//!     if let Some(s) = conn.ticker_subscription("XBTUSDTM") { subs.push(s); }
//!
//!     let (tx, mut rx)    = mpsc::channel::<DataMessage>(1024);
//!     let (sd_tx, sd_rx)  = watch::channel(false);
//!     let config = WsRunnerConfig::from_ping_interval(conn.ping_interval_secs);
//!
//!     tokio::spawn(run_feed(conn.ws_url().to_string(), subs, conn, tx, config, sd_rx));
//!
//!     while let Some(msg) = rx.recv().await {
//!         println!("{msg:?}");
//!     }
//!     Ok(())
//! }
//! ```
//!
//! # Module layout
//!
//! ```text
//! exchange_apiws
//! ├── connectors  — exchange-specific config: KucoinEnv, KuCoin builder, ExchangeConfig trait
//! │                 (extension point — add new exchanges here)
//! ├── rest/
//! │   ├── account  — balance, positions, auto-deposit, risk limit, funding history
//! │   ├── market   — klines, ticker, order book, funding rate, mark price, contracts
//! │   └── orders   — place, close, cancel, stop orders; fill history; KuCoinClient::calc_contracts
//! ├── ws/
//! │   ├── connect  — bullet-private / bullet-public token negotiation
//! │   ├── feed     — KucoinConnector: subscription builders + frame parser
//! │   ├── runner   — run_feed: async connect/ping/reconnect loop
//! │   └── types    — WsToken, WsMessage
//! ├── actors   — ExchangeConnector trait, DataMessage and all data types
//! ├── auth     — HMAC-SHA256 signing (key version 2, KuCoin-specific)
//! ├── book     — LocalOrderBook: snapshot+delta assembly with gap detection
//! ├── client   — KuCoinClient (KuCoin-signed HTTP), Credentials
//! ├── http     — PublicRestClient (unauthenticated HTTP); shared helpers
//! ├── error    — ExchangeError, Result
//! ├── types    — Candle, Side, OrderType, TimeInForce, STP
//! └── prelude  — curated glob-import: `use exchange_apiws::prelude::*;`
//! ```

// ── Always-on modules (KuCoin + shared runtime) ───────────────────────────────

pub mod actors;
pub mod auth;
pub mod book;
pub mod client;
pub mod connectors;
pub mod error;
pub mod http;
pub mod rest;
pub mod tls;
pub mod types;
pub mod ws;

// ── Optional per-exchange modules ─────────────────────────────────────────────

#[cfg(feature = "binance")]
pub mod binance;
#[cfg(feature = "bybit")]
pub mod bybit;
#[cfg(feature = "coinbase")]
pub mod coinbase;
#[cfg(feature = "cryptocom")]
pub mod cryptocom;
#[cfg(feature = "kraken")]
pub mod kraken;
#[cfg(feature = "okx")]
pub mod okx;

// ── Primary re-exports ────────────────────────────────────────────────────────

#[cfg(feature = "binance")]
pub use binance::{
    BinanceConnector, BinanceRestClient, BinanceUserDataConnector, BinanceUserDataRest,
};
#[cfg(feature = "bybit")]
pub use bybit::{
    BybitCategory, BybitConnector, BybitCredentials, BybitOrderAck, BybitOrderRequest,
    BybitOrderSide, BybitOrderType, BybitPrivateClient, BybitPrivateConnector, BybitRestClient,
    BybitTimeInForce,
};
pub use client::{Credentials, KuCoinClient};
#[cfg(feature = "coinbase")]
pub use coinbase::{CoinbaseChannel, CoinbaseConnector};
pub use connectors::{ExchangeConfig, KuCoin, KucoinEnv};
#[cfg(feature = "cryptocom")]
pub use cryptocom::{
    CryptocomConnector, CryptocomCredentials, CryptocomPrivateClient, CryptocomRestClient,
    CryptocomUserConnector,
};
pub use error::{ExchangeError, Result};
pub use http::PublicRestClient;
#[cfg(feature = "kraken")]
pub use kraken::{KrakenConnector, KrakenCredentials, KrakenPrivateClient, KrakenRestClient};
#[cfg(feature = "okx")]
pub use okx::{OkxChannel, OkxConnector};
pub use tls::ensure_crypto_provider;
pub use types::{Candle, OrderType, STP, Side, TimeInForce};

// ── WS convenience re-exports ─────────────────────────────────────────────────

pub use ws::{
    EventListener, KucoinConnector, RunnerEvent, SupervisedConfig, WsFeedEndpoint, WsOrderAck,
    WsOrderClient, WsRunnerConfig, run_feed, run_feed_supervised,
};

// ── Prelude ────────────────────────────────────────────────────────────────────

/// Curated glob-import surface for the common case.
///
/// `use exchange_apiws::prelude::*;` brings the error types, the unified
/// data model ([`DataMessage`](crate::actors::DataMessage) and the
/// [`ExchangeConnector`](crate::actors::ExchangeConnector) trait), the WS
/// runner entry points, and every enabled exchange's client + connector
/// into scope at once — so feed and trading code doesn't have to reach
/// into the per-module paths.
///
/// Per-exchange items follow the same Cargo features as the crate root
/// (`binance`, `bybit`, `kraken`, `cryptocom`), so the prelude only
/// surfaces what you've enabled.
///
/// ```
/// use exchange_apiws::prelude::*;
///
/// fn handle(msg: DataMessage) -> Result<()> {
///     if let DataMessage::Trade(t) = msg {
///         let _ = (t.exchange, t.symbol, t.price);
///     }
///     Ok(())
/// }
/// ```
pub mod prelude {
    // Errors.
    pub use crate::error::{ExchangeError, Result};

    // Unified data model + connector trait.
    pub use crate::actors::{
        AdvancedOrderUpdate, BalanceUpdate, CandleData, DataMessage, ExchangeConnector,
        FundingData, InstrumentEvent, OrderBookData, OrderUpdate, PositionChange, TickerData,
        TradeData, TradeSide, WebSocketConfig,
    };

    // Local order-book maintenance.
    pub use crate::book::{BookApply, LocalOrderBook};

    // WS runner + supervised feed + observability.
    pub use crate::ws::{
        EventListener, RunnerEvent, SupervisedConfig, WsFeedEndpoint, WsRunnerConfig, run_feed,
        run_feed_supervised,
    };

    // Shared HTTP + common order/value types.
    pub use crate::http::PublicRestClient;
    pub use crate::types::{Candle, OrderType, STP, Side, TimeInForce};

    // KuCoin (always on).
    pub use crate::client::{Credentials, KuCoinClient};
    pub use crate::connectors::{ExchangeConfig, KuCoin, KucoinEnv};
    pub use crate::ws::{KucoinConnector, WsOrderAck, WsOrderClient};

    // Per-exchange clients + connectors (feature-gated).
    #[cfg(feature = "binance")]
    pub use crate::binance::{
        BinanceConnector, BinanceRestClient, BinanceUserDataConnector, BinanceUserDataRest,
    };
    #[cfg(feature = "bybit")]
    pub use crate::bybit::{BybitCategory, BybitConnector, BybitRestClient};
    #[cfg(feature = "cryptocom")]
    pub use crate::cryptocom::{
        CryptocomConnector, CryptocomCredentials, CryptocomPrivateClient, CryptocomRestClient,
        CryptocomUserConnector,
    };
    #[cfg(feature = "kraken")]
    pub use crate::kraken::{
        KrakenConnector, KrakenCredentials, KrakenPrivateClient, KrakenRestClient,
    };
}
