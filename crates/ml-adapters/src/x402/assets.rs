//! Which tokens the adapter can price, and how.
//!
//! x402 quotes amounts in a token's atomic units and names the token by its
//! contract address. Turning that into money takes the token's decimals and
//! symbol; verifying a payment for it, later, takes the EIP-712 domain its
//! contract signs under. None of that is on the wire, and none of it is
//! guessed: a token the registry does not know is refused.

use super::types::{Address, ChainId};
use ml_core::Currency;
use std::collections::HashMap;

/// Base mainnet.
pub const BASE: ChainId = ChainId::new(8453);
/// Base Sepolia, the testnet.
pub const BASE_SEPOLIA: ChainId = ChainId::new(84532);

/// A token the adapter can price.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    /// The symbol the amount is expressed in: the money's currency.
    pub symbol: Currency,
    /// Atomic units per whole token, as a power of ten.
    pub decimals: u8,
    /// The EIP-712 domain name the contract signs under.
    pub domain_name: String,
    /// The EIP-712 domain version the contract signs under.
    pub domain_version: String,
}

/// The tokens known on each chain.
#[derive(Clone, Debug, Default)]
pub struct AssetRegistry {
    assets: HashMap<(ChainId, Address), Asset>,
}

impl AssetRegistry {
    /// A registry that knows no token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Know `asset` at `contract` on `chain`.
    #[must_use]
    pub fn with_asset(mut self, chain: ChainId, contract: Address, asset: Asset) -> Self {
        self.assets.insert((chain, contract), asset);
        self
    }

    /// USDC on Base mainnet and on Base Sepolia, with the addresses and
    /// EIP-712 domains the x402 reference implementation lists.
    #[must_use]
    // The literals below are constants a test checks; `expect` cannot fire.
    #[allow(clippy::missing_panics_doc)]
    pub fn base() -> Self {
        let usdc = |name: &str| Asset {
            symbol: Currency::new("USDC").expect("USDC is a currency code"),
            decimals: 6,
            domain_name: name.to_owned(),
            domain_version: "2".to_owned(),
        };
        let address = |s: &str| s.parse().expect("a well-formed address");
        Self::new()
            .with_asset(
                BASE,
                address("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
                usdc("USD Coin"),
            )
            .with_asset(
                BASE_SEPOLIA,
                address("0x036CbD53842c5426634e7929541eC2318f3dCF7e"),
                usdc("USDC"),
            )
    }

    /// The token at `contract` on `chain`, if known.
    #[must_use]
    pub fn get(&self, chain: ChainId, contract: Address) -> Option<&Asset> {
        self.assets.get(&(chain, contract))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_knows_usdc_on_both_chains_and_nothing_else() {
        let registry = AssetRegistry::base();
        let mainnet: Address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
            .parse()
            .unwrap();
        let sepolia: Address = "0x036CbD53842c5426634e7929541eC2318f3dCF7e"
            .parse()
            .unwrap();
        assert_eq!(registry.get(BASE, mainnet).unwrap().domain_name, "USD Coin");
        assert_eq!(
            registry.get(BASE_SEPOLIA, sepolia).unwrap().domain_name,
            "USDC"
        );
        assert_eq!(registry.get(BASE, sepolia), None);
        assert_eq!(registry.get(ChainId::new(1), mainnet), None);
    }
}
