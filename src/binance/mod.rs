//! Binance integration — public REST today, public WebSocket in a follow-up.
//!
//! No API keys are required: every endpoint exposed here is unauthenticated.
//! Authenticated endpoints (account, orders) are intentionally out of scope
//! — this crate is a market-data and infrastructure library for exchanges
//! where the user has either no keys or only public access.

pub mod rest;

pub use rest::{
    BinanceBookTicker, BinanceFundingRate, BinanceKline, BinanceMarkPrice, BinanceOpenInterest,
    BinanceOrderBook, BinanceRestClient, BinanceTicker24h, BinanceTrade,
};
