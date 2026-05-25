//! Bybit integration — public REST today, public WebSocket in a follow-up.
//!
//! No API keys are required: every endpoint exposed here is unauthenticated.
//! Bybit v5 unifies spot, linear (USDT perpetual), and inverse contracts
//! under one API surface keyed by a `category` parameter — encoded in this
//! crate as the [`BybitCategory`] enum.

pub mod rest;

pub use rest::{
    BybitCategory, BybitFundingRate, BybitKline, BybitListResult, BybitLongShortRatio,
    BybitOpenInterest, BybitOrderBook, BybitRestClient, BybitTicker, BybitTrade,
};
