// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Lossless parsing for allocation amounts.

use alloy::primitives::U256;

const WEI_PER_GRT: u32 = 18;
const MAX_U256_DEC_DIGITS: usize = 78;

#[derive(Debug, Clone)]
pub struct AmountParseError {
    pub reason: String,
}

impl AmountParseError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Parse an amount string into wei (U256), using an explicit, lossless policy:
/// - Integer strings are interpreted as wei (base-10) unless prefixed with 0x (hex wei)
/// - Decimal or scientific-notation strings are interpreted as GRT and converted to wei
pub fn parse_amount_wei(amount: &str) -> Result<U256, AmountParseError> {
    let amount = amount.trim();
    if amount.is_empty() {
        return Err(AmountParseError::new("amount cannot be empty"));
    }

    if let Some(hex) = amount
        .strip_prefix("0x")
        .or_else(|| amount.strip_prefix("0X"))
    {
        return U256::from_str_radix(hex, 16)
            .map_err(|e| AmountParseError::new(format!("failed to parse hex wei: {e}")));
    }

    if amount.contains('.') || amount.contains('e') || amount.contains('E') {
        return parse_grt_to_wei(amount);
    }

    U256::from_str_radix(amount, 10)
        .map_err(|e| AmountParseError::new(format!("failed to parse wei: {e}")))
}

fn parse_grt_to_wei(amount: &str) -> Result<U256, AmountParseError> {
    let (mantissa, exponent) = split_exponent(amount)?;
    let mantissa = mantissa.trim();
    if mantissa.is_empty() {
        return Err(AmountParseError::new("amount cannot be empty"));
    }

    if mantissa.starts_with('-') {
        return Err(AmountParseError::new("amount cannot be negative"));
    }

    let mantissa = mantissa.strip_prefix('+').unwrap_or(mantissa);
    let (int_part, frac_part) = split_decimal(mantissa)?;

    let int_part = int_part.trim();
    let frac_part = frac_part.trim();

    let mut digits = String::with_capacity(int_part.len() + frac_part.len());
    if !int_part.is_empty() {
        if !int_part.chars().all(|c| c.is_ascii_digit()) {
            return Err(AmountParseError::new("invalid decimal mantissa"));
        }
        digits.push_str(int_part);
    }
    if !frac_part.is_empty() {
        if !frac_part.chars().all(|c| c.is_ascii_digit()) {
            return Err(AmountParseError::new("invalid decimal mantissa"));
        }
        digits.push_str(frac_part);
    }

    if digits.is_empty() {
        return Err(AmountParseError::new("invalid decimal mantissa"));
    }

    let frac_len = frac_part.len() as i32;
    let shift = WEI_PER_GRT as i32 - frac_len + exponent;

    if shift >= 0 {
        let result_len = digits.len() + shift as usize;
        if result_len > MAX_U256_DEC_DIGITS {
            return Err(AmountParseError::new("amount out of range"));
        }
        let mut wei = digits;
        wei.extend(std::iter::repeat_n('0', shift as usize));
        return U256::from_str_radix(&wei, 10)
            .map_err(|e| AmountParseError::new(format!("amount out of range: {e}")));
    }

    let shift_abs = (-shift) as usize;
    if digits.len() <= shift_abs {
        if digits.chars().all(|c| c == '0') {
            return Ok(U256::ZERO);
        }
        return Err(AmountParseError::new("amount has fractional wei"));
    }

    let split_pos = digits.len() - shift_abs;
    if digits[split_pos..].chars().any(|c| c != '0') {
        return Err(AmountParseError::new("amount has fractional wei"));
    }

    let wei = &digits[..split_pos];
    U256::from_str_radix(wei, 10)
        .map_err(|e| AmountParseError::new(format!("amount out of range: {e}")))
}

fn split_exponent(amount: &str) -> Result<(&str, i32), AmountParseError> {
    let mut parts = amount.splitn(2, ['e', 'E']);
    let mantissa = parts.next().unwrap_or("");
    let Some(exp_part) = parts.next() else {
        return Ok((mantissa, 0));
    };
    if exp_part.is_empty() {
        return Err(AmountParseError::new("invalid exponent"));
    }
    let exponent = exp_part
        .parse::<i32>()
        .map_err(|_| AmountParseError::new("invalid exponent"))?;
    Ok((mantissa, exponent))
}

fn split_decimal(amount: &str) -> Result<(&str, &str), AmountParseError> {
    let mut parts = amount.splitn(3, '.');
    let int_part = parts.next().unwrap_or("");
    let frac_part = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(AmountParseError::new("invalid decimal mantissa"));
    }
    Ok((int_part, frac_part))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_amount_wei_integer() {
        let amount = parse_amount_wei("1000000000000000000").unwrap();
        assert_eq!(amount, U256::from(1_000_000_000_000_000_000u128));
    }

    #[test]
    fn test_parse_amount_large_wei() {
        let amount_str = "100000000000000000000000000000000000000";
        let amount = parse_amount_wei(amount_str).unwrap();
        let expected = U256::from_str_radix(amount_str, 10).unwrap();
        assert_eq!(amount, expected);
    }

    #[test]
    fn test_parse_amount_grt_decimal() {
        let amount = parse_amount_wei("100.5").unwrap();
        assert_eq!(amount, U256::from(100_500_000_000_000_000_000u128));
    }

    #[test]
    fn test_parse_amount_grt_scientific() {
        let amount = parse_amount_wei("1e-18").unwrap();
        assert_eq!(amount, U256::from(1u8));
    }

    #[test]
    fn test_parse_amount_grt_fractional_wei_error() {
        let err = parse_amount_wei("1e-19").unwrap_err();
        assert!(err.reason.contains("fractional wei"));
    }

    #[test]
    fn test_parse_amount_reject_negative() {
        let err = parse_amount_wei("-1.0").unwrap_err();
        assert!(err.reason.contains("negative"));
    }
}
