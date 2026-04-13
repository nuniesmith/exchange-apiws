8.2 exchange-apiws

✓  Strengths
• Clean separation of REST vs WS
• run_feed handles TLS, ping, exponential-backoff reconnect automatically
• auth.rs HMAC signing has unit tests
• WsRunnerConfig is well-designed for flexible timeout configuration
• REST mock integration tests (wiremock) cover balance, positions, orders,
  fills, calc_contracts, envelope unwrapping, and HTTP error paths

✓  Completed
• contract_value fallback bug — calc_contracts now errors explicitly via
  ExchangeError::Order when multiplier is None; covered by
  calc_contracts_errors_when_multiplier_is_missing test
• get_open_stop_orders — implemented in rest/orders.rs with StopOrderDetail
  type; re-exported from rest/mod.rs
• Stop order test coverage — added to tests/rest_mock.rs:
    - get_open_stop_orders (happy path + empty list)
    - place_stop_order (stop-market + stop-limit)
    - cancel_stop_order
    - cancel_all_stop_orders (with results + no open stops)
    - get_done_orders (filled orders + empty list)
    - get_order by ID (happy path + not-found error path)
• Transfer endpoint tests — transfer_to_main, transfer_to_futures, and
  get_transfer_list covered in tests/rest_mock.rs (happy path + error paths,
  empty list, max_count cap enforcement)
• Account endpoint tests — added to tests/rest_mock.rs:
    - set_auto_deposit (true/false happy paths + api error propagation)
    - set_risk_limit_level (happy path + api error propagation)
    - add_position_margin (IN and OUT directions)
    - get_account_overview_all (multi-currency + empty list)
• WS integration test harness — tests/ws_types.rs spins up a local
  tokio-tungstenite server per test scenario; covers:
    - run_feed_delivers_messages
    - run_feed_shuts_down_cleanly
    - run_feed_reconnects_on_disconnect
    - run_feed_exhausts_reconnects_returns_error
• Duplicate test sections removed — rest_mock.rs had two passes of
  set_auto_deposit, set_risk_limit_level, and get_account_overview_all
  tests; first-pass (thinner) duplicates removed, keeping the complete
  second-pass versions with error-propagation coverage.

⚠  Still open
• Multi-exchange support — only KuCoin is implemented; the
  ExchangeConnector trait is the extension point for new venues.

# exchange-apiws — Roadmap

## Context

Canadian account — no Binance or Bybit API keys, but **all public REST and WS
endpoints are freely accessible**. KuCoin, Kraken, and Crypto.com API keys
are available for **spot only**.

---

## 1. Architecture prerequisites (do first)

Everything below depends on these two foundational pieces.

### 1a. PublicRestClient

`KuCoinClient` calls `build_headers` on every request — it cannot make
unauthenticated calls. A new `PublicRestClient` is needed for Binance,
Bybit, and the public endpoints of all other exchanges.

```
src/http.rs   (new)
  PublicRestClient
    - reqwest::Client (rustls, 10s timeout)
    - base_url: String
    - get<T: DeserializeOwned>(path, params) -> Result<T>
    - same retry / 429-backoff logic as KuCoinClient
    - no envelope unwrapping — caller decides shape
```

Authenticated exchanges (Kraken, Crypto.com) will wrap this client and
add their own signing layer, rather than sharing KuCoinClient's KuCoin-
specific HMAC path.

### 1b. Envelope trait

Each exchange wraps responses differently:

| Exchange    | Envelope shape                                      |
|-------------|-----------------------------------------------------|
| KuCoin      | `{"code":"200000","data":{…}}`                      |
| Binance     | bare JSON — no wrapper                              |
| Bybit       | `{"retCode":0,"result":{…}}`                        |
| Kraken      | `{"result":{…},"error":[]}`                         |
| Crypto.com  | `{"code":0,"result":{…}}`                           |

