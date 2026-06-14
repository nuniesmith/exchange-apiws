# kucoin-dip-buyer

A small, readable "buy the dip" trading bot for **KuCoin BTC futures**, built on
two crates:

- [`indicators-ta`](../../../indicators-ta) — RSI + a trend EMA from candles.
- [`exchange-apiws`](../..) — live data (REST klines + a public WebSocket
  ticker feed) and order placement.

It is a companion crate, **not** part of the `exchange-apiws` library. It lives
in its own workspace so it never affects the library's build or publish.

> ⚠️ **Trading futures with leverage can lose money fast, including more than
> your stop intends in a gap.** This bot ships in **dry-run** mode and trades
> nothing until you explicitly enable live mode. It is example/educational
> code, not financial advice. Start on dry-run, read the logs, and only go
> live with money you can afford to lose.

---

## How it decides

It's a three-state machine. All the rules are pure functions in
[`src/strategy.rs`](src/strategy.rs) and covered by unit tests (`cargo test`).

**Entry (when `Flat`)** — on each closed candle:
1. **RSI cross-up.** RSI(14) was at/below the oversold level (default 30) on the
   previous bar and is back above it now. Waiting for the *cross up* means we
   buy when momentum turns, instead of catching a falling knife while it keeps
   dropping.
2. **Trend filter** (optional, on by default). Price must be above the trend
   EMA (default EMA-200 on the same timeframe) — i.e. only buy dips inside an
   uptrend. Turn off with `DIP_TREND_FILTER=0` to buy every oversold cross.

**Exit (when `Long` / `Runner`)** — on each live ticker price:
- **Stop-loss:** price ≤ entry × (1 − `STOP_LOSS_PCT`) → close everything.
- **Take-profit:** price ≥ entry × (1 + `TAKE_PROFIT_PCT`) → **scale out**: sell
  `SELL_FRACTION` (default 90 %), keep the rest as a **runner**.
- **Runner:** the kept remainder rides a **trailing stop** `TRAIL_PCT` below its
  high-water mark, floored at break-even — so the runner can't turn a win into a
  loss.

```
Flat ──RSI cross-up in uptrend──▶ Long ──TP──▶ Runner ──trailing stop──▶ Flat
                                   └────────── SL ─────────────────────▶ Flat
```

---

## ⚠️ Reality check for a ~38 USDT account

Two things genuinely matter at this size, and the bot is built around them:

1. **This crate trades KuCoin *futures*, not spot.** You don't hold actual BTC;
   you hold a **long perpetual position** (XBTUSDTM). "Selling back to USDT" =
   closing the position and realising USDT PnL. Futures use **leverage** and can
   be **liquidated**.

2. **The 1-contract minimum forces leverage.** XBTUSDTM's contract is
   `0.001 BTC`. At BTC ≈ \$65k that's ~\$65 notional, needing ~\$65 margin at 1×
   — more than \$38. So with \$38 you **cannot open even the minimum position at
   1×**; you need roughly **2–3×** just to place one contract. The bot defaults
   to **5×** (`DIP_LEVERAGE`), and refuses to enter if it can't afford a contract
   rather than erroring at the exchange.

   **Pick leverage with the liquidation distance in mind — the stop only saves
   you if it triggers *before* liquidation.** Isolated-long liquidation sits
   roughly `1/leverage` below entry:

   | Leverage | ≈ Liquidation | Safe stop-loss |
   |---|---|---|
   | 5× | ~−20 % | −3 % default is very safe |
   | 10× | ~−10 % | keep stop ≤ ~−5 % |
   | 20× | ~−5 % | keep stop ≤ ~−2.5 %, watch wicks |

   The bot computes your actual liquidation price from the contract's
   maintenance-margin rate and **warns at startup if your stop-loss is at or
   near liquidation** for the chosen leverage. Higher leverage = smaller moves
   liquidate you, so size down and keep the stop well inside the liquidation
   distance.

   It also means **"sell 90 % / keep 10 %" can't run on a 1-contract position** —
   you can't sell 0.9 of a contract. When the position is too small to split,
   the bot keeps the whole single contract as the runner and trails it. The
   scale-out only kicks in once a position is ≥ ~10 contracts.

   The banner the bot prints on startup tells you exactly how many contracts
   your balance affords right now.

