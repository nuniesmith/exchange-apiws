//! OKX integration — public WebSocket market data (v5).
//!
//! Currently a public-feed connector: trades, tickers, and order books over
//! `wss://ws.okx.com:8443/ws/v5/public`. Signed REST / private channels can
//! follow the same pattern as the other exchanges if needed.

pub mod ws;

pub use ws::{OkxChannel, OkxConnector};
