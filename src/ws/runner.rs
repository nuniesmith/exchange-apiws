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
//! ## KuCoin rate limits enforced here
//!
//! KuCoin enforces a limit of **100 client-to-server messages per 10 seconds**
//! per connection (applies to subscribe, unsubscribe, and ping messages).
//! Exceeding this may cause the server to disconnect the connection.
//! The runner enforces this limit with a sliding window before sending any
//! outbound message.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use tokio::sync::{mpsc, watch};
//! use exchange_apiws::{KuCoinClient, Credentials, KucoinEnv};
//! use exchange_apiws::actors::{DataMessage, ExchangeConnector};
//! use exchange_apiws::ws::{KucoinConnector, run_feed, WsRunnerConfig};
//!
//! #[tokio::main]
//! async fn main() -> exchange_apiws::Result<()> {
//!     let client = KuCoinClient::new(Credentials::from_env()?, KucoinEnv::LiveFutures)?;
//!     let token  = client.get_ws_token_public().await?;
//!     let conn   = Arc::new(KucoinConnector::new(&token, KucoinEnv::LiveFutures)?);
//!
//!     let mut subs = vec![];
//!     if let Some(s) = conn.trade_subscription("XBTUSDTM")  { subs.push(s); }
//!     if let Some(s) = conn.ticker_subscription("XBTUSDTM") { subs.push(s); }
//!
//!     let (tx, mut rx)               = mpsc::channel::<DataMessage>(1024);
//!     let (shutdown_tx, shutdown_rx) = watch::channel(false);
//!     let config = WsRunnerConfig::from_ping_interval(conn.ping_interval_secs);
//!
//!     tokio::spawn(run_feed(conn.ws_url().to_string(), subs, conn, tx, config, shutdown_rx));
//!
//!     while let Some(msg) = rx.recv().await {
//!         println!("{msg:?}");
//!     }
//!     let _ = shutdown_tx.send(true);
//!     Ok(())
//! }
//! ```

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio::time::{interval, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::actors::{DataMessage, ExchangeConnector};
use crate::error::{ExchangeError, Result};

// ── Observability ─────────────────────────────────────────────────────────────

/// Notable transitions emitted by the runner so callers can update metrics,
/// dashboards, or alerting without log scraping.
///
/// Plug in via [`WsRunnerConfig::on_event`] and the runner will hand each
/// event to your callback synchronously. Keep the callback fast — push to a
/// channel or atomic counter rather than blocking on network/I/O.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum RunnerEvent {
    /// A WS session ended via the Disconnected path (not a clean shutdown).
    /// The runner will sleep `reconnect_delay` and try again unless the
    /// budget is exhausted (in which case [`Self::ReconnectsExhausted`]
    /// follows). `cascade_start` mirrors the condition Fix 2 uses to
    /// promote the close-frame log to WARN.
    SessionEnded {
        /// Attempt counter at the moment the session began (0 = first try).
        attempt: u32,
        /// How many seconds the recv loop was active before disconnect.
        uptime_secs: u64,
        /// `true` when this looks like a stale-token cascade start
        /// (`attempt == 0` AND `uptime_secs < 5`).
        cascade_start: bool,
    },
    /// The runner exhausted `max_reconnect_attempts` and is about to return
    /// [`ExchangeError::WsDisconnected`] to the caller.
    ReconnectsExhausted {
        /// Final attempt count when the budget was breached.
        attempts: u32,
    },
    /// [`run_feed_supervised`] is invoking the refresh closure after an
    /// inner cycle exhausted. Emitted before the closure is awaited.
    TokenRefresh {
        /// 1-based cycle number — `1` is the first refresh, etc.
        cycle: u32,
    },
    /// [`run_feed_supervised`] exhausted `max_refresh_cycles` and is about
    /// to return [`ExchangeError::WsDisconnected`].
    RefreshExhausted {
        /// Number of refresh cycles attempted.
        cycles: u32,
    },
}

