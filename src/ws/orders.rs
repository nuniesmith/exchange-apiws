//! KuCoin WebSocket order placement.
//!
//! [`WsOrderClient`] opens a long-lived private WS connection and lets the
//! caller place / cancel orders with the latency profile of a WS frame
//! instead of a REST round-trip. Each request carries a `clientOid`; the
//! client routes the server's matching ack back to the awaiting future
//! via a `oneshot` channel.
//!
//! # Wire protocol
//!
//! The exact KuCoin wsapi schema has shifted over time and isn't as
//! exhaustively documented as the public-feed protocol. This module
//! implements the shape observed in current KuCoin docs:
//!
//! Request:
//! ```json
//! {
//!   "id":   "<request-uuid>",
//!   "type": "openOrder",
//!   "topic":"/contractMarket/order",
//!   "privateChannel": true,
//!   "response": true,
//!   "data": {
//!     "clientOid": "...",
//!     "side":      "buy",
//!     "symbol":    "XBTUSDTM",
//!     "type":      "limit",
//!     "size":      1,
//!     "leverage":  "10",
//!     "price":     "30000"
//!   }
//! }
//! ```
//!
//! Ack:
//! ```json
//! {
//!   "id":   "<request-uuid>",
//!   "type": "ack",
//!   "data": {"clientOid":"...","orderId":"..."}
//! }
//! ```
//!
//! Treat callers' use of this module as an opt-in fast path — if KuCoin's
//! wsapi schema diverges in production, the request-/response-shape
//! helpers ([`build_place_order_frame`], [`build_cancel_order_frame`])
//! can be overridden by the caller via [`WsOrderClient::send_raw`].
//!
//! # Rate limiting
//!
//! Outbound frames go through the same [`WsMsgGuard`](super::runner)
//! sliding-window limiter as the data-feed runner (100 msg / 10 s per
//! connection). The guard is per-connection, not per-account — each
//! `WsOrderClient` instance has its own.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{ExchangeError, Result};
use crate::types::{OrderType, Side};
use crate::ws::runner::WsMsgGuard;

/// Default deadline for a single order request/ack round-trip.
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 5;

// ── Public types ─────────────────────────────────────────────────────────────

/// Server response to a single order request, matched by `client_oid`.
///
/// `success = true` means KuCoin accepted the request (an `"ack"` or
/// `"received"` frame); `success = false` carries the failure code and
/// message KuCoin returned.
#[derive(Debug, Clone)]
pub struct WsOrderAck {
    /// Client-supplied correlation ID — matches the one sent in the request.
    pub client_oid: String,
    /// Exchange-assigned order ID when `success` is `true`.
    pub order_id: Option<String>,
    /// `true` for `ack` / `received` frames, `false` for `error`.
    pub success: bool,
    /// KuCoin error code (e.g. `"400100"`) when `success` is `false`.
    pub error_code: Option<String>,
    /// Human-readable error message when `success` is `false`.
    pub error_msg: Option<String>,
    /// The full raw response frame — useful for fields not modelled here.
    pub raw: Value,
}

// ── Client ───────────────────────────────────────────────────────────────────

/// Sender end of the internal outbound queue — every place/cancel turns into
/// a single text frame pushed here.
type OutboundQueue = mpsc::UnboundedSender<String>;

/// Map from `clientOid` to the oneshot the requester is awaiting on.
type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<WsOrderAck>>>>;

/// KuCoin WebSocket order client.
///
/// Cheap to clone — shares the connection / pending map / outbound queue
/// across handles. Drop the last clone (or call [`Self::close`]) to tear
/// the connection down.
#[derive(Clone)]
pub struct WsOrderClient {
    outbound: OutboundQueue,
    pending: Pending,
    closed: Arc<AtomicBool>,
    request_timeout: Duration,
}

