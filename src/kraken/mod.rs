//! Kraken integration — public REST today, private REST + WS in
//! follow-ups.
//!
//! Kraken's API has two distinct sides: an **unauthenticated public** side
//! (market data, system status — implemented here) and an
//! **authenticated private** side (trading, account, withdrawals — not
//! yet wired up; requires Kraken's HMAC-SHA512-over-SHA256 signing
//! scheme). The public-only client this module exposes is enough for
//! market-data ingestion and uses no credentials.

pub mod rest;

pub use rest::{
    KrakenAsset, KrakenAssetPair, KrakenOrderBook, KrakenRestClient, KrakenSystemStatus,
    KrakenTicker,
};