/// Type-erased synchronous listener for [`RunnerEvent`]s.
///
/// Constructed via [`EventListener::new`] from any
/// `Fn(RunnerEvent) + Send + Sync + 'static`. Cheap to clone — wraps an
/// `Arc<dyn Fn ...>` internally.
#[derive(Clone)]
pub struct EventListener(Arc<dyn Fn(RunnerEvent) + Send + Sync>);

impl EventListener {
    /// Wrap a closure as an [`EventListener`].
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(RunnerEvent) + Send + Sync + 'static,
    {
        Self(Arc::new(f))
    }
}

impl std::fmt::Debug for EventListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventListener(<callback>)")
    }
}

// ── Config ────────────────────────────────────────────────────────────────────

/// Tuning parameters for the WS runner.
#[derive(Debug, Clone)]
pub struct WsRunnerConfig {
    /// How often to send an application-level KuCoin ping (seconds).
    pub ping_interval_secs: u64,
    /// Base reconnect delay (seconds). Doubles on each attempt up to
    /// [`max_reconnect_delay_secs`].
    pub reconnect_delay_secs: u64,
    /// Hard ceiling on the per-attempt reconnect delay (seconds).
    ///
    /// Defaults to 30 s (6× the 5 s base). The 5 → 10 → 20 → 30 → 30 …
    /// schedule keeps the longest tail at 30 s, which is short enough that
    /// a typical reconnect window (5 attempts) finishes in ≈ 95 s rather
    /// than the older 9 min. Raise for non-latency-sensitive contexts.
    pub max_reconnect_delay_secs: u64,
    /// Give up and return [`ExchangeError::WsDisconnected`] after this many
    /// consecutive failed reconnect attempts. Set to `u32::MAX` to retry forever.
    ///
    /// Defaults to 5 (down from the previous 10) so a transient blip still
    /// has room to recover but a true cascade surfaces `WsDisconnected` in
    /// roughly 95 s — fast enough for the caller's outer wrapper (typically
    /// [`run_feed_supervised`]) to re-negotiate a token.
    pub max_reconnect_attempts: u32,
    /// Maximum time to wait for the WebSocket handshake to complete (seconds).
    ///
    /// Wraps `connect_async` in [`tokio::time::timeout`]. A stalled TLS or
    /// HTTP-upgrade handshake would otherwise hang indefinitely; the runner
    /// would only escape when the OS surfaces a TCP error, which can take
    /// minutes. Defaults to 10 s — plenty of margin for a healthy connect
    /// while bounding the worst case.
    pub connect_timeout_secs: u64,
    /// Maximum duration of total silence (no frames received from the
    /// server, including KuCoin's pong reply to our ping) before treating
    /// the connection as dead (seconds).
    ///
    /// Defends against half-closed TCP connections where reads block forever
    /// because the OS hasn't yet surfaced the dead socket. KuCoin sends a
    /// pong on each of our ~18 s pings, so 60 s of silence implies ≥ 3
    /// missed pongs — almost certainly a dead connection. Set to 0 to
    /// disable the idle check entirely.
    pub idle_timeout_secs: u64,
    /// Optional listener for [`RunnerEvent`]s. Called synchronously from
    /// inside the runner — push to a channel rather than blocking on
    /// network/I/O. Both [`run_feed`] and [`run_feed_supervised`] use this
    /// listener.
    pub on_event: Option<EventListener>,
}

impl Default for WsRunnerConfig {
    fn default() -> Self {
        Self {
            ping_interval_secs: 20,
            reconnect_delay_secs: 5,
            // 5 attempts × 30 s ceiling ≈ 95 s worst-case before surfacing
            // WsDisconnected. The old 10 × 80 s ≈ 9 min was tuned for
            // transient blips; cascades benefit from a faster bail.
            max_reconnect_delay_secs: 30,
            max_reconnect_attempts: 5,
            connect_timeout_secs: 10,
            idle_timeout_secs: 60,
            on_event: None,
        }
    }
}

impl WsRunnerConfig {
    /// Build from the ping interval advertised by a KuCoin instance server.
    ///
    /// Pass `connector.ping_interval_secs` after calling [`crate::ws::KucoinConnector::new`].
    pub fn from_ping_interval(ping_interval_secs: u64) -> Self {
        Self {
            ping_interval_secs,
            ..Default::default()
        }
    }

