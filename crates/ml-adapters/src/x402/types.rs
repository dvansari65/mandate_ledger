//! The wire types this adapter reads, as x402 v2 defines them, and the two
//! identifiers it parses strictly.

use ml_core::AdapterError;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use std::fmt;
use std::str::FromStr;

/// An EVM account address: twenty bytes, written as `0x` and forty hex
/// digits. Parsed from all-lower, all-upper, or EIP-55 checksummed mixed
/// case — a mixed-case address whose checksum is wrong is a typo, and is
/// refused. Written in lower case.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Address([u8; 20]);

impl Address {
    /// The zero address. Tokens sent to it are gone.
    pub const ZERO: Self = Self([0; 20]);

    /// The raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// From raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    /// Parse, saying why not when it is not an address.
    pub(super) fn parse(s: &str) -> Result<Self, &'static str> {
        let hex = s
            .strip_prefix("0x")
            .filter(|hex| hex.len() == 40)
            .ok_or("not 0x and 40 hex digits")?;
        let mut bytes = [0u8; 20];
        hex::decode_to_slice(hex, &mut bytes).map_err(|_| "not 0x and 40 hex digits")?;
        let mixed = hex.bytes().any(|b| b.is_ascii_lowercase())
            && hex.bytes().any(|b| b.is_ascii_uppercase());
        if mixed && !checksummed(hex) {
            return Err("mixed case, but not its EIP-55 checksum");
        }
        Ok(Self(bytes))
    }
}

/// EIP-55: each letter is upper case exactly when the matching nibble of
/// `keccak256(lower-case hex)` is 8 or more.
fn checksummed(hex: &str) -> bool {
    let hash = Keccak256::digest(hex.to_ascii_lowercase().as_bytes());
    hex.bytes().enumerate().all(|(i, c)| {
        let nibble = if i % 2 == 0 {
            hash[i / 2] >> 4
        } else {
            hash[i / 2] & 0x0f
        };
        !c.is_ascii_alphabetic() || c.is_ascii_uppercase() == (nibble >= 8)
    })
}

impl FromStr for Address {
    type Err = AdapterError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).map_err(|why| AdapterError::Malformed(format!("address: {why}")))
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// An EVM chain, named the CAIP-2 way: `eip155:<chain id>`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChainId(u64);

impl ChainId {
    /// The chain id.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The numeric id.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl FromStr for ChainId {
    type Err = AdapterError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let unsupported =
            || AdapterError::Unsupported("network is not an eip155 chain id".to_owned());
        let id = s.strip_prefix("eip155:").ok_or_else(unsupported)?;
        // A chain id is a positive decimal number with no leading zero, so
        // that one chain has one name.
        let canonical =
            !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) && !id.starts_with('0');
        if !canonical {
            return Err(unsupported());
        }
        id.parse().map(Self).map_err(|_| unsupported())
    }
}

impl fmt::Display for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "eip155:{}", self.0)
    }
}

impl fmt::Debug for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// An account on a chain, the CAIP-10 way: `eip155:8453:0x…`, address in
/// lower case. This is the x402 merchant id: the same address on two
/// chains is two merchants, so a mandate names the chain it pays on.
#[must_use]
pub fn account_id(chain: ChainId, address: Address) -> String {
    format!("{chain}:{address}")
}

/// What is being sold: x402's `ResourceInfo`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceInfo {
    /// The protected resource.
    pub url: String,
    /// What it is, for people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The media type of the response.
    #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// The service hosting it.
    #[serde(
        default,
        rename = "serviceName",
        skip_serializing_if = "Option::is_none"
    )]
    pub service_name: Option<String>,
    /// Discovery tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// An icon for the service.
    #[serde(default, rename = "iconUrl", skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

