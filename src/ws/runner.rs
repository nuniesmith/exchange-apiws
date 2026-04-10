//! WebSocket feed runner — connection, ping, reconnect, message dispatch.
//!
//! [`run_feed`] is the single entry point. It drives any [`ExchangeConnector`]
//! through the full session lifecycle:
//!
//! ```text
//! connect → subscribe → recv loop ──► parse → tx.send(DataMessage)
//!     ▲           │ ping tick
//!     │           ▼
//!     └── reconnect (exponential backoff)
//! ```
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use tokio::sync::{mpsc, watch};
//! use kucoin_apiws::{KuCoinClient, Credentials, KucoinEnv};
//! use kucoin_apiws::ws::{KucoinConnector, run_feed, WsRunnerConfig};
//! use kucoin_apiws::actors::DataMessage;
//!
//! #[tokio::main]
//! async fn main() -> kucoin_apiws::Result<()> {
//!     let client = KuCoinClient::new(Credentials::from_env()?, KucoinEnv::LiveFutures);
//!     let token  = client.get_ws_token_public().await?;
//!     let conn   = Arc::new(KucoinConnector::new(&token, KucoinEnv::LiveFutures)?);
//!
//!     // Build subscriptions for XBTUSDTM — trades + ticker.
//!     let mut subs = vec![];
//!     if let Some(s) = conn.trade_subscription("XBTUSDTM")  { subs.push(s); }
//!     if let Some(s) = conn.ticker_subscription("XBTUSDTM") { subs.push(s); }
//!
//!     let (tx, mut rx)        = mpsc::channel::<DataMessage>(1024);
//!     let (shutdown_tx, shutdown_rx) = watch::channel(false);
//!     let config = WsRunnerConfig::from_ping_interval(conn.ping_interval_secs);
//!
//!     tokio::spawn(run_feed(conn.ws_url().to_string(), subs, conn, tx, config, shutdown_rx));
//!
//!     while let Some(msg) = rx.recv().await {
//!         println!("{msg:?}");
//!     }
//!
//!     let _ = shutdown_tx.send(true);
//!     Ok(())
//! }
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio::time::interval;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::actors::{DataMessage, ExchangeConnector};
use crate::error::{BotError, Result};
use crate::ws::types::WsMessage;

// ── Config ────────────────────────────────────────────────────────────────────

/// Tuning parameters for the WS runner.
#[derive(Debug, Clone)]
pub struct WsRunnerConfig {
    /// How often to send an application-level KuCoin ping (seconds).
    pub ping_interval_secs: u64,
    /// Base reconnect delay (seconds). Doubles on each attempt, capped at 16×.
    pub reconnect_delay_secs: u64,
    /// Give up and return [`BotError::WsDisconnected`] after this many
    /// consecutive failed reconnect attempts. Set to `u32::MAX` to retry forever.
    pub max_reconnect_attempts: u32,
}

impl Default for WsRunnerConfig {
    fn default() -> Self {
        Self {
            ping_interval_secs: 20,
            reconnect_delay_secs: 5,
            max_reconnect_attempts: 10,
        }
    }
}

