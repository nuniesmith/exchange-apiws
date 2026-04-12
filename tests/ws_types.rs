//! WebSocket frame parse tests — exercises `WsMessage` serialisation and
//! deserialisation without requiring a live connection.
//!
//! Run with:
//! ```text
//! cargo test --test ws_types
//! ```

use exchange_apiws::ws::types::WsMessage;

// ── Ping round-trip ───────────────────────────────────────────────────────────

#[test]
fn ping_frame_serialises_and_round_trips() {
    let ping = WsMessage::ping();

    assert_eq!(ping.msg_type, "ping");
    assert!(!ping.id.is_empty(), "ping must carry a client-generated id");
    assert!(ping.topic.is_none());
    assert!(ping.data.is_none());
    assert!(ping.subject.is_none());
    assert!(ping.private_channel.is_none());
    assert!(ping.response.is_none());

    // Serialise → deserialise round-trip.
    let json = serde_json::to_string(&ping).expect("serialise failed");
    let decoded: WsMessage = serde_json::from_str(&json).expect("deserialise failed");

    assert_eq!(decoded.msg_type, "ping");
    assert_eq!(decoded.id, ping.id, "id must survive the round-trip");
    assert!(decoded.topic.is_none());
    assert!(decoded.data.is_none());
}

#[test]
fn ping_frame_omits_null_optional_fields() {
    // KuCoin's WS protocol is chatty about unknown fields so we must not send
    // null-valued optional keys — only absent ones.
    let json = serde_json::to_string(&WsMessage::ping()).expect("serialise failed");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let obj = v.as_object().unwrap();

    assert!(!obj.contains_key("topic"),         "topic must be omitted");
    assert!(!obj.contains_key("subject"),        "subject must be omitted");
    assert!(!obj.contains_key("data"),           "data must be omitted");
    assert!(!obj.contains_key("privateChannel"), "privateChannel must be omitted");
    assert!(!obj.contains_key("response"),       "response must be omitted");
}

// ── Market-data push ──────────────────────────────────────────────────────────

#[test]
fn ticker_push_deserialises_correctly() {
    let raw = r#"{
        "type":    "message",
        "topic":   "/contractMarket/ticker:XBTUSDTM",
        "subject": "tickerV2",
        "data": {
            "price":        "86000.5",
            "bestBidSize":  10,
            "bestAskSize":  5,
            "ts":           1713000000000
        }
    }"#;

    let msg: WsMessage = serde_json::from_str(raw).expect("deserialise failed");

    assert_eq!(msg.msg_type, "message");
    assert_eq!(
        msg.topic.as_deref(),
        Some("/contractMarket/ticker:XBTUSDTM")
    );
    assert_eq!(msg.subject.as_deref(), Some("tickerV2"));
    assert!(msg.data.is_some(), "data payload must be present");

    let data = msg.data.unwrap();
    assert_eq!(data["price"], "86000.5");
    assert_eq!(data["bestBidSize"], 10);
}

#[test]
fn message_without_id_defaults_to_empty_string() {
    // Server-sent data pushes omit the `id` field; the `#[serde(default)]`
    // annotation on WsMessage must coerce that to an empty string.
    let raw = r#"{ "type": "message", "topic": "/some/topic", "data": {} }"#;
    let msg: WsMessage = serde_json::from_str(raw).expect("deserialise failed");
    assert_eq!(msg.id, "", "missing id should default to empty string");
}

// ── Server control frames ─────────────────────────────────────────────────────

#[test]
fn welcome_frame_deserialises() {
    let raw = r#"{ "id": "welcome-xyz", "type": "welcome" }"#;
    let msg: WsMessage = serde_json::from_str(raw).expect("deserialise failed");
    assert_eq!(msg.msg_type, "welcome");
    assert_eq!(msg.id, "welcome-xyz");
}

#[test]
fn ack_frame_deserialises() {
    let raw = r#"{ "id": "sub-001", "type": "ack" }"#;
    let msg: WsMessage = serde_json::from_str(raw).expect("deserialise failed");
    assert_eq!(msg.msg_type, "ack");
    assert_eq!(msg.id, "sub-001");
}

// ── Subscribe frame construction ──────────────────────────────────────────────

#[test]
fn subscribe_frame_serialises_with_correct_shape() {
    let sub = WsMessage {
        id:              "sub-ticker-001".into(),
        msg_type:        "subscribe".into(),
        topic:           Some("/contractMarket/ticker:XBTUSDTM".into()),
        subject:         None,
        data:            None,
        private_channel: Some(false),
        response:        Some(true),
    };

    let json = serde_json::to_string(&sub).expect("serialise failed");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(v["type"],           "subscribe");
    assert_eq!(v["id"],             "sub-ticker-001");
    assert_eq!(v["topic"],          "/contractMarket/ticker:XBTUSDTM");
    assert_eq!(v["privateChannel"], false);
    assert_eq!(v["response"],       true);
    // Optional fields that are None must be absent — not null.
    assert!(!v.as_object().unwrap().contains_key("subject"));
    assert!(!v.as_object().unwrap().contains_key("data"));
}
