#![cfg(feature = "binance")]

//! Binance WS integration test via a local `tokio-tungstenite` server.
//!
//! Spins up a real WS endpoint that pushes Binance-format combined-stream
//! frames, then drives [`BinanceConnector`] through [`run_feed`] to verify
//! the connector + runner combination delivers a heterogeneous mix of
//! `DataMessage` variants end-to-end.
//!
//! Run with:
//! ```text
//! cargo test --test binance_ws_mock
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use exchange_apiws::actors::{DataMessage, ExchangeConnector};
use exchange_apiws::binance::BinanceConnector;
use exchange_apiws::ws::{WsRunnerConfig, run_feed};

/// Bind a random local port, return `(ws_url, listener)`.
async fn bind_local() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local port");
    let port = listener.local_addr().unwrap().port();
    (format!("ws://127.0.0.1:{port}"), listener)
}

/// Builds a `WsRunnerConfig` that finishes fast in tests.
const fn fast_config() -> WsRunnerConfig {
    WsRunnerConfig {
        ping_interval_secs: 60,
        reconnect_delay_secs: 0,
        max_reconnect_delay_secs: 1,
        max_reconnect_attempts: 1,
        connect_timeout_secs: 5,
        idle_timeout_secs: 0,
        on_event: None,
    }
}

/// Server pushes one frame of each Binance type — trade, ticker (bookTicker),
/// candle (kline), order-book delta, order-book snapshot (partial depth), and
/// funding (markPriceUpdate). The test asserts the connector emits all six
/// `DataMessage` variants through the runner.
#[tokio::test]
async fn run_feed_delivers_all_binance_variants() {
    let (url, listener) = bind_local().await;
    let (sd_tx, sd_rx) = watch::channel(false);

    let frames: Vec<String> = vec![
        // aggTrade → Trade
        r#"{"stream":"btcusdt@aggTrade","data":{"e":"aggTrade","E":1700000000000,"s":"BTCUSDT","a":1,"p":"96000.0","q":"0.1","f":1,"l":1,"T":1700000000050,"m":false,"M":true}}"#.into(),
        // bookTicker → Ticker
        r#"{"stream":"btcusdt@bookTicker","data":{"u":1,"s":"BTCUSDT","b":"95999.0","B":"1.0","a":"96001.0","A":"1.0"}}"#.into(),
        // kline (closed) → Candle
        r#"{"stream":"btcusdt@kline_1m","data":{"e":"kline","E":1700000000000,"s":"BTCUSDT","k":{"t":1700000000000,"T":1700000059999,"s":"BTCUSDT","i":"1m","f":1,"L":2,"o":"96000.0","c":"96100.0","h":"96200.0","l":"95900.0","v":"10.0","n":5,"x":true,"q":"961000.0","V":"5.0","Q":"480500.0","B":"0"}}}"#.into(),
        // depthUpdate → OrderBook (delta)
        r#"{"stream":"btcusdt@depth@100ms","data":{"e":"depthUpdate","E":1700000000100,"s":"BTCUSDT","U":1,"u":2,"b":[["96000.0","1.5"]],"a":[["96001.0","0.5"]]}}"#.into(),
        // partial depth (no `e`) → OrderBook (snapshot)
        r#"{"stream":"btcusdt@depth5@100ms","data":{"lastUpdateId":3,"bids":[["96000.0","1.0"]],"asks":[["96001.0","0.5"]]}}"#.into(),
        // markPriceUpdate → FundingRate
        r#"{"stream":"btcusdt@markPrice@1s","data":{"e":"markPriceUpdate","E":1700000000200,"s":"BTCUSDT","p":"96010.0","i":"96005.0","P":"96012.0","r":"0.0001","T":1700028800000}}"#.into(),
    ];

    let frame_count = frames.len();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut ws = accept_async(stream).await.expect("ws handshake");
        for f in frames {
            ws.send(Message::Text(f.into())).await.unwrap();
        }
        // Hold the connection open so the recv loop drains all frames before
        // the test signals shutdown.
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });

    let connector: Arc<dyn ExchangeConnector> =
        Arc::new(BinanceConnector::with_url(&url, "binance"));
    let (tx, mut rx) = mpsc::channel::<DataMessage>(16);

    let feed = tokio::spawn(run_feed(
        url,
        vec![],
        connector,
        tx,
        fast_config(),
        sd_rx,
    ));

    let mut got = Vec::with_capacity(frame_count);
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                Some(m) => {
                    got.push(m);
                    if got.len() == frame_count { break; }
                }
                None => break,
            },
            () = &mut deadline => panic!(
                "timed out: expected {frame_count} messages, got {n}: {got:?}",
                n = got.len(),
            ),
        }
    }

    // Every variant should be represented exactly once.
    let kinds: Vec<&'static str> = got
        .iter()
        .map(|m| match m {
            DataMessage::Trade(_) => "trade",
            DataMessage::Ticker(_) => "ticker",
            DataMessage::Candle(_) => "candle",
            DataMessage::OrderBook(ob) if ob.is_snapshot => "book.snap",
            DataMessage::OrderBook(_) => "book.delta",
            DataMessage::FundingRate(_) => "funding",
            _ => "other",
        })
        .collect();
    assert!(kinds.contains(&"trade"), "missing trade: {kinds:?}");
    assert!(kinds.contains(&"ticker"), "missing ticker: {kinds:?}");
    assert!(kinds.contains(&"candle"), "missing candle: {kinds:?}");
    assert!(kinds.contains(&"book.delta"), "missing depth delta: {kinds:?}");
    assert!(kinds.contains(&"book.snap"), "missing depth snapshot: {kinds:?}");
    assert!(kinds.contains(&"funding"), "missing funding: {kinds:?}");

    sd_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), feed)
        .await
        .expect("run_feed did not finish in time");
}
