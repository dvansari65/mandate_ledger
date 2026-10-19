//! EIP-3009 `transferWithAuthorization`, as USDC verifies it: the EIP-712
//! digest a payer signs, the signer it recovers to, and the nonce that
//! binds a payment to the ledger's context.
//!
//! Recovery follows the token contract's own rules (Circle's
//! `ECRecover.sol`): a 65-byte `r ‖ s ‖ v` signature, `v` of 27 or 28, `s`
//! in the lower half of the curve order. A signature the contract would
//! reject is rejected here, so nothing is recorded as paid that cannot
//! settle. Contract wallets (EIP-1271, ERC-6492) are verified on chain and
//! are not accepted by this offline check.

use super::assets::Asset;
use super::types::{Address, ChainId};
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use k256::elliptic_curve::scalar::IsHigh as _;
use ml_core::ContextId;
use sha3::{Digest, Keccak256};

const DOMAIN_TYPE: &str =
    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";
const TRANSFER_TYPE: &str = "TransferWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)";

/// The tag a binding nonce is derived under. Versioned, so the derivation
/// can change without one nonce ever meaning two things.
const NONCE_TAG: &str = "mandate-ledger:x402-nonce:v1:";

/// What a payer signs: EIP-3009's `TransferWithAuthorization`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Authorization {
    /// The payer.
    pub from: Address,
    /// The payee.
    pub to: Address,
    /// The amount, in atomic units.
    pub value: u128,
    /// Valid strictly after this Unix time.
    pub valid_after: u64,
    /// Valid strictly before this Unix time.
    pub valid_before: u64,
    /// Single use per payer and token.
    pub nonce: [u8; 32],
}

fn keccak(bytes: &[u8]) -> [u8; 32] {
    Keccak256::digest(bytes).into()
}

/// One ABI word: a value right-aligned in 32 bytes.
fn word(value: &[u8]) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[32 - value.len()..].copy_from_slice(value);
    word
}

/// The digest the payer signed: `keccak256(0x1901 ‖ domain ‖ struct)`,
/// with the token's EIP-712 domain on `chain` at `contract`.
#[must_use]
pub fn digest(chain: ChainId, contract: Address, token: &Asset, auth: &Authorization) -> [u8; 32] {
    let domain: Vec<u8> = [
        keccak(DOMAIN_TYPE.as_bytes()),
        keccak(token.domain_name.as_bytes()),
        keccak(token.domain_version.as_bytes()),
        word(&chain.get().to_be_bytes()),
        word(contract.as_bytes()),
    ]
    .concat();
    let message: Vec<u8> = [
        keccak(TRANSFER_TYPE.as_bytes()),
        word(auth.from.as_bytes()),
        word(auth.to.as_bytes()),
        word(&auth.value.to_be_bytes()),
        word(&auth.valid_after.to_be_bytes()),
        word(&auth.valid_before.to_be_bytes()),
        auth.nonce,
    ]
    .concat();
    let mut preimage = Vec::with_capacity(66);
    preimage.extend_from_slice(&[0x19, 0x01]);
    preimage.extend_from_slice(&keccak(&domain));
    preimage.extend_from_slice(&keccak(&message));
    keccak(&preimage)
}

/// The address that signed `digest`, under the token contract's rules.
pub fn recover(digest: &[u8; 32], signature: &[u8; 65]) -> Result<Address, &'static str> {
    let recovery = match signature[64] {
        27 => 0,
        28 => 1,
        _ => return Err("signature v must be 27 or 28"),
    };
    let signature =
        Signature::from_slice(&signature[..64]).map_err(|_| "signature r or s is out of range")?;
    if bool::from(signature.s().is_high()) {
        return Err("signature s is not in the lower half of the curve order");
    }
    let recovery = RecoveryId::from_byte(recovery).ok_or("signature v must be 27 or 28")?;
    let key = VerifyingKey::recover_from_prehash(digest, &signature, recovery)
        .map_err(|_| "signature does not recover to a key")?;
    let point = key.to_sec1_point(false);
    let hash = keccak(&point.as_bytes()[1..]);
    let mut address = [0u8; 20];
    address.copy_from_slice(&hash[12..]);
    Ok(Address::from_bytes(address))
}

