//! `kucoin-apiws` — KuCoin Futures REST client and WebSocket feed.
//!
//! # Quick start
//!
//! ```no_run
//! use std::sync::Arc;
//! use tokio::sync::{mpsc, watch};
//! use kucoin_apiws::{Credentials, KuCoinClient, KucoinEnv};
//! use kucoin_apiws::actors::DataMessage;
//! use kucoin_apiws::ws::{KucoinConnector, WsRunnerConfig, run_feed};
//!
//! #[tokio::main]
//! async fn main() -> kucoin_apiws::Result<()> {
//!     let creds  = Credentials::from_env()?;
//!     let client = KuCoinClient::new(creds, KucoinEnv::LiveFutures);
//!
//!     // ── REST ──────────────────────────────────────────────────────────────
//!     let bal  = client.get_balance("USDT").await?;
//!     let pos  = client.get_position("XBTUSDTM").await?;
//!     let bars = client.fetch_klines("XBTUSDTM", 200, "1").await?;
//!
//!     // ── WebSocket ─────────────────────────────────────────────────────────
//!     let token = client.get_ws_token_public().await?;
//!     let conn  = Arc::new(KucoinConnector::new(&token, KucoinEnv::LiveFutures)?);
//!
//!     let mut subs = vec![];
//!     if let Some(s) = conn.trade_subscription("XBTUSDTM")  { subs.push(s); }
//!     if let Some(s) = conn.ticker_subscription("XBTUSDTM") { subs.push(s); }
//!
//!     let (tx, mut rx)          = mpsc::channel::<DataMessage>(1024);
//!     let (sd_tx, sd_rx)        = watch::channel(false);
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
//! kucoin_apiws
//! ├── rest/
//! │   ├── account  — balance, position, auto-deposit, risk limit
//! │   ├── market   — klines, ticker, order book snapshot
//! │   └── orders   — place, close, cancel; calc_contracts utility
//! ├── ws/
//! │   ├── connect  — bullet-private / bullet-public token negotiation
//! │   ├── feed     — KucoinConnector: subscription builders + frame parser
//! │   ├── runner   — run_feed: async connect/ping/reconnect loop
//! │   └── types    — WsToken, WsMessage
//! ├── actors   — ExchangeConnector trait, DataMessage, TradeData, …
//! ├── auth     — HMAC-SHA256 signing (key version 2)
//! ├── client   — KuCoinClient, Credentials, KucoinEnv
//! ├── error    — BotError, Result
//! └── types    — Candle, Side, OrderType, contract_value
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
pub use error::{BotError, Result};
pub use types::{Candle, OrderType, Side};

// ── WS convenience re-exports ─────────────────────────────────────────────────

pub use ws::{KucoinConnector, WsRunnerConfig, run_feed};