    /// Emit `event` to the listener if one is configured. No-op when
    /// `on_event` is `None`, so wiring up a listener is a single call.
    #[inline]
    fn emit(&self, event: RunnerEvent) {
        if let Some(listener) = &self.on_event {
            (listener.0)(event);
        }
    }
}

// ── Rate-limit guard ──────────────────────────────────────────────────────────

/// Sliding-window rate limiter for outbound WS messages.
///
/// KuCoin allows 100 client→server messages per 10 seconds per connection.
/// This tracks send times in a `VecDeque` and sleeps if the window is full.
struct WsMsgGuard {
    window: VecDeque<Instant>,
    max_msgs: usize,
    window_dur: Duration,
}

impl WsMsgGuard {
    fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(100),
            max_msgs: 100,
            window_dur: Duration::from_secs(10),
        }
    }

    /// Call before every outbound send. Sleeps if the 100/10s quota is full.
    async fn check(&mut self) {
        let now = Instant::now();
        // Drop timestamps older than the window.
        while self
            .window
            .front()
            .is_some_and(|t| now - *t > self.window_dur)
        {
            self.window.pop_front();
        }
        if self.window.len() >= self.max_msgs {
            // Sleep until the oldest message falls out of the window.
            if let Some(oldest) = self.window.front() {
                let wait = self.window_dur.saturating_sub(now - *oldest);
                if !wait.is_zero() {
                    warn!(
                        wait_ms = wait.as_millis(),
                        "WS outbound rate limit reached (100/10s) — throttling"
                    );
                    tokio::time::sleep(wait).await;
                }
            }
        }
        self.window.push_back(Instant::now());
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Drive a WebSocket feed for any [`ExchangeConnector`].
///
/// Subscribes to all topics in `subscriptions` on connect, forwards parsed
/// [`DataMessage`]s to `tx`, and reconnects automatically on any disconnect.
///
/// The reconnect attempt counter resets to zero whenever a session ran
/// successfully for at least [`STABLE_SESSION_SECS`] seconds. This means
/// a stable connection that eventually drops is treated the same as a fresh
/// start — it won't exhaust the attempt budget just from normal daily
/// reconnects.
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
/// `Err(ExchangeError::WsDisconnected)` if max reconnect attempts are exhausted.
pub async fn run_feed(
    ws_url: impl Into<String>,
    subscriptions: Vec<String>,
    connector: Arc<dyn ExchangeConnector>,
    tx: mpsc::Sender<DataMessage>,
    config: WsRunnerConfig,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    /// A session that ran at least this long is considered stable.
    /// After a stable session the attempt counter resets so normal
    /// daily reconnects (token expiry, rolling restarts, etc.) don't
    /// burn the retry budget.
    const STABLE_SESSION_SECS: u64 = 60;

    let url = ws_url.into();
    let mut attempts: u32 = 0;

    loop {
        if attempts > 0 {
            // Exponential backoff capped at config.max_reconnect_delay_secs.
            let exp = (attempts - 1).min(63); // guard against overflow on shift
            let delay = config
                .reconnect_delay_secs
                .saturating_mul(1u64 << exp.min(4)) // double each step
                .min(config.max_reconnect_delay_secs);
            warn!(
                attempt = attempts,
                max = config.max_reconnect_attempts,
                delay_secs = delay,
                exchange = connector.exchange_name(),
                "WS reconnecting"
            );
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }

        let session_start = Instant::now();
        let outcome = single_session(
            &url,
            &subscriptions,
            connector.clone(),
            tx.clone(),
            &config,
            &mut shutdown,
            attempts,
        )
        .await;

        match outcome {
            SessionOutcome::ShutdownRequested => {
                info!(
                    exchange = connector.exchange_name(),
                    "WS feed shut down cleanly"
                );
                return Ok(());
            }
            SessionOutcome::ReceiverDropped => {
                info!("DataMessage receiver dropped; stopping WS feed");
                return Ok(());
            }
            SessionOutcome::Disconnected => {
                let uptime_secs = session_start.elapsed().as_secs();
                // `attempts` here is the value the just-ended session was
                // running under; emit the event before mutating it.
                config.emit(RunnerEvent::SessionEnded {
                    attempt: attempts,
                    uptime_secs,
                    cascade_start: is_cascade_start(attempts, uptime_secs),
                });

                // If the session was stable for long enough, treat this as
                // a fresh start rather than a retry.  Normal causes: token
                // expiry (KuCoin tokens last ~24 h), rolling server restart,
                // or a clean network handoff.
                if uptime_secs >= STABLE_SESSION_SECS {
                    info!(
                        exchange = connector.exchange_name(),
                        uptime_secs,
                        "WS stable session ended — resetting reconnect counter",
                    );
                    attempts = 0;
                } else {
                    attempts += 1;
                    if attempts > config.max_reconnect_attempts {
                        error!(
                            max = config.max_reconnect_attempts,
                            exchange = connector.exchange_name(),
                            "WS max reconnect attempts exhausted"
                        );
                        config.emit(RunnerEvent::ReconnectsExhausted { attempts });
                        return Err(ExchangeError::WsDisconnected {
                            url: url.to_string(),
                            attempts,
                        });
                    }
                }
            }
        }
    }
}

// ── Internal session ──────────────────────────────────────────────────────────

/// A session that ended within this many seconds of subscribe is treated as
/// a cascade indicator (likely stale token) rather than a normal rotation.
///
/// Healthy KuCoin sessions live for minutes to hours; cascades close within
/// roughly a second of subscribe. 5 s gives generous margin for slow
/// networks without misclassifying real rotations.
const CASCADE_DETECT_SECS: u64 = 5;

/// Returns `true` when a session-ending event looks like the start of a
/// disconnect cascade (likely stale token), warranting WARN-level logging
/// rather than INFO.
///
/// A cascade is the combination of:
/// - `attempt == 0` — this is the first session in a new chain (the runner
///   resets the attempt counter after any session that lived for
///   [`run_feed`]'s `STABLE_SESSION_SECS` window).
/// - `uptime_secs < CASCADE_DETECT_SECS` — the session ended too quickly to
///   be a routine token rotation or rolling restart.
///
/// Subsequent attempts in a cascade are already visible via the WARN
/// "WS reconnecting" log emitted by [`run_feed`], so this only fires once
/// per cascade — at the moment with the most diagnostic value.
const fn is_cascade_start(attempt: u32, uptime_secs: u64) -> bool {
    attempt == 0 && uptime_secs < CASCADE_DETECT_SECS
}

enum SessionOutcome {
    ShutdownRequested,
    ReceiverDropped,
    Disconnected,
}

// The connect-subscribe-recv-loop is most readable as one linear function;
// splitting purely to satisfy the line-count lint would obscure the flow.
#[allow(clippy::too_many_lines)]
async fn single_session(
    url: &str,
    subscriptions: &[String],
    connector: Arc<dyn ExchangeConnector>,
    tx: mpsc::Sender<DataMessage>,
    config: &WsRunnerConfig,
    shutdown: &mut watch::Receiver<bool>,
    attempt: u32,
) -> SessionOutcome {
    info!(url, exchange = connector.exchange_name(), "WS connecting");

    let connect_timeout = Duration::from_secs(config.connect_timeout_secs);
    let ws_stream = match timeout(connect_timeout, connect_async(url)).await {
        Ok(Ok((stream, _resp))) => stream,
        Ok(Err(e)) => {
            warn!(error = %e, exchange = connector.exchange_name(), "WS connect failed");
            return SessionOutcome::Disconnected;
        }
        Err(_elapsed) => {
            warn!(
                timeout_secs = config.connect_timeout_secs,
                url,
                exchange = connector.exchange_name(),
                "WS connect timed out — handshake stalled"
            );
            return SessionOutcome::Disconnected;
        }
    };

    let (mut write, mut read) = ws_stream.split();
    let mut guard = WsMsgGuard::new();

    // Send all subscription messages before entering the recv loop.
    for sub in subscriptions {
        guard.check().await;
        if let Err(e) = write.send(Message::Text(sub.clone().into())).await {
            warn!(error = %e, "failed to send subscription");
            return SessionOutcome::Disconnected;
        }
        debug!(topic = ?sub, "subscribed");
    }

    info!(
        exchange = connector.exchange_name(),
        "WS connected and subscribed"
    );

    // Mark the start of the recv-loop phase so we can distinguish a normal
    // long-uptime rotation from a cascade where the server closes us within
    // a second of subscribe.
    let subscribed_at = Instant::now();

    // Track the last time we received any frame so the ping branch can fire
    // an idle-timeout abort when a half-closed TCP connection leaves reads
    // hanging forever. Seeded to "now" so a quiet symbol doesn't trip on
    // the very first tick.
    let mut last_frame_at = Instant::now();

    let mut ping_tick = interval(Duration::from_secs(config.ping_interval_secs));
    ping_tick.tick().await; // discard the immediate first tick

    loop {
        tokio::select! {
            biased; // prioritise shutdown check under high message load

            // ── Shutdown signal ──────────────────────────────────────────────
            Ok(()) = shutdown.changed() => {
                if *shutdown.borrow() {
                    guard.check().await;
                    let _ = write.send(Message::Close(None)).await;
                    return SessionOutcome::ShutdownRequested;
                }
            }

            // ── Incoming WS frame ────────────────────────────────────────────
            frame = read.next() => {
                // Any non-None outcome counts as receiving something from
                // the wire — including errors and close frames — so the
                // idle check below correctly resets on real activity.
                if frame.is_some() {
                    last_frame_at = Instant::now();
                }
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
                        let uptime_secs = subscribed_at.elapsed().as_secs();
                        let close_code = frame.as_ref().map(|f| u16::from(f.code));
                        let close_reason = frame
                            .as_ref()
                            .map(|f| f.reason.to_string())
                            .unwrap_or_default();
                        if is_cascade_start(attempt, uptime_secs) {
                            // First attempt + sub-5 s uptime = the server is
                            // closing us right after subscribe. Almost always
                            // a stale-token cascade. Surface this at WARN so
                            // production log filters see the close reason.
                            warn!(
                                uptime_secs,
                                attempt,
                                close_code,
                                close_reason = %close_reason,
                                exchange = connector.exchange_name(),
                                "WS server closed connection early — likely cascade start"
                            );
                        } else {
                            info!(
                                uptime_secs,
                                attempt,
                                close_code,
                                close_reason = %close_reason,
                                exchange = connector.exchange_name(),
                                "WS server closed connection"
                            );
                        }
                        return SessionOutcome::Disconnected;
                    }
                    Some(Ok(Message::Binary(_))) => {
                        debug!("unexpected binary frame — ignored");
                    }
                    Some(Ok(_)) => {} // Pong / other frame variants — no action
                    Some(Err(e)) => {
                        // KuCoin periodically resets connections as part of
                        // normal server maintenance. A first-attempt drop is
                        // not worth alarming on — it almost always recovers
                        // immediately.  Only escalate to WARN once we've
                        // already retried at least once, signalling a
                        // persistent problem rather than a routine rotation.
                        if attempt == 0 {
                            debug!(error = %e, exchange = connector.exchange_name(), "WS read error");
                        } else {
                            warn!(error = %e, attempt, exchange = connector.exchange_name(), "WS read error");
                        }
                        return SessionOutcome::Disconnected;
                    }
                    None => {
                        let uptime_secs = subscribed_at.elapsed().as_secs();
                        if is_cascade_start(attempt, uptime_secs) {
                            // Stream ended without a close frame — usually a
                            // half-closed TCP connection. At attempt 0 with
                            // sub-5 s uptime this looks like a cascade too.
                            warn!(
                                uptime_secs,
                                attempt,
                                exchange = connector.exchange_name(),
                                "WS stream ended without close frame — likely cascade start"
                            );
                        } else {
                            debug!(
                                uptime_secs,
                                attempt,
                                exchange = connector.exchange_name(),
                                "WS stream closed"
                            );
                        }
                        return SessionOutcome::Disconnected;
                    }
                }
            }

            // ── Application-level ping ───────────────────────────────────────
            _ = ping_tick.tick() => {
                // Piggyback the idle-timeout check on the ping cadence
                // rather than running a separate timer.  KuCoin sends a
                // text pong on each of our pings, so on a healthy
                // connection `last_frame_at` is refreshed within one
                // ping interval and the check below never trips.
                if config.idle_timeout_secs > 0 {
                    let idle = last_frame_at.elapsed();
                    if idle >= Duration::from_secs(config.idle_timeout_secs) {
                        warn!(
                            idle_secs = idle.as_secs(),
                            limit_secs = config.idle_timeout_secs,
                            exchange = connector.exchange_name(),
                            "WS idle timeout — no frames received; dropping connection"
                        );
                        return SessionOutcome::Disconnected;
                    }
                }

                // Per-connector ping format. Returns None for exchanges
                // (e.g. Binance) that drive heartbeats from the server side
                // via protocol-level Ping/Pong frames — in that case the
                // tick still runs to drive the idle check above but we
                // don't send anything outbound.
                if let Some(ping) = connector.ping_message() {
                    guard.check().await;
                    if let Err(e) = write.send(Message::Text(ping.into())).await {
                        warn!(error = %e, "ping send failed");
                        return SessionOutcome::Disconnected;
                    }
                    debug!(exchange = connector.exchange_name(), "sent ping");
                }
            }
        }
    }
}

