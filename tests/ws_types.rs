//! WebSocket feed runner integration tests.
//!
//! Spins up a real local `tokio-tungstenite` server for each test scenario
//! so `run_feed` is exercised end-to-end — connect, subscribe, receive,
//! reconnect, and shutdown — without any live exchange credentials or network
//! access.
//!
//! # Test map
//!
//! | Test | What it verifies |
//! |------|-----------------|
//! | `run_feed_delivers_messages` | frames pushed by the server arrive as `DataMessage`s |
//! | `run_feed_shuts_down_cleanly` | shutdown watch flips → `run_feed` returns `Ok(())` |
//! | `run_feed_reconnects_on_disconnect` | server drop → reconnect → messages still arrive |
//! | `run_feed_exhausts_reconnects_returns_error` | repeated drops exhaust budget → `WsDisconnected` |
//!
//! Run with:
//! ```text
//! cargo test --test ws_feed
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};

use exchange_apiws::{
    ExchangeError,
    actors::{DataMessage, ExchangeConnector, TickerData, WebSocketConfig},
    ws::{WsRunnerConfig, run_feed},
};

// ── Stub connector ─────────────────────────────────────────────────────────────

/// Minimal `ExchangeConnector` used by all WS tests.
///
/// `parse_message` converts every non-empty text frame into a synthetic
/// `DataMessage::Ticker` so tests can assert on delivered message counts
/// without depending on exchange-specific JSON shapes.
struct StubConnector {
    url: String,
}

impl StubConnector {
    fn new(url: impl Into<String>) -> Arc<Self> {
        Arc::new(Self { url: url.into() })
    }
}

impl ExchangeConnector for StubConnector {
    fn exchange_name(&self) -> &str {
        "stub"
    }

    fn ws_url(&self) -> &str {
        &self.url
    }

    fn build_ws_config(&self, symbol: &str) -> WebSocketConfig {
        WebSocketConfig {
            url: self.url.clone(),
            exchange: "stub".into(),
            symbol: symbol.into(),
            subscription_msg: None,
            ping_interval_secs: 60,
            reconnect_delay_secs: 0,
            max_reconnect_attempts: 3,
        }
    }

    fn subscription_message(&self, _symbol: &str) -> Option<String> {
        None
    }

    fn parse_message(&self, raw: &str) -> exchange_apiws::Result<Vec<DataMessage>> {
        if raw.is_empty() {
            return Ok(vec![]);
        }
        Ok(vec![DataMessage::Ticker(TickerData {
            symbol: "TEST".into(),
            exchange: "stub".into(),
            price: 100.0,
            best_bid: 99.9,
            best_ask: 100.1,
            exchange_ts: 0,
            receipt_ts: 0,
        })])
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Bind a random OS port and return `(ws_url, listener)`.
async fn bind_local() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind local WS port");
    let port = listener.local_addr().unwrap().port();
    (format!("ws://127.0.0.1:{port}"), listener)
}

/// Accept one WS connection from `listener`.
async fn accept_one(listener: &TcpListener) -> WebSocketStream<TcpStream> {
    let (stream, _) = listener.accept().await.expect("server accept failed");
    accept_async(stream).await.expect("WS handshake failed")
}

/// Runner config with zero reconnect delay so tests finish fast.
fn fast_config(max_reconnect_attempts: u32) -> WsRunnerConfig {
    WsRunnerConfig {
        ping_interval_secs: 60,
        reconnect_delay_secs: 0,
        max_reconnect_attempts,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// Server sends three text frames then stays open while the test counts
/// delivered `DataMessage`s.  After three arrive the test sends shutdown;
/// `run_feed` must return `Ok(())`.
#[tokio::test]
async fn run_feed_delivers_messages() {
    let (url, listener) = bind_local().await;
    let (sd_tx, sd_rx) = watch::channel(false);

    // Server: send 3 frames, then drain until the client sends Close.
    tokio::spawn(async move {
        let mut ws = accept_one(&listener).await;
        for i in 0u8..3 {
            ws.send(Message::Text(format!("frame-{i}").into()))
                .await
                .unwrap();
        }
        // Hold the connection open so run_feed stays in the recv loop
        // (not the reconnect path) when the shutdown signal arrives.
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let connector = StubConnector::new(&url);
    let (tx, mut rx) = mpsc::channel::<DataMessage>(16);

    let feed = tokio::spawn(run_feed(url, vec![], connector, tx, fast_config(1), sd_rx));

    // Collect exactly 3 messages with a generous deadline.
    let mut count = 0usize;
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                Some(_) => {
                    count += 1;
                    if count == 3 { break; }
                }
                None => break,
            },
            _ = &mut deadline => panic!("timed out waiting for 3 messages, got {count}"),
        }
    }
    assert_eq!(count, 3);

    // Shutdown — run_feed must exit cleanly.
    sd_tx.send(true).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), feed)
        .await
        .expect("run_feed did not finish in time")
        .expect("task panicked");

