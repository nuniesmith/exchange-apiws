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
//! | `run_feed_connect_timeout_aborts_handshake` | stalled WS upgrade is bounded by `connect_timeout_secs` |
//! | `run_feed_idle_timeout_drops_silent_connection` | sub-`idle_timeout_secs` silence drops the half-closed conn |
//! | `runner_emits_session_ended_and_exhausted_events` | `RunnerEvent::SessionEnded` + `ReconnectsExhausted` fire |
//! | `supervised_emits_token_refresh_and_exhausted_events` | supervised path emits `TokenRefresh` + `RefreshExhausted` |
//!
//! Run with:
//! ```text
//! cargo test --test ws_feed
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};

use exchange_apiws::{
    ExchangeError,
    actors::{DataMessage, ExchangeConnector, TickerData, WebSocketConfig},
    ws::{
        EventListener, RunnerEvent, SupervisedConfig, WsFeedEndpoint, WsRunnerConfig, run_feed,
        run_feed_supervised,
    },
};
use std::sync::Mutex;

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
    fn exchange_name(&self) -> &'static str {
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
const fn fast_config(max_reconnect_attempts: u32) -> WsRunnerConfig {
    WsRunnerConfig {
        ping_interval_secs: 60,
        reconnect_delay_secs: 0,
        max_reconnect_delay_secs: 80,
        max_reconnect_attempts,
        // Generous timeouts so the existing tests never trip them; the
        // dedicated connect-/idle-timeout tests below use tight values.
        connect_timeout_secs: 10,
        idle_timeout_secs: 0, // disable idle check for the default tests
        on_event: None,
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
            () = &mut deadline => panic!("timed out waiting for 3 messages, got {count}"),
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
    let _ = tokio::time::timeout(Duration::from_secs(5), feed)
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
const fn fast_supervised(max_reconnect_attempts: u32, max_refresh_cycles: u32) -> SupervisedConfig {
    SupervisedConfig {
        runner: WsRunnerConfig {
            ping_interval_secs: 60,
            reconnect_delay_secs: 0,
            max_reconnect_delay_secs: 1,
            max_reconnect_attempts,
            connect_timeout_secs: 10,
            idle_timeout_secs: 0,
            on_event: None,
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
            connect_timeout_secs: 10,
            idle_timeout_secs: 0,
            on_event: None,
        },
        max_refresh_cycles: 5,
        refresh_delay_secs: 10,
    };

    let feed = tokio::spawn(run_feed_supervised(connector, tx, config, sd_rx, refresh));

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

// ── Timeout tests ──────────────────────────────────────────────────────────────

/// Server accepts the TCP connection and reads the WS upgrade request but
/// never replies — exactly the stalled-handshake case that would hang
/// `connect_async` forever without a timeout. With `connect_timeout_secs = 1`
/// and `max_reconnect_attempts = 1`, `run_feed` must give up within a few
/// seconds rather than blocking until the OS notices the dead socket.
#[tokio::test]
async fn run_feed_connect_timeout_aborts_handshake() {
    let (url, listener) = bind_local().await;
    let (_sd_tx, sd_rx) = watch::channel(false);

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                // Consume the HTTP upgrade request so the client thinks the
                // server is alive, then sit silent. `connect_async` will
                // block waiting for the 101 Switching Protocols response.
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;
                tokio::time::sleep(Duration::from_secs(30)).await;
            });
        }
    });

    let config = WsRunnerConfig {
        ping_interval_secs: 60,
        reconnect_delay_secs: 0,
        max_reconnect_delay_secs: 1,
        max_reconnect_attempts: 1,
        connect_timeout_secs: 1,
        idle_timeout_secs: 0,
        on_event: None,
    };

    let connector = StubConnector::new(&url);
    let (tx, _rx) = mpsc::channel::<DataMessage>(16);

    // Bound: 2 attempts × 1 s connect_timeout + slack. 10 s is plenty
    // and would catch any regression that lets the handshake hang.
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_feed(url, vec![], connector, tx, config, sd_rx),
    )
    .await
    .expect("run_feed hung past the connect_timeout — Fix 4 regression");

    assert!(
        matches!(result, Err(ExchangeError::WsDisconnected { .. })),
        "expected WsDisconnected after exhausting attempts, got {result:?}"
    );
}

/// Server completes the WS handshake then goes silent — no frames, no
/// pongs to our pings. With `ping_interval_secs = 1` and
/// `idle_timeout_secs = 2`, the idle check inside the ping branch must
/// fire on the second ping tick and abort the session.
#[tokio::test]
async fn run_feed_idle_timeout_drops_silent_connection() {
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
            // Drain inbound frames silently — never reply.  Each session
            // sits like this until our idle timer terminates it.
            while let Some(frame) = ws.next().await {
                match frame {
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
        }
    });

    let config = WsRunnerConfig {
        ping_interval_secs: 1,
        reconnect_delay_secs: 0,
        max_reconnect_delay_secs: 1,
        max_reconnect_attempts: 1,
        connect_timeout_secs: 5,
        idle_timeout_secs: 2,
        on_event: None,
    };

    let connector = StubConnector::new(&url);
    let (tx, _rx) = mpsc::channel::<DataMessage>(16);

    // Bound: 2 attempts × ~2 s idle + handshake/reconnect overhead.
    // 10 s catches a regression that disables the idle path.
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_feed(url, vec![], connector, tx, config, sd_rx),
    )
    .await
    .expect("run_feed hung past the idle_timeout — Fix 4 regression");

    assert!(
        matches!(result, Err(ExchangeError::WsDisconnected { .. })),
        "expected WsDisconnected from idle path, got {result:?}"
    );
}

