# exchange-apiws

[![Crates.io](https://img.shields.io/crates/v/exchange-apiws.svg)](https://crates.io/crates/exchange-apiws)
[![Docs.rs](https://docs.rs/exchange-apiws/badge.svg)](https://docs.rs/exchange-apiws)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Async Rust client for exchange REST APIs and WebSocket feeds.

**KuCoin** (Futures + Spot) is fully implemented. The crate is architected to be exchange-agnostic — adding a new exchange means implementing one trait and the shared runner handles the rest.

---

## Features

### KuCoin — REST

| Method | Endpoint |
|--------|----------|
| `get_balance(currency)` | `GET /api/v1/account-overview` |
| `get_account_overview(currency)` | `GET /api/v1/account-overview` |
| `get_account_overview_all()` | `GET /api/v2/account-overview-all` |
| `get_position(symbol)` | `GET /api/v1/position` |
| `get_all_positions()` | `GET /api/v1/positions` |
| `add_position_margin(symbol, margin, direction)` | `POST /api/v1/position/changeMargin` |
| `set_auto_deposit(symbol, bool)` | `POST /api/v1/position/changeAutoDeposit` |
| `set_risk_limit_level(symbol, level)` | `POST /api/v1/position/risk-limit-level/change` |
| `get_risk_limit_levels(symbol)` | `GET /api/v1/contracts/risk-limit/{symbol}` |
| `fetch_klines(symbol, limit, granularity)` | `GET /api/v1/kline/query` |
| `fetch_klines_extended(...)` | paginated kline fetch |
| `get_orderbook_snapshot(symbol)` | `GET /api/v1/level2/snapshot` |
| `get_funding_rate(symbol)` | `GET /api/v1/funding-rate/{symbol}/current` |
| `get_funding_history(symbol, max_count)` | `GET /api/v1/funding-history` |
| `get_mark_price(symbol)` | `GET /api/v1/mark-price/{symbol}/current` |
| `get_active_contracts()` | `GET /api/v1/contracts/active` |
| `get_contract(symbol)` | `GET /api/v1/contracts/{symbol}` |
| `get_ticker(symbol)` | `GET /api/v1/ticker` |
| `place_order(...)` | `POST /api/v1/orders` |
| `close_position(symbol, qty, leverage)` | `POST /api/v1/orders` |
| `cancel_order(order_id)` | `DELETE /api/v1/orders/{id}` |
| `cancel_all_orders(symbol)` | `DELETE /api/v1/orders?symbol=…` |
| `get_open_orders(symbol)` | `GET /api/v1/orders?status=active` |
| `get_done_orders(symbol, max_count)` | `GET /api/v1/orders?status=done` |
| `get_order(order_id)` | `GET /api/v1/orders/{id}` |
| `get_recent_fills(symbol)` | `GET /api/v1/recentFills` |
| `place_stop_order(...)` | `POST /api/v1/stopOrders` |
| `cancel_stop_order(order_id)` | `DELETE /api/v1/stopOrders/{id}` |
| `cancel_all_stop_orders(symbol)` | `DELETE /api/v1/stopOrders?symbol=…` |
| `get_open_stop_orders(symbol)` | `GET /api/v1/stopOrders?status=active` |
| `transfer_to_main(currency, amount)` | `POST /api/v1/transfer-out` |
| `transfer_to_futures(currency, amount)` | `POST /api/v1/transfer-in` |
| `get_transfer_list(currency, transfer_type, max_count)` | `GET /api/v1/transfer-list` |

### KuCoin — WebSocket

| Subscription helper | Topic | Feed type |
|---------------------|-------|-----------|
| `trade_subscription(symbol)` | `/contractMarket/execution:{sym}` | `DataMessage::Trade` |
| `ticker_subscription(symbol)` | `/contractMarket/tickerV2:{sym}` | `DataMessage::Ticker` |
| `orderbook_depth_subscription(symbol, depth)` | `/contractMarket/level2Depth{5\|50}:{sym}` | `DataMessage::OrderBook` (snapshot) |
| `orderbook_l2_subscription(symbol)` | `/contractMarket/level2:{sym}` | `DataMessage::OrderBook` (delta) |
| `order_updates_subscription()` ⚑ | `/contractMarket/tradeOrders` | `DataMessage::OrderUpdate` |
| `position_subscription(symbol)` ⚑ | `/contract/position:{sym}` | `DataMessage::PositionChange` |
| `balance_subscription()` ⚑ | `/contractAccount/wallet` | `DataMessage::BalanceUpdate` |

⚑ Requires a **private** WS token — call `client.get_ws_token_private()`.

---

## Installation

```toml
[dependencies]
exchange-apiws = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

Set your credentials as environment variables:

```
KC_KEY=your_api_key
KC_SECRET=your_api_secret
KC_PASSPHRASE=your_passphrase
```

---

## Quick start

### REST

```rust
use exchange_apiws::{Credentials, KuCoin};

#[tokio::main]
async fn main() -> exchange_apiws::Result<()> {
    let client = KuCoin::futures(Credentials::from_env()?).rest_client()?;

    let balance  = client.get_balance("USDT").await?;
    let position = client.get_position("XBTUSDTM").await?;
    let candles  = client.fetch_klines("XBTUSDTM", 200, "1").await?;
    let funding  = client.get_funding_rate("XBTUSDTM").await?;

    println!("balance={balance:.2}  qty={}  funding={:.6}", position.current_qty, funding.value);
    Ok(())
}
```

### Public WebSocket feed

```rust
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use exchange_apiws::{Credentials, KuCoin, actors::DataMessage};
use exchange_apiws::ws::{KucoinConnector, WsRunnerConfig, run_feed};

#[tokio::main]
async fn main() -> exchange_apiws::Result<()> {
    let kucoin = KuCoin::futures(Credentials::from_env()?);
    let client = kucoin.rest_client()?;
    let token  = client.get_ws_token_public().await?;
    let conn   = Arc::new(KucoinConnector::new(&token, kucoin.env())?);

    let subs = vec![
        conn.trade_subscription("XBTUSDTM").unwrap(),
        conn.ticker_subscription("XBTUSDTM").unwrap(),
    ];

    let (tx, mut rx)               = mpsc::channel::<DataMessage>(1024);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(run_feed(
        conn.ws_url().to_string(),
        subs,
        conn,
        tx,
        WsRunnerConfig::default(),
        shutdown_rx,
    ));

    while let Some(msg) = rx.recv().await {
        println!("{msg:?}");
    }
    let _ = shutdown_tx.send(true);
    Ok(())
}
```

### Private WebSocket feed (order fills + positions)

```rust
// Use get_ws_token_private() and add private subscriptions:
let kucoin = KuCoin::futures(Credentials::from_env()?);
let client = kucoin.rest_client()?;
let token  = client.get_ws_token_private().await?;
let conn   = Arc::new(KucoinConnector::new(&token, kucoin.env())?);

let subs = vec![
    conn.order_updates_subscription().unwrap(),   // fills & status changes
    conn.position_subscription("XBTUSDTM").unwrap(),
    conn.balance_subscription().unwrap(),
];
// ... same run_feed setup as above
```

### Contract sizing

`calc_contracts` is an async method on `KuCoinClient` — it calls `GET /api/v1/contracts/{symbol}` to retrieve the contract multiplier at runtime, so it returns a `Result`.

```rust
let client    = KuCoin::futures(Credentials::from_env()?).rest_client()?;
let contracts = client.calc_contracts(
    "XBTUSDTM",
    96_000.0,   // current price
    1_000.0,    // available balance (USDT)
    10,         // leverage
    0.02,       // risk 2% of balance
    50,         // max contracts cap
).await?;
println!("{contracts} contracts");
```

---

## Placing orders

```rust
use exchange_apiws::types::{Side, OrderType, TimeInForce, STP};

// Market order
client.place_order(
    "XBTUSDTM", Side::Buy, 5, 10,
    OrderType::Market, None, false, None,
).await?;

// Limit order with IOC + STP
client.place_order(
    "XBTUSDTM", Side::Sell, 3, 10,
    OrderType::Limit,
    Some(TimeInForce::IOC),
    false,
    Some(STP::CN),
).await?;

// Stop-market order (close on breach)
client.place_stop_order(
    "XBTUSDTM", Side::Sell, 5, 10,
    93_000.0, "down", None, true,
).await?;
```

---

## Rate limits

### REST

KuCoin enforces per-UID rate limits per resource pool. VIP0 Futures quota is 2,000 requests / 30 seconds. The client automatically:

- Retries transient failures with exponential backoff (3 attempts, 1.5× factor)
- Reads the `gw-ratelimit-reset` header on HTTP 429 and sleeps for the exact reset window
- Returns `ExchangeError::Api` with the KuCoin error code on non-200000 responses

### WebSocket

KuCoin allows **100 client→server messages per 10 seconds per connection** (subscribe, unsubscribe, ping). The runner enforces this with a sliding-window guard before every outbound send — subscriptions sent at startup are rate-limited too, so large subscription batches at connect time will be transparently throttled.

---

## Error handling

All fallible functions return `Result<T>` where the error type is `ExchangeError`:

```rust
use exchange_apiws::ExchangeError;

match client.get_position("XBTUSDTM").await {
    Ok(pos)  => println!("{}", pos.current_qty),
    Err(ExchangeError::Api { code, message }) => eprintln!("KuCoin error {code}: {message}"),
    Err(ExchangeError::WsDisconnected)        => eprintln!("WS gave up after max reconnects"),
    Err(e)                                    => eprintln!("other error: {e}"),
}
```

---

## Authentication

KuCoin API v2 HMAC-SHA256 signing is implemented in `auth::build_headers`. The prehash string is `{timestamp}{METHOD}{endpoint}{body}`. The passphrase is itself HMAC-signed (not sent raw), which is the v2 requirement.

Credentials are loaded from environment variables with `Credentials::from_env()`:

| Variable | Description |
|----------|-------------|
| `KC_KEY` | API key |
| `KC_SECRET` | API secret |
| `KC_PASSPHRASE` | API passphrase |

---

## KuCoin-specific notes

**Leverage** is a per-order field in KuCoin Futures, not an account setting. Pass `leverage` in `place_order` and `close_position`. Use `set_risk_limit_level` to change the max position size tier.

**Inverse vs. linear contracts** — `calc_contracts` fetches the contract multiplier live via `get_contract`. Inverse (USD-margined) contracts like `XBTUSDM` have a multiplier of 1 USD. Linear (USDT-margined) contracts like `XBTUSDTM` express a base-coin multiplier (0.001 BTC per contract).

**Private WS token expiry** — WS tokens are valid for the lifetime of the connection. The runner reconnects automatically; call `get_ws_token_private()` again inside the reconnect flow if you need long-lived private feeds.

---

## Roadmap

- [ ] Binance Futures REST + WS
- [ ] OKX REST + WS
- [ ] Bybit REST + WS
- [ ] KuCoin Unified Trade Account (UTA) endpoints
- [ ] KuCoin spot margin orders
- [ ] WebSocket order placement (`wsapi.kucoin.com`)

---

## License

MIT — see [LICENSE](LICENSE).