Add a small `Envelope` trait (or free function) per exchange crate module
so each client can unwrap its own format and surface errors as
`ExchangeError::Api`.

### 1c. DataMessage additions

New feed types that don't map to existing variants:

| Variant (new)      | Used by                                     |
|--------------------|---------------------------------------------|
| `Candle(CandleData)` | Binance kline stream, Bybit kline, Kraken OHLC, Crypto.com candlestick |
| `FundingRate(FundingData)` | Binance mark-price stream, Bybit ticker extended |

`CandleData` fields: `symbol`, `exchange`, `interval`, `open_ts`,
`open`, `high`, `low`, `close`, `volume`, `is_closed`, `receipt_ts`.

---

## 2. KuCoin — remaining work

### 2a. Unified Trade Account (UTA) REST endpoints

KuCoin Unified combines Spot + Futures margin in one account.
Base URL: `https://api.kucoin.com` (same as Spot).

| Method | Endpoint |
|--------|----------|
| `get_unified_account()` | `GET /api/v3/account/summary` |
| `get_unified_margin()` | `GET /api/v3/margin/accounts` |
| `get_cross_margin_symbols()` | `GET /api/v1/isolated/accounts` |

Add `KucoinEnv::Unified` routing (already in `KucoinEnv` enum, just needs
REST methods in `src/rest/account.rs` and test coverage in `rest_mock.rs`).

### 2b. Spot margin orders

| Method | Endpoint |
|--------|----------|
| `place_margin_order(symbol, side, size, price, leverage)` | `POST /api/v1/margin/order` |
| `get_margin_order(order_id)` | `GET /api/v1/margin/orders/{id}` |
| `cancel_margin_order(order_id)` | `DELETE /api/v1/margin/orders/{id}` |
| `get_margin_fills(symbol)` | `GET /api/v1/margin/fills` |
| `get_margin_balance(currency)` | `GET /api/v1/margin/account` |

Add `src/rest/margin.rs`, re-export from `rest/mod.rs`, full wiremock
coverage in `rest_mock.rs`.

### 2c. WebSocket order placement

KuCoin's `wsapi.kucoin.com` supports placing and cancelling orders over WS
for ultra-low latency. This requires a **private** WS token and a separate
connection to `wss://wsapi.kucoin.com`.

Scope:
- `src/ws/orders.rs` (new) — `WsOrderClient` wrapping a tungstenite sink
- `place_order_ws(symbol, side, size, leverage, order_type, price)` → sends
  JSON frame, awaits ack with matching `clientOid`
- `cancel_order_ws(order_id)` → same pattern
- Response matching by `clientOid` in a `HashMap<String, oneshot::Sender>`
- Rate limit: 100 msg/10s (shared with the existing runner guard)
- Tests: local tungstenite echo server verifying frame shape and ack routing

---

## 3. Binance — public only

No API keys. All endpoints below are unauthenticated.

### 3a. REST (`src/binance/rest.rs`)

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

Response: bare JSON arrays/objects (no envelope wrapper).

### 3b. WebSocket (`src/binance/ws.rs`)

Implements `ExchangeConnector`. Binance WS uses URL-encoded stream names
rather than subscription messages after connect.

Base URLs:
- Spot: `wss://stream.binance.com:9443/ws/<streamName>`
- Spot combined: `wss://stream.binance.com:9443/stream?streams=<a>/<b>`
- Futures: `wss://fstream.binance.com/ws/<streamName>`

| Subscription helper | Stream name | DataMessage variant |
|---------------------|-------------|---------------------|
| `trade_subscription(symbol)` | `<symbol>@aggTrade` | `Trade` |
| `ticker_subscription(symbol)` | `<symbol>@bookTicker` | `Ticker` |
| `kline_subscription(symbol, interval)` | `<symbol>@kline_<interval>` | `Candle` |
| `depth_subscription(symbol)` | `<symbol>@depth@100ms` | `OrderBook` (delta) |
| `depth_snapshot_subscription(symbol, levels)` | `<symbol>@depth{5\|10\|20}@100ms` | `OrderBook` (snapshot) |
| `mark_price_subscription(symbol)` (futures) | `<symbol>@markPrice@1s` | `FundingRate` |

