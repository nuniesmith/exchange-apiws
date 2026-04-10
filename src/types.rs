//! Shared domain types — candles, order sides, contract sizing.

use serde::{Deserialize, Serialize};

// ── Candle ─────────────────────────────────────────────────────────────────────

/// OHLCV candle (millisecond timestamp).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    /// Unix timestamp in **milliseconds**.
    pub time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

impl Candle {
    /// Parse from KuCoin raw kline array `[ts_ms, open, high, low, close, volume]`.
    ///
    /// KuCoin returns prices/volumes as strings inside the inner array.
    pub fn from_raw(arr: &[serde_json::Value]) -> Option<Self> {
        let num = |i: usize| -> Option<f64> {
            let v = arr.get(i)?;
            // KuCoin sends numbers as strings in kline arrays.
            if let Some(s) = v.as_str() {
                s.parse().ok()
            } else {
                v.as_f64()
            }
        };
        Some(Self {
            time: arr.get(0)?.as_i64()?,
            open: num(1)?,
            high: num(2)?,
            low: num(3)?,
            close: num(4)?,
            volume: num(5)?,
        })
    }
}

// ── Side ───────────────────────────────────────────────────────────────────────

/// Order / trade side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }

    /// The opposing side.
    pub fn flip(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── OrderType ─────────────────────────────────────────────────────────────────

/// Futures order type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderType {
    Market,
    Limit,
}

impl OrderType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Limit => "limit",
        }
    }
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Contract sizing ───────────────────────────────────────────────────────────

/// Notional value (in quote currency) of **one** contract for the given symbol.
///
/// Used by `calc_contracts` to size positions correctly.
/// Inverse (USD-margined) contracts are denominated in USD;
/// linear (USDT-margined) contracts express the base-coin multiplier.
pub fn contract_value(symbol: &str) -> f64 {
    match symbol {
        // Inverse / coin-margined (1 USD per contract)
        "XBTUSDM" => 1.0,
        "ETHUSDM" => 1.0,
        // Linear / USDT-margined — base-coin multiplier
        "XBTUSDTM" => 0.001,  // 0.001 BTC
        "ETHUSDTM" => 0.01,   // 0.01 ETH
        "SOLUSDTM" => 0.1,    // 0.1 SOL
        "BNBUSDTM" => 0.01,   // 0.01 BNB
        "XRPUSDTM" => 10.0,   // 10 XRP
        "DOGEUSDTM" => 100.0, // 100 DOGE
        "ADAUSDTM" => 10.0,
        "AVAXUSDTM" => 0.1,
        "LINKUSDTM" => 1.0,
        _ => 1.0, // safe fallback — log a warning if used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candle_parse_string_fields() {
        let arr = serde_json::json!([
            1713000000000i64,
            "86000.0",
            "87000.0",
            "85000.0",
            "86500.0",
            "1234.5"
        ]);
        let row = arr.as_array().unwrap();
        let c = Candle::from_raw(row).expect("should parse");
        assert_eq!(c.time, 1713000000000);
        assert!((c.open - 86000.0).abs() < 1e-9);
        assert!((c.close - 86500.0).abs() < 1e-9);
    }

    #[test]
    fn side_flip() {
        assert_eq!(Side::Buy.flip(), Side::Sell);
        assert_eq!(Side::Sell.flip(), Side::Buy);
    }
}