// ── Supervised feed (token-refresh) ───────────────────────────────────────────

/// Endpoint returned by a token-refresh callback for [`run_feed_supervised`].
///
/// Pairs a fresh WSS URL with the subscription messages that should be sent
/// on the new session. Both must be returned together because some exchanges
/// embed connection-scoped IDs in their subscription frames.
#[derive(Debug, Clone)]
pub struct WsFeedEndpoint {
    /// Full WSS URL including any token / connectId query parameters.
    pub url: String,
    /// Subscription messages to send immediately after the new connection
    /// is established.
    pub subscriptions: Vec<String>,
}

/// Tuning parameters for [`run_feed_supervised`].
///
/// The supervisor wraps [`run_feed`] in an outer loop that refreshes the WS
/// token when the inner runner exhausts its per-cycle reconnect budget. The
/// inner budget (`runner.max_reconnect_attempts`) should be set low enough
/// to detect a stale-token cascade quickly — typically 3 attempts — because
/// a fresh token usually restores the feed in one shot.
#[derive(Debug, Clone)]
pub struct SupervisedConfig {
    /// Inner-runner configuration.
    ///
    /// `runner.max_reconnect_attempts` is the **per-cycle** ceiling. When the
    /// inner [`run_feed`] returns [`ExchangeError::WsDisconnected`] after
    /// this many attempts, the supervisor calls the refresh closure and
    /// starts a new cycle. Defaults to 3 (down from the bare-runner default
    /// of 10) so cascades are detected in roughly 35 s rather than 9 min.
    pub runner: WsRunnerConfig,
    /// Maximum number of refresh cycles before giving up.
    ///
    /// Default: `u32::MAX` (effectively unlimited; rely on the caller's
    /// shutdown signal). Set this lower if you want the supervisor to
    /// surface a fatal `WsDisconnected` to the caller after a bounded number
    /// of refresh attempts.
    pub max_refresh_cycles: u32,
    /// Delay before invoking the refresh closure after a cycle exhausts.
    ///
    /// Default: 5 s. Prevents tight loops if the refresh endpoint is also
    /// failing, and gives the exchange a brief breather before the new
    /// session begins.
    pub refresh_delay_secs: u64,
}

