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
//! | `supervised_refreshes_on_inner_exhaustion` | cycle exhaustion triggers refresh callback |
//! | `supervised_recovers_with_new_endpoint` | refresh-returned URL takes over and delivers messages |
//! | `supervised_propagates_refresh_error` | refresh closure error is surfaced to the caller |
//! | `supervised_exhausts_refresh_cycles` | bounded cycle budget → `WsDisconnected` after N cycles |
//! | `supervised_shuts_down_during_refresh` | shutdown during refresh delay exits cleanly |
//!
//! Run with:
//! ```text
//! cargo test --test ws_feed
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};

use exchange_apiws::{
    ExchangeError,
    actors::{DataMessage, ExchangeConnector, TickerData, WebSocketConfig},
    ws::{SupervisedConfig, WsFeedEndpoint, WsRunnerConfig, run_feed, run_feed_supervised},
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
        max_reconnect_delay_secs: 80,
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

// ── Supervised feed tests ──────────────────────────────────────────────────────

/// Runner config used by every supervised test: zero reconnect delay so the
/// per-cycle budget is exhausted in milliseconds rather than seconds.
fn fast_supervised(max_reconnect_attempts: u32, max_refresh_cycles: u32) -> SupervisedConfig {
    SupervisedConfig {
        runner: WsRunnerConfig {
            ping_interval_secs: 60,
            reconnect_delay_secs: 0,
            max_reconnect_delay_secs: 1,
            max_reconnect_attempts,
        },
        max_refresh_cycles,
        // Keep the inter-cycle delay tiny so the suite still finishes fast.
        refresh_delay_secs: 0,
    }
}

/// Server closes every connection immediately. With per-cycle attempts=2 and
/// max_refresh_cycles=3 the supervisor must call `refresh` 1 (bootstrap) +
/// 3 (one per exhausted cycle) = 4 times before giving up with
/// `WsDisconnected`.
#[tokio::test]
async fn supervised_refreshes_on_inner_exhaustion() {
    let (url, listener) = bind_local().await;
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

    let refresh_calls = Arc::new(AtomicU32::new(0));
    let refresh = {
        let url = url.clone();
        let refresh_calls = refresh_calls.clone();
        move || {
            let url = url.clone();
            let refresh_calls = refresh_calls.clone();
            async move {
                refresh_calls.fetch_add(1, Ordering::SeqCst);
                Ok(WsFeedEndpoint {
                    url,
                    subscriptions: vec![],
                })
            }
        }
    };

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_feed_supervised(connector, tx, fast_supervised(2, 3), sd_rx, refresh),
    )
    .await
    .expect("run_feed_supervised did not finish in time");

    assert!(
        matches!(result, Err(ExchangeError::WsDisconnected { .. })),
        "expected WsDisconnected after exhausting refresh cycles, got {result:?}"
    );
    // 1 bootstrap + 3 post-exhaustion = 4 invocations.
    assert_eq!(
        refresh_calls.load(Ordering::SeqCst),
        4,
        "expected 4 refresh invocations (bootstrap + 3 cycles)"
    );
}

/// First endpoint closes every connection. Refresh returns a second endpoint
/// (different local port) whose server sends one frame then holds open.
/// The supervisor must reach the second endpoint and deliver the message.
#[tokio::test]
async fn supervised_recovers_with_new_endpoint() {
    let (bad_url, bad_listener) = bind_local().await;
    let (good_url, good_listener) = bind_local().await;
    let (sd_tx, sd_rx) = watch::channel(false);

    // Bad server — close every connection immediately.
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = bad_listener.accept().await else {
                break;
            };
            let Ok(mut ws) = accept_async(stream).await else {
                continue;
            };
            let _ = ws.close(None).await;
        }
    });

    // Good server — send one message, then hold the connection open.
    tokio::spawn(async move {
        let mut ws = accept_one(&good_listener).await;
        ws.send(Message::Text("recovered".into())).await.unwrap();
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let connector = StubConnector::new(&bad_url);
    let (tx, mut rx) = mpsc::channel::<DataMessage>(16);

    // Closure returns the bad URL the first time (bootstrap) and the good URL
    // on every subsequent call.
    let calls = Arc::new(AtomicU32::new(0));
    let refresh = {
        let bad_url = bad_url.clone();
        let good_url = good_url.clone();
        let calls = calls.clone();
        move || {
            let bad_url = bad_url.clone();
            let good_url = good_url.clone();
            let calls = calls.clone();
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                let url = if n == 0 { bad_url } else { good_url };
                Ok(WsFeedEndpoint {
                    url,
                    subscriptions: vec![],
                })
            }
        }
    };

    let feed = tokio::spawn(run_feed_supervised(
        connector,
        tx,
        fast_supervised(1, 5),
        sd_rx,
        refresh,
    ));

    let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for message from recovered endpoint")
        .expect("channel closed before message arrived");

    assert!(
        matches!(msg, DataMessage::Ticker(_)),
        "unexpected variant: {msg:?}"
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "refresh closure should have been called at least twice (bootstrap + recovery)"
    );

    sd_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), feed)
        .await
        .expect("run_feed_supervised did not finish")
        .expect("task panicked");
}

