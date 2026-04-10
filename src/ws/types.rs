//! WebSocket types — Server negotiation, message envelopes, and payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Negotiation Types (REST) ──────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstanceServer {
    pub endpoint: String,
    pub encrypt: bool,
    pub protocol: String,
    pub ping_interval: u64,
    pub ping_timeout: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WsToken {
    pub token: String,
    #[serde(rename = "instanceServers")]
    pub instance_servers: Vec<InstanceServer>,
}

// ── WebSocket Protocol Envelopes ──────────────────────────────────────────────

/// The standard KuCoin WebSocket envelope.
#[derive(Debug, Serialize, Deserialize)]
pub struct WsMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "privateChannel")]
    pub private_channel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<bool>,
}

impl WsMessage {
    /// Generates KuCoin's required application-level ping.
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
}
