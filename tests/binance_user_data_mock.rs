//! Integration tests for the Binance user-data `listenKey` lifecycle
//! ([`BinanceUserDataRest`]) against a local `wiremock` server — no live
//! credentials or network access. Verifies the HTTP method, path, the
//! `X-MBX-APIKEY` header, the `listenKey` query param, and error propagation.
#![cfg(feature = "binance")]

use exchange_apiws::binance::BinanceUserDataRest;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(base: &str) -> BinanceUserDataRest {
    BinanceUserDataRest::with_base_url("test-api-key", base).expect("client builds")
}

#[tokio::test]
async fn create_listen_key_returns_key_and_sends_api_key_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/userDataStream"))
        .and(header("X-MBX-APIKEY", "test-api-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "listenKey": "pqia91ma19a5s61cv6a81va65sdf19v8a65a1a5s61cv6a81va65sdf19v8a65a1"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let key = client(&server.uri())
        .create_listen_key()
        .await
        .expect("create_listen_key succeeds");
    assert!(key.starts_with("pqia91ma"), "got {key}");
}

#[tokio::test]
async fn keepalive_listen_key_puts_with_query_param() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/v3/userDataStream"))
        .and(query_param("listenKey", "abc"))
        .and(header("X-MBX-APIKEY", "test-api-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    client(&server.uri())
        .keepalive_listen_key("abc")
        .await
        .expect("keepalive succeeds");
}

#[tokio::test]
async fn close_listen_key_deletes_with_query_param() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v3/userDataStream"))
        .and(query_param("listenKey", "abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    client(&server.uri())
        .close_listen_key("abc")
        .await
        .expect("close succeeds");
}

#[tokio::test]
async fn non_2xx_status_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/userDataStream"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "code": -2014, "msg": "API-key format invalid."
        })))
        .mount(&server)
        .await;

    let err = client(&server.uri()).create_listen_key().await;
    assert!(err.is_err(), "401 should surface as an error");
}
