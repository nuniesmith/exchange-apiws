//! Crypto.com integration — public REST today, private REST + WS in
//! follow-ups.
//!
//! Crypto.com's API splits into an **unauthenticated public** side
//! (market data) and an **authenticated private** side (trading,
//! account, withdrawals — uses HMAC-SHA256 with a deterministic
//! parameter-string signature in the body, distinct from KuCoin's and
//! Kraken's signing schemes). Only the public side is wired up here.

pub mod rest;

pub use rest::{
    CryptocomCandle, CryptocomInstrument, CryptocomOrderBook, CryptocomRestClient, CryptocomTicker,
    CryptocomTrade, CryptocomValuation, unwrap_cryptocom_envelope,
};
