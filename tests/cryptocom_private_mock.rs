#![cfg(feature = "cryptocom")]

//! `CryptocomPrivateClient` integration tests via `wiremock`.
//!
//! Verifies the body-encoded signing scheme end-to-end:
//! - Every request POSTs JSON with `id`, `method`, `api_key`, `params`,
//!   `nonce`, `sig` fields
//! - The hex `sig` round-trips through the canonical signing algorithm
//! - Each endpoint returns the raw `result` Value
//! - Crypto.com error envelopes (non-zero `code`) propagate as
//!   `ExchangeError::Api`
//!
//! | Test | Endpoint |
//! |------|----------|
//! | `request_body_carries_signed_envelope` | `/private/get-account-summary` |
//! | `place_order_sends_full_params` | `/private/create-order` |
//! | `cancel_order_returns_result_value` | `/private/cancel-order` |
//! | `cancel_all_orders_returns_count_value` | `/private/cancel-all-orders` |
//! | `get_open_orders_returns_value` | `/private/get-open-orders` |
//! | `get_order_detail_returns_value` | `/private/get-order-detail` |
//! | `get_trades_returns_value` | `/private/get-trades` |
//! | `get_deposit_address_returns_value` | `/private/get-deposit-address` |
//! | `create_withdrawal_returns_id` | `/private/create-withdrawal` |
//! | `get_withdrawal_history_returns_value` | `/private/get-withdrawal-history` |
//! | `error_envelope_surfaces_as_api_error` | error propagation |
//!
//! Run with:
//! ```text
//! cargo test --test cryptocom_private_mock
//! ```

use exchange_apiws::{CryptocomCredentials, CryptocomPrivateClient, ExchangeError};
use exchange_apiws::cryptocom::sign_cryptocom_request;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok_envelope(result: Value) -> Value {
    json!({"id": -1, "method": "private/...", "code": 0, "result": result})
}

fn sim_client(server: &MockServer) -> CryptocomPrivateClient {
    CryptocomPrivateClient::with_base_url(
        CryptocomCredentials::new("sim-key", "sim-secret"),
        server.uri(),
    )
    .expect("client build")
}

#[tokio::test]
async fn request_body_carries_signed_envelope() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-account-summary"))
        // Spot-check the envelope shape — actual sig value is non-deterministic
        // (nonce/id change per call), but every request must carry these keys.
        .and(body_partial_json(json!({
            "method": "private/get-account-summary",
            "api_key": "sim-key"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "accounts": [{"currency": "BTC", "balance": "0.5"}]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .get_account_summary(Some("BTC"))
        .await
        .expect("account summary");
    assert_eq!(v["accounts"][0]["currency"], "BTC");
}

#[tokio::test]
async fn signed_envelope_matches_canonical_algorithm() {
    // Single test that pins the exact signing-payload contract by
    // recomputing the sig from intercepted (id, nonce, params) and
    // comparing — guards against accidental sort-order regressions.
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let server = MockServer::start().await;
    let captured: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));

    Mock::given(method("POST"))
        .and(path("/private/get-account-summary"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({}))))
        .expect(1)
        .mount(&server)
        .await;

    // wiremock doesn't expose the captured body via matchers, so we
    // intercept by sending the request and asking wiremock for its
    // received_requests log.
    let _ = sim_client(&server)
        .get_account_summary(Some("BTC"))
        .await
        .expect("first call to capture envelope");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let body: Value = serde_json::from_slice(&received[0].body).expect("json body");
    *captured.lock().await = Some(body.clone());

    let id = body["id"].as_i64().expect("id");
    let nonce = body["nonce"].as_i64().expect("nonce");
    let params = body["params"].clone();
    let observed_sig = body["sig"].as_str().expect("sig").to_string();

    let recomputed = sign_cryptocom_request(
        "private/get-account-summary",
        id,
        "sim-key",
        &params,
        nonce,
        "sim-secret",
    )
    .expect("recompute sig");
    assert_eq!(
        observed_sig, recomputed,
        "observed sig must match canonical signing of intercepted (method, id, api_key, params, nonce)"
    );
}

#[tokio::test]
async fn place_order_sends_full_params() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/create-order"))
        .and(body_partial_json(json!({
            "method": "private/create-order",
            "params": {
                "instrument_name": "BTC_USDT",
                "side": "BUY",
                "type": "LIMIT",
                "quantity": "0.01",
                "price": "30000"
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "order_id": "abc123",
            "client_oid": "co-1"
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .place_order("BTC_USDT", "BUY", "LIMIT", "0.01", Some("30000"))
        .await
        .expect("place order");
    assert_eq!(v["order_id"], "abc123");
}

#[tokio::test]
async fn cancel_order_returns_result_value() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/cancel-order"))
        .and(body_partial_json(json!({
            "params": {"instrument_name": "BTC_USDT", "order_id": "abc"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "order_id": "abc"
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .cancel_order("BTC_USDT", "abc")
        .await
        .expect("cancel");
    assert_eq!(v["order_id"], "abc");
}

#[tokio::test]
async fn cancel_all_orders_returns_count_value() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/cancel-all-orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({"count": 3}))))
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .cancel_all_orders("BTC_USDT")
        .await
        .expect("cancel all");
    assert_eq!(v["count"], 3);
}

#[tokio::test]
async fn get_open_orders_returns_value() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-open-orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "data": [{"order_id": "o1", "side": "BUY"}]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .get_open_orders(Some("BTC_USDT"))
        .await
        .expect("open orders");
    assert_eq!(v["data"][0]["order_id"], "o1");
}

#[tokio::test]
async fn get_order_detail_returns_value() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-order-detail"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "order_id": "abc",
            "status": "FILLED"
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .get_order_detail("abc")
        .await
        .expect("order detail");
    assert_eq!(v["status"], "FILLED");
}

#[tokio::test]
async fn get_trades_returns_value() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-trades"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "data": [{"trade_id": "t1", "price": "96000"}]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .get_trades(Some("BTC_USDT"))
        .await
        .expect("trades");
    assert_eq!(v["data"][0]["trade_id"], "t1");
}

#[tokio::test]
async fn get_deposit_address_returns_value() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-deposit-address"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "deposit_address_list": [{"currency": "BTC", "address": "bc1q..."}]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .get_deposit_address("BTC")
        .await
        .expect("deposit address");
    assert!(v["deposit_address_list"][0]["address"].as_str().unwrap().starts_with("bc1q"));
}

#[tokio::test]
async fn create_withdrawal_returns_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/create-withdrawal"))
        .and(body_partial_json(json!({
            "params": {"currency": "BTC", "amount": "0.05", "address": "bc1q..."}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "id": 12345,
            "status": "0"
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .create_withdrawal("BTC", "0.05", "bc1q...")
        .await
        .expect("withdrawal");
    assert_eq!(v["id"], 12345);
}

#[tokio::test]
async fn get_withdrawal_history_returns_value() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-withdrawal-history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "withdrawal_list": [{"id": 1, "status": "5"}]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let v = sim_client(&server)
        .get_withdrawal_history("BTC")
        .await
        .expect("withdrawal history");
    assert_eq!(v["withdrawal_list"][0]["status"], "5");
}

#[tokio::test]
async fn error_envelope_surfaces_as_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-account-summary"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": -1,
            "method": "private/get-account-summary",
            "code": 10004,
            "message": "BAD_REQUEST",
            "result": {}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let result = sim_client(&server).get_account_summary(None).await;
    match result {
        Err(ExchangeError::Api { code, message }) => {
            assert_eq!(code, "10004");
            assert!(message.contains("BAD_REQUEST"));
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}
