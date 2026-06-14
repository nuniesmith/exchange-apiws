# kucoin-dip-buyer — TODO

A working backlog for the bot, roughly in priority order. The **P0** items are
the ones that matter *before* running live at 5–20× — at that leverage the
liquidation distance is small (~−5% at 20×), so the safety/execution gaps below
are what actually protect the account.

Checkboxes so you can track progress. API hints point at the exact
`exchange-apiws` / `indicators-ta` calls to use.

---

## P0 — safety before live (do these first at 5–20×)

- [ ] **Server-side stop-loss order on entry.** Right now the stop is
  *client-side* — it only fires while the bot is running and receiving ticks. If
  the bot crashes or the WS drops, an open 20× long is unprotected. On every
  entry, also place a reduce-only **stop order** on the exchange so KuCoin
  enforces it.
  → `KuCoinClient::place_stop_order(symbol, Side::Sell, size, lev, stop_price,
  "down", None, reduce_only=true)`. Cancel/replace it on scale-out
  (`cancel_stop_order` / `get_open_stop_orders`).
- [ ] **Daily loss circuit breaker.** Stop opening new positions after N
  consecutive losses or once realized PnL for the day ≤ −X%. Add
  `DIP_MAX_DAILY_LOSS_PCT` / `DIP_MAX_CONSEC_LOSSES`; track in `Strategy`.
- [ ] **Verify fills instead of assuming them.** `try_enter`/`react_exit`
  currently assume a market order fills at `price`. Poll the order after placing
  and use the real average fill price for entry/stop math.
  → `KuCoinClient::get_order(order_id)` / `get_recent_fills(symbol)`.
- [ ] **Confirm isolated margin + sane risk-limit tier on startup.** Cross
  margin can drain the whole account on one bad trade. Make isolated explicit and
  log the tier.
  → `get_risk_limit_levels` / `set_risk_limit_level`; consider
  `set_auto_deposit(false)` so a position can't silently pull more margin.
- [ ] **ATR-based stop / take-profit** instead of fixed %. A fixed −3% is too
  tight in calm regimes and too loose in volatile ones. Size the stop off
  volatility and re-check it's inside the liquidation distance.
  → `indicators::IncrementalAtr` (or `indicators::atr(&high,&low,&close,p)`).

## P1 — make the edge real

- [ ] **Backtest harness.** Replay historical klines through `Strategy` offline
  and report win-rate / avg R / max drawdown before risking money. Reuse
  `fetch_klines_extended` to pull history; the strategy is already pure, so this
  is mostly a driver loop + a PnL accumulator. *(Highest-value P1 — tune TP/SL/RSI
  here, not with live money.)*
- [ ] **Maker (limit) entries to cut fees.** Market orders pay taker (~0.06%);
  a post-only limit at/below the bid pays maker (~0.02%) and improves the entry.
  → `place_order(..., OrderType::Limit, Some(price), Some(TimeInForce::GTC), ...)`;
  add a timeout that falls back to market if unfilled.
- [ ] **Stronger entry filter.** Cut false dips with a higher-timeframe trend
  check and/or the crate's regime + confluence engines.
  → `indicators::RegimeDetector`, `indicators::ConfluenceEngine`,
  `indicators::compute_signal`.
- [ ] **Avoid entries right before funding.** Don't open seconds before an 8h
  funding settlement (you'd pay immediately). Skip entries within M minutes of
  the next settlement.
  → `get_funding_rate(symbol)` (`time_point` = next settlement) or the
  `DataMessage::InstrumentEvent` `"funding.rate"` subject.
- [ ] **DCA / laddered entries (optional).** Instead of one entry, scale into a
  deepening dip with 2–3 rungs and average down — with a hard total-size cap.

## P2 — reliability, ops, observability

- [ ] **Private WS feed for fills/position/balance** instead of REST polling —
  lower latency on exits and accurate live PnL.
  → `get_ws_token_private`, `WsOrderClient`, and the
  `DataMessage::{OrderUpdate, PositionChange, BalanceUpdate}` variants.
- [ ] **Trade journal.** Append every entry/exit to a CSV/JSONL with price,
  size, fees, reason, realized PnL. Foundation for the metrics above.
- [ ] **Alerts.** Telegram/Discord webhook on entry, exit, stop hit, and circuit
  breaker — so you don't have to watch logs.
- [ ] **Kill switch / flatten-on-exit flag.** Optional `DIP_FLATTEN_ON_STOP=1`
  to market-close any open position on Ctrl-C (default keeps it open).
- [ ] **State persistence.** Persist the dry-run sim position to disk so restarts
  resume cleanly (live mode already reconciles from the exchange).
- [ ] **CI for the bot crate.** It's excluded from the library's CI; add a small
  workflow (or a Makefile target) running `cargo test`/`clippy` for `bots/`.
- [ ] **Deployment.** Dockerfile + a systemd unit / compose file for 24/7
  running, with `RUST_LOG`, restart-on-failure, and secrets via env.
- [ ] **More tests.** Edge cases (gaps through both TP and SL in one tick, reconcile
  races) and a property test that the runner stop never resolves below break-even.

## Stretch — spot path

- [ ] **Spot buy-the-dip mode.** Add KuCoin **spot** order methods to
  `exchange-apiws` (`KuCoin::spot` already exists for the base URL; the order
  bodies differ — `funds`/`size` in base units, no leverage) and a `DIP_SPOT=1`
  mode. Lets you buy *fractional* BTC with any dollar amount, no liquidation —
  a better fit for "keep some, sell the rest" at a small balance.

---

### Done
- [x] RSI cross-up entry with EMA trend filter (pure, unit-tested).
- [x] Scale-out take-profit + break-even trailing runner; 1-contract fallback.
- [x] Client-side stop-loss.
- [x] Dry-run default; affordability check for the 1-contract minimum.
- [x] Live REST sizing/orders + public WS ticker feed (supervised, auto-reconnect).
- [x] Liquidation-distance guard: warns when the stop-loss is at/near liquidation
  for the configured leverage.
