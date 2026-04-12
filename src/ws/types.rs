//! WebSocket types — Server negotiation, message envelopes, and payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Negotiation Types (REST) ──────────────────────────────────────────────────

/// A KuCoin WebSocket instance server returned by the bullet endpoint.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstanceServer {
    /// Full WSS endpoint URL for this server.
    pub endpoint: String,
    /// Whether TLS encryption is required (always `true` for KuCoin).
    pub encrypt: bool,
    /// Transport protocol — always `"websocket"`.
    pub protocol: String,
    /// Recommended application-level ping interval in milliseconds.
    pub ping_interval: u64,
    /// Server-side ping timeout in milliseconds.
    pub ping_timeout: u64,
}

/// Token and server list returned by the bullet negotiation endpoints.
#[derive(Debug, Deserialize, Clone)]
pub struct WsToken {
    /// Authentication token to include as `?token=…` in the WSS URL.
    pub token: String,
    #[serde(rename = "instanceServers")]
    /// Available WebSocket servers; connect to the first one.
    pub instance_servers: Vec<InstanceServer>,
}

// ── WebSocket Protocol Envelopes ──────────────────────────────────────────────

/// The standard KuCoin WebSocket envelope.
#[derive(Debug, Serialize, Deserialize)]
pub struct WsMessage {
    /// Client-generated unique ID for request/response correlation.
    /// Absent on market-data pushes; only present on control frames
    /// (subscribe, ping, pong, welcome, ack).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(rename = "type")]
    /// Message type — e.g. `"subscribe"`, `"message"`, `"ping"`, `"welcome"`.
    pub msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Topic string identifying the data channel (e.g. `/contractMarket/ticker:XBTUSDTM`).
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Sub-topic or event name within the topic.
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Payload object for data messages.
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "privateChannel")]
    /// `true` when subscribing to a private (authenticated) channel.
    pub private_channel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// `true` to request an acknowledgement frame from the server.
    pub response: Option<bool>,
}

impl WsMessage {
    /// Generates KuCoin's required application-level ping as a struct.
    ///
    /// The `id` field is populated with a fresh UUID for request/response
    /// correlation. Prefer [`WsMessage::ping_json`] on the hot send path to
    /// avoid repeated allocations when the id is not used.
    pub fn ping() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            msg_type: "ping".to_string(),
            topic: None,
            subject: None,
            data: None,
            private_channel: None,
            response: None,
        }
    }

    /// Returns a pre-computed JSON string for the KuCoin application-level ping.
    ///
    /// KuCoin pings do not require request/response correlation — the server
    /// responds with `{"type":"pong"}` regardless of the `id` field. This
    /// avoids a UUID allocation and a `serde_json::to_string` call on every
    /// ping tick.
    #[inline]
    pub const fn ping_json() -> &'static str {
        r#"{"type":"ping"}"#
    }
}