Ping: Binance sends a ping frame — runner responds with pong. No
application-level ping needed.

---

## 4. Bybit — public only

No API keys. All endpoints below are unauthenticated.

### 4a. REST (`src/bybit/rest.rs`)

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
Envelope: `{"retCode":0,"result":{…}}` — unwrap or surface retCode != 0
as `ExchangeError::Api`.

### 4b. WebSocket (`src/bybit/ws.rs`)

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

Note: first `orderbook.*` message is a snapshot (`type:"snapshot"`),
subsequent messages are deltas (`type:"delta"`). Set `is_snapshot`
accordingly in `parse_message`.

---

## 5. Kraken — spot, authenticated

API keys available. Signing: HMAC-SHA512 over
`URI + SHA256(nonce + encoded_body)`, base64-encoded, sent as
`API-Sign` header alongside `API-Key`.

Add `src/kraken/auth.rs` (separate from `src/auth.rs` which is KuCoin-
specific).

### 5a. Public REST (`src/kraken/rest.rs`)

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

Envelope: `{"result":{…},"error":[]}` — non-empty `error` array →
`ExchangeError::Api`.

### 5b. Private REST (authenticated)

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

### 5c. WebSocket (`src/kraken/ws.rs`)

Implements `ExchangeConnector`.

Base URL: `wss://ws.kraken.com/v2`
Private: `wss://ws-auth.kraken.com/v2`

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

⚑ Private channel — requires a WS auth token from
`POST /0/private/GetWebSocketsToken`.

---

## 6. Crypto.com — spot, authenticated

API keys available. Signing: HMAC-SHA256 over a deterministic parameter
string. Sent as `sig` field in the request body (not a header).

Add `src/cryptocom/auth.rs`.

### 6a. Public REST (`src/cryptocom/rest.rs`)

Base URL: `https://api.crypto.com/exchange/v1`.

| Method | Endpoint |
|--------|----------|
| `get_instruments()` | `GET /public/get-instruments` |
| `get_orderbook(instrument, depth)` | `GET /public/get-book` |
| `get_candlestick(instrument, timeframe)` | `GET /public/get-candlestick` |
| `get_ticker(instrument)` | `GET /public/get-ticker` |
| `get_recent_trades(instrument)` | `GET /public/get-trades` |
| `get_funding_rate(instrument)` | `GET /public/get-valuations` |

Envelope: `{"code":0,"result":{…}}` — non-zero `code` →
`ExchangeError::Api`.

### 6b. Private REST (authenticated)

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

### 6c. WebSocket (`src/cryptocom/ws.rs`)

Implements `ExchangeConnector`.

Public: `wss://stream.crypto.com/exchange/v1/market`
Private: `wss://stream.crypto.com/exchange/v1/user`

Ping: send `{"method":"public/heartbeat"}` every 30 s;
server responds with heartbeat — respond with `{"method":"public/respond-heartbeat","id":<same_id>}`.

Subscribe message:
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

⚑ Private — connect to the private WS URL with a signed auth frame
sent immediately after connect.

---

## Implementation order (suggested)

1. `PublicRestClient` + `Envelope` trait  
2. `DataMessage::Candle` + `DataMessage::FundingRate` variants  
3. Binance public REST + WS (simplest envelope, most documentation)  
4. Bybit public REST + WS  
5. KuCoin UTA + spot margin REST  
6. KuCoin WS order placement  
7. Kraken public REST + WS  
8. Kraken private REST + private WS  
9. Crypto.com public REST + WS  
10. Crypto.com private REST + private WS  

---

## File layout (target)

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
