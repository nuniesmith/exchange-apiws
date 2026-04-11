//! `exchange-apiws` — Exchange REST and WebSocket clients.
//!
//! Currently supports **KuCoin** (Spot, Futures, Unified).
//! The crate is designed to be exchange-agnostic: new exchanges implement the
//! [`actors::ExchangeConnector`] trait and the shared runner drives their feeds.
//!
//! # Quick start
//!
//! ```no_run
//! use std::sync::Arc;
//! use tokio::sync::{mpsc, watch};
//! use exchange_apiws::{Credentials, KuCoinClient, KucoinEnv};
//! use exchange_apiws::actors::DataMessage;
//! use exchange_apiws::ws::{KucoinConnector, WsRunnerConfig, run_feed};
//!
//! #[tokio::main]
//! async fn main() -> exchange_apiws::Result<()> {
//!     let creds  = Credentials::from_env()?;
//!     let client = KuCoinClient::new(creds, KucoinEnv::LiveFutures);
//!
//!     // ── REST ──────────────────────────────────────────────────────────────
//!     let bal  = client.get_balance("USDT").await?;
//!     let pos  = client.get_position("XBTUSDTM").await?;
//!     let bars = client.fetch_klines("XBTUSDTM", 200, "1").await?;
//!
//!     // ── WebSocket (public) ────────────────────────────────────────────────
//!     let token = client.get_ws_token_public().await?;
//!     let conn  = Arc::new(KucoinConnector::new(&token, KucoinEnv::LiveFutures)?);
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
//! ├── rest/
//! │   ├── account  — balance, positions, auto-deposit, risk limit, funding history
//! │   ├── market   — klines, ticker, order book, funding rate, mark price, contracts
//! │   └── orders   — place, close, cancel, stop orders; fill history; calc_contracts
//! ├── ws/
//! │   ├── connect  — bullet-private / bullet-public token negotiation
//! │   ├── feed     — KucoinConnector: subscription builders + frame parser
//! │   ├── runner   — run_feed: async connect/ping/reconnect loop
//! │   └── types    — WsToken, WsMessage
//! ├── actors   — ExchangeConnector trait, DataMessage and all data types
//! ├── auth     — HMAC-SHA256 signing (key version 2)
//! ├── client   — KuCoinClient, Credentials, KucoinEnv
//! ├── error    — ExchangeError, Result
//! └── types    — Candle, Side, OrderType, TimeInForce, STP, contract_value
//! ```

pub mod actors;
pub mod auth;
pub mod client;
pub mod error;
pub mod rest;
pub mod types;
pub mod ws;

// ── Primary re-exports ────────────────────────────────────────────────────────

pub use client::{Credentials, KuCoinClient, KucoinEnv};
pub use error::{ExchangeError, Result};
pub use types::{Candle, OrderType, STP, Side, TimeInForce};

// ── WS convenience re-exports ─────────────────────────────────────────────────

pub use ws::{KucoinConnector, WsRunnerConfig, run_feed};
