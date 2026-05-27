//! Crypto.com integration — public REST, private REST (signed), WS in
//! follow-up.
//!
//! Crypto.com's API splits into an **unauthenticated public** side
//! (market data — [`CryptocomRestClient`]) and an **authenticated
//! private** side ([`CryptocomPrivateClient`]) that uses HMAC-SHA256
//! with a deterministic parameter-string signature placed inside the
//! JSON body's `sig` field (distinct from KuCoin's HMAC-SHA256 header
//! scheme in [`crate::auth`] and Kraken's HMAC-SHA512 scheme in
//! [`crate::kraken::auth`]).

pub mod auth;
pub mod private;
pub mod rest;

pub use auth::{CryptocomCredentials, build_params_string, sign_cryptocom_request};
pub use private::CryptocomPrivateClient;
pub use rest::{
    CryptocomCandle, CryptocomInstrument, CryptocomOrderBook, CryptocomRestClient, CryptocomTicker,
    CryptocomTrade, CryptocomValuation, unwrap_cryptocom_envelope,
};