// ── Observability tests ───────────────────────────────────────────────────────

/// Helper: collect events into a shared Vec via an [`EventListener`].
fn collecting_listener() -> (Arc<Mutex<Vec<RunnerEvent>>>, EventListener) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let listener = {
        let events = events.clone();
        EventListener::new(move |ev| events.lock().unwrap().push(ev))
    };
    (events, listener)
}

/// Server closes every connection immediately. With `max_reconnect_attempts = 2`,
/// the listener should observe two `SessionEnded` events (both with
/// `cascade_start = true`) followed by `ReconnectsExhausted { attempts: 3 }`
/// — `attempts` is the value at the moment the budget was breached, i.e.
/// `max + 1`.
#[tokio::test]
async fn runner_emits_session_ended_and_exhausted_events() {
    let (url, listener_socket) = bind_local().await;
    let (_sd_tx, sd_rx) = watch::channel(false);

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener_socket.accept().await else {
                break;
            };
            let Ok(mut ws) = accept_async(stream).await else {
                continue;
            };
            let _ = ws.close(None).await;
        }
    });

    let (events, listener) = collecting_listener();
    let mut config = fast_config(2);
    config.on_event = Some(listener);

    let connector = StubConnector::new(&url);
    let (tx, _rx) = mpsc::channel::<DataMessage>(16);

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_feed(url, vec![], connector, tx, config, sd_rx),
    )
    .await
    .expect("run_feed hung")
    .err();

    assert!(
        matches!(result, Some(ExchangeError::WsDisconnected { .. })),
        "expected WsDisconnected, got {result:?}"
    );

    // Snapshot the events and release the lock immediately.
    let events: Vec<RunnerEvent> = events.lock().unwrap().clone();
    let session_ends = events
        .iter()
        .filter(|e| matches!(e, RunnerEvent::SessionEnded { .. }))
        .count();
    assert!(
        session_ends >= 2,
        "expected ≥ 2 SessionEnded events, got {session_ends}: {events:?}"
    );

    // First session-end should be flagged as a cascade start (attempt 0,
    // server closes immediately = sub-5 s uptime).
    let first = events
        .iter()
        .find_map(|e| match e {
            RunnerEvent::SessionEnded {
                attempt,
                cascade_start,
                ..
            } => Some((*attempt, *cascade_start)),
            _ => None,
        })
        .expect("missing SessionEnded");
    assert_eq!(
        first,
        (0, true),
        "first SessionEnded should be cascade_start"
    );

    // Final event must be ReconnectsExhausted with the breach attempt count.
    let exhausted = events.iter().rev().find_map(|e| match e {
        RunnerEvent::ReconnectsExhausted { attempts } => Some(*attempts),
        _ => None,
    });
    assert_eq!(
        exhausted,
        Some(3),
        "expected ReconnectsExhausted(3) (max=2, breached at 3): {events:?}"
    );
}

/// Supervised path with `max_refresh_cycles = 2` and a server that closes
/// every connection. Expect two `TokenRefresh` events (cycle 1, cycle 2)
/// followed by `RefreshExhausted { cycles: 3 }` once the budget is breached.
#[tokio::test]
async fn supervised_emits_token_refresh_and_exhausted_events() {
    let (url, listener_socket) = bind_local().await;
    let (_sd_tx, sd_rx) = watch::channel(false);

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener_socket.accept().await else {
                break;
            };
            let Ok(mut ws) = accept_async(stream).await else {
                continue;
            };
            let _ = ws.close(None).await;
        }
    });

    let (events, listener) = collecting_listener();
    let mut config = fast_supervised(1, 2);
    config.runner.on_event = Some(listener);

    let connector = StubConnector::new(&url);
    let (tx, _rx) = mpsc::channel::<DataMessage>(16);

    let refresh = {
        let url = url.clone();
        move || {
            let url = url.clone();
            async move {
                Ok(WsFeedEndpoint {
                    url,
                    subscriptions: vec![],
                })
            }
        }
    };

    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        run_feed_supervised(connector, tx, config, sd_rx, refresh),
    )
    .await
    .expect("supervisor hung");

    let events: Vec<RunnerEvent> = events.lock().unwrap().clone();
    let refresh_cycles: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            RunnerEvent::TokenRefresh { cycle } => Some(*cycle),
            _ => None,
        })
        .collect();
    assert_eq!(
        refresh_cycles,
        vec![1, 2],
        "expected TokenRefresh for cycles 1,2: {events:?}"
    );

    let exhausted = events.iter().rev().find_map(|e| match e {
        RunnerEvent::RefreshExhausted { cycles } => Some(*cycles),
        _ => None,
    });
    assert_eq!(
        exhausted,
        Some(3),
        "expected RefreshExhausted(3) once budget breached: {events:?}"
    );
}
