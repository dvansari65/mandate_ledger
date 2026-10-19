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
///
/// On one chain a symbol names one token. A mandate budgets money by
/// symbol and the merchant id names the chain, so two tokens sharing a
/// symbol on one chain could not be told apart: a payment in one would
/// pass for a cart priced in the other.
#[derive(Clone, Debug, Default)]
pub struct AssetRegistry {
    assets: HashMap<(ChainId, Address), Asset>,
}

/// Why a token cannot join a registry.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// The contract is already known on that chain.
    #[error("{contract} is already registered on {chain}")]
    DuplicateContract {
        /// The chain.
        chain: ChainId,
        /// The contract.
        contract: Address,
    },
    /// Another contract on that chain already has the symbol.
    #[error("{symbol} on {chain} is already {existing}")]
    DuplicateSymbol {
        /// The chain.
        chain: ChainId,
        /// The symbol.
        symbol: String,
        /// The contract that has it.
        existing: Address,
    },
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

    /// Know `asset` at `contract` on `chain`, unless the contract is
    /// already known there or another token there has its symbol.
    pub fn with_asset(
        mut self,
        chain: ChainId,
        contract: Address,
        asset: Asset,
    ) -> Result<Self, RegistryError> {
        if self.assets.contains_key(&(chain, contract)) {
            return Err(RegistryError::DuplicateContract { chain, contract });
        }
        if let Some((&(_, existing), _)) = self
            .assets
            .iter()
            .find(|((c, _), known)| *c == chain && known.symbol == asset.symbol)
        {
            return Err(RegistryError::DuplicateSymbol {
                chain,
                symbol: asset.symbol.as_str().to_owned(),
                existing,
            });
        }
        self.assets.insert((chain, contract), asset);
        Ok(self)
    }

    /// USDC on Base mainnet, as the x402 reference implementation lists it
    /// (`typescript/packages/mechanisms/evm/src/defaultAssets.ts`).
    #[must_use]
    // The literals are constants a test checks; `expect` cannot fire.
    #[allow(clippy::missing_panics_doc)]
    pub fn base() -> Self {
        Self::new()
            .with_asset(
                BASE,
                address("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
                usdc("USD Coin"),
            )
            .expect("one token in an empty registry")
    }

    /// USDC on Base Sepolia, the testnet, as the reference implementation
    /// lists it. For sandboxes and tests; never alongside [`Self::base`].
    #[must_use]
    #[allow(clippy::missing_panics_doc)]
    pub fn base_sepolia() -> Self {
        Self::new()
            .with_asset(
                BASE_SEPOLIA,
                address("0x036CbD53842c5426634e7929541eC2318f3dCF7e"),
                usdc("USDC"),
            )
            .expect("one token in an empty registry")
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

    #[test]
    fn a_chain_has_one_token_per_symbol_and_one_entry_per_contract() {
        let mainnet = address("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
        let other = address("0xd9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca");
        // Another token calling itself USDC on Base.
        assert_eq!(
            AssetRegistry::base()
                .with_asset(BASE, other, usdc("USD Base Coin"))
                .unwrap_err(),
            RegistryError::DuplicateSymbol {
                chain: BASE,
                symbol: "USDC".to_owned(),
                existing: mainnet,
            }
        );
        assert_eq!(
            AssetRegistry::base()
                .with_asset(BASE, mainnet, usdc("USD Coin"))
                .unwrap_err(),
            RegistryError::DuplicateContract {
                chain: BASE,
                contract: mainnet,
            }
        );
        // The same symbol on another chain is another merchant's money,
        // told apart by the chain in the merchant id.
        assert!(
            AssetRegistry::base()
                .with_asset(BASE_SEPOLIA, other, usdc("USDC"))
                .is_ok()
        );
    }
}
