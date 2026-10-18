//! The wire types this adapter reads, as x402 v2 defines them, and the two
//! identifiers it parses strictly.

use ml_core::AdapterError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// An EVM account address: twenty bytes, written as `0x` and forty hex
/// digits. Parsed from either case; written in lower case.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Address([u8; 20]);

impl Address {
    /// The raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

impl FromStr for Address {
    type Err = AdapterError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let malformed =
            || AdapterError::Malformed("not an EVM address: 0x and 40 hex digits".into());
        let hex = s.strip_prefix("0x").ok_or_else(malformed)?;
        if hex.len() != 40 {
            return Err(malformed());
        }
        let mut bytes = [0u8; 20];
        hex::decode_to_slice(hex, &mut bytes).map_err(|_| malformed())?;
        Ok(Self(bytes))
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

/// The x402 cart document: the resource the client is buying and the
/// payment requirement it accepted, named as the x402 `PaymentPayload`
/// names them, so a client builds it from the 402 it received.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct X402Cart {
    /// The protocol version. Always 2.
    #[serde(rename = "x402Version")]
    pub x402_version: u8,
    /// What is being bought.
    pub resource: ResourceInfo,
    /// How it is being paid for.
    pub accepted: PaymentRequirements,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_from_either_case_and_print_lower() {
        let mixed: Address = "0x209693Bc6afc0C5328bA36FaF03C514EF312287C"
            .parse()
            .unwrap();
        let lower: Address = "0x209693bc6afc0c5328ba36faf03c514ef312287c"
            .parse()
            .unwrap();
        assert_eq!(mixed, lower);
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
}
