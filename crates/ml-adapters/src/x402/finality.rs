//! What a host hands the x402 rail to show a payment settled: the
//! facilitator's answer, or the settlement transaction's receipt.

use super::types::Address;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

/// Finality evidence for one x402 payment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum X402Finality {
    /// The facilitator's answer to `POST /settle`. It does not identify
    /// the authorization it settled, so it is only as good as the
    /// facilitator and the host that pairs it with this payment.
    Facilitator {
        /// x402's `SettlementResponse`, as the facilitator sent it.
        response: SettlementResponse,
    },
    /// The settlement transaction's receipt, as `eth_getTransactionReceipt`
    /// returns it, with the chain head it was read against. It names the
    /// authorization it executed, so the rail checks it shows exactly this
    /// payment.
    Receipt {
        /// The chain the receipt is from, CAIP-2.
        network: String,
        /// The latest block number when the receipt was read.
        head: u64,
        /// The receipt.
        receipt: TransactionReceipt,
    },
}

/// x402's `SettlementResponse`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementResponse {
    /// Whether the facilitator settled the payment.
    pub success: bool,
    /// Why not, when it did not.
    #[serde(
        default,
        rename = "errorReason",
        skip_serializing_if = "Option::is_none"
    )]
    pub error_reason: Option<String>,
    /// The payer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    /// The transaction hash; empty when nothing was broadcast.
    pub transaction: String,
    /// The chain, CAIP-2.
    pub network: String,
    /// The amount settled, in atomic units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    /// Protocol extensions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Value>,
}

/// The fields of a JSON-RPC transaction receipt the rail reads. Others are
/// ignored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionReceipt {
    /// The transaction hash.
    #[serde(rename = "transactionHash")]
    pub transaction_hash: String,
    /// The block it was included in, a hex quantity.
    #[serde(rename = "blockNumber")]
    pub block_number: String,
    /// `0x1` if it succeeded, `0x0` if it reverted.
    pub status: String,
    /// The events it emitted.
    pub logs: Vec<Log>,
}

/// One event in a receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Log {
    /// The contract that emitted it.
    pub address: String,
    /// The event signature, then its indexed arguments.
    pub topics: Vec<String>,
    /// Its other arguments, ABI-encoded.
    pub data: String,
}

/// A JSON-RPC quantity: `0x` and up to 16 hex digits.
pub(super) fn quantity(value: &str) -> Option<u64> {
    let hex = value.strip_prefix("0x")?;
    if hex.is_empty() || hex.len() > 16 {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

fn word(value: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    hex::decode_to_slice(value.strip_prefix("0x")?, &mut out).ok()?;
    Some(out)
}

/// An address in an indexed topic: twelve zero bytes, then the address.
fn topic_address(topic: &str) -> Option<Address> {
    let word = word(topic)?;
    let (zeros, address) = word.split_at(12);
    if zeros.iter().any(|b| *b != 0) {
        return None;
    }
    let mut bytes = [0u8; 20];
    bytes.copy_from_slice(address);
    Some(Address::from_bytes(bytes))
}

fn event(signature: &str) -> [u8; 32] {
    Keccak256::digest(signature.as_bytes()).into()
}

impl Log {
    fn emitted_by(&self, contract: Address) -> bool {
        Address::parse(&self.address).is_ok_and(|a| a == contract)
    }

    fn is(&self, signature: &str, arguments: usize) -> bool {
        self.topics.len() == 1 + arguments
            && word(&self.topics[0]).is_some_and(|t| t == event(signature))
    }

    /// `AuthorizationUsed(authorizer, nonce)` from `token`: EIP-3009's
    /// record that this exact authorization executed.
    pub(super) fn authorization_used(&self, token: Address) -> Option<(Address, [u8; 32])> {
        if !self.emitted_by(token) || !self.is("AuthorizationUsed(address,bytes32)", 2) {
            return None;
        }
        Some((topic_address(&self.topics[1])?, word(&self.topics[2])?))
    }

    /// `Transfer(from, to, value)` from `token`, with a value that fits
    /// the range the rail handles.
    pub(super) fn transfer(&self, token: Address) -> Option<(Address, Address, u128)> {
        if !self.emitted_by(token) || !self.is("Transfer(address,address,uint256)", 2) {
            return None;
        }
        let value = word(&self.data)?;
        let (high, low) = value.split_at(16);
        if high.iter().any(|b| *b != 0) {
            return None;
        }
        let value = u128::from_be_bytes(low.try_into().ok()?);
        Some((
            topic_address(&self.topics[1])?,
            topic_address(&self.topics[2])?,
            value,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_signatures_are_the_tokens() {
        // As foundry's `cast sig-event` computes them.
        assert_eq!(
            hex::encode(event("Transfer(address,address,uint256)")),
            "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
        );
        assert_eq!(
            hex::encode(event("AuthorizationUsed(address,bytes32)")),
            "98de503528ee59b575ef0c0a2576a82497bfc029a5685b209e9ec333479b10a5"
        );
    }

    #[test]
    fn quantities_are_hex() {
        assert_eq!(quantity("0x0"), Some(0));
        assert_eq!(quantity("0x1b4"), Some(436));
        for bad in ["", "0x", "1b4", "0xzz", "0x10000000000000000"] {
            assert_eq!(quantity(bad), None, "{bad:?}");
        }
    }
}