---

## Should you sell the dip, or keep some in BTC?

Short version of the design baked into the defaults:

- **Always exit with a plan, and always use a stop.** A "dip" can keep dipping;
  hope is not a stop-loss.
- **Take most of the profit, let a little ride.** The default sells ~90 % at the
  take-profit and trails the last ~10 % at break-even+. You lock in the bulk of
  the gain (and free the margin to catch the next dip) while keeping a free
  option on a bigger move. Your "keep 10 %, sell 90 %" instinct is exactly this —
  it's a sound default once the account is big enough to split a position.
- **At \$38 it's all-or-nothing per trade** (1 contract), so in practice the bot
  trails the whole contract after take-profit instead of splitting. The split
  behaviour is there and tested for when the account grows.

Tune `TAKE_PROFIT_PCT`, `STOP_LOSS_PCT`, `SELL_FRACTION`, and `TRAIL_PCT` to your
risk appetite. Keep TP comfortably above round-trip fees (~0.12 % taker) — the
2 % default clears them easily.

---

## Run it

From this directory (`bots/kucoin-dip-buyer`):

```bash
# Dry-run — no keys needed, no orders sent. Watches live BTC data and logs
# every decision it WOULD make.
cargo run

# Dry-run against your real balance (read-only) — needs API keys:
export KC_KEY=... KC_SECRET=... KC_PASSPHRASE=...
cargo run

# LIVE — sends real orders. Only after you've watched dry-run behave.
DIP_LIVE=1 cargo run --release
```

> This bot expects `exchange-apiws` and `indicators-ta` checked out **side by
> side** (the default layout), since it depends on them by relative path.

Stop with **Ctrl-C**. Shutdown stops trading but **does not** close an open
position — manage that yourself or let the stop/take-profit handle it.

---

## Configuration (environment variables)

| Variable | Default | Meaning |
|---|---|---|
| `DIP_LIVE` | `0` | `1` = place real orders. Otherwise dry-run. |
| `DIP_SYMBOL` | `XBTUSDTM` | Futures contract. |
| `DIP_GRANULARITY` | `15` | Candle timeframe in minutes. |
| `DIP_LOOKBACK` | `400` | Bars pulled for the indicators. |
| `DIP_RSI_PERIOD` | `14` | RSI period. |
| `DIP_RSI_OVERSOLD` | `30` | Oversold level; entry on a cross up through it. |
| `DIP_TREND_EMA_PERIOD` | `200` | Trend filter EMA period. |
| `DIP_TREND_FILTER` | `1` | `0` to buy dips regardless of trend. |
| `DIP_LEVERAGE` | `5` | Per-order leverage (5–20× typical; mind liquidation). |
| `DIP_RISK_FRACTION` | `0.90` | Fraction of balance used as margin per entry. |
| `DIP_MAX_CONTRACTS` | `50` | Hard cap on position size. |
| `DIP_TAKE_PROFIT_PCT` | `0.02` | Take-profit, +2 % above entry. |
| `DIP_STOP_LOSS_PCT` | `0.03` | Stop-loss, −3 % below entry. |
| `DIP_SELL_FRACTION` | `0.90` | Fraction sold at take-profit. |
| `DIP_TRAIL_PCT` | `0.015` | Trailing-stop distance for the runner. |
| `DIP_POLL_SECS` | `30` | Seconds between kline polls. |
| `DIP_PAPER_BALANCE` | `38` | Simulated balance for keyless dry-run. |
| `RUST_LOG` | `info` | Log level (`debug` for more detail). |

---

## Tests

```bash
cargo test     # strategy state machine: entries, scale-out, trailing, stops
```

The strategy logic is fully unit-tested offline. The live data and order paths
are exercised against KuCoin's REST + WebSocket APIs at runtime (any host with
network access to `api-futures.kucoin.com`).
