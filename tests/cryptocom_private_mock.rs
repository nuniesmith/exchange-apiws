#![allow(missing_docs)] // empty crate when feature off; no-op when on
#![cfg(feature = "cryptocom")]

//! `CryptocomPrivateClient` integration tests via `wiremock`.
//!
//! Verifies the body-encoded signing scheme end-to-end and that each endpoint
//! deserialises Crypto.com's `result` envelope into the typed response models:
//! - Every request POSTs JSON with `id`, `method`, `api_key`, `params`,
//!   `nonce`, `sig` fields
//! - The hex `sig` round-trips through the canonical signing algorithm
//! - Each endpoint returns its typed struct(s), including the string/number
//!   coercion the wallet endpoints need
//! - Crypto.com error envelopes (non-zero `code`) propagate as
//!   `ExchangeError::Api`
//!
//! | Test | Endpoint |
//! |------|----------|
//! | `request_body_carries_signed_envelope` | `/private/get-account-summary` |
//! | `signed_envelope_matches_canonical_algorithm` | signing contract |
//! | `place_order_sends_full_params` | `/private/create-order` |
//! | `cancel_order_returns_ack` | `/private/cancel-order` |
//! | `cancel_all_orders_succeeds` | `/private/cancel-all-orders` |
//! | `get_open_orders_returns_typed_orders` | `/private/get-open-orders` |
//! | `get_order_detail_returns_typed_order` | `/private/get-order-detail` |
//! | `get_trades_returns_typed_trades` | `/private/get-trades` |
//! | `get_deposit_address_returns_typed_list` | `/private/get-deposit-address` |
//! | `create_withdrawal_returns_ack` | `/private/create-withdrawal` |
//! | `get_withdrawal_history_returns_typed_list` | `/private/get-withdrawal-history` |
//! | `error_envelope_surfaces_as_api_error` | error propagation |
//!
//! Run with:
//! ```text
//! cargo test --test cryptocom_private_mock
//! ```

use exchange_apiws::cryptocom::sign_cryptocom_request;
use exchange_apiws::{CryptocomCredentials, CryptocomPrivateClient, ExchangeError};
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
        // `balance` is sent as a JSON *number* here to exercise the string/number
        // coercion; `available` as a string. Both must normalise to `String`.
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "accounts": [
                {"currency": "BTC", "balance": 0.5, "available": "0.4", "order": "0.1", "stake": "0"}
            ]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let balances = sim_client(&server)
        .get_account_summary(Some("BTC"))
        .await
        .expect("account summary");
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].currency, "BTC");
    assert_eq!(balances[0].balance.as_deref(), Some("0.5"));
    assert_eq!(balances[0].available.as_deref(), Some("0.4"));
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

    let ack = sim_client(&server)
        .place_order("BTC_USDT", "BUY", "LIMIT", "0.01", Some("30000"))
        .await
        .expect("place order");
    assert_eq!(ack.order_id, "abc123");
    assert_eq!(ack.client_oid.as_deref(), Some("co-1"));
}

