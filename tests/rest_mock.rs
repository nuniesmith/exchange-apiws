//! REST integration tests — exercises `get_balance`, `place_order`, envelope
//! unwrapping, and `calc_contracts` against a local `wiremock` server so no
//! live credentials or network access are needed.
//!
//! Run with:
//! ```text
//! cargo test --test rest_mock
//! ```

use exchange_apiws::{
    ExchangeError, KuCoinClient,
    client::Credentials,
    types::{OrderType, Side},
};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a `KuCoinClient` pointed at the wiremock server (sim credentials —
/// auth headers are generated but the mock ignores them).
fn sim_client(base_url: &str) -> KuCoinClient {
    KuCoinClient::with_base_url(Credentials::sim(), base_url)
}

/// Minimal KuCoin success envelope wrapping `data`.
fn ok_envelope(data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "code": "200000", "data": data })
}

/// KuCoin error envelope (non-200000 code).
fn err_envelope(code: &str, msg: &str) -> serde_json::Value {
    serde_json::json!({ "code": code, "msg": msg })
}

// ── get_balance ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_balance_returns_available_balance() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/account-overview"))
        .and(query_param("currency", "USDT"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "availableBalance": 1234.56,
                "orderMargin":      0.0,
                "positionMargin":   0.0,
                "unrealisedPNL":    0.0,
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let balance = client.get_balance("USDT").await.expect("get_balance failed");
    assert!(
        (balance - 1234.56).abs() < 1e-9,
        "expected 1234.56, got {balance}"
    );
}

// ── place_order ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn place_market_order_returns_order_id() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(serde_json::json!({ "orderId": "order-abc-123" }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .place_order(
            "XBTUSDTM",
            Side::Buy,
            1,
            10,
            OrderType::Market,
            None,  // no limit price — market order
            None,  // default TIF
            false, // not reduce-only
            None,  // no STP
        )
        .await
        .expect("place_order failed");

    assert_eq!(resp.order_id, "order-abc-123");
}

#[tokio::test]
async fn place_limit_order_without_price_returns_order_error() {
    // Client-side guard: should reject before hitting the network.
    let server = MockServer::start().await;
    let client = sim_client(&server.uri());

    let err = client
        .place_order(
            "XBTUSDTM",
            Side::Buy,
            1,
            10,
            OrderType::Limit,
            None,  // missing price — invalid for a limit order
            None,
            false,
            None,
        )
        .await
        .expect_err("should have returned an error for missing limit price");

    assert!(
        matches!(err, ExchangeError::Order(_)),
        "expected Order error, got {err:?}"
    );
    // Mock received no requests — validation fired before the HTTP call.
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

// ── Envelope unwrapping ───────────────────────────────────────────────────────

#[tokio::test]
async fn api_error_envelope_surfaces_as_exchange_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/account-overview"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(err_envelope("400100", "KC-API-KEY not exists.")),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .get_balance("USDT")
        .await
        .expect_err("should have returned an Api error");

    match err {
        ExchangeError::Api { code, message } => {
            assert_eq!(code, "400100");
            assert!(
                message.contains("KC-API-KEY"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected ExchangeError::Api, got {other:?}"),
    }
}

#[tokio::test]
async fn success_envelope_with_nested_data_deserializes_correctly() {
    // Verifies that the unwrap_envelope layer strips the outer code/data
    // wrapper before handing the payload to serde.
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(
            // Extra fields the client doesn't know about should be ignored.
            serde_json::json!({
                "orderId":  "envelope-test-456",
                "unknown":  "field",
                "nested":   { "also": "ignored" },
            }),
        )))
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let resp = client
        .place_order("XBTUSDTM", Side::Sell, 2, 5, OrderType::Market, None, None, true, None)
        .await
        .expect("envelope unwrap failed");

    assert_eq!(resp.order_id, "envelope-test-456");
}

// ── calc_contracts (runtime multiplier fetch) ─────────────────────────────────

#[tokio::test]
async fn calc_contracts_uses_runtime_multiplier() {
    let server = MockServer::start().await;

    // Stub the contract metadata endpoint.
    Mock::given(method("GET"))
        .and(path("/api/v1/contracts/XBTUSDTM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "symbol":     "XBTUSDTM",
                "multiplier": 0.001,   // 0.001 BTC per contract — the live value
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());

    // balance=10_000 USDT, leverage=10, risk=2% → notional=200 USDT
    // notional_per_ct = 86_000 * 0.001 = 86 USDT
    // margin_per_ct   = 86 / 10 = 8.6 USDT
    // raw             = 200 / 8.6 ≈ 23 contracts
    let n = client
        .calc_contracts("XBTUSDTM", 86_000.0, 10_000.0, 10, 0.02, 100)
        .await
        .expect("calc_contracts failed");

    assert_eq!(n, 23, "expected 23 contracts, got {n}");
}

#[tokio::test]
async fn calc_contracts_errors_when_multiplier_is_missing() {
    let server = MockServer::start().await;

    // Contract exists but `multiplier` is absent (e.g. a new listing not yet
    // fully populated by the exchange).
    Mock::given(method("GET"))
        .and(path("/api/v1/contracts/NEWTOKEN"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(serde_json::json!({
                "symbol": "NEWTOKEN",
                // no "multiplier" field
            }))),
        )
        .mount(&server)
        .await;

    let client = sim_client(&server.uri());
    let err = client
        .calc_contracts("NEWTOKEN", 1.0, 1000.0, 10, 0.01, 50)
        .await
        .expect_err("should have errored on missing multiplier");

    assert!(
        matches!(err, ExchangeError::Order(_)),
        "expected Order error, got {err:?}"
    );
}
