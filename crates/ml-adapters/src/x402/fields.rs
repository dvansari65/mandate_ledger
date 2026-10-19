//! Parsing the strings x402 puts on the wire: addresses, whole numbers,
//! 32-byte values. Shared by the cart and the rail, so both read a field
//! the same way and say the same thing when it is wrong.

use super::assets::Asset;
use super::types::Address;
use ml_core::{AdapterError, Money};
use rust_decimal::Decimal;

/// The most digits a `uint256` has.
const MAX_UINT_DIGITS: usize = 78;

pub(super) fn malformed(what: &str) -> AdapterError {
    AdapterError::Malformed(what.to_owned())
}

pub(super) fn unsupported(what: &str) -> AdapterError {
    AdapterError::Unsupported(what.to_owned())
}

/// An address field. Something that is not even trying to be an EVM
/// address — a role constant such as `merchant`, a fiat code — is valid
/// x402 this adapter does not support; a broken one is malformed.
pub(super) fn address(field: &str, value: &str) -> Result<Address, AdapterError> {
    if !value.starts_with("0x") {
        return Err(unsupported(&format!("{field}: only an EVM address")));
    }
    Address::parse(value).map_err(|why| malformed(&format!("{field}: {why}")))
}

/// A whole decimal number written canonically — `0`, or no leading zero,
/// so one value has one spelling — of at most 78 digits, the range of a
/// `uint256`, within what this adapter handles.
pub(super) fn uint(field: &str, value: &str) -> Result<u128, AdapterError> {
    let canonical = !value.is_empty()
        && value.len() <= MAX_UINT_DIGITS
        && value.bytes().all(|b| b.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'));
    if !canonical {
        return Err(malformed(&format!(
            "{field}: a whole number, without leading zeros"
        )));
    }
    value
        .parse()
        .map_err(|_| unsupported(&format!("{field}: beyond the range this adapter handles")))
}

/// Atomic units into money, exactly: the token's decimals become the scale.
pub(super) fn money(atomic: u128, token: &Asset) -> Result<Money, AdapterError> {
    let units = i128::try_from(atomic)
        .ok()
        .and_then(|units| Decimal::try_from_i128_with_scale(units, u32::from(token.decimals)).ok())
        .ok_or_else(|| unsupported("amount: beyond the range this adapter prices"))?;
    Ok(Money::new(units, token.symbol.clone()))
}

/// `0x` and exactly `2 * N` hex digits, as `N` bytes.
pub(super) fn bytes<const N: usize>(field: &str, value: &str) -> Result<[u8; N], AdapterError> {
    let mut out = [0u8; N];
    value
        .strip_prefix("0x")
        .filter(|hex| hex.len() == 2 * N)
        .and_then(|hex| hex::decode_to_slice(hex, &mut out).ok())
        .ok_or_else(|| malformed(&format!("{field}: 0x and {} hex digits", 2 * N)))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uints_are_canonical() {
        assert_eq!(uint("v", "0").unwrap(), 0);
        assert_eq!(uint("v", "10000").unwrap(), 10_000);
        for bad in ["", "-1", "1.5", "1e6", " 1", "0x10", "010", "00"] {
            assert!(
                matches!(uint("v", bad), Err(AdapterError::Malformed(_))),
                "{bad:?}"
            );
        }
        assert!(matches!(
            uint("v", &"9".repeat(39)),
            Err(AdapterError::Unsupported(_))
        ));
        assert!(matches!(
            uint("v", &"1".repeat(79)),
            Err(AdapterError::Malformed(_))
        ));
    }

    #[test]
    fn bytes_are_exact_length_hex() {
        assert_eq!(bytes::<2>("b", "0xab01").unwrap(), [0xab, 0x01]);
        for bad in ["ab01", "0xab0", "0xab012", "0xzz01", ""] {
            assert!(bytes::<2>("b", bad).is_err(), "{bad:?}");
        }
    }
}
