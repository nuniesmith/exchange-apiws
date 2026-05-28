//! Run a KuCoin Futures WS feed under [`run_feed_supervised`] with a
//! `RunnerEvent` listener wired to a metrics counter.
//!
//! This is the **recommended pattern** for production trading bots:
//! cascades that exhaust the inner reconnect budget trigger a fresh
//! token negotiation via the closure, and the observability hook gives
//! you per-event counts without log scraping.
//!
//! Requires `KC_KEY`, `KC_SECRET`, `KC_PASSPHRASE` env vars.
//!
//! Run with:
//!
//! ```text
//! cargo run --example kucoin_supervised_feed
//! ```
//!
//! Ctrl-C exits cleanly via the shutdown channel.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use exchange_apiws::actors::{DataMessage, ExchangeConnector};
use exchange_apiws::ws::{
    EventListener, RunnerEvent, SupervisedConfig, WsFeedEndpoint, run_feed_supervised,
};
use exchange_apiws::{Credentials, KuCoin, KucoinConnector};

#[tokio::main]
async fn main() -> exchange_apiws::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let kucoin = KuCoin::futures(Credentials::from_env()?);
    let client = Arc::new(kucoin.rest_client()?);
    let env = kucoin.env();

    // Connector is fixed; only URL + subscriptions are refreshed per cycle.
    let initial_token = client.get_ws_token_public().await?;
    let connector = Arc::new(KucoinConnector::new(&initial_token, env)?);

    // Refresh closure — called on bootstrap and after every cycle exhaustion.
    let refresh = {
        let client = client.clone();
        move || {
            let client = client.clone();
            async move {
                let token = client.get_ws_token_public().await?;
                let conn = KucoinConnector::new(&token, env)?;
                let mut subs = vec![];
                if let Some(s) = conn.trade_subscription("XBTUSDTM") {
                    subs.push(s);
                }
                if let Some(s) = conn.ticker_subscription("XBTUSDTM") {
                    subs.push(s);
                }
                Ok(WsFeedEndpoint {
                    url: conn.ws_url().to_string(),
                    subscriptions: subs,
                })
            }
        }
    };

    // Observability: count cascade-start session-ends + token refreshes.
    // Replace the inner `println!` with a Redis pipeline or
    // tracing::warn! in production.
    let cascade_count = Arc::new(AtomicU64::new(0));
    let refresh_count = Arc::new(AtomicU64::new(0));
    let listener = EventListener::new({
        let cc = cascade_count.clone();
        let rc = refresh_count.clone();
        move |ev| match ev {
            RunnerEvent::SessionEnded {
                cascade_start: true,
                ..
            } => {
                let n = cc.fetch_add(1, Ordering::Relaxed) + 1;
                eprintln!("[event] cascade-start session-end #{n}");
            }
            RunnerEvent::TokenRefresh { cycle } => {
                let n = rc.fetch_add(1, Ordering::Relaxed) + 1;
                eprintln!("[event] token refresh #{n} (cycle={cycle})");
            }
            RunnerEvent::RefreshExhausted { cycles } => {
                eprintln!("[event] FATAL: supervisor exhausted after {cycles} refreshes");
            }
            _ => {}
        }
    });

    let mut config = SupervisedConfig::default();
    config.runner.on_event = Some(listener);

    let (tx, mut rx) = mpsc::channel::<DataMessage>(2048);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Ctrl-C handler.
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("ctrl-c received, shutting down");
        let _ = shutdown_tx_clone.send(true);
    });

    let feed_handle = tokio::spawn(run_feed_supervised(
        connector,
        tx,
        config,
        shutdown_rx,
        refresh,
    ));

    // Consume messages — replace with your bot's routing.
    let mut total = 0_u64;
    let mut last_print = std::time::Instant::now();
    while let Some(msg) = rx.recv().await {
        total += 1;
        // Ticker / orderbook / funding / etc — quiet by default; print
        // only trades. `DataMessage` is `#[non_exhaustive]` so a plain
        // if-let is cleanest here.
        if let DataMessage::Trade(t) = msg
            && last_print.elapsed() >= Duration::from_secs(5)
        {
            eprintln!(
                "[{:6}] last trade {} {:?} {}@{}",
                total, t.symbol, t.side, t.amount, t.price
            );
            last_print = std::time::Instant::now();
        }
    }

    // Wait for the feed task to finish cleanly.
    let _ = feed_handle.await;
    eprintln!(
        "exit — total messages {total}, cascades {}, refreshes {}",
        cascade_count.load(Ordering::Relaxed),
        refresh_count.load(Ordering::Relaxed),
    );
    Ok(())
}
