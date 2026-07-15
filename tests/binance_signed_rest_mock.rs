#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![cfg(feature = "binance")]

//! Binance **signed** REST integration tests via `wiremock`.
//!
//! Drives the HMAC-signed client end-to-end against a mock server: the request
//! must carry the `X-MBX-APIKEY` header and a `&signature=` query parameter,
//! and the typed response must decode. A Binance `{code,msg}` error body must
//! surface as [`ExchangeError::Api`].
//!
//! | Test | Endpoint |
//! |------|----------|
//! | `get_account_sends_key_header_and_signature` | `GET /api/v3/account` |
//! | `place_order_posts_signed_and_decodes_ack` | `POST /api/v3/order` |
//! | `error_envelope_surfaces_as_api_error` | `{code,msg}` propagation |
//!
//! Run with:
//! ```text
//! cargo test --test binance_signed_rest_mock
//! ```

use exchange_apiws::{BinanceCredentials, BinanceSignedRest, ExchangeError};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const API_KEY: &str = "test-api-key";

fn client_for(server: &MockServer) -> BinanceSignedRest {
    BinanceSignedRest::with_base_url(
        BinanceCredentials::new(API_KEY, "test-secret"),
        server.uri(),
    )
    .expect("build binance signed client")
}

/// Matcher asserting the request query carries a non-empty `signature` param —
/// proof the request was actually signed (its exact value is verified by the
/// known-answer unit test in `binance::auth`).
fn has_signature(req: &Request) -> bool {
    req.url
        .query_pairs()
        .any(|(k, v)| k == "signature" && !v.is_empty())
}

#[tokio::test]
async fn get_account_sends_key_header_and_signature() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/account"))
        .and(header("X-MBX-APIKEY", API_KEY))
        .and(has_signature)
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "makerCommission": 15,
            "takerCommission": 15,
            "canTrade": true,
            "canWithdraw": true,
            "canDeposit": true,
            "accountType": "SPOT",
            "updateTime": 1_700_000_000_000_u64,
            "balances": [
                {"asset": "BTC", "free": "0.50000000", "locked": "0.10000000"},
                {"asset": "USDT", "free": "0.00000000", "locked": "0.00000000"}
            ],
            "permissions": ["SPOT"]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let account = client_for(&server)
        .get_account()
        .await
        .expect("get_account");
    assert!(account.can_trade);
    assert_eq!(account.account_type.as_deref(), Some("SPOT"));
    let btc = account.balance("BTC").expect("BTC balance");
    assert_eq!(btc.free, "0.50000000"); // wire precision preserved
    assert!((btc.total_f64() - 0.6).abs() < 1e-9);
    // Only BTC is non-zero.
    let held: Vec<&str> = account
        .non_zero_balances()
        .map(|b| b.asset.as_str())
        .collect();
    assert_eq!(held, vec!["BTC"]);
}

#[tokio::test]
async fn place_order_posts_signed_and_decodes_ack() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/order"))
        .and(header("X-MBX-APIKEY", API_KEY))
        .and(has_signature)
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "symbol": "BTCUSDT",
            "orderId": 28,
            "clientOrderId": "abc123",
            "transactTime": 1_507_725_176_595_u64,
            "status": "NEW"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let ack = client_for(&server)
        .place_order("BTCUSDT", "BUY", "MARKET", "0.01", None, None)
        .await
        .expect("place_order");
    assert_eq!(ack.symbol, "BTCUSDT");
    assert_eq!(ack.order_id, 28);
    assert_eq!(ack.status.as_deref(), Some("NEW"));
}

#[tokio::test]
async fn error_envelope_surfaces_as_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/account"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "code": -2015,
            "msg": "Invalid API-key, IP, or permissions for action."
        })))
        .mount(&server)
        .await;

    let err = client_for(&server)
        .get_account()
        .await
        .expect_err("expected an API error");
    match err {
        ExchangeError::Api { code, message } => {
            assert_eq!(code, "-2015");
            assert!(message.contains("Invalid API-key"));
        }
        other => panic!("expected ExchangeError::Api, got {other:?}"),
    }
}
