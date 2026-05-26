//! Kraken integration — public REST, private REST (signed), WS in follow-up.
//!
//! Kraken's API splits cleanly into an **unauthenticated public** side
//! (market data, system status — [`KrakenRestClient`]) and an
//! **authenticated private** side (trading, account, withdrawals —
//! [`KrakenPrivateClient`]). The private side uses HMAC-SHA512 signing
//! implemented in [`auth`]; the public envelope handling is shared.

pub mod auth;
pub mod private;
pub mod rest;

pub use auth::{KrakenCredentials, form_encode, sign_kraken_request};
pub use private::{
    KrakenAddOrderResponse, KrakenCancelResponse, KrakenClosedOrders, KrakenOpenOrders,
    KrakenOrder, KrakenOrderDescr, KrakenPrivateClient, KrakenWithdrawResponse,
};
pub use rest::{
    KrakenAsset, KrakenAssetPair, KrakenOrderBook, KrakenRestClient, KrakenSystemStatus,
    KrakenTicker, unwrap_kraken_envelope,
};
