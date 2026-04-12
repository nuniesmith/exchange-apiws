//! Shared domain types — candles, order sides, contract sizing.

use serde::{Deserialize, Serialize};

// ── Candle ─────────────────────────────────────────────────────────────────────

/// OHLCV candle (millisecond timestamp).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    /// Unix timestamp in **milliseconds**.
    pub time: i64,
    /// Opening price for the period.
    pub open: f64,
    /// Highest price during the period.
    pub high: f64,
    /// Lowest price during the period.
    pub low: f64,
    /// Closing price for the period.
    pub close: f64,
    /// Total traded volume for the period.
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
            time: arr.first()?.as_i64()?,
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
    /// Long / buy side.
    Buy,
    /// Short / sell side.
    Sell,
}

impl Side {
    /// Returns the lowercase string representation used by the KuCoin REST API.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }

    /// The opposing side.
    #[must_use]
    pub const fn flip(self) -> Self {
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
    /// Execute immediately at the current market price.
    Market,
    /// Execute only at the specified limit price or better.
    Limit,
}

impl OrderType {
    /// Returns the lowercase string representation used by the KuCoin REST API.
    pub const fn as_str(self) -> &'static str {
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candle_parse_string_fields() {
        let arr = serde_json::json!([
            1_713_000_000_000_i64,
            "86000.0",
            "87000.0",
            "85000.0",
            "86500.0",
            "1234.5"
        ]);
        let row = arr.as_array().unwrap();
        let c = Candle::from_raw(row).expect("should parse");
        assert_eq!(c.time, 1_713_000_000_000);
        assert!((c.open - 86000.0).abs() < 1e-9);
        assert!((c.close - 86500.0).abs() < 1e-9);
    }

    #[test]
    fn side_flip() {
        assert_eq!(Side::Buy.flip(), Side::Sell);
        assert_eq!(Side::Sell.flip(), Side::Buy);
    }
}

// ── TimeInForce ───────────────────────────────────────────────────────────────

/// How long an order remains active before execution or expiry.
///
/// See KuCoin docs: GTC is the safe default. Market orders do not support TIF.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TimeInForce {
    /// Good Till Canceled — expires only when explicitly canceled.
    #[default]
    GTC,
    /// Good Till Time — expires at a caller-specified timestamp.
    GTT,
    /// Immediate Or Cancel — execute what can fill immediately, cancel the rest.
    IOC,
    /// Fill Or Kill — cancel the whole order if it cannot be completely filled.
    FOK,
}

impl TimeInForce {
    /// Returns the uppercase string representation used by the KuCoin REST API.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GTC => "GTC",
            Self::GTT => "GTT",
            Self::IOC => "IOC",
            Self::FOK => "FOK",
        }
    }
}

impl std::fmt::Display for TimeInForce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── STP (Self-Trade Prevention) ───────────────────────────────────────────────

/// Self-Trade Prevention strategy.
///
/// When set, orders placed by the same UID cannot execute against each other.
/// Only the taker's STP setting is enforced. Futures STP operates at the
/// master-UID level by default (all sub-UIDs are covered).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum STP {
    /// Decrease and Cancel — reduce the larger order by the smaller size, cancel the smaller.
    DC,
    /// Cancel Old — cancel the resting order.
    CO,
    /// Cancel New — cancel the incoming order.
    CN,
    /// Cancel Both — cancel both the resting and incoming orders.
    CB,
}

impl STP {
    /// Returns the uppercase string representation used by the KuCoin REST API.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DC => "DC",
            Self::CO => "CO",
            Self::CN => "CN",
            Self::CB => "CB",
        }
    }
}

impl std::fmt::Display for STP {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