impl WsOrderClient {
    /// Connect to a KuCoin wsapi endpoint.
    ///
    /// `ws_url` is the full WSS URL including any token query parameters
    /// (e.g. `wss://wsapi.kucoin.com/?token=…&connectId=…`). Typically the
    /// caller builds it from a [`crate::ws::WsToken`] returned by
    /// [`crate::KuCoinClient::get_ws_token_private`] in the same way
    /// [`crate::ws::KucoinConnector::new`] does.
    pub async fn connect(ws_url: impl Into<String>) -> Result<Self> {
        let url = ws_url.into();
        info!(url, "WS-order: connecting");

        let (ws, _resp) = connect_async(&url).await?;
        let (mut write, mut read) = ws.split();

        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));

        // Writer task — drains outbound_rx into the WS sink, with the
        // shared 100 msg/10s rate-limit guard between sends.
        let writer_closed = closed.clone();
        tokio::spawn(async move {
            let mut guard = WsMsgGuard::new();
            while let Some(frame) = outbound_rx.recv().await {
                if writer_closed.load(Ordering::SeqCst) {
                    break;
                }
                guard.check().await;
                if let Err(e) = write.send(Message::Text(frame.into())).await {
                    warn!(error = %e, "WS-order: send failed; tearing down");
                    writer_closed.store(true, Ordering::SeqCst);
                    break;
                }
            }
            let _ = write.send(Message::Close(None)).await;
        });

        // Reader task — routes inbound frames to the awaiting oneshot via
        // clientOid lookup. Welcomes / pongs / unrelated frames are dropped.
        let reader_pending = pending.clone();
        let reader_closed = closed.clone();
        tokio::spawn(async move {
            while let Some(frame) = read.next().await {
                if reader_closed.load(Ordering::SeqCst) {
                    break;
                }
                match frame {
                    Ok(Message::Text(text)) => {
                        if let Some(ack) = parse_inbound(&text) {
                            let mut map = reader_pending.lock().await;
                            if let Some(tx) = map.remove(&ack.client_oid) {
                                let _ = tx.send(ack);
                            } else {
                                debug!(
                                    client_oid = %ack.client_oid,
                                    "WS-order: unmatched ack; dropping"
                                );
                            }
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => {
                        reader_closed.store(true, Ordering::SeqCst);
                        break;
                    }
                    _ => {}
                }
            }
            // Fail any still-pending requests so callers don't block forever.
            let mut map = reader_pending.lock().await;
            for (_oid, tx) in map.drain() {
                let _ = tx.send(WsOrderAck {
                    client_oid: String::new(),
                    order_id: None,
                    success: false,
                    error_code: Some("connection_closed".into()),
                    error_msg: Some("WS-order connection closed".into()),
                    raw: Value::Null,
                });
            }
        });

        Ok(Self {
            outbound: outbound_tx,
            pending,
            closed,
            request_timeout: Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS),
        })
    }

    /// Override the default per-request timeout (5 s).
    #[must_use]
    pub const fn with_request_timeout(mut self, d: Duration) -> Self {
        self.request_timeout = d;
        self
    }

    /// Tear the connection down. Pending requests resolve with a
    /// `connection_closed` error so callers don't hang.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Returns `true` once the connection has been torn down (either via
    /// [`Self::close`] or because the reader/writer task observed an error).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    // ── KuCoin-specific request helpers ─────────────────────────────────────

    /// Place a futures order over the WS connection and await the ack.
    ///
    /// `client_oid` is generated automatically (UUID v4). Returns the
    /// `WsOrderAck` carrying the exchange-assigned `order_id` on success.
    #[allow(clippy::too_many_arguments, clippy::similar_names)]
    pub async fn place_order(
        &self,
        symbol: &str,
        side: Side,
        size: u32,
        leverage: u32,
        order_type: OrderType,
        price: Option<f64>,
    ) -> Result<WsOrderAck> {
        let client_oid = Uuid::new_v4().to_string();
        let frame =
            build_place_order_frame(&client_oid, symbol, side, size, leverage, order_type, price);
        self.send_and_await(&client_oid, frame).await
    }

    /// Cancel an order by ID over the WS connection and await the ack.
    pub async fn cancel_order(&self, order_id: &str) -> Result<WsOrderAck> {
        let client_oid = Uuid::new_v4().to_string();
        let frame = build_cancel_order_frame(&client_oid, order_id);
        self.send_and_await(&client_oid, frame).await
    }

    /// Lower-level escape hatch: send any text frame and route the response
    /// by `client_oid`. Use when the KuCoin wire format diverges from the
    /// shape this module assumes and you need to construct the JSON yourself.
    ///
    /// The caller must include `client_oid` in the request and the server
    /// must echo it in the response `data`.
    pub async fn send_raw(&self, client_oid: &str, frame: String) -> Result<WsOrderAck> {
        self.send_and_await(client_oid, frame).await
    }

    async fn send_and_await(&self, client_oid: &str, frame: String) -> Result<WsOrderAck> {
        if self.is_closed() {
            return Err(ExchangeError::Order("WS-order connection is closed".into()));
        }

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(client_oid.to_string(), tx);

        // Push the frame into the outbound queue. If the writer task has
        // already exited the send will fail.
        if self.outbound.send(frame).is_err() {
            self.pending.lock().await.remove(client_oid);
            return Err(ExchangeError::Order(
                "WS-order writer task is closed".into(),
            ));
        }

        match tokio::time::timeout(self.request_timeout, rx).await {
            Ok(Ok(ack)) => Ok(ack),
            Ok(Err(_)) => {
                // oneshot's Sender was dropped — almost always means the
                // reader task closed before delivering a response.
                self.pending.lock().await.remove(client_oid);
                Err(ExchangeError::Order(
                    "WS-order response channel closed before ack arrived".into(),
                ))
            }
            Err(_) => {
                // Timeout — drop the pending entry so a late ack doesn't
                // linger in the map.
                self.pending.lock().await.remove(client_oid);
                Err(ExchangeError::Order(format!(
                    "WS-order request timed out after {} s",
                    self.request_timeout.as_secs()
                )))
            }
        }
    }
}