/// One way to pay: x402's `PaymentRequirements`, as the wire carries it.
/// Every field is kept as sent; [`ExactTerms`](super::ExactTerms) is the
/// checked reading of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentRequirements {
    /// The payment scheme. This adapter reads `exact`.
    pub scheme: String,
    /// The network, in CAIP-2 form.
    pub network: String,
    /// The amount, in the asset's atomic units, as a decimal string.
    pub amount: String,
    /// The token contract.
    pub asset: String,
    /// The payee.
    #[serde(rename = "payTo")]
    pub pay_to: String,
    /// How long the client has to pay, in seconds.
    #[serde(rename = "maxTimeoutSeconds")]
    pub max_timeout_seconds: u32,
    /// Scheme data: for `exact` on EVM, the asset's EIP-712 domain, and the
    /// reserved `assetTransferMethod` and `paymentFlow` keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

/// The x402 cart document: the resource the client is buying, the payment
/// requirement it accepted, and the extensions it echoes — the
/// `PaymentPayload` without its `payload`. Nothing else is accepted at the
/// top level: a cart that carried the signed payment would hash the
/// signature, so a re-signed retry would be a new purchase, and the
/// binding nonce derived from the cart could never be inside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct X402Cart {
    /// The protocol version. Always 2.
    #[serde(rename = "x402Version")]
    pub x402_version: u8,
    /// What is being bought.
    pub resource: ResourceInfo,
    /// How it is being paid for.
    pub accepted: PaymentRequirements,
    /// Protocol extensions, as the server advertised them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_in_one_case_or_checksummed_and_print_lower() {
        let mixed: Address = "0x209693Bc6afc0C5328bA36FaF03C514EF312287C"
            .parse()
            .unwrap();
        let lower: Address = "0x209693bc6afc0c5328ba36faf03c514ef312287c"
            .parse()
            .unwrap();
        let upper: Address = "0x209693BC6AFC0C5328BA36FAF03C514EF312287C"
            .parse()
            .unwrap();
        assert_eq!(mixed, lower);
        assert_eq!(upper, lower);
        assert_eq!(
            mixed.to_string(),
            "0x209693bc6afc0c5328ba36faf03c514ef312287c"
        );
        for bad in [
            "209693bc6afc0c5328ba36faf03c514ef312287c",
            "0x209693bc6afc0c5328ba36faf03c514ef312287",
            "0x209693bc6afc0c5328ba36faf03c514ef312287cc",
            "0xZ09693bc6afc0c5328ba36faf03c514ef312287c",
            "",
        ] {
            assert!(bad.parse::<Address>().is_err(), "{bad}");
        }
    }

    /// The four examples in EIP-55 itself, and each with one letter's case
    /// flipped.
    #[test]
    fn eip55_checksums_are_enforced_on_mixed_case() {
        for good in [
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
            "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
            "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
        ] {
            assert!(Address::parse(good).is_ok(), "{good}");
            let i = good[2..].find(|c: char| c.is_ascii_alphabetic()).unwrap() + 2;
            let mut typo = good.to_owned();
            let flipped = if good.as_bytes()[i].is_ascii_uppercase() {
                good[i..=i].to_ascii_lowercase()
            } else {
                good[i..=i].to_ascii_uppercase()
            };
            typo.replace_range(i..=i, &flipped);
            assert_eq!(
                Address::parse(&typo),
                Err("mixed case, but not its EIP-55 checksum"),
                "{typo}"
            );
        }
    }

    #[test]
    fn chain_ids_are_eip155_and_canonical() {
        assert_eq!(
            "eip155:8453".parse::<ChainId>().unwrap(),
            ChainId::new(8453)
        );
        assert_eq!(ChainId::new(84532).to_string(), "eip155:84532");
        for bad in [
            "base",
            "eip155:",
            "eip155:08453",
            "eip155:0",
            "solana:5eykt",
            "eip155:-1",
            "EIP155:1",
        ] {
            assert!(
                matches!(bad.parse::<ChainId>(), Err(AdapterError::Unsupported(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn an_account_names_its_chain() {
        let address = Address::parse("0x209693Bc6afc0C5328bA36FaF03C514EF312287C").unwrap();
        assert_eq!(
            account_id(ChainId::new(8453), address),
            "eip155:8453:0x209693bc6afc0c5328ba36faf03c514ef312287c"
        );
        assert_ne!(
            account_id(ChainId::new(8453), address),
            account_id(ChainId::new(84532), address)
        );
    }
}
