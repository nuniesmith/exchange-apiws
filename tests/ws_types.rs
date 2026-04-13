//! WebSocket type and connector tests.
//!
//! Covers three layers:
//!
//! 1. **`WsMessage` serde** — serialisation, deserialisation, field omission,
//!    round-trips for every frame type the protocol uses.
//! 2. **`WsToken` / `InstanceServer` serde** — negotiation types round-trip
//!    and the connector URL is assembled correctly.
//! 3. **`KucoinConnector::parse_message`** — every public and private topic
//!    is routed to the correct `DataMessage` variant with the right fields;
//!    control frames and unknown topics return empty vecs.
//!
//! Run with:
//! ```text
//! cargo test --test ws_types
//! ```

use exchange_apiws::{
    KucoinEnv,
    actors::{DataMessage, ExchangeConnector, TradeSide},
    ws::{
        KucoinConnector,
        types::{InstanceServer, WsMessage, WsToken},
    },
};

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Build a `KucoinConnector` with fake negotiation data so parse_message
/// and subscription builders can be exercised without a live connection.
fn make_connector(env: KucoinEnv) -> KucoinConnector {
    let token = WsToken {
        token: "test-token-abc".to_string(),
        instance_servers: vec![InstanceServer {
            endpoint: "wss://push1-v2.kucoin.com/endpoint".to_string(),
            encrypt: true,
            protocol: "websocket".to_string(),
            ping_interval: 18_000,
            ping_timeout: 10_000,
        }],
    };
    KucoinConnector::new(&token, env).expect("connector build failed")
}

fn futures_connector() -> KucoinConnector {
    make_connector(KucoinEnv::LiveFutures)
}

fn spot_connector() -> KucoinConnector {
    make_connector(KucoinEnv::LiveSpot)
}

// ── WsMessage: ping ───────────────────────────────────────────────────────────

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

    let json = serde_json::to_string(&ping).expect("serialise failed");
    let decoded: WsMessage = serde_json::from_str(&json).expect("deserialise failed");

    assert_eq!(decoded.msg_type, "ping");
    assert_eq!(decoded.id, ping.id, "id must survive the round-trip");
    assert!(decoded.topic.is_none());
    assert!(decoded.data.is_none());
}

#[test]
fn ping_frame_omits_null_optional_fields() {
    let json = serde_json::to_string(&WsMessage::ping()).expect("serialise failed");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let obj = v.as_object().unwrap();

    assert!(!obj.contains_key("topic"), "topic must be omitted");
    assert!(!obj.contains_key("subject"), "subject must be omitted");
    assert!(!obj.contains_key("data"), "data must be omitted");
    assert!(
        !obj.contains_key("privateChannel"),
        "privateChannel must be omitted"
    );
    assert!(!obj.contains_key("response"), "response must be omitted");
}

#[test]
fn ping_json_is_valid_parseable_json_with_correct_type() {
    let raw = WsMessage::ping_json();
    let v: serde_json::Value = serde_json::from_str(raw).expect("ping_json() must be valid JSON");
    assert_eq!(v["type"], "ping", "ping_json type field must be 'ping'");
}

#[test]
fn two_pings_carry_distinct_ids() {
    let a = WsMessage::ping();
    let b = WsMessage::ping();
    assert_ne!(a.id, b.id, "each ping must have a unique id");
}

// ── WsMessage: server control frames ─────────────────────────────────────────

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

#[test]
fn pong_frame_deserialises() {
    let raw = r#"{ "id": "pong-42", "type": "pong" }"#;
    let msg: WsMessage = serde_json::from_str(raw).expect("deserialise failed");
    assert_eq!(msg.msg_type, "pong");
    assert_eq!(msg.id, "pong-42");
}

#[test]
fn error_frame_deserialises_with_data_payload() {
    // KuCoin sends { "type": "error", "code": 401, "data": "token expired" }
    // for auth failures. The envelope should survive deserialisation intact.
    let raw = r#"{ "id": "", "type": "error", "data": "token expired" }"#;
    let msg: WsMessage = serde_json::from_str(raw).expect("deserialise failed");
    assert_eq!(msg.msg_type, "error");
    assert!(msg.data.is_some(), "error data payload must be present");
}

#[test]
fn message_without_id_defaults_to_empty_string() {
    let raw = r#"{ "type": "message", "topic": "/some/topic", "data": {} }"#;
    let msg: WsMessage = serde_json::from_str(raw).expect("deserialise failed");
    assert_eq!(msg.id, "", "missing id should default to empty string");
}