impl Default for SupervisedConfig {
    fn default() -> Self {
        Self {
            runner: WsRunnerConfig {
                max_reconnect_attempts: 3,
                ..WsRunnerConfig::default()
            },
            max_refresh_cycles: u32::MAX,
            refresh_delay_secs: 5,
        }
    }
}

impl SupervisedConfig {
    /// Build a supervised config from an existing [`WsRunnerConfig`].
    ///
    /// Preserves all runner fields, then overrides `max_reconnect_attempts`
    /// to 3 if the caller left it at the bare-runner default — supervised
    /// callers almost always want the tighter per-cycle ceiling so a
    /// cascade triggers a token refresh quickly.
    pub fn from_runner(mut runner: WsRunnerConfig) -> Self {
        if runner.max_reconnect_attempts == WsRunnerConfig::default().max_reconnect_attempts {
            runner.max_reconnect_attempts = 3;
        }
        Self {
            runner,
            max_refresh_cycles: u32::MAX,
            refresh_delay_secs: 5,
        }
    }
}

/// Drive a WebSocket feed with automatic token re-negotiation on cascade failure.
///
/// Behaves like [`run_feed`], but when the inner runner exhausts its
/// reconnect budget the supervisor invokes `refresh` to obtain a fresh
/// [`WsFeedEndpoint`] and starts a new cycle. Use this when the suspected
/// disconnect cause is token invalidation — refreshing the token usually
/// restores the feed in seconds, whereas the bare runner would retry the
/// dead endpoint for `max_reconnect_attempts × max_reconnect_delay_secs`
/// (up to ~9 min with defaults).
///
/// # Arguments
/// - `connector` — Connector used to parse incoming frames. The connector is
///   re-used across cycles; only the URL and subscription messages change.
/// - `tx`        — Downstream channel for parsed messages.
/// - `config`    — Per-cycle reconnect budget and refresh settings.
/// - `shutdown`  — Send `true` to request a graceful close (honoured both
///   inside [`run_feed`] and while waiting for the next refresh).
/// - `refresh`   — Async closure that returns a fresh [`WsFeedEndpoint`].
///   Called once on entry to bootstrap the first session, then again after
///   every exhausted cycle.
///
/// # Returns
/// `Ok(())` on clean shutdown.
/// `Err(ExchangeError::WsDisconnected)` if `max_refresh_cycles` is reached.
/// `Err(_)` if the refresh closure itself fails (e.g. the REST endpoint is
/// unreachable) — the error is propagated unchanged.
///
/// # Example
///
/// ```no_run
/// use std::sync::Arc;
/// use tokio::sync::{mpsc, watch};
/// use exchange_apiws::{Credentials, KuCoin};
/// use exchange_apiws::actors::{DataMessage, ExchangeConnector};
/// use exchange_apiws::ws::{KucoinConnector, SupervisedConfig, WsFeedEndpoint, run_feed_supervised};
///
/// # async fn example() -> exchange_apiws::Result<()> {
/// let kucoin = KuCoin::futures(Credentials::from_env()?);
/// let client = Arc::new(kucoin.rest_client()?);
/// let env    = kucoin.env();
///
/// // The connector is fixed; only URL + subscriptions are refreshed.
/// let initial_token = client.get_ws_token_public().await?;
/// let connector = Arc::new(KucoinConnector::new(&initial_token, env)?);
///
/// // Closure called on bootstrap and after every cascade.
/// let refresh = {
///     let client = client.clone();
///     move || {
///         let client = client.clone();
///         async move {
///             let token = client.get_ws_token_public().await?;
///             let conn  = KucoinConnector::new(&token, env)?;
///             let subs  = vec![
///                 conn.trade_subscription("XBTUSDTM").unwrap(),
///                 conn.ticker_subscription("XBTUSDTM").unwrap(),
///             ];
///             Ok(WsFeedEndpoint { url: conn.ws_url().to_string(), subscriptions: subs })
///         }
///     }
/// };
///
/// let (tx, mut rx)               = mpsc::channel::<DataMessage>(1024);
/// let (shutdown_tx, shutdown_rx) = watch::channel(false);
///
/// tokio::spawn(run_feed_supervised(
///     connector,
///     tx,
///     SupervisedConfig::default(),
///     shutdown_rx,
///     refresh,
/// ));
///
/// while let Some(msg) = rx.recv().await { println!("{msg:?}"); }
/// let _ = shutdown_tx.send(true);
/// # Ok(())
/// # }
/// ```
pub async fn run_feed_supervised<F, Fut>(
    connector: Arc<dyn ExchangeConnector>,
    tx: mpsc::Sender<DataMessage>,
    config: SupervisedConfig,
    shutdown: watch::Receiver<bool>,
    refresh: F,
) -> Result<()>
where
    F: Fn() -> Fut + Send,
    Fut: Future<Output = Result<WsFeedEndpoint>> + Send,
{
    // Bootstrap — fetch the initial endpoint via the same closure used for
    // subsequent refreshes. Lets the caller route both paths through one
    // implementation rather than passing initial URL + subs separately.
    let WsFeedEndpoint {
        url: mut current_url,
        subscriptions: mut current_subs,
    } = refresh().await?;

    let mut cycle: u32 = 0;

    loop {
        let result = run_feed(
            current_url.clone(),
            current_subs.clone(),
            connector.clone(),
            tx.clone(),
            config.runner.clone(),
            shutdown.clone(),
        )
        .await;

        match result {
            Ok(()) => return Ok(()), // clean shutdown
            Err(ExchangeError::WsDisconnected { attempts, url }) => {
                cycle += 1;
                if cycle > config.max_refresh_cycles {
                    error!(
                        cycle,
                        max = config.max_refresh_cycles,
                        exchange = connector.exchange_name(),
                        "supervisor exhausted refresh budget"
                    );
                    config
                        .runner
                        .emit(RunnerEvent::RefreshExhausted { cycles: cycle });
                    return Err(ExchangeError::WsDisconnected { url, attempts });
                }

                // A shutdown that fired during the inner exhaustion path
                // shouldn't trigger a token refresh.
                if *shutdown.borrow() {
                    info!(
                        exchange = connector.exchange_name(),
                        "shutdown requested before token refresh — exiting"
                    );
                    return Ok(());
                }

                warn!(
                    cycle,
                    inner_attempts = attempts,
                    refresh_delay_secs = config.refresh_delay_secs,
                    exchange = connector.exchange_name(),
                    "WS cycle exhausted — refreshing token"
                );

                // Sleep with shutdown awareness so an in-flight shutdown
                // doesn't have to wait the full refresh_delay before the
                // supervisor returns.
                let mut shutdown_wait = shutdown.clone();
                tokio::select! {
                    biased;
                    Ok(()) = shutdown_wait.changed() => {
                        if *shutdown_wait.borrow() {
                            info!(
                                exchange = connector.exchange_name(),
                                "shutdown requested during refresh delay — exiting"
                            );
                            return Ok(());
                        }
                    }
                    () = tokio::time::sleep(Duration::from_secs(config.refresh_delay_secs)) => {}
                }

                config.runner.emit(RunnerEvent::TokenRefresh { cycle });
                match refresh().await {
                    Ok(endpoint) => {
                        info!(
                            cycle,
                            exchange = connector.exchange_name(),
                            "token refreshed — starting new feed cycle"
                        );
                        current_url = endpoint.url;
                        current_subs = endpoint.subscriptions;
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            cycle,
                            exchange = connector.exchange_name(),
                            "token refresh failed — surfacing error to caller"
                        );
                        return Err(e);
                    }
                }
            }
            Err(other) => return Err(other), // non-disconnect errors propagate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_cascade_start;

    #[test]
    fn cascade_start_fires_on_fresh_short_session() {
        // attempt 0, sub-threshold uptime → the canonical cascade signature.
        assert!(is_cascade_start(0, 0));
        assert!(is_cascade_start(0, 4));
    }

    #[test]
    fn cascade_start_not_for_normal_rotation() {
        // attempt 0 but the session was stable — likely a normal rotation
        // (token expiry, rolling restart). Should NOT be treated as cascade.
        assert!(!is_cascade_start(0, 5));
        assert!(!is_cascade_start(0, 60));
        assert!(!is_cascade_start(0, 86_400));
    }

    #[test]
    fn cascade_start_not_for_subsequent_attempts() {
        // attempts > 0 are already visible via the WARN "reconnecting" log
        // emitted by run_feed, so this only fires once per cascade.
        assert!(!is_cascade_start(1, 0));
        assert!(!is_cascade_start(5, 0));
        assert!(!is_cascade_start(10, 3));
    }
}