// ── Frame builders ───────────────────────────────────────────────────────────

/// Build the JSON text frame for an order-placement request.
///
/// Public so callers using [`WsOrderClient::send_raw`] can reuse the
/// canonical shape; intended for reading more than for direct invocation.
// `side` and `size` are conventional names from KuCoin's API — renaming
// for the similar-names lint would hurt readability for KuCoin users.
#[allow(clippy::similar_names)]
#[must_use]
pub fn build_place_order_frame(
    client_oid: &str,
    symbol: &str,
    side: Side,
    size: u32,
    leverage: u32,
    order_type: OrderType,
    price: Option<f64>,
) -> String {
    let mut data = json!({
        "clientOid": client_oid,
        "side":      side.as_str(),
        "symbol":    symbol,
        "type":      order_type.as_str(),
        "size":      size,
        "leverage":  leverage.to_string(),
    });
    if let Some(p) = price {
        data["price"] = json!(p.to_string());
    }
    let frame = json!({
        "id":             Uuid::new_v4().to_string(),
        "type":           "openOrder",
        "topic":          "/contractMarket/order",
        "privateChannel": true,
        "response":       true,
        "data":           data,
    });
    frame.to_string()
}

/// Build the JSON text frame for an order-cancel request.
#[must_use]
pub fn build_cancel_order_frame(client_oid: &str, order_id: &str) -> String {
    let frame = json!({
        "id":             Uuid::new_v4().to_string(),
        "type":           "cancelOrder",
        "topic":          "/contractMarket/order",
        "privateChannel": true,
        "response":       true,
        "data": {
            "clientOid": client_oid,
            "orderId":   order_id,
        },
    });
    frame.to_string()
}

// ── Inbound parser ───────────────────────────────────────────────────────────