// ── WsMessage: market-data push ───────────────────────────────────────────────

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

// ── WsMessage: subscribe frame construction ───────────────────────────────────

#[test]
fn subscribe_frame_serialises_with_correct_shape() {
    let sub = WsMessage {
        id: "sub-ticker-001".into(),
        msg_type: "subscribe".into(),
        topic: Some("/contractMarket/ticker:XBTUSDTM".into()),
        subject: None,
        data: None,
        private_channel: Some(false),
        response: Some(true),
    };

    let json = serde_json::to_string(&sub).expect("serialise failed");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(v["type"], "subscribe");
    assert_eq!(v["id"], "sub-ticker-001");
    assert_eq!(v["topic"], "/contractMarket/ticker:XBTUSDTM");
    assert_eq!(v["privateChannel"], false);
    assert_eq!(v["response"], true);
    assert!(!v.as_object().unwrap().contains_key("subject"));
    assert!(!v.as_object().unwrap().contains_key("data"));
}

#[test]
fn private_subscribe_frame_sets_private_channel_true() {
    let sub = WsMessage {
        id: "priv-sub-001".into(),
        msg_type: "subscribe".into(),
        topic: Some("/contractMarket/tradeOrders".into()),
        subject: None,
        data: None,
        private_channel: Some(true),
        response: Some(true),
    };
    let json = serde_json::to_string(&sub).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["privateChannel"], true);
}

// ── WsToken / InstanceServer ──────────────────────────────────────────────────

#[test]
fn instance_server_deserialises_correctly() {
    let raw = r#"{
        "endpoint":     "wss://push1-v2.kucoin.com/endpoint",
        "encrypt":      true,
        "protocol":     "websocket",
        "pingInterval": 18000,
        "pingTimeout":  10000
    }"#;

    let server: InstanceServer = serde_json::from_str(raw).expect("deserialise failed");

    assert_eq!(server.endpoint, "wss://push1-v2.kucoin.com/endpoint");
    assert!(server.encrypt);
    assert_eq!(server.protocol, "websocket");
    assert_eq!(server.ping_interval, 18_000);
    assert_eq!(server.ping_timeout, 10_000);
}

#[test]
fn ws_token_deserialises_and_selects_first_server() {
    let raw = r#"{
        "token": "abc123token",
        "instanceServers": [
            {
                "endpoint":     "wss://server1.kucoin.com/endpoint",
                "encrypt":      true,
                "protocol":     "websocket",
                "pingInterval": 18000,
                "pingTimeout":  10000
            },
            {
                "endpoint":     "wss://server2.kucoin.com/endpoint",
                "encrypt":      true,
                "protocol":     "websocket",
                "pingInterval": 18000,
                "pingTimeout":  10000
            }
        ]
    }"#;

    let token: WsToken = serde_json::from_str(raw).expect("deserialise failed");

    assert_eq!(token.token, "abc123token");
    assert_eq!(token.instance_servers.len(), 2);
    assert_eq!(
        token.instance_servers[0].endpoint,
        "wss://server1.kucoin.com/endpoint"
    );
}

// ── KucoinConnector construction ──────────────────────────────────────────────

#[test]
fn connector_builds_negotiated_url_with_token_and_connect_id() {
    let conn = futures_connector();
    let url = conn.ws_url();

    assert!(
        url.starts_with("wss://push1-v2.kucoin.com/endpoint?token=test-token-abc&connectId="),
        "unexpected URL: {url}"
    );
}

#[test]
fn connector_ping_interval_converts_ms_to_secs() {
    // instance server has pingInterval=18_000 ms → should be 18 s
    let conn = futures_connector();
    assert_eq!(conn.ping_interval_secs, 18);
}

#[test]
fn connector_errors_when_no_instance_servers_provided() {
    let empty_token = WsToken {
        token: "t".to_string(),
        instance_servers: vec![],
    };
    let result = KucoinConnector::new(&empty_token, KucoinEnv::LiveFutures);
    assert!(result.is_err(), "should fail with no instance servers");
}

// ── Subscription builders ─────────────────────────────────────────────────────

fn parse_sub(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("subscription must be valid JSON")
}

