//! Which tokens the adapter can price, and how.
//!
//! x402 quotes amounts in a token's atomic units and names the token by its
//! contract address. Turning that into money takes the token's decimals and
//! symbol; verifying a payment for it, later, takes the EIP-712 domain its
//! contract signs under. None of that is on the wire, and none of it is
//! guessed: a token the registry does not know is refused.
//!
//! A registry is one deployment's choice of networks. Mainnet and testnet
//! come separately, and a production registry holds no testnet token: the
//! money a mandate budgets is the token's symbol, and USDC on a testnet is
//! not USDC.

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

/// USDC: six decimals, EIP-712 domain `name` version 2.
fn usdc(domain_name: &str) -> Asset {
    Asset {
        symbol: Currency::new("USDC").expect("USDC is a currency code"),
        decimals: 6,
        domain_name: domain_name.to_owned(),
        domain_version: "2".to_owned(),
    }
}

fn address(hex: &str) -> Address {
    Address::parse(hex).expect("a checksummed address")
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

    /// USDC on Base mainnet, as the x402 reference implementation lists it
    /// (`typescript/packages/mechanisms/evm/src/defaultAssets.ts`).
    #[must_use]
    // The literals are constants a test checks; `expect` cannot fire.
    #[allow(clippy::missing_panics_doc)]
    pub fn base() -> Self {
        Self::new().with_asset(
            BASE,
            address("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            usdc("USD Coin"),
        )
    }

    /// USDC on Base Sepolia, the testnet, as the reference implementation
    /// lists it. For sandboxes and tests; never alongside [`Self::base`].
    #[must_use]
    #[allow(clippy::missing_panics_doc)]
    pub fn base_sepolia() -> Self {
        Self::new().with_asset(
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
    fn mainnet_and_testnet_registries_are_separate() {
        let mainnet = address("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
        let sepolia = address("0x036CbD53842c5426634e7929541eC2318f3dCF7e");
        let base = AssetRegistry::base();
        assert_eq!(base.get(BASE, mainnet).unwrap().domain_name, "USD Coin");
        assert_eq!(base.get(BASE_SEPOLIA, sepolia), None);
        let testnet = AssetRegistry::base_sepolia();
        assert_eq!(
            testnet.get(BASE_SEPOLIA, sepolia).unwrap().domain_name,
            "USDC"
        );
        assert_eq!(testnet.get(BASE, mainnet), None);
        assert_eq!(base.get(ChainId::new(1), mainnet), None);
    }
}