/// The EIP-3009 nonce that binds a payment to `ctx`:
/// `keccak256("mandate-ledger:x402-nonce:v1:" ‖ ctx)`, over UTF-8. A
/// payer that uses it signs, along with the payee and the amount, which
/// purchase the payment is for — the one thing x402 does not carry.
#[must_use]
pub fn binding_nonce(ctx: &ContextId) -> [u8; 32] {
    keccak(format!("{NONCE_TAG}{ctx}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::x402::AssetRegistry;
    use crate::x402::assets::BASE_SEPOLIA;

    fn hex32(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        hex::decode_to_slice(s.trim_start_matches("0x"), &mut out).unwrap();
        out
    }

    fn address(s: &str) -> Address {
        Address::parse(s).unwrap()
    }

    fn usdc_sepolia() -> (Address, Asset) {
        let contract = address("0x036CbD53842c5426634e7929541eC2318f3dCF7e");
        let token = AssetRegistry::base_sepolia()
            .get(BASE_SEPOLIA, contract)
            .unwrap()
            .clone();
        (contract, token)
    }

    /// The example payload in the x402 specification
    /// (`specs/schemes/exact/scheme_exact_evm.md`), whose signature is
    /// genuine: foundry's `cast wallet verify` accepts it.
    fn specification_example() -> (Authorization, [u8; 65]) {
        let auth = Authorization {
            from: address("0x857b06519E91e3A54538791bDbb0E22373e36b66"),
            to: address("0x209693Bc6afc0C5328bA36FaF03C514EF312287C"),
            value: 10_000,
            valid_after: 1_740_672_089,
            valid_before: 1_740_672_154,
            nonce: hex32("0xf3746613c2d920b5fdabc0856f2aeb2d4f88ee6037b8cc5d04a71a4462f13480"),
        };
        let mut signature = [0u8; 65];
        hex::decode_to_slice(
            "2d6a7588d6acca505cbf0d9a4a227e0c52c6c34008c8e8986a1283259764173608a2ce6496642e377d6da8dbbf5836e9bd15092f9ecab05ded3d6293af148b571c",
            &mut signature,
        )
        .unwrap();
        (auth, signature)
    }

    #[test]
    fn the_type_hashes_are_the_tokens() {
        // As foundry's `cast keccak` computes them; the first is USDC's
        // TRANSFER_WITH_AUTHORIZATION_TYPEHASH.
        assert_eq!(
            keccak(TRANSFER_TYPE.as_bytes()),
            hex32("0x7c7c6cdb67a18743f49ec6fa9b35f50d52ed05cbed4cc592e13b44501c1a2267")
        );
        assert_eq!(
            keccak(DOMAIN_TYPE.as_bytes()),
            hex32("0x8b73c3c69bb8fe3d512ecc4cf759cc79239f7b179b0ffacaa9a75d522b39400f")
        );
    }

    #[test]
    fn the_specification_example_recovers_to_its_payer() {
        let (contract, token) = usdc_sepolia();
        let (auth, signature) = specification_example();
        let digest = digest(BASE_SEPOLIA, contract, &token, &auth);
        assert_eq!(recover(&digest, &signature), Ok(auth.from));
        // Any change to what was signed recovers someone else.
        let other = Authorization {
            value: 10_001,
            ..auth
        };
        let digest = super::digest(BASE_SEPOLIA, contract, &token, &other);
        assert_ne!(recover(&digest, &signature), Ok(auth.from));
    }

    #[test]
    fn signatures_the_token_would_reject_are_rejected() {
        let (contract, token) = usdc_sepolia();
        let (auth, signature) = specification_example();
        let digest = digest(BASE_SEPOLIA, contract, &token, &auth);
        // v of 0 or 1, as some signers write it.
        let mut low_v = signature;
        low_v[64] -= 27;
        assert_eq!(
            recover(&digest, &low_v),
            Err("signature v must be 27 or 28")
        );
        // The same signature with s mirrored into the upper half of the
        // order and v flipped: valid secp256k1, rejected by the contract.
        let s = Signature::from_slice(&signature[..64]).unwrap();
        let mirrored = Signature::from_scalars(s.r(), -*s.s()).unwrap();
        let mut high_s = [0u8; 65];
        high_s[..64].copy_from_slice(&mirrored.to_bytes());
        high_s[64] = if signature[64] == 27 { 28 } else { 27 };
        assert_eq!(
            recover(&digest, &high_s),
            Err("signature s is not in the lower half of the curve order")
        );
        assert!(recover(&digest, &[0u8; 65]).is_err());
    }

    #[test]
    fn the_binding_nonce_is_keccak_over_the_tag_and_the_context() {
        // `cast keccak "mandate-ledger:x402-nonce:v1:ctx_00112233445566778899aabbccddeeff"`
        let ctx = ContextId::new("ctx_00112233445566778899aabbccddeeff").unwrap();
        assert_eq!(binding_nonce(&ctx), hex32(BINDING_NONCE_VECTOR));
    }

    const BINDING_NONCE_VECTOR: &str =
        "0xc5154956a3a22b450a6d336134e247ebfce3273d357363897538f0ea9de9f082";
}
