# exchange-apiws

[![Crates.io](https://img.shields.io/crates/v/exchange-apiws.svg)](https://crates.io/crates/exchange-apiws)
[![Docs.rs](https://docs.rs/exchange-apiws/badge.svg)](https://docs.rs/exchange-apiws)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Async Rust client for exchange REST APIs and WebSocket feeds.

**KuCoin** (Futures + Spot) is fully implemented. The crate is architected to be exchange-agnostic — adding a new exchange means implementing one trait and the shared runner handles the rest.

---

## Table of Contents

- [Features](#features)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Placing Orders](#placing-orders)
- [Rate Limits](#rate-limits)
- [Error Handling](#error-handling)
- [Authentication](#authentication)
- [KuCoin-Specific Notes](#kucoin-specific-notes)
- [Roadmap](#roadmap)
- [License](#license)

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

## Quick Start

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

## Placing Orders

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

## Rate Limits

### REST

KuCoin enforces per-UID rate limits per resource pool. VIP0 Futures quota is 2,000 requests / 30 seconds. The client automatically:

- Retries transient failures with exponential backoff (3 attempts, 1.5× factor)
- Reads the `gw-ratelimit-reset` header on HTTP 429 and sleeps for the exact reset window
- Returns `ExchangeError::Api` with the KuCoin error code on non-200000 responses

### WebSocket

KuCoin allows **100 client→server messages per 10 seconds per connection** (subscribe, unsubscribe, ping). The runner enforces this with a sliding-window guard before every outbound send — subscriptions sent at startup are rate-limited too, so large subscription batches at connect time will be transparently throttled.

---

## Error Handling

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

## KuCoin-Specific Notes

**Leverage** is a per-order field in KuCoin Futures, not an account setting. Pass `leverage` in `place_order` and `close_position`. Use `set_risk_limit_level` to change the max position size tier.

**Inverse vs. linear contracts** — `calc_contracts` fetches the contract multiplier live via `get_contract`. Inverse (USD-margined) contracts like `XBTUSDM` have a multiplier of 1 USD. Linear (USDT-margined) contracts like `XBTUSDTM` express a base-coin multiplier (0.001 BTC per contract).

**Private WS token expiry** — WS tokens are valid for the lifetime of the connection. The runner reconnects automatically; call `get_ws_token_private()` again inside the reconnect flow if you need long-lived private feeds.

---

## Roadmap

> **Exchange coverage note:** public REST and WebSocket endpoints for all exchanges are freely accessible without API keys. Authenticated endpoints are planned for Kraken and Crypto.com (spot only).

### Architecture prerequisites

These foundational pieces unlock everything below.

#### `PublicRestClient` (`src/http.rs`)

`KuCoinClient` calls `build_headers` on every request and cannot make unauthenticated calls. A new `PublicRestClient` is needed for Binance, Bybit, and the public endpoints of all other exchanges.

```
src/http.rs   (new)
  PublicRestClient
    - reqwest::Client (rustls, 10s timeout)
    - base_url: String
    - get<T: DeserializeOwned>(path, params) -> Result<T>
    - same retry / 429-backoff logic as KuCoinClient
    - no envelope unwrapping — caller decides shape
```

Authenticated exchanges (Kraken, Crypto.com) will wrap this client and add their own signing layer, rather than sharing KuCoinClient's KuCoin-specific HMAC path.

#### Envelope trait

Each exchange wraps responses differently:

| Exchange | Envelope shape |
|----------|---------------|
| KuCoin | `{"code":"200000","data":{…}}` |
| Binance | bare JSON — no wrapper |
| Bybit | `{"retCode":0,"result":{…}}` |
| Kraken | `{"result":{…},"error":[]}` |
| Crypto.com | `{"code":0,"result":{…}}` |

A small `Envelope` trait (or free function) per exchange module will unwrap each format and surface errors as `ExchangeError::Api`.

#### `DataMessage` additions

New feed types that don't map to existing variants:

| Variant | Used by |
|---------|---------|
| `Candle(CandleData)` | Binance kline stream, Bybit kline, Kraken OHLC, Crypto.com candlestick |
| `FundingRate(FundingData)` | Binance mark-price stream, Bybit ticker extended |

`CandleData` fields: `symbol`, `exchange`, `interval`, `open_ts`, `open`, `high`, `low`, `close`, `volume`, `is_closed`, `receipt_ts`.

---

### KuCoin — remaining work

#### Unified Trade Account (UTA) REST endpoints

KuCoin Unified combines Spot + Futures margin in one account.
Base URL: `https://api.kucoin.com`.

| Method | Endpoint |
|--------|----------|
| `get_unified_account()` | `GET /api/v3/account/summary` |
| `get_unified_margin()` | `GET /api/v3/margin/accounts` |
| `get_cross_margin_symbols()` | `GET /api/v1/isolated/accounts` |

`KucoinEnv::Unified` routing already exists in the enum — needs REST methods in `src/rest/account.rs` and wiremock coverage in `rest_mock.rs`.

#### Spot margin orders (`src/rest/margin.rs`)

| Method | Endpoint |
|--------|----------|
| `place_margin_order(symbol, side, size, price, leverage)` | `POST /api/v1/margin/order` |
| `get_margin_order(order_id)` | `GET /api/v1/margin/orders/{id}` |
| `cancel_margin_order(order_id)` | `DELETE /api/v1/margin/orders/{id}` |
| `get_margin_fills(symbol)` | `GET /api/v1/margin/fills` |
| `get_margin_balance(currency)` | `GET /api/v1/margin/account` |

#### WebSocket order placement (`src/ws/orders.rs`)

KuCoin's `wsapi.kucoin.com` supports placing and cancelling orders over WS for ultra-low latency. Requires a private WS token and a separate connection to `wss://wsapi.kucoin.com`.

- `WsOrderClient` wrapping a tungstenite sink
- `place_order_ws(symbol, side, size, leverage, order_type, price)` → sends JSON frame, awaits ack with matching `clientOid`
- `cancel_order_ws(order_id)` → same pattern
- Response matching by `clientOid` in a `HashMap<String, oneshot::Sender>`
- Rate limit: 100 msg/10s (shared with the existing runner guard)

---

### Binance — public only

All endpoints below are unauthenticated.

#### REST (`src/binance/rest.rs`)

Uses `PublicRestClient` pointed at:
- Spot: `https://api.binance.com`
- Futures (USDT-M): `https://fapi.binance.com`

| Method | Endpoint |
|--------|----------|
| `get_klines(symbol, interval, limit)` | `GET /api/v3/klines` |
| `get_orderbook(symbol, limit)` | `GET /api/v3/depth` |
| `get_recent_trades(symbol, limit)` | `GET /api/v3/trades` |
| `get_ticker(symbol)` | `GET /api/v3/ticker/bookTicker` |
| `get_ticker_24h(symbol)` | `GET /api/v3/ticker/24hr` |
| `get_exchange_info()` | `GET /api/v3/exchangeInfo` |
| `get_futures_klines(symbol, interval, limit)` | `GET /fapi/v1/klines` |
| `get_futures_funding_rate(symbol)` | `GET /fapi/v1/fundingRate` |
| `get_futures_mark_price(symbol)` | `GET /fapi/v1/premiumIndex` |
| `get_futures_open_interest(symbol)` | `GET /fapi/v1/openInterest` |

Responses are bare JSON arrays/objects with no envelope wrapper.

#### WebSocket (`src/binance/ws.rs`)

Implements `ExchangeConnector`. Binance WS uses URL-encoded stream names rather than post-connect subscription messages.

Base URLs:
- Spot: `wss://stream.binance.com:9443/ws/<streamName>`
- Spot combined: `wss://stream.binance.com:9443/stream?streams=<a>/<b>`
- Futures: `wss://fstream.binance.com/ws/<streamName>`

| Subscription helper | Stream name | DataMessage |
|---------------------|-------------|-------------|
| `trade_subscription(symbol)` | `<symbol>@aggTrade` | `Trade` |
| `ticker_subscription(symbol)` | `<symbol>@bookTicker` | `Ticker` |
| `kline_subscription(symbol, interval)` | `<symbol>@kline_<interval>` | `Candle` |
| `depth_subscription(symbol)` | `<symbol>@depth@100ms` | `OrderBook` (delta) |
| `depth_snapshot_subscription(symbol, levels)` | `<symbol>@depth{5\|10\|20}@100ms` | `OrderBook` (snapshot) |
| `mark_price_subscription(symbol)` *(futures)* | `<symbol>@markPrice@1s` | `FundingRate` |

Ping: Binance sends a ping frame — runner responds with pong. No application-level ping needed.

---

### Bybit — public only

All endpoints below are unauthenticated.

#### REST (`src/bybit/rest.rs`)

Uses `PublicRestClient` pointed at `https://api.bybit.com`.

| Method | Endpoint |
|--------|----------|
| `get_klines(category, symbol, interval, limit)` | `GET /v5/market/kline` |
| `get_orderbook(category, symbol, limit)` | `GET /v5/market/orderbook` |
| `get_tickers(category, symbol)` | `GET /v5/market/tickers` |
| `get_recent_trades(category, symbol, limit)` | `GET /v5/market/recent-trade` |
| `get_instruments(category)` | `GET /v5/market/instruments-info` |
| `get_funding_rate(symbol)` | `GET /v5/market/funding/history` |
| `get_open_interest(symbol, interval)` | `GET /v5/market/open-interest` |
| `get_long_short_ratio(symbol, period)` | `GET /v5/market/account-ratio` |

`category` values: `"spot"`, `"linear"` (USDT perp), `"inverse"`.
Envelope: `{"retCode":0,"result":{…}}` — non-zero `retCode` surfaces as `ExchangeError::Api`.

#### WebSocket (`src/bybit/ws.rs`)

Implements `ExchangeConnector`.

Base URLs:
- Spot public: `wss://stream.bybit.com/v5/public/spot`
- Linear public: `wss://stream.bybit.com/v5/public/linear`
- Inverse public: `wss://stream.bybit.com/v5/public/inverse`

Subscription message format (sent after connect):
```json
{"op":"subscribe","args":["orderbook.50.BTCUSDT"]}
```

Ping: send `{"op":"ping"}` every 20 s; server responds `{"op":"pong"}`.

| Subscription helper | Topic arg | DataMessage |
|---------------------|-----------|-------------|
| `trade_subscription(symbol)` | `publicTrade.<symbol>` | `Trade` |
| `ticker_subscription(symbol)` | `tickers.<symbol>` | `Ticker` |
| `kline_subscription(symbol, interval)` | `kline.<interval>.<symbol>` | `Candle` |
| `orderbook_subscription(symbol, depth)` | `orderbook.<depth>.<symbol>` | `OrderBook` |

Note: the first `orderbook.*` message is a snapshot (`type:"snapshot"`); subsequent messages are deltas (`type:"delta"`). Set `is_snapshot` accordingly in `parse_message`.

---

### Kraken — spot, authenticated

Signing: HMAC-SHA512 over `URI + SHA256(nonce + encoded_body)`, base64-encoded, sent as `API-Sign` alongside `API-Key`. Add `src/kraken/auth.rs` (separate from the KuCoin-specific `src/auth.rs`).

#### Public REST (`src/kraken/rest.rs`)

Base URL: `https://api.kraken.com`.

| Method | Endpoint |
|--------|----------|
| `get_assets()` | `GET /0/public/Assets` |
| `get_asset_pairs(pair)` | `GET /0/public/AssetPairs` |
| `get_ticker(pair)` | `GET /0/public/Ticker` |
| `get_ohlc(pair, interval)` | `GET /0/public/OHLC` |
| `get_orderbook(pair, count)` | `GET /0/public/Depth` |
| `get_recent_trades(pair)` | `GET /0/public/Trades` |
| `get_spread(pair)` | `GET /0/public/Spread` |
| `get_system_status()` | `GET /0/public/SystemStatus` |

Envelope: `{"result":{…},"error":[]}` — non-empty `error` array surfaces as `ExchangeError::Api`.

#### Private REST (authenticated)

| Method | Endpoint |
|--------|----------|
| `get_balance()` | `POST /0/private/Balance` |
| `get_open_orders()` | `POST /0/private/OpenOrders` |
| `get_closed_orders()` | `POST /0/private/ClosedOrders` |
| `place_order(pair, side, order_type, volume, price)` | `POST /0/private/AddOrder` |
| `cancel_order(txid)` | `POST /0/private/CancelOrder` |
| `cancel_all_orders()` | `POST /0/private/CancelAll` |
| `get_trades_history()` | `POST /0/private/TradesHistory` |
| `get_ledger(asset)` | `POST /0/private/Ledgers` |
| `withdraw(asset, key, amount)` | `POST /0/private/Withdraw` |
| `get_withdrawal_status(asset)` | `POST /0/private/WithdrawStatus` |

#### WebSocket (`src/kraken/ws.rs`)

Implements `ExchangeConnector`.

Base URLs:
- Public: `wss://ws.kraken.com/v2`
- Private: `wss://ws-auth.kraken.com/v2`

Ping: send `{"method":"ping"}` every 30 s.

Subscribe message format:
```json
{"method":"subscribe","params":{"channel":"ticker","symbol":["BTC/USD"]}}
```

| Subscription helper | Channel | DataMessage |
|---------------------|---------|-------------|
| `trade_subscription(pair)` | `trade` | `Trade` |
| `ticker_subscription(pair)` | `ticker` | `Ticker` |
| `ohlc_subscription(pair, interval)` | `ohlc` | `Candle` |
| `orderbook_subscription(pair, depth)` | `book` | `OrderBook` |
| `order_updates_subscription()` ⚑ | `executions` | `OrderUpdate` |
| `balance_subscription()` ⚑ | `balances` | `BalanceUpdate` |

⚑ Private channel — requires a WS auth token from `POST /0/private/GetWebSocketsToken`.

---

### Crypto.com — spot, authenticated

Signing: HMAC-SHA256 over a deterministic parameter string, sent as a `sig` field in the request body (not a header). Add `src/cryptocom/auth.rs`.

#### Public REST (`src/cryptocom/rest.rs`)

Base URL: `https://api.crypto.com/exchange/v1`.

| Method | Endpoint |
|--------|----------|
| `get_instruments()` | `GET /public/get-instruments` |
| `get_orderbook(instrument, depth)` | `GET /public/get-book` |
| `get_candlestick(instrument, timeframe)` | `GET /public/get-candlestick` |
| `get_ticker(instrument)` | `GET /public/get-ticker` |
| `get_recent_trades(instrument)` | `GET /public/get-trades` |
| `get_funding_rate(instrument)` | `GET /public/get-valuations` |

Envelope: `{"code":0,"result":{…}}` — non-zero `code` surfaces as `ExchangeError::Api`.

#### Private REST (authenticated)

| Method | Endpoint |
|--------|----------|
| `get_account_summary(currency)` | `POST /private/get-account-summary` |
| `place_order(instrument, side, type, quantity, price)` | `POST /private/create-order` |
| `cancel_order(order_id)` | `POST /private/cancel-order` |
| `cancel_all_orders(instrument)` | `POST /private/cancel-all-orders` |
| `get_open_orders(instrument)` | `POST /private/get-open-orders` |
| `get_order_detail(order_id)` | `POST /private/get-order-detail` |
| `get_trades(instrument)` | `POST /private/get-trades` |
| `get_deposit_address(currency)` | `POST /private/get-deposit-address` |
| `create_withdrawal(currency, amount, address)` | `POST /private/create-withdrawal` |
| `get_withdrawal_history(currency)` | `POST /private/get-withdrawal-history` |

#### WebSocket (`src/cryptocom/ws.rs`)

Implements `ExchangeConnector`.

Base URLs:
- Public: `wss://stream.crypto.com/exchange/v1/market`
- Private: `wss://stream.crypto.com/exchange/v1/user`

Ping: send `{"method":"public/heartbeat"}` every 30 s; respond to the server heartbeat with `{"method":"public/respond-heartbeat","id":<same_id>}`.

Subscribe message format:
```json
{"id":1,"method":"subscribe","params":{"channels":["book.BTC_USDT.10"]}}
```

| Subscription helper | Channel pattern | DataMessage |
|---------------------|-----------------|-------------|
| `trade_subscription(instrument)` | `trade.<instrument>` | `Trade` |
| `ticker_subscription(instrument)` | `ticker.<instrument>` | `Ticker` |
| `kline_subscription(instrument, timeframe)` | `candlestick.<tf>.<instrument>` | `Candle` |
| `orderbook_subscription(instrument, depth)` | `book.<instrument>.<depth>` | `OrderBook` |
| `order_updates_subscription()` ⚑ | `user.order.<instrument>` | `OrderUpdate` |
| `balance_subscription()` ⚑ | `user.balance` | `BalanceUpdate` |

⚑ Private — connect to the private WS URL with a signed auth frame sent immediately after connect.

---

### Implementation order

| Step | Work item |
|------|-----------|
| 1 | `PublicRestClient` + `Envelope` trait |
| 2 | `DataMessage::Candle` + `DataMessage::FundingRate` variants |
| 3 | Binance public REST + WS |
| 4 | Bybit public REST + WS |
| 5 | KuCoin UTA + spot margin REST |
| 6 | KuCoin WS order placement |
| 7 | Kraken public REST + WS |
| 8 | Kraken private REST + private WS |
| 9 | Crypto.com public REST + WS |
| 10 | Crypto.com private REST + private WS |

### Target file layout

```
src/
├── http.rs              (new) PublicRestClient
├── binance/
│   ├── mod.rs
│   ├── rest.rs
│   └── ws.rs
├── bybit/
│   ├── mod.rs
│   ├── rest.rs
│   └── ws.rs
├── kraken/
│   ├── mod.rs
│   ├── auth.rs
│   ├── rest.rs
│   └── ws.rs
├── cryptocom/
│   ├── mod.rs
│   ├── auth.rs
│   ├── rest.rs
│   └── ws.rs
├── rest/               (existing — KuCoin)
│   ├── account.rs      + UTA endpoints
│   ├── margin.rs       (new) KuCoin spot margin
│   ├── market.rs
│   ├── mod.rs
│   └── orders.rs
└── ws/                 (existing — KuCoin)
    ├── orders.rs       (new) WS order placement
    └── …
tests/
├── binance_rest.rs     (new)
├── bybit_rest.rs       (new)
├── kraken_rest.rs      (new)
├── cryptocom_rest.rs   (new)
├── rest_mock.rs        (existing — extend for margin, UTA)
└── ws_types.rs         (existing)
```

---

## License

MIT — see [LICENSE](LICENSE).