impl WsRunnerConfig {
    /// Build from the ping interval advertised by a KuCoin instance server.
    ///
    /// Pass `connector.ping_interval_secs` after calling [`KucoinConnector::new`].
    pub fn from_ping_interval(ping_interval_secs: u64) -> Self {
        Self {
            ping_interval_secs,
            ..Default::default()
        }
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Drive a WebSocket feed for any [`ExchangeConnector`].
///
/// Subscribes to all topics in `subscriptions` on connect, forwards parsed
/// [`DataMessage`]s to `tx`, and reconnects automatically on any disconnect.
///
/// # Arguments
/// - `ws_url`        — Full WSS URL with token query params.
/// - `subscriptions` — JSON subscription messages (build with the connector's helpers).
/// - `connector`     — Shared connector used to parse incoming frames.
/// - `tx`            — Downstream channel that receives parsed messages.
/// - `config`        — Ping interval, backoff, and max retry settings.
/// - `shutdown`      — Send `true` to request a graceful close.
///
/// # Returns
/// `Ok(())` on clean shutdown.
/// `Err(BotError::WsDisconnected)` if max reconnect attempts are exhausted.
pub async fn run_feed(
    ws_url: impl Into<String>,
    subscriptions: Vec<String>,
    connector: Arc<dyn ExchangeConnector>,
    tx: mpsc::Sender<DataMessage>,
    config: WsRunnerConfig,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let url = ws_url.into();
    let mut attempts: u32 = 0;

    loop {
        if attempts > 0 {
            // Exponential backoff, capped at 16× the base.
            let exp = (attempts - 1).min(4);
            let delay = config.reconnect_delay_secs.saturating_mul(1 << exp);
            warn!(
                attempt = attempts,
                max = config.max_reconnect_attempts,
                delay_secs = delay,
                exchange = connector.exchange_name(),
                "WS reconnecting"
            );
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }

        let outcome = single_session(
            &url,
            &subscriptions,
            connector.clone(),
            tx.clone(),
            &config,
            &mut shutdown,
        )
        .await;

        match outcome {
            SessionOutcome::ShutdownRequested => {
                info!(exchange = connector.exchange_name(), "WS feed shut down");
                return Ok(());
            }
            SessionOutcome::ReceiverDropped => {
                info!("DataMessage receiver dropped; stopping feed");
                return Ok(());
            }
            SessionOutcome::Disconnected => {
                attempts += 1;
                if attempts > config.max_reconnect_attempts {
                    error!(
                        max = config.max_reconnect_attempts,
                        exchange = connector.exchange_name(),
                        "WS max reconnect attempts exhausted"
                    );
                    return Err(BotError::WsDisconnected);
                }
            }
        }
    }
}

// ── Internal session ──────────────────────────────────────────────────────────

enum SessionOutcome {
    ShutdownRequested,
    ReceiverDropped,
    Disconnected,
}

async fn single_session(
    url: &str,
    subscriptions: &[String],
    connector: Arc<dyn ExchangeConnector>,
    tx: mpsc::Sender<DataMessage>,
    config: &WsRunnerConfig,
    shutdown: &mut watch::Receiver<bool>,
) -> SessionOutcome {
    info!(url, exchange = connector.exchange_name(), "WS connecting");

    let ws_stream = match connect_async(url).await {
        Ok((stream, _resp)) => stream,
        Err(e) => {
            warn!(error = %e, "WS connect failed");
            return SessionOutcome::Disconnected;
        }
    };

    let (mut write, mut read) = ws_stream.split();

    // Send all subscription messages before entering the recv loop.
    for sub in subscriptions {
        if let Err(e) = write.send(Message::Text(sub.clone())).await {
            warn!(error = %e, "failed to send subscription");
            return SessionOutcome::Disconnected;
        }
        debug!(topic = ?sub, "subscribed");
    }

    info!(exchange = connector.exchange_name(), "WS connected");

    let mut ping_tick = interval(Duration::from_secs(config.ping_interval_secs));
    ping_tick.tick().await; // discard the immediate first tick

    loop {
        tokio::select! {
            biased; // check shutdown before processing data under high load

            // ── Shutdown signal ──────────────────────────────────────────────
            Ok(()) = shutdown.changed() => {
                if *shutdown.borrow() {
                    let _ = write.send(Message::Close(None)).await;
                    return SessionOutcome::ShutdownRequested;
                }
            }

            // ── Incoming WS frame ────────────────────────────────────────────
            frame = read.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        match connector.parse_message(&text) {
                            Ok(msgs) => {
                                for msg in msgs {
                                    if tx.send(msg).await.is_err() {
                                        return SessionOutcome::ReceiverDropped;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, raw = %text, "parse_message error — skipping frame");
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // Respond to protocol-level pings from the server.
                        if let Err(e) = write.send(Message::Pong(data)).await {
                            warn!(error = %e, "pong send failed");
                            return SessionOutcome::Disconnected;
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        info!(frame = ?frame, "server closed WS connection");
                        return SessionOutcome::Disconnected;
                    }
                    Some(Ok(Message::Binary(_))) => {
                        debug!("unexpected binary frame — ignored");
                    }
                    Some(Ok(_)) => {} // Pong / Frame variants — no action needed
                    Some(Err(e)) => {
                        warn!(error = %e, "WS read error");
                        return SessionOutcome::Disconnected;
                    }
                    None => {
                        debug!("WS stream closed");
                        return SessionOutcome::Disconnected;
                    }
                }
            }

            // ── Application-level ping ───────────────────────────────────────
            _ = ping_tick.tick() => {
                match serde_json::to_string(&WsMessage::ping()) {
                    Ok(text) => {
                        if let Err(e) = write.send(Message::Text(text)).await {
                            warn!(error = %e, "ping send failed");
                            return SessionOutcome::Disconnected;
                        }
                        debug!(exchange = connector.exchange_name(), "sent ping");
                    }
                    Err(e) => warn!(error = %e, "ping serialisation failed"),
                }
            }
        }
    }
}