#[test]
fn trade_subscription_futures_uses_execution_topic() {
    let conn = futures_connector();
    let json = conn
        .trade_subscription("XBTUSDTM")
        .expect("should return Some");
    let v = parse_sub(&json);
    assert_eq!(v["type"], "subscribe");
    assert!(
        v["topic"]
            .as_str()
            .unwrap()
            .contains("/contractMarket/execution:XBTUSDTM")
    );
    assert_eq!(v["privateChannel"], false);
    assert_eq!(v["response"], true);
}

#[test]
fn trade_subscription_spot_uses_match_topic() {
    let conn = spot_connector();
    let json = conn
        .trade_subscription("BTC-USDT")
        .expect("should return Some");
    let v = parse_sub(&json);
    assert!(
        v["topic"]
            .as_str()
            .unwrap()
            .contains("/market/match:BTC-USDT")
    );
}

#[test]
fn ticker_subscription_futures_uses_tickerv2_topic() {
    let conn = futures_connector();
    let json = conn
        .ticker_subscription("ETHUSDTM")
        .expect("should return Some");
    let v = parse_sub(&json);
    assert!(
        v["topic"]
            .as_str()
            .unwrap()
            .contains("/contractMarket/tickerV2:ETHUSDTM")
    );
    assert_eq!(v["privateChannel"], false);
}

#[test]
fn ticker_subscription_spot_uses_market_ticker_topic() {
    let conn = spot_connector();
    let json = conn
        .ticker_subscription("ETH-USDT")
        .expect("should return Some");
    let v = parse_sub(&json);
    assert!(
        v["topic"]
            .as_str()
            .unwrap()
            .contains("/market/ticker:ETH-USDT")
    );
}

#[test]
fn orderbook_depth_subscription_clamps_to_5_or_50() {
    let conn = futures_connector();

    let sub5 = conn
        .orderbook_depth_subscription("XBTUSDTM", 3)
        .expect("Some");
    let sub50 = conn
        .orderbook_depth_subscription("XBTUSDTM", 20)
        .expect("Some");

    let v5 = parse_sub(&sub5);
    let v50 = parse_sub(&sub50);

    assert!(
        v5["topic"]
            .as_str()
            .unwrap()
            .contains("level2Depth5:XBTUSDTM"),
        "depth ≤ 5 should clamp to 5"
    );
    assert!(
        v50["topic"]
            .as_str()
            .unwrap()
            .contains("level2Depth50:XBTUSDTM"),
        "depth > 5 should clamp to 50"
    );
}

#[test]
fn orderbook_l2_subscription_uses_level2_topic() {
    let conn = futures_connector();
    let json = conn.orderbook_l2_subscription("SOLUSDTM").expect("Some");
    let v = parse_sub(&json);
    assert!(
        v["topic"]
            .as_str()
            .unwrap()
            .contains("/contractMarket/level2:SOLUSDTM")
    );
    assert_eq!(v["privateChannel"], false);
}

#[test]
fn order_updates_subscription_is_private() {
    let conn = futures_connector();
    let json = conn.order_updates_subscription().expect("Some");
    let v = parse_sub(&json);
    assert!(
        v["topic"]
            .as_str()
            .unwrap()
            .contains("/contractMarket/tradeOrders")
    );
    assert_eq!(v["privateChannel"], true);
}

#[test]
fn position_subscription_is_private_with_symbol() {
    let conn = futures_connector();
    let json = conn.position_subscription("XBTUSDTM").expect("Some");
    let v = parse_sub(&json);
    assert!(
        v["topic"]
            .as_str()
            .unwrap()
            .contains("/contract/position:XBTUSDTM")
    );
    assert_eq!(v["privateChannel"], true);
}

#[test]
fn balance_subscription_is_private() {
    let conn = futures_connector();
    let json = conn.balance_subscription().expect("Some");
    let v = parse_sub(&json);
    assert!(
        v["topic"]
            .as_str()
            .unwrap()
            .contains("/contractAccount/wallet")
    );
    assert_eq!(v["privateChannel"], true);
}

#[test]
fn instrument_subscription_is_public() {
    let conn = futures_connector();
    let json = conn.instrument_subscription("XBTUSDTM").expect("Some");
    let v = parse_sub(&json);
    assert!(
        v["topic"]
            .as_str()
            .unwrap()
            .contains("/contract/instrument:XBTUSDTM")
    );
    assert_eq!(v["privateChannel"], false);
}