#[tokio::test]
async fn cancel_order_returns_ack() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/cancel-order"))
        .and(body_partial_json(json!({
            "params": {"instrument_name": "BTC_USDT", "order_id": "abc"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "order_id": "abc",
            "client_oid": "co-1"
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let ack = sim_client(&server)
        .cancel_order("BTC_USDT", "abc")
        .await
        .expect("cancel");
    assert_eq!(ack.order_id, "abc");
}

#[tokio::test]
async fn cancel_all_orders_succeeds() {
    let server = MockServer::start().await;
    // Crypto.com returns an empty result body on success; the call resolves to
    // `()` and must not error.
    Mock::given(method("POST"))
        .and(path("/private/cancel-all-orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({}))))
        .expect(1)
        .mount(&server)
        .await;

    sim_client(&server)
        .cancel_all_orders("BTC_USDT")
        .await
        .expect("cancel all");
}

#[tokio::test]
async fn get_open_orders_returns_typed_orders() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-open-orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "data": [{
                "account_id": "52e7c00f-1324-5a6z-bfgt-de445bde21a5",
                "order_id": "o1",
                "client_oid": "1613571154900",
                "order_type": "LIMIT",
                "time_in_force": "GOOD_TILL_CANCEL",
                "side": "BUY",
                "exec_inst": [],
                "quantity": "0.0100",
                "limit_price": "50000.0",
                "order_value": "500.000000",
                "maker_fee_rate": "0.000250",
                "taker_fee_rate": "0.000400",
                "avg_price": "0.0",
                "cumulative_quantity": "0.0000",
                "cumulative_value": "0.000000",
                "cumulative_fee": "0.000000",
                "status": "ACTIVE",
                "order_date": "2021-02-17",
                "instrument_name": "BTC_USDT",
                "fee_instrument_name": "USDT",
                "create_time": 1_613_575_617_173_i64,
                "update_time": 1_613_575_617_173_i64
            }]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let orders = sim_client(&server)
        .get_open_orders(Some("BTC_USDT"))
        .await
        .expect("open orders");
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].order_id, "o1");
    assert_eq!(orders[0].instrument_name, "BTC_USDT");
    assert_eq!(orders[0].status, "ACTIVE");
    assert!((orders[0].quantity_f64() - 0.01).abs() < 1e-9);
}

#[tokio::test]
async fn get_order_detail_returns_typed_order() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-order-detail"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "account_id": "ae075bef-1234-4321-bd6g-bb9007252a63",
            "order_id": "abc",
            "client_oid": "CCXT_c2d2152cc32d40a3ae7fbf",
            "order_type": "LIMIT",
            "time_in_force": "GOOD_TILL_CANCEL",
            "side": "BUY",
            "exec_inst": [],
            "quantity": "0.00020",
            "limit_price": "20000.00",
            "order_value": "4",
            "avg_price": "20000.0",
            "cumulative_quantity": "0.00020",
            "cumulative_value": "4.0",
            "cumulative_fee": "0.0000001",
            "status": "FILLED",
            "order_date": "2023-06-15",
            "instrument_name": "BTC_USD",
            "fee_instrument_name": "BTC",
            "create_time": 1_686_870_220_684_i64,
            "create_time_ns": "1686870220684239675",
            "update_time": 1_686_870_220_684_i64
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let order = sim_client(&server)
        .get_order_detail("abc")
        .await
        .expect("order detail");
    assert_eq!(order.order_id, "abc");
    assert_eq!(order.status, "FILLED");
    assert_eq!(order.create_time_ns.as_deref(), Some("1686870220684239675"));
    assert!((order.avg_price_f64() - 20_000.0).abs() < 1e-9);
    assert!((order.cumulative_quantity_f64() - 0.0002).abs() < 1e-9);
}

#[tokio::test]
async fn get_trades_returns_typed_trades() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-trades"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "data": [{
                "account_id": "ds075abc-1234-4321-bd6g-ff9007252r63",
                "event_date": "2023-06-16",
                "journal_type": "TRADING",
                "side": "BUY",
                "instrument_name": "BTC_USD",
                "fees": "-0.0000000525",
                "trade_id": "t1",
                "trade_match_id": "4611686018455978480",
                "create_time": 1_686_941_992_887_i64,
                "traded_price": "96000",
                "traded_quantity": "0.00021",
                "fee_instrument_name": "BTC",
                "client_oid": "d1c70a60-810e-4c92-b2a0-72b931cb31e0",
                "taker_side": "TAKER",
                "order_id": "6142909895036331486",
                "create_time_ns": "1686941992887207066"
            }]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let trades = sim_client(&server)
        .get_trades(Some("BTC_USD"))
        .await
        .expect("trades");
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].trade_id, "t1");
    assert_eq!(trades[0].side, "BUY");
    assert_eq!(trades[0].fees.as_deref(), Some("-0.0000000525"));
    assert!((trades[0].traded_price_f64() - 96_000.0).abs() < 1e-9);
    assert!((trades[0].traded_quantity_f64() - 0.00021).abs() < 1e-9);
}

#[tokio::test]
async fn get_deposit_address_returns_typed_list() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-deposit-address"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "deposit_address_list": [{
                "currency": "BTC",
                "create_time": 1_686_730_755_000_i64,
                "id": "3737377",
                "address": "bc1qexampleaddress0000000000000000000000",
                "status": "1",
                "network": "BTC"
            }]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let addresses = sim_client(&server)
        .get_deposit_address("BTC")
        .await
        .expect("deposit address");
    assert_eq!(addresses.len(), 1);
    assert_eq!(addresses[0].id, "3737377");
    assert_eq!(addresses[0].status, "1");
    assert_eq!(addresses[0].network.as_deref(), Some("BTC"));
    assert!(addresses[0].address.starts_with("bc1q"));
}

#[tokio::test]
async fn create_withdrawal_returns_ack() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/create-withdrawal"))
        .and(body_partial_json(json!({
            "params": {"currency": "BTC", "amount": "0.05", "address": "bc1q..."}
        })))
        // The wallet endpoint sends `id`/`amount`/`fee` as JSON *numbers* — the
        // typed model must coerce them to `String`.
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "id": 12345,
            "amount": 0.05,
            "fee": 0.0004,
            "symbol": "BTC",
            "address": "bc1qexampleaddress0000000000000000000000",
            "client_wid": "my_withdrawal_002",
            "create_time": 1_607_063_412_000_i64,
            "network_id": null
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let ack = sim_client(&server)
        .create_withdrawal("BTC", "0.05", "bc1q...")
        .await
        .expect("withdrawal");
    assert_eq!(ack.id, "12345");
    assert_eq!(ack.amount.as_deref(), Some("0.05"));
    assert_eq!(ack.fee.as_deref(), Some("0.0004"));
    assert_eq!(ack.symbol.as_deref(), Some("BTC"));
    assert!(ack.network_id.is_none());
}

#[tokio::test]
async fn get_withdrawal_history_returns_typed_list() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/private/get-withdrawal-history"))
        // Here `id` is a JSON *string* (contrast create-withdrawal) and
        // `amount`/`fee` are numbers — all normalise to `String`.
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "withdrawal_list": [{
                "currency": "BTC",
                "client_wid": "",
                "fee": 0.0005,
                "create_time": 1_688_613_850_000_i64,
                "id": "5275977",
                "update_time": 1_688_613_850_000_i64,
                "amount": 0.0005,
                "address": "1234NMEWbiF8ZkwUMxmfzMxi2A1MQ44bMn",
                "status": "5",
                "txid": "",
                "network_id": "BTC"
            }]
        }))))
        .expect(1)
        .mount(&server)
        .await;

    let history = sim_client(&server)
        .get_withdrawal_history("BTC")
        .await
        .expect("withdrawal history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, "5275977");
    assert_eq!(history[0].currency, "BTC");
    assert_eq!(history[0].status, "5");
    assert_eq!(history[0].amount.as_deref(), Some("0.0005"));
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