/// Parse an inbound WS frame into a [`WsOrderAck`] if it carries one.
///
/// Returns `None` for welcomes, pongs, or any frame without a matching
/// `data.clientOid` field.
fn parse_inbound(raw: &str) -> Option<WsOrderAck> {
    let json: Value = serde_json::from_str(raw).ok()?;
    let msg_type = json.get("type").and_then(Value::as_str).unwrap_or("");

    // Welcome / pong / acks-without-data slip through here.
    if matches!(msg_type, "welcome" | "pong" | "ping") {
        return None;
    }

    let data = json.get("data")?;
    // Order-related frames always have a clientOid inside `data`.
    let client_oid = data.get("clientOid").and_then(Value::as_str)?.to_string();
    let order_id = data
        .get("orderId")
        .and_then(Value::as_str)
        .map(str::to_string);

    // Explicit list of success-shaped types is kept alongside the default
    // arm for documentation — clippy warns about the duplicate body, allow it.
    #[allow(clippy::match_same_arms)]
    let (success, error_code, error_msg) = match msg_type {
        "ack" | "received" | "order" => (true, None, None),
        "error" => {
            let code = json
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| data.get("code").and_then(Value::as_str).map(str::to_string));
            let msg = data
                .get("msg")
                .and_then(Value::as_str)
                .or_else(|| json.get("msg").and_then(Value::as_str))
                .or_else(|| data.as_str())
                .map(str::to_string);
            (false, code, msg)
        }
        // Treat any other type as a success — KuCoin sometimes emits
        // "openOrderAck" or similar; if it has a clientOid we route it.
        _ => (true, None, None),
    };

    Some(WsOrderAck {
        client_oid,
        order_id,
        success,
        error_code,
        error_msg,
        raw: json,
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_order_frame_includes_required_fields_limit() {
        let frame = build_place_order_frame(
            "oid-1",
            "XBTUSDTM",
            Side::Buy,
            1,
            10,
            OrderType::Limit,
            Some(30_000.0),
        );
        let v: Value = serde_json::from_str(&frame).expect("frame is JSON");
        assert_eq!(v["type"], "openOrder");
        assert_eq!(v["topic"], "/contractMarket/order");
        assert_eq!(v["data"]["clientOid"], "oid-1");
        assert_eq!(v["data"]["side"], "buy");
        assert_eq!(v["data"]["type"], "limit");
        assert_eq!(v["data"]["symbol"], "XBTUSDTM");
        assert_eq!(v["data"]["size"], 1);
        assert_eq!(v["data"]["leverage"], "10");
        // Prices go on the wire as strings to preserve KuCoin's decimal precision.
        assert_eq!(v["data"]["price"], "30000");
    }

    #[test]
    fn place_order_frame_market_has_no_price() {
        let frame = build_place_order_frame(
            "oid-1",
            "XBTUSDTM",
            Side::Sell,
            1,
            10,
            OrderType::Market,
            None,
        );
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert!(
            v["data"].get("price").is_none(),
            "market orders must not include a price"
        );
        assert_eq!(v["data"]["type"], "market");
    }

    #[test]
    fn cancel_order_frame_carries_order_id_and_client_oid() {
        let frame = build_cancel_order_frame("oid-1", "order-xyz");
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["type"], "cancelOrder");
        assert_eq!(v["data"]["clientOid"], "oid-1");
        assert_eq!(v["data"]["orderId"], "order-xyz");
    }

    #[test]
    fn parse_ack_frame_returns_success() {
        let raw = r#"{"id":"req-1","type":"ack","data":{"clientOid":"oid-1","orderId":"order-7"}}"#;
        let ack = parse_inbound(raw).expect("ack should parse");
        assert_eq!(ack.client_oid, "oid-1");
        assert_eq!(ack.order_id.as_deref(), Some("order-7"));
        assert!(ack.success);
        assert!(ack.error_code.is_none());
    }

    #[test]
    fn parse_error_frame_returns_failure_with_code() {
        let raw = r#"{"id":"req-1","type":"error","code":"400100","data":{"clientOid":"oid-1","msg":"bad size"}}"#;
        let ack = parse_inbound(raw).expect("error should parse");
        assert!(!ack.success);
        assert_eq!(ack.error_code.as_deref(), Some("400100"));
        assert_eq!(ack.error_msg.as_deref(), Some("bad size"));
        assert_eq!(ack.client_oid, "oid-1");
    }

    #[test]
    fn parse_welcome_returns_none() {
        let raw = r#"{"id":"server-1","type":"welcome"}"#;
        assert!(parse_inbound(raw).is_none());
    }

    #[test]
    fn parse_pong_returns_none() {
        let raw = r#"{"id":"client-1","type":"pong"}"#;
        assert!(parse_inbound(raw).is_none());
    }

    #[test]
    fn parse_frame_without_client_oid_returns_none() {
        // A subscribe-ack without clientOid in the data shouldn't route.
        let raw = r#"{"id":"req-2","type":"ack","data":{"topic":"/x/y"}}"#;
        assert!(parse_inbound(raw).is_none());
    }
}