#[test]
fn stop_orders_subscription_is_private() {
    let conn = futures_connector();
    let json = conn.stop_orders_subscription().expect("Some");
    let v = parse_sub(&json);
    assert!(
        v["topic"]
            .as_str()
            .unwrap()
            .contains("/contractMarket/advancedOrders")
    );
    assert_eq!(v["privateChannel"], true);
}

#[test]
fn each_subscription_carries_a_unique_id() {
    let conn = futures_connector();
    let a = conn.ticker_subscription("XBTUSDTM").unwrap();
    let b = conn.ticker_subscription("XBTUSDTM").unwrap();
    let va: serde_json::Value = serde_json::from_str(&a).unwrap();
    let vb: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_ne!(
        va["id"], vb["id"],
        "each subscription must carry a unique id"
    );
}

// ── parse_message: control frames → empty vec ─────────────────────────────────

#[test]
fn parse_welcome_returns_empty_vec() {
    let conn = futures_connector();
    let msgs = conn
        .parse_message(r#"{"id":"w1","type":"welcome"}"#)
        .expect("parse failed");
    assert!(msgs.is_empty());
}

#[test]
fn parse_ack_returns_empty_vec() {
    let conn = futures_connector();
    let msgs = conn
        .parse_message(r#"{"id":"s1","type":"ack"}"#)
        .expect("parse failed");
    assert!(msgs.is_empty());
}

#[test]
fn parse_pong_returns_empty_vec() {
    let conn = futures_connector();
    let msgs = conn
        .parse_message(r#"{"id":"p1","type":"pong"}"#)
        .expect("parse failed");
    assert!(msgs.is_empty());
}

#[test]
fn parse_unknown_topic_returns_empty_vec() {
    let conn = futures_connector();
    let raw = r#"{"type":"message","topic":"/unknown/feed:XYZ","data":{"val":1}}"#;
    let msgs = conn.parse_message(raw).expect("parse failed");
    assert!(msgs.is_empty());
}

#[test]
fn parse_message_missing_data_field_returns_empty_vec() {
    let conn = futures_connector();
    // Legitimate "message" type but data is absent — should not error, just skip.
    let raw = r#"{"type":"message","topic":"/contractMarket/tickerV2:XBTUSDTM"}"#;
    let msgs = conn.parse_message(raw).expect("parse failed");
    assert!(msgs.is_empty());
}

#[test]
fn parse_malformed_json_returns_error() {
    let conn = futures_connector();
    let result = conn.parse_message("{ not valid json");
    assert!(result.is_err(), "malformed JSON should return Err");
}

// ── parse_message: public data topics ────────────────────────────────────────

#[test]
fn parse_futures_trade_execution() {
    let conn = futures_connector();
    // KuCoin futures execution push — `ts` is nanoseconds.
    let raw = r#"{
        "type":    "message",
        "topic":   "/contractMarket/execution:XBTUSDTM",
        "subject": "match",
        "data": {
            "symbol":       "XBTUSDTM",
            "side":         "sell",
            "price":        "71050.5",
            "size":         "2",
            "tradeId":      "trade-99",
            "ts":           1713000000000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    assert_eq!(msgs.len(), 1);

    let DataMessage::Trade(trade) = &msgs[0] else {
        panic!("expected DataMessage::Trade, got {:?}", msgs[0]);
    };

    assert_eq!(trade.symbol, "XBTUSDTM");
    assert_eq!(trade.side, TradeSide::Sell);
    assert!(
        (trade.price - 71050.5).abs() < 1e-6,
        "price mismatch: {}",
        trade.price
    );
    assert!(
        (trade.amount - 2.0).abs() < 1e-9,
        "amount mismatch: {}",
        trade.amount
    );
    assert_eq!(trade.trade_id, "trade-99");
    assert!(trade.exchange_ts > 0);
    assert!(trade.receipt_ts > 0);
}

#[test]
fn parse_trade_buy_side() {
    let conn = futures_connector();
    let raw = r#"{
        "type":  "message",
        "topic": "/contractMarket/execution:ETHUSDTM",
        "data":  { "side": "buy", "price": "2200.0", "size": "5", "ts": 1713000000000000000 }
    }"#;
    let msgs = conn.parse_message(raw).expect("parse failed");
    let DataMessage::Trade(trade) = &msgs[0] else {
        panic!()
    };
    assert_eq!(trade.side, TradeSide::Buy);
}

#[test]
fn parse_futures_ticker_v2() {
    let conn = futures_connector();
    let raw = r#"{
        "type":    "message",
        "topic":   "/contractMarket/tickerV2:XBTUSDTM",
        "subject": "tickerV2",
        "data": {
            "bestBidPrice": "71000.0",
            "bestAskPrice": "71001.0",
            "price":        "71000.5",
            "ts":           1713000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    assert_eq!(msgs.len(), 1);

    let DataMessage::Ticker(ticker) = &msgs[0] else {
        panic!("expected DataMessage::Ticker, got {:?}", msgs[0]);
    };

    assert_eq!(ticker.symbol, "XBTUSDTM");
    assert!(
        (ticker.best_bid - 71000.0).abs() < 1e-6,
        "best_bid: {}",
        ticker.best_bid
    );
    assert!(
        (ticker.best_ask - 71001.0).abs() < 1e-6,
        "best_ask: {}",
        ticker.best_ask
    );
    assert!(
        (ticker.price - 71000.5).abs() < 1e-6,
        "price: {}",
        ticker.price
    );
    assert!(ticker.exchange_ts > 0);
}

#[test]
fn parse_spot_ticker() {
    // Spot uses /market/ticker and bestBid / bestAsk (not bestBidPrice / bestAskPrice).
    let conn = spot_connector();
    let raw = r#"{
        "type":    "message",
        "topic":   "/market/ticker:BTC-USDT",
        "subject": "trade.ticker",
        "data": {
            "bestBid": "70999.0",
            "bestAsk": "71000.0",
            "price":   "70999.5",
            "time":    1713000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    assert_eq!(msgs.len(), 1);
    let DataMessage::Ticker(ticker) = &msgs[0] else {
        panic!()
    };
    assert_eq!(ticker.symbol, "BTC-USDT");
    assert!((ticker.best_bid - 70999.0).abs() < 1e-6);
}

#[test]
fn parse_orderbook_depth5_snapshot() {
    let conn = futures_connector();
    let raw = r#"{
        "type":    "message",
        "topic":   "/contractMarket/level2Depth5:XBTUSDTM",
        "subject": "level2",
        "data": {
            "asks": [["71001.0","3"],["71002.0","1"]],
            "bids": [["71000.0","5"],["70999.0","2"]],
            "ts":   1713000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    assert_eq!(msgs.len(), 1);

    let DataMessage::OrderBook(ob) = &msgs[0] else {
        panic!("expected DataMessage::OrderBook, got {:?}", msgs[0]);
    };

    assert_eq!(ob.symbol, "XBTUSDTM");
    assert!(ob.is_snapshot, "depth5 must be a snapshot");
    assert_eq!(ob.asks.len(), 2);
    assert_eq!(ob.bids.len(), 2);
    assert!((ob.asks[0][0] - 71001.0).abs() < 1e-6);
    assert!((ob.bids[0][0] - 71000.0).abs() < 1e-6);
}

#[test]
fn parse_level2_delta_sell_side() {
    let conn = futures_connector();
    // KuCoin level2 incremental: change = "price,side,qty"
    let raw = r#"{
        "type":    "message",
        "topic":   "/contractMarket/level2:XBTUSDTM",
        "subject": "level2",
        "data": {
            "change":    "71005.0,sell,10",
            "timestamp": 1713000000001
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    assert_eq!(msgs.len(), 1);

    let DataMessage::OrderBook(ob) = &msgs[0] else {
        panic!()
    };
    assert!(!ob.is_snapshot, "level2 incremental must not be a snapshot");
    assert_eq!(ob.asks.len(), 1);
    assert!(ob.bids.is_empty());
    assert!((ob.asks[0][0] - 71005.0).abs() < 1e-6);
    assert!((ob.asks[0][1] - 10.0).abs() < 1e-9);
}

#[test]
fn parse_level2_delta_buy_side() {
    let conn = futures_connector();
    let raw = r#"{
        "type":  "message",
        "topic": "/contractMarket/level2:XBTUSDTM",
        "data":  { "change": "70998.0,buy,5", "timestamp": 1713000000002 }
    }"#;
    let msgs = conn.parse_message(raw).expect("parse failed");
    let DataMessage::OrderBook(ob) = &msgs[0] else {
        panic!()
    };
    assert!(ob.asks.is_empty());
    assert_eq!(ob.bids.len(), 1);
    assert!((ob.bids[0][0] - 70998.0).abs() < 1e-6);
}

#[test]
fn parse_level2_delta_qty_zero_signals_removal() {
    let conn = futures_connector();
    let raw = r#"{
        "type":  "message",
        "topic": "/contractMarket/level2:XBTUSDTM",
        "data":  { "change": "71005.0,sell,0", "timestamp": 1713000000003 }
    }"#;
    let msgs = conn.parse_message(raw).expect("parse failed");
    let DataMessage::OrderBook(ob) = &msgs[0] else {
        panic!()
    };
    // qty == 0 signals removal — the parser should still return the entry
    // so the caller can clear that price level from its local book.
    assert_eq!(ob.asks[0][1], 0.0, "removal entry must have qty=0");
}

#[test]
fn parse_instrument_event_mark_price() {
    let conn = futures_connector();
    let raw = r#"{
        "type":    "message",
        "topic":   "/contract/instrument:XBTUSDTM",
        "subject": "mark.index.price",
        "data": {
            "markPrice":  71050.0,
            "indexPrice": 71040.0,
            "timestamp":  1713000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    assert_eq!(msgs.len(), 1);

    let DataMessage::InstrumentEvent(ev) = &msgs[0] else {
        panic!("expected DataMessage::InstrumentEvent, got {:?}", msgs[0]);
    };

    assert_eq!(ev.symbol, "XBTUSDTM");
    assert_eq!(ev.subject, "mark.index.price");
    assert!(ev.mark_price.is_some());
    assert!((ev.mark_price.unwrap() - 71050.0).abs() < 1e-6);
    assert!(ev.index_price.is_some());
    assert!((ev.index_price.unwrap() - 71040.0).abs() < 1e-6);
    assert!(ev.funding_rate.is_none());
}

#[test]
fn parse_instrument_event_funding_rate() {
    let conn = futures_connector();
    let raw = r#"{
        "type":    "message",
        "topic":   "/contract/instrument:XBTUSDTM",
        "subject": "funding.rate",
        "data": {
            "fundingRate":    0.0001,
            "predictedValue": 0.00008,
            "timestamp":      1713000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    let DataMessage::InstrumentEvent(ev) = &msgs[0] else {
        panic!()
    };
    assert_eq!(ev.subject, "funding.rate");
    assert!(ev.funding_rate.is_some());
    assert!(ev.predicted_funding_rate.is_some());
    assert!(ev.mark_price.is_none());
}

// ── parse_message: private data topics ────────────────────────────────────────

#[test]
fn parse_order_update_fill() {
    let conn = futures_connector();
    let raw = r#"{
        "type":    "message",
        "topic":   "/contractMarket/tradeOrders",
        "subject": "orderChange",
        "data": {
            "symbol":     "XBTUSDTM",
            "orderId":    "order-fill-001",
            "clientOid":  "my-oid-1",
            "side":       "buy",
            "type":       "market",
            "status":     "filled",
            "price":      "0",
            "size":       "10",
            "filledSize": "10",
            "remainSize": "0",
            "fee":        "1.5",
            "ts":         1713000000000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    assert_eq!(msgs.len(), 1);

    let DataMessage::OrderUpdate(ou) = &msgs[0] else {
        panic!("expected DataMessage::OrderUpdate, got {:?}", msgs[0]);
    };

    assert_eq!(ou.order_id, "order-fill-001");
    assert_eq!(ou.client_oid.as_deref(), Some("my-oid-1"));
    assert_eq!(ou.side, TradeSide::Buy);
    assert_eq!(ou.status, "filled");
    assert_eq!(ou.size, 10);
    assert_eq!(ou.filled_size, 10);
    assert_eq!(ou.remaining_size, 0);
    assert!((ou.fee - 1.5).abs() < 1e-9);
}

#[test]
fn parse_order_update_partial_fill() {
    let conn = futures_connector();
    let raw = r#"{
        "type":  "message",
        "topic": "/contractMarket/tradeOrders",
        "data":  {
            "symbol":     "ETHUSDTM",
            "orderId":    "order-partial-002",
            "side":       "sell",
            "type":       "limit",
            "status":     "partialFilled",
            "price":      "2200.0",
            "size":       "20",
            "filledSize": "5",
            "remainSize": "15",
            "fee":        "0.5",
            "ts":         1713000000000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    let DataMessage::OrderUpdate(ou) = &msgs[0] else {
        panic!()
    };
    assert_eq!(ou.status, "partialFilled");
    assert_eq!(ou.filled_size, 5);
    assert_eq!(ou.remaining_size, 15);
    assert_eq!(ou.side, TradeSide::Sell);
}

#[test]
fn parse_position_change() {
    let conn = futures_connector();
    let raw = r#"{
        "type":    "message",
        "topic":   "/contract/position:XBTUSDTM",
        "subject": "position.change",
        "data": {
            "currentQty":       30,
            "avgEntryPrice":    71000.0,
            "unrealisedPnl":    150.0,
            "realisedPnl":      0.0,
            "changeReason":     "positionChange",
            "currentTimestamp": 1713000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    assert_eq!(msgs.len(), 1);

    let DataMessage::PositionChange(pc) = &msgs[0] else {
        panic!("expected DataMessage::PositionChange, got {:?}", msgs[0]);
    };

    assert_eq!(pc.symbol, "XBTUSDTM");
    assert_eq!(pc.current_qty, 30);
    assert!((pc.avg_entry_price - 71000.0).abs() < 1e-6);
    assert!((pc.unrealised_pnl - 150.0).abs() < 1e-9);
    assert_eq!(pc.change_reason, "positionChange");
}

#[test]
fn parse_position_change_flat() {
    let conn = futures_connector();
    let raw = r#"{
        "type":  "message",
        "topic": "/contract/position:SOLUSDTM",
        "data":  {
            "currentQty":       0,
            "avgEntryPrice":    0.0,
            "unrealisedPnl":    0.0,
            "realisedPnl":      25.4,
            "changeReason":     "closePosition",
            "currentTimestamp": 1713000010000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    let DataMessage::PositionChange(pc) = &msgs[0] else {
        panic!()
    };
    assert_eq!(pc.current_qty, 0);
    assert_eq!(pc.change_reason, "closePosition");
}

#[test]
fn parse_balance_update() {
    let conn = futures_connector();
    let raw = r#"{
        "type":    "message",
        "topic":   "/contractAccount/wallet",
        "subject": "availableBalance.change",
        "data": {
            "currency":         "USDT",
            "availableBalance": 9850.25,
            "holdBalance":      149.75,
            "event":            "orderMargin.create",
            "timestamp":        1713000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    assert_eq!(msgs.len(), 1);

    let DataMessage::BalanceUpdate(bu) = &msgs[0] else {
        panic!("expected DataMessage::BalanceUpdate, got {:?}", msgs[0]);
    };

    assert_eq!(bu.currency, "USDT");
    assert!((bu.available_balance - 9850.25).abs() < 1e-6);
    assert!((bu.hold_balance - 149.75).abs() < 1e-6);
    assert_eq!(bu.event, "orderMargin.create");
}

#[test]
fn parse_advanced_order_update_triggered() {
    let conn = futures_connector();
    let raw = r#"{
        "type":    "message",
        "topic":   "/contractMarket/advancedOrders",
        "subject": "stopOrder",
        "data": {
            "symbol":    "XBTUSDTM",
            "orderId":   "stop-order-001",
            "clientOid": "my-stop-1",
            "side":      "sell",
            "type":      "market",
            "status":    "triggered",
            "stop":      "down",
            "stopPrice": "70000.0",
            "size":      10,
            "ts":        1713000000000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    assert_eq!(msgs.len(), 1);

    let DataMessage::AdvancedOrderUpdate(aou) = &msgs[0] else {
        panic!(
            "expected DataMessage::AdvancedOrderUpdate, got {:?}",
            msgs[0]
        );
    };

    assert_eq!(aou.order_id, "stop-order-001");
    assert_eq!(aou.status, "triggered");
    assert_eq!(aou.side, TradeSide::Sell);
    assert_eq!(aou.stop.as_deref(), Some("down"));
    assert!(aou.stop_price.is_some());
    assert_eq!(aou.size, 10);
}

#[test]
fn parse_advanced_order_update_cancelled() {
    let conn = futures_connector();
    let raw = r#"{
        "type":  "message",
        "topic": "/contractMarket/advancedOrders",
        "data":  {
            "symbol":    "ETHUSDTM",
            "orderId":   "stop-order-002",
            "side":      "buy",
            "type":      "market",
            "status":    "cancel",
            "size":      5,
            "ts":        1713000000000000000
        }
    }"#;

    let msgs = conn.parse_message(raw).expect("parse failed");
    let DataMessage::AdvancedOrderUpdate(aou) = &msgs[0] else {
        panic!()
    };
    assert_eq!(aou.status, "cancel");
}
