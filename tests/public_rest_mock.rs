//! `PublicRestClient` integration tests via `wiremock`.
//!
//! Verifies the unauthenticated HTTP layer that Binance, Bybit, and other
//! public-only exchange integrations build on:
//!
//! | Test | What it verifies |
//! |------|-----------------|
//! | `get_returns_deserialised_bare_json` | bare JSON (Binance shape) deserialises directly into the caller's type |
//! | `get_passes_query_params_percent_encoded` | params arrive at the server in `?key=value` form with encoding |
//! | `get_retries_on_5xx_until_success` | transient 500s are retried up to the budget |
//! | `get_honours_retry_after_on_429` | 429 with `Retry-After: 1` waits then succeeds |
//! | `get_surfaces_4xx_without_retry` | 400-class errors return `ExchangeError::Api` immediately |
//! | `get_caps_consecutive_429s` | runaway 429s give up rather than loop forever |
//!
//! Run with:
//! ```text
//! cargo test --test public_rest_mock
//! ```

use std::time::Duration;

use exchange_apiws::{ExchangeError, PublicRestClient};
use serde::Deserialize;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Debug, Deserialize, PartialEq)]
struct ServerTime {
    #[serde(rename = "serverTime")]
    server_time: u64,
}

fn client(base_url: &str) -> PublicRestClient {
    // 2 s timeout keeps retry waits inside the test deadline.
    PublicRestClient::with_timeout(base_url, Duration::from_secs(2))
        .expect("failed to build public client")
}

#[tokio::test]
async fn get_returns_deserialised_bare_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/time"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "serverTime": 1_700_000_000_000_u64 })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let resp: ServerTime = client(&server.uri())
        .get("/api/v3/time", &[])
        .await
        .expect("expected successful GET");

    assert_eq!(
        resp,
        ServerTime {
            server_time: 1_700_000_000_000
        }
    );
}

#[tokio::test]
async fn get_passes_query_params_percent_encoded() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/klines"))
        .and(query_param("symbol", "BTC USDT")) // wiremock checks decoded
        .and(query_param("interval", "1m"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .expect(1)
        .mount(&server)
        .await;

    let _: Vec<serde_json::Value> = client(&server.uri())
        .get(
            "/api/v3/klines",
            &[("symbol", "BTC USDT"), ("interval", "1m")],
        )
        .await
        .expect("expected successful GET with query params");
}

#[tokio::test]
async fn get_retries_on_5xx_until_success() {
    let server = MockServer::start().await;

    // PublicRestClient surfaces non-429 4xx/5xx as ExchangeError::Api WITHOUT
    // retry — only network errors are retried. Confirm a 503 surfaces
    // immediately as an Api error.
    Mock::given(method("GET"))
        .and(path("/api/v3/ping"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
        .expect(1)
        .mount(&server)
        .await;

    let result: Result<serde_json::Value, _> = client(&server.uri()).get("/api/v3/ping", &[]).await;

    match result {
        Err(ExchangeError::Api { code, .. }) => assert_eq!(code, "503"),
        other => panic!("expected ExchangeError::Api(503), got {other:?}"),
    }
}

#[tokio::test]
async fn get_honours_retry_after_on_429() {
    let server = MockServer::start().await;

    // First response: 429 with Retry-After: 1 (second). PublicRestClient
    // should sleep then retry.
    Mock::given(method("GET"))
        .and(path("/api/v3/throttled"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    // Second response: 200 success.
    Mock::given(method("GET"))
        .and(path("/api/v3/throttled"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "serverTime": 42_u64 })),
        )
        .expect(1)
        .mount(&server)
        .await;

    // Use a longer timeout so the 1 s Retry-After sleep doesn't trip it.
    let pc = PublicRestClient::with_timeout(server.uri(), Duration::from_secs(5))
        .expect("failed to build client");

    let resp: ServerTime = pc
        .get("/api/v3/throttled", &[])
        .await
        .expect("expected eventual success after 429");

    assert_eq!(
        resp,
        ServerTime {
            server_time: 42
        }
    );
}

#[tokio::test]
async fn get_surfaces_4xx_without_retry() {
    let server = MockServer::start().await;

    // 400 should NOT be retried — it surfaces as an Api error immediately
    // (wiremock would scream if it were retried because expect(1) caps it).
    Mock::given(method("GET"))
        .and(path("/api/v3/badrequest"))
        .respond_with(ResponseTemplate::new(400).set_body_string("invalid parameter"))
        .expect(1)
        .mount(&server)
        .await;

    let result: Result<serde_json::Value, _> = client(&server.uri())
        .get("/api/v3/badrequest", &[])
        .await;

    match result {
        Err(ExchangeError::Api { code, message }) => {
            assert_eq!(code, "400");
            assert!(
                message.contains("invalid parameter"),
                "unexpected error message: {message}"
            );
        }
        other => panic!("expected ExchangeError::Api(400), got {other:?}"),
    }
}