/// Server closes every connection. Refresh returns an error on the second
/// invocation. The supervisor must propagate that error to its caller.
#[tokio::test]
async fn supervised_propagates_refresh_error() {
    let (url, listener) = bind_local().await;
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

    let calls = Arc::new(AtomicU32::new(0));
    let refresh = {
        let url = url.clone();
        let calls = calls.clone();
        move || {
            let url = url.clone();
            let calls = calls.clone();
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(WsFeedEndpoint {
                        url,
                        subscriptions: vec![],
                    })
                } else {
                    Err(ExchangeError::Auth("token endpoint down".into()))
                }
            }
        }
    };

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        run_feed_supervised(connector, tx, fast_supervised(1, 5), sd_rx, refresh),
    )
    .await
    .expect("run_feed_supervised did not finish in time");

    assert!(
        matches!(result, Err(ExchangeError::Auth(_))),
        "expected propagated Auth error, got {result:?}"
    );
}

/// `max_refresh_cycles=0` means "no refresh allowed". The supervisor exits
/// with `WsDisconnected` after the very first cycle exhausts, having called
/// the refresh closure exactly once (the bootstrap).
#[tokio::test]
async fn supervised_exhausts_refresh_cycles() {
    let (url, listener) = bind_local().await;
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

    let calls = Arc::new(AtomicU32::new(0));
    let refresh = {
        let url = url.clone();
        let calls = calls.clone();
        move || {
            let url = url.clone();
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(WsFeedEndpoint {
                    url,
                    subscriptions: vec![],
                })
            }
        }
    };

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        run_feed_supervised(connector, tx, fast_supervised(1, 0), sd_rx, refresh),
    )
    .await
    .expect("run_feed_supervised did not finish in time");

    assert!(
        matches!(result, Err(ExchangeError::WsDisconnected { .. })),
        "expected WsDisconnected, got {result:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "refresh should run exactly once (bootstrap; no cycles allowed)"
    );
}

/// Server closes every connection. The refresh closure blocks long enough
/// for the test to assert shutdown is honoured before refresh can complete.
/// The supervisor must exit `Ok(())` without waiting for the slow refresh.
#[tokio::test]
async fn supervised_shuts_down_during_refresh() {
    let (url, listener) = bind_local().await;
    let (sd_tx, sd_rx) = watch::channel(false);

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

    // Refresh succeeds first time (bootstrap). On subsequent calls it
    // signals readiness so the test knows the supervisor reached the
    // refresh wait, then sleeps long enough that shutdown should beat it.
    let waiting = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(AtomicU32::new(0));
    let refresh = {
        let url = url.clone();
        let waiting = waiting.clone();
        let calls = calls.clone();
        move || {
            let url = url.clone();
            let waiting = waiting.clone();
            let calls = calls.clone();
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                if n > 0 {
                    waiting.notify_one();
                    // Sleep longer than the test deadline so shutdown must
                    // be honoured for the supervisor to return in time.
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
                Ok(WsFeedEndpoint {
                    url,
                    subscriptions: vec![],
                })
            }
        }
    };

    // Bump refresh delay so we have time to observe the supervisor inside
    // the refresh-delay window for one of the test variants. The closure
    // covers the "stuck inside refresh()" case too.
    let config = SupervisedConfig {
        runner: WsRunnerConfig {
            ping_interval_secs: 60,
            reconnect_delay_secs: 0,
            max_reconnect_delay_secs: 1,
            max_reconnect_attempts: 1,
        },
        max_refresh_cycles: 5,
        refresh_delay_secs: 10,
    };

    let feed = tokio::spawn(run_feed_supervised(
        connector, tx, config, sd_rx, refresh,
    ));

    // Once the first cycle exhausts the supervisor enters the refresh
    // delay; fire shutdown well before that 10-second sleep elapses.
    tokio::time::sleep(Duration::from_millis(500)).await;
    sd_tx.send(true).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), feed)
        .await
        .expect("supervisor did not exit promptly after shutdown")
        .expect("task panicked");

    assert!(
        result.is_ok(),
        "expected Ok(()) on shutdown during refresh wait, got {result:?}"
    );
    // `waiting` notify is unused in this assertion path but kept above to
    // document the synchronisation point inside the refresh closure.
    let _ = waiting;
}