    assert!(result.is_ok(), "expected Ok(()), got {result:?}");
}

/// Server keeps the connection open; the test requests shutdown after
/// receiving one message.  `run_feed` must return `Ok(())`.
#[tokio::test]
async fn run_feed_shuts_down_cleanly() {
    let (url, listener) = bind_local().await;
    let (sd_tx, sd_rx) = watch::channel(false);

    tokio::spawn(async move {
        let mut ws = accept_one(&listener).await;
        ws.send(Message::Text("hello".into())).await.unwrap();
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let connector = StubConnector::new(&url);
    let (tx, mut rx) = mpsc::channel::<DataMessage>(16);

    let feed = tokio::spawn(run_feed(url, vec![], connector, tx, fast_config(3), sd_rx));

    // Wait for the first message so the session is definitely live.
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for first message")
        .expect("channel closed before message");

    sd_tx.send(true).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), feed)
        .await
        .expect("run_feed did not finish in time")
        .expect("task panicked");

    assert!(result.is_ok(), "expected Ok(()), got {result:?}");
}

/// First connection: server closes immediately (no messages sent).
/// Second connection: server sends one message then holds the connection.
/// `run_feed` must reconnect and deliver the message from the second session.
#[tokio::test]
async fn run_feed_reconnects_on_disconnect() {
    let (url, listener) = bind_local().await;
    let (sd_tx, sd_rx) = watch::channel(false);

    tokio::spawn(async move {
        // Connection 1 — close immediately to force a reconnect.
        {
            let mut ws = accept_one(&listener).await;
            let _ = ws.close(None).await;
        }
        // Connection 2 — send one message then hold open.
        {
            let mut ws = accept_one(&listener).await;
            ws.send(Message::Text("reconnect-payload".into()))
                .await
                .unwrap();
            while let Some(frame) = ws.next().await {
                match frame {
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
        }
    });

    let connector = StubConnector::new(&url);
    let (tx, mut rx) = mpsc::channel::<DataMessage>(16);

    let feed = tokio::spawn(run_feed(url, vec![], connector, tx, fast_config(5), sd_rx));

    // The message from the second session must arrive within the deadline.
    let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for reconnect message")
        .expect("channel closed before message arrived");

    assert!(
        matches!(msg, DataMessage::Ticker(_)),
        "unexpected message variant: {msg:?}"
    );

    sd_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), feed)
        .await
        .expect("run_feed did not finish")
        .expect("task panicked");
}

/// Server closes every connection immediately.  With `max_reconnect_attempts`
/// set to 2, `run_feed` must give up and return `Err(WsDisconnected)`.
#[tokio::test]
async fn run_feed_exhausts_reconnects_returns_error() {
    let (url, listener) = bind_local().await;
    // Keep the sender alive so the shutdown watch never fires on its own.
    let (_sd_tx, sd_rx) = watch::channel(false);

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let Ok(mut ws) = accept_async(stream).await else {
                continue;
            };
            let _ = ws.close(None).await;
        }
    });

    let connector = StubConnector::new(&url);
    let (tx, _rx) = mpsc::channel::<DataMessage>(16);

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_feed(url, vec![], connector, tx, fast_config(2), sd_rx),
    )
    .await
    .expect("run_feed did not finish within the timeout");

    match result {
        Err(ExchangeError::WsDisconnected { attempts, .. }) => {
            assert!(
                attempts > 0,
                "expected at least one reconnect attempt, got {attempts}"
            );
        }
        other => panic!("expected WsDisconnected, got {other:?}"),
    }
}
