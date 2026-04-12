//! Shared domain types — candles, order sides, contract sizing.

use serde::{Deserialize, Serialize};

use crate::error::{ExchangeError, Result};

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
    ///
    /// # Errors
    /// Returns [`ExchangeError::InsufficientData`] when the array is too short
    /// or a field cannot be parsed as a number, with a message identifying
    /// which index and value failed.
    pub fn from_raw(arr: &[serde_json::Value]) -> Result<Self> {
        if arr.len() < 6 {
            return Err(ExchangeError::InsufficientData(format!(
                "candle array too short: expected ≥6 elements, got {}",
                arr.len()
            )));
        }

        let time = arr[0].as_i64().ok_or_else(|| {
            ExchangeError::InsufficientData(format!(
                "candle[0] (timestamp) is not an integer: {:?}",
                arr[0]
            ))
        })?;

        let num = |i: usize, field: &str| -> Result<f64> {
            let v = &arr[i];
            if let Some(s) = v.as_str() {
                s.parse::<f64>().map_err(|_| {
                    ExchangeError::InsufficientData(format!(
                        "candle[{i}] ({field}) could not be parsed as f64: {s:?}"
                    ))
                })
            } else {
                v.as_f64().ok_or_else(|| {
                    ExchangeError::InsufficientData(format!(
                        "candle[{i}] ({field}) is not a number: {v:?}"
                    ))
                })
            }
        };

        Ok(Self {
            time,
            open: num(1, "open")?,
            high: num(2, "high")?,
            low: num(3, "low")?,
            close: num(4, "close")?,
            volume: num(5, "volume")?,
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

// ── Tests ─────────────────────────────────────────────────────────────────────

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
    fn candle_too_short_returns_err() {
        let arr = serde_json::json!([1_713_000_000_000_i64, "86000.0"]);
        let row = arr.as_array().unwrap();
        let err = Candle::from_raw(row).unwrap_err();
        assert!(matches!(err, ExchangeError::InsufficientData(_)));
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn candle_bad_field_returns_err_with_context() {
        let arr = serde_json::json!([
            1_713_000_000_000_i64,
            "not-a-number",
            "87000.0",
            "85000.0",
            "86500.0",
            "1234.5"
        ]);
        let row = arr.as_array().unwrap();
        let err = Candle::from_raw(row).unwrap_err();
        assert!(matches!(err, ExchangeError::InsufficientData(_)));
        // Error message should name which field and value failed.
        assert!(err.to_string().contains("open"));
    }

    #[test]
    fn side_flip() {
        assert_eq!(Side::Buy.flip(), Side::Sell);
        assert_eq!(Side::Sell.flip(), Side::Buy);
    }
}
