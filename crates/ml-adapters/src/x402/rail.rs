//! The x402 rail: verifies an EIP-3009 payment offline, and judges the
//! evidence that it settled.
//!
//! It reports what a payment proves and lets the engine compare. The payee
//! is `authorization.to` on the payment's chain, so a redirect is the
//! engine's `MERCHANT_BINDING_MISMATCH`; the amount is `authorization.value`
//! in the token's money, so a short payment is `AMOUNT_MISMATCH`; and the
//! context is bound only when the signed nonce is the context's
//! [binding nonce](super::binding_nonce), so any other payment is
//! `UNBOUND_PROOF`. Nothing unsigned binds: `accepted` and `resource` are
//! the client's word, and a cart hash over them would prove nothing.
//!
//! What it refuses itself, as `PROOF_INVALID`, is a payment no facilitator
//! would settle: a signature that does not recover to the payer under the
//! token's own rules, an authorization to anyone but the `payTo` it claims
//! to meet or for any other amount, or one outside its validity window.

use super::assets::AssetRegistry;
use super::cart::X402CartAdapter;
use super::eip3009::{binding_nonce, digest, recover};
use super::fields::{bytes, money, uint};
use super::finality::{X402Finality, quantity};
use super::payment::PaymentPayload;
use super::types::{Address, ChainId, account_id};
use ml_core::{
    Clock, FinalityStatus, MerchantId, PaymentExpectation, Rail, RailError, SettlementExpectation,
    SystemClock, VerifiedProof,
};
use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;
use std::time::Duration;

/// How long before `validBefore` a payment must still have, in seconds,
/// for a facilitator to settle it: the reference facilitator's own margin
/// for a block to land.
const SETTLEMENT_MARGIN_SECS: i128 = 6;

/// When the rail calls a payment final, or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FinalityPolicy {
    /// Confirmations a receipt needs, counting its own block.
    pub confirmations: NonZeroU32,
    /// Whether the facilitator's word settles it: its success is final,
    /// and its failure is failure once the authorization has expired.
    /// Otherwise its answers are pending until a receipt arrives.
    pub trust_facilitator: bool,
    /// How long after `validBefore`, by this host's clock, a payment a
    /// trusted facilitator failed to settle is taken as unable to settle.
    /// The token checks `validBefore` against the chain's block time, which
    /// can trail this clock — an L2 catching up after downtime stamps its
    /// blocks behind real time — so the grace is the operator's bound on
    /// how far.
    pub expiry_grace: Duration,
}

/// One EIP-3009 authorization, the way the chain keys it — the payer's
/// nonce on one token contract on one chain — and when it expires. Written
/// `eip155:<chain>:<token>:<payer>:<nonce>:<validBefore>`, it is the
/// rail's payment reference; without the expiry, it is the payment's
/// single-use nonce, since the chain spends a nonce once whatever else the
/// payer signed with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthorizationRef {
    /// The chain.
    pub chain: ChainId,
    /// The token contract.
    pub token: Address,
    /// The payer.
    pub from: Address,
    /// The authorization's nonce.
    pub nonce: [u8; 32],
    /// The Unix time it can be executed until, exclusive.
    pub valid_before: u64,
}

impl AuthorizationRef {
    /// The authorization as the chain keys it: what a payment spends once.
    #[must_use]
    pub fn identity(&self) -> String {
        format!(
            "{}:{}:{}:0x{}",
            self.chain,
            self.token,
            self.from,
            hex::encode(self.nonce)
        )
    }
}

impl fmt::Display for AuthorizationRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.identity(), self.valid_before)
    }
}

impl FromStr for AuthorizationRef {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wrong = || format!("`{s}` is not an x402 payment reference");
        let parts: Vec<&str> = s.split(':').collect();
        let ["eip155", chain, token, from, nonce, valid_before] = parts[..] else {
            return Err(wrong());
        };
        let parsed = Self {
            chain: format!("eip155:{chain}").parse().map_err(|_| wrong())?,
            token: Address::parse(token).map_err(|_| wrong())?,
            from: Address::parse(from).map_err(|_| wrong())?,
            nonce: bytes("nonce", nonce).map_err(|_| wrong())?,
            valid_before: valid_before.parse().map_err(|_| wrong())?,
        };
        // One payment, one spelling.
        if parsed.to_string() != s {
            return Err(wrong());
        }
        Ok(parsed)
    }
}

/// The x402 rail.
pub struct X402Rail<C = SystemClock> {
    id: String,
    terms: X402CartAdapter,
    policy: FinalityPolicy,
    clock: C,
}

fn invalid(what: impl fmt::Display) -> RailError {
    RailError::InvalidProof(what.to_string())
}

impl<C: Clock> X402Rail<C> {
    /// A rail named `id` — recorded with every payment — that knows the
    /// tokens in `registry`, settles by `policy`, and reads the time from
    /// `clock`, which should be the ledger's.
    pub fn new(
        id: impl Into<String>,
        registry: AssetRegistry,
        policy: FinalityPolicy,
        clock: C,
    ) -> Self {
        Self {
            id: id.into(),
            terms: X402CartAdapter::new(registry),
            policy,
            clock,
        }
    }

    fn final_or_pending(&self, transaction: &str, confirmations: u64) -> FinalityStatus {
        let needed = self.policy.confirmations.get();
        if confirmations >= u64::from(needed) {
            FinalityStatus::Final {
                reference: transaction.to_ascii_lowercase(),
            }
        } else {
            FinalityStatus::Pending {
                reason: format!("{confirmations} of {needed} confirmations"),
            }
        }
    }

    fn judge_facilitator(
        &self,
        payment: &AuthorizationRef,
        expected: &SettlementExpectation<'_>,
        response: &super::finality::SettlementResponse,
    ) -> Result<FinalityStatus, RailError> {
        if response.network != payment.chain.to_string() {
            return Err(invalid("the settlement response is for another network"));
        }
        if let Some(payer) = &response.payer {
            if Address::parse(payer).ok() != Some(payment.from) {
                return Err(invalid("the settlement response is for another payer"));
            }
        }
        if let Some(amount) = &response.amount {
            let token = self.token(payment)?;
            let settled = uint("amount", amount)
                .and_then(|atomic| money(atomic, token))
                .map_err(invalid)?;
            if settled != *expected.amount {
                return Err(invalid(format!(
                    "the facilitator settled {settled}, the payment is {}",
                    expected.amount
                )));
            }
        }
        if response.success {
            bytes::<32>("transaction", &response.transaction).map_err(invalid)?;
            return Ok(if self.policy.trust_facilitator {
                FinalityStatus::Final {
                    reference: response.transaction.to_ascii_lowercase(),
                }
            } else {
                FinalityStatus::Pending {
                    reason: format!(
                        "the facilitator reports {} settled; this rail needs its receipt",
                        response.transaction
                    ),
                }
            });
        }
        let reason = response
            .error_reason
            .as_deref()
            .unwrap_or("settlement failed");
        // The specification's one non-terminal failure: broadcast, but not
        // seen to confirm. The transaction may still land.
        if reason == "settlement_pending" || !self.policy.trust_facilitator {
            return Ok(FinalityStatus::Pending {
                reason: format!("the facilitator reports {reason}"),
            });
        }
        // A failed settlement spends nothing. Whoever holds the signed
        // authorization can still execute it until it expires, and until
        // then the reservation it holds must stand.
        let grace = i128::from(self.policy.expiry_grace.as_secs());
        let now = i128::from(self.clock.now().as_secs());
        if now < i128::from(payment.valid_before) + grace {
            return Ok(FinalityStatus::Pending {
                reason: format!(
                    "the facilitator reports {reason}; the authorization can still be \
                     settled until {} and the grace after it",
                    payment.valid_before
                ),
            });
        }
        Ok(FinalityStatus::Failed {
            reason: reason.to_owned(),
        })
    }

    fn judge_receipt(
        &self,
        payment: &AuthorizationRef,
        expected: &SettlementExpectation<'_>,
        head: u64,
        receipt: &super::finality::TransactionReceipt,
    ) -> Result<FinalityStatus, RailError> {
        bytes::<32>("transactionHash", &receipt.transaction_hash).map_err(invalid)?;
        let block = quantity(&receipt.block_number)
            .ok_or_else(|| invalid("blockNumber: not a hex quantity"))?;
        let confirmations = head
            .checked_sub(block)
            .map(|behind| behind + 1)
            .ok_or_else(|| invalid("the head is older than the receipt's block"))?;
        match receipt.status.as_str() {
            "0x1" => {}
            // A reverted settlement spends nothing: the authorization can
            // still execute until it expires.
            "0x0" => {
                return Ok(FinalityStatus::Pending {
                    reason: format!(
                        "{} reverted; the authorization can still be settled until it expires",
                        receipt.transaction_hash
                    ),
                });
            }
            _ => return Err(invalid("status: not 0x0 or 0x1")),
        }
        let token = self.token(payment)?;
        let used = receipt
            .logs
            .iter()
            .filter_map(|log| log.authorization_used(payment.token))
            .any(|used| used == (payment.from, payment.nonce));
        let paid = receipt
            .logs
            .iter()
            .filter_map(|log| log.transfer(payment.token))
            .any(|(from, to, value)| {
                from == payment.from
                    && MerchantId::new(account_id(payment.chain, to)).ok().as_ref()
                        == Some(expected.merchant)
                    && money(value, token).ok().as_ref() == Some(expected.amount)
            });
        if !(used && paid) {
            return Err(invalid(
                "the receipt does not execute this authorization, paying the merchant this amount",
            ));
        }
        Ok(self.final_or_pending(&receipt.transaction_hash, confirmations))
    }

    fn token(&self, payment: &AuthorizationRef) -> Result<&super::assets::Asset, RailError> {
        self.terms
            .registry()
            .get(payment.chain, payment.token)
            .ok_or_else(|| invalid("the payment's token is not one this rail knows"))
    }
}

impl<C: Clock> Rail for X402Rail<C> {
    type Proof = PaymentPayload;
    type Finality = X402Finality;

    fn id(&self) -> &str {
        &self.id
    }

    fn verify_proof(
        &self,
        proof: &PaymentPayload,
        expected: &PaymentExpectation<'_>,
    ) -> Result<VerifiedProof, RailError> {
        if proof.x402_version != 2 {
            return Err(invalid("x402Version: only 2"));
        }
        let terms = self.terms.terms(&proof.accepted).map_err(invalid)?;
        let (auth, signature) = proof.payload.parse().map_err(invalid)?;

        // What the reference facilitator checks before it will settle,
        // with its error codes.
        if auth.to != terms.pay_to {
            return Err(invalid(
                "invalid_exact_evm_payload_recipient_mismatch: authorization.to is not accepted.payTo",
            ));
        }
        if auth.value != terms.atomic_amount {
            return Err(invalid(
                "invalid_exact_evm_payload_authorization_value_mismatch: authorization.value is not accepted.amount",
            ));
        }
        let signer = recover(
            &digest(terms.chain, terms.asset, &terms.token, &auth),
            &signature,
        )
        .map_err(|why| invalid(format!("invalid_exact_evm_payload_signature: {why}")))?;
        if signer != auth.from {
            return Err(invalid(
                "invalid_exact_evm_payload_signature: not signed by authorization.from",
            ));
        }
        let payment = AuthorizationRef {
            chain: terms.chain,
            token: terms.asset,
            from: auth.from,
            nonce: auth.nonce,
            valid_before: auth.valid_before,
        };
        let reference = payment.to_string();

        // Freshness is for a payment being recorded. The one this context
        // already recorded is answered again whenever it is presented.
        if expected.recorded_reference != Some(reference.as_str()) {
            let now = i128::from(self.clock.now().as_secs());
            if i128::from(auth.valid_before) < now + SETTLEMENT_MARGIN_SECS {
                return Err(invalid(
                    "invalid_exact_evm_payload_authorization_valid_before: expired, or too close to expiry to settle",
                ));
            }
            if i128::from(auth.valid_after) > now {
                return Err(invalid(
                    "invalid_exact_evm_payload_authorization_valid_after: not yet valid",
                ));
            }
        }

        Ok(VerifiedProof {
            nonce: payment.identity(),
            reference,
            amount: money(auth.value, &terms.token).map_err(invalid)?,
            bound_ctx: (auth.nonce == binding_nonce(expected.ctx)).then(|| expected.ctx.clone()),
            bound_cart: None,
            bound_merchant: Some(
                MerchantId::new(account_id(terms.chain, auth.to)).map_err(invalid)?,
            ),
        })
    }

    fn check_finality(
        &self,
        finality: &X402Finality,
        expected: &SettlementExpectation<'_>,
    ) -> Result<FinalityStatus, RailError> {
        let payment: AuthorizationRef = expected.payment_reference.parse().map_err(invalid)?;
        match finality {
            X402Finality::Facilitator { response } => {
                self.judge_facilitator(&payment, expected, response)
            }
            X402Finality::Receipt {
                network,
                head,
                receipt,
            } => {
                if *network != payment.chain.to_string() {
                    return Err(invalid("the receipt is from another network"));
                }
                self.judge_receipt(&payment, expected, *head, receipt)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::x402::finality::{Log, SettlementResponse, TransactionReceipt};
    use crate::x402::payment::{AuthorizationBody, ExactEvmPayload};
    use crate::x402::{Authorization, BASE_SEPOLIA, PaymentRequirements};
    use k256::ecdsa::SigningKey;
    use ml_core::{ContextId, FixedClock, Hash32, Money};
    use serde_json::json;
    use sha3::{Digest, Keccak256};

    const NOW: i64 = 1_800_000_000;
    const USDC: &str = "0x036cbd53842c5426634e7929541ec2318f3dcf7e";
    const PAY_TO: &str = "0x209693bc6afc0c5328ba36faf03c514ef312287c";
    const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
    const AUTHORIZATION_USED: &str =
        "0x98de503528ee59b575ef0c0a2576a82497bfc029a5685b209e9ec333479b10a5";

    fn key() -> SigningKey {
        SigningKey::from_slice(&[0x5a; 32]).unwrap()
    }

    fn address_of(key: &SigningKey) -> Address {
        let point = key.verifying_key().to_sec1_point(false);
        let hash = Keccak256::digest(&point.as_bytes()[1..]);
        Address::from_bytes(hash[12..].try_into().unwrap())
    }

    fn address(s: &str) -> Address {
        Address::parse(s).unwrap()
    }

    fn ctx() -> ContextId {
        ContextId::new("ctx_00112233445566778899aabbccddeeff").unwrap()
    }

    const GRACE: u64 = 60;

    fn rail_at(trust_facilitator: bool, now: i64) -> X402Rail<FixedClock> {
        X402Rail::new(
            "x402",
            AssetRegistry::base_sepolia(),
            FinalityPolicy {
                confirmations: NonZeroU32::new(3).unwrap(),
                trust_facilitator,
                expiry_grace: Duration::from_secs(GRACE),
            },
            FixedClock::at(now),
        )
    }

    fn rail(trust_facilitator: bool) -> X402Rail<FixedClock> {
        rail_at(trust_facilitator, NOW)
    }

    fn accepted() -> PaymentRequirements {
        serde_json::from_value(json!({
            "scheme": "exact",
            "network": "eip155:84532",
            "amount": "10000",
            "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
            "payTo": "0x209693Bc6afc0C5328bA36FaF03C514EF312287C",
            "maxTimeoutSeconds": 60,
            "extra": {"name": "USDC", "version": "2"}
        }))
        .unwrap()
    }

    /// An authorization of what `accepted()` asks, by `key()`, bound to `ctx()`.
    fn authorization() -> Authorization {
        Authorization {
            from: address_of(&key()),
            to: address(PAY_TO),
            value: 10_000,
            valid_after: 0,
            valid_before: u64::try_from(NOW + 60).unwrap(),
            nonce: binding_nonce(&ctx()),
        }
    }

    fn sign(auth: &Authorization, key: &SigningKey) -> [u8; 65] {
        let token = AssetRegistry::base_sepolia()
            .get(BASE_SEPOLIA, address(USDC))
            .unwrap()
            .clone();
        let digest = digest(BASE_SEPOLIA, address(USDC), &token, auth);
        let (signature, recovery) = key.sign_prehash_recoverable(&digest);
        let mut out = [0u8; 65];
        out[..64].copy_from_slice(&signature.to_bytes());
        out[64] = 27 + recovery.to_byte();
        out
    }

    fn payload(auth: &Authorization, signature: &[u8; 65]) -> PaymentPayload {
        PaymentPayload {
            x402_version: 2,
            resource: None,
            accepted: accepted(),
            payload: ExactEvmPayload {
                signature: format!("0x{}", hex::encode(signature)),
                authorization: AuthorizationBody {
                    from: auth.from.to_string(),
                    to: auth.to.to_string(),
                    value: auth.value.to_string(),
                    valid_after: auth.valid_after.to_string(),
                    valid_before: auth.valid_before.to_string(),
                    nonce: format!("0x{}", hex::encode(auth.nonce)),
                },
            },
            extensions: None,
        }
    }

    fn signed(auth: &Authorization) -> PaymentPayload {
        payload(auth, &sign(auth, &key()))
    }

    fn amount() -> Money {
        Money::parse("0.010000", "USDC").unwrap()
    }

    fn merchant() -> MerchantId {
        MerchantId::new(format!("eip155:84532:{PAY_TO}")).unwrap()
    }

    fn verify_at(
        now: i64,
        proof: &PaymentPayload,
        recorded: Option<&str>,
    ) -> Result<VerifiedProof, RailError> {
        let (ctx, amount, merchant) = (ctx(), amount(), merchant());
        rail_at(true, now).verify_proof(
            proof,
            &PaymentExpectation {
                ctx: &ctx,
                cart_hash: &Hash32::ZERO,
                amount: &amount,
                merchant: &merchant,
                recorded_reference: recorded,
            },
        )
    }

    fn verify(proof: &PaymentPayload) -> Result<VerifiedProof, RailError> {
        verify_at(NOW, proof, None)
    }

    fn refused(proof: &PaymentPayload) -> String {
        match verify(proof) {
            Err(RailError::InvalidProof(why)) => why,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn payment_ref() -> AuthorizationRef {
        AuthorizationRef {
            chain: BASE_SEPOLIA,
            token: address(USDC),
            from: address_of(&key()),
            nonce: binding_nonce(&ctx()),
            valid_before: authorization().valid_before,
        }
    }

    fn reference() -> String {
        payment_ref().to_string()
    }

    #[test]
    fn a_bound_payment_reports_what_it_proves() {
        let verified = verify(&signed(&authorization())).unwrap();
        assert_eq!(verified.reference, reference());
        // The nonce the store spends once is the authorization alone: the
        // chain executes a payer's nonce once, whatever expiry it carried.
        assert_eq!(verified.nonce, payment_ref().identity());
        assert_eq!(
            verified.reference,
            format!("{}:{}", verified.nonce, NOW + 60)
        );
        assert_eq!(verified.amount, amount());
        assert_eq!(verified.amount.to_string(), "0.010000 USDC");
        assert_eq!(verified.bound_ctx, Some(ctx()));
        assert_eq!(verified.bound_cart, None);
        assert_eq!(verified.bound_merchant, Some(merchant()));
    }

    #[test]
    fn any_other_nonce_binds_nothing() {
        let auth = Authorization {
            nonce: [7; 32],
            ..authorization()
        };
        let verified = verify(&signed(&auth)).unwrap();
        assert_eq!(verified.bound_ctx, None);
        assert_eq!(verified.bound_cart, None);
    }

    #[test]
    fn a_payment_no_facilitator_would_settle_is_refused() {
        let auth = authorization();
        // Changed after signing: the signature no longer recovers the payer.
        let mut tampered = signed(&auth);
        tampered.payload.authorization.value = "10001".to_owned();
        tampered.accepted.amount = "10001".to_owned();
        assert!(refused(&tampered).starts_with("invalid_exact_evm_payload_signature"));
        // Signed by someone other than `from`.
        let stranger = SigningKey::from_slice(&[0x6b; 32]).unwrap();
        assert!(
            refused(&payload(&auth, &sign(&auth, &stranger)))
                .contains("not signed by authorization.from")
        );
        // Paying someone other than `payTo`, or another amount.
        let elsewhere = Authorization {
            to: address("0x857b06519E91e3A54538791bDbb0E22373e36b66"),
            ..auth
        };
        assert!(refused(&signed(&elsewhere)).contains("recipient_mismatch"));
        let short = Authorization {
            value: 9_999,
            ..auth
        };
        assert!(refused(&signed(&short)).contains("value_mismatch"));
        // Outside its window, with the facilitator's six-second margin.
        let expiring = Authorization {
            valid_before: u64::try_from(NOW + 5).unwrap(),
            ..auth
        };
        assert!(refused(&signed(&expiring)).contains("valid_before"));
        let early = Authorization {
            valid_after: u64::try_from(NOW + 1).unwrap(),
            ..auth
        };
        assert!(refused(&signed(&early)).contains("valid_after"));
        // Another version, an unknown token, a contract-wallet signature.
        let mut v1 = signed(&auth);
        v1.x402_version = 1;
        assert!(refused(&v1).contains("x402Version"));
        let mut unknown = signed(&auth);
        unknown.accepted.asset = "0x0000000000000000000000000000000000000001".to_owned();
        assert!(refused(&unknown).contains("asset"));
        let mut long = signed(&auth);
        long.payload.signature.push_str("00");
        assert!(refused(&long).contains("contract-wallet"));
    }

    #[test]
    fn the_recorded_payment_is_answered_again_after_it_expires() {
        let proof = signed(&authorization());
        let later = NOW + 3_600;
        // Fresh, it is refused once expired...
        assert!(matches!(
            verify_at(later, &proof, None),
            Err(RailError::InvalidProof(why)) if why.contains("valid_before")
        ));
        // ...but the payment this context recorded is the same payment
        // whenever it is presented.
        let reference = reference();
        let verified = verify_at(later, &proof, Some(&reference)).unwrap();
        assert_eq!(verified.reference, reference);
        // A different payment gets no such pass.
        let other = signed(&Authorization {
            nonce: [9; 32],
            ..authorization()
        });
        assert!(verify_at(later, &other, Some(&reference)).is_err());
        // And the signature is checked all the same.
        let mut forged = proof.clone();
        forged.payload.signature = format!("0x{}", hex::encode([1u8; 65]));
        assert!(verify_at(later, &forged, Some(&reference)).is_err());
    }

    #[test]
    fn the_window_edges_are_the_facilitators() {
        let at_margin = Authorization {
            valid_before: u64::try_from(NOW + 6).unwrap(),
            valid_after: u64::try_from(NOW).unwrap(),
            ..authorization()
        };
        assert!(verify(&signed(&at_margin)).is_ok());
    }

    #[test]
    fn a_reference_has_one_spelling() {
        let r = reference();
        assert_eq!(r.parse::<AuthorizationRef>().unwrap().to_string(), r);
        assert!(r.starts_with("eip155:84532:0x036cbd53842c5426634e7929541ec2318f3dcf7e:"));
        for bad in [
            r.to_uppercase(),
            r.replace("eip155:", "eip155:0"),
            format!("{r}:x"),
            // A second spelling of the same expiry.
            r.replace(&format!(":{}", NOW + 60), &format!(":0{}", NOW + 60)),
            payment_ref().identity(),
            String::new(),
        ] {
            assert!(bad.parse::<AuthorizationRef>().is_err(), "{bad}");
        }
    }

    fn settle(
        rail: &X402Rail<FixedClock>,
        finality: &X402Finality,
    ) -> Result<FinalityStatus, RailError> {
        let (ctx, reference, amount, merchant) = (ctx(), reference(), amount(), merchant());
        rail.check_finality(
            finality,
            &SettlementExpectation {
                ctx: &ctx,
                payment_reference: &reference,
                amount: &amount,
                merchant: &merchant,
            },
        )
    }

    const TX: &str = "0xAB00000000000000000000000000000000000000000000000000000000000001";

    fn response(success: bool, error: Option<&str>) -> X402Finality {
        X402Finality::Facilitator {
            response: SettlementResponse {
                success,
                error_reason: error.map(str::to_owned),
                payer: Some(address_of(&key()).to_string()),
                transaction: TX.to_owned(),
                network: "eip155:84532".to_owned(),
                amount: Some("10000".to_owned()),
                extensions: None,
            },
        }
    }

    #[test]
    fn a_trusted_facilitator_settles_and_fails_payments() {
        let trusted = rail(true);
        assert_eq!(
            settle(&trusted, &response(true, None)).unwrap(),
            FinalityStatus::Final {
                reference: TX.to_ascii_lowercase()
            }
        );
        // Its failure stands only once the authorization can no longer
        // execute: past validBefore, and the grace after it.
        let expires = NOW + 60;
        let grace = i64::try_from(GRACE).unwrap();
        for now in [NOW, expires, expires + grace - 1] {
            assert!(
                matches!(
                    settle(
                        &rail_at(true, now),
                        &response(false, Some("insufficient_funds"))
                    )
                    .unwrap(),
                    FinalityStatus::Pending { .. }
                ),
                "at {now}"
            );
        }
        assert_eq!(
            settle(
                &rail_at(true, expires + grace),
                &response(false, Some("insufficient_funds"))
            )
            .unwrap(),
            FinalityStatus::Failed {
                reason: "insufficient_funds".to_owned()
            }
        );
        // The specification's non-terminal failure is pending, trusted or not.
        assert!(matches!(
            settle(&trusted, &response(false, Some("settlement_pending"))).unwrap(),
            FinalityStatus::Pending { .. }
        ));
    }

    #[test]
    fn an_untrusted_facilitator_settles_nothing() {
        let untrusted = rail(false);
        for answer in [
            response(true, None),
            response(false, Some("insufficient_funds")),
        ] {
            assert!(
                matches!(
                    settle(&untrusted, &answer).unwrap(),
                    FinalityStatus::Pending { .. }
                ),
                "{answer:?}"
            );
        }
    }

    #[test]
    fn a_settlement_response_for_another_payment_is_refused() {
        let trusted = rail(true);
        let edit = |f: &dyn Fn(&mut SettlementResponse)| {
            let mut answer = response(true, None);
            let X402Finality::Facilitator { response } = &mut answer else {
                unreachable!()
            };
            f(response);
            answer
        };
        for answer in [
            edit(&|r| r.network = "eip155:8453".to_owned()),
            edit(&|r| r.payer = Some(PAY_TO.to_owned())),
            edit(&|r| r.amount = Some("9999".to_owned())),
            edit(&|r| r.transaction = String::new()),
        ] {
            assert!(
                matches!(settle(&trusted, &answer), Err(RailError::InvalidProof(_))),
                "{answer:?}"
            );
        }
    }

    fn topic(address: &str) -> String {
        format!("0x{:0>64}", &address[2..])
    }

    fn receipt(status: &str, block: u64, head: u64, logs: Vec<Log>) -> X402Finality {
        X402Finality::Receipt {
            network: "eip155:84532".to_owned(),
            head,
            receipt: TransactionReceipt {
                transaction_hash: TX.to_owned(),
                block_number: format!("{block:#x}"),
                status: status.to_owned(),
                logs,
            },
        }
    }

    fn used(token: &str) -> Log {
        Log {
            address: token.to_owned(),
            topics: vec![
                AUTHORIZATION_USED.to_owned(),
                topic(&address_of(&key()).to_string()),
                format!("0x{}", hex::encode(binding_nonce(&ctx()))),
            ],
            data: "0x".to_owned(),
        }
    }

    fn transfer(token: &str, to: &str, value: u128) -> Log {
        Log {
            address: token.to_owned(),
            topics: vec![
                TRANSFER.to_owned(),
                topic(&address_of(&key()).to_string()),
                topic(to),
            ],
            data: format!("0x{value:064x}"),
        }
    }

    #[test]
    fn a_receipt_settles_at_the_threshold() {
        let trusted = rail(true);
        let logs = || vec![used(USDC), transfer(USDC, PAY_TO, 10_000)];
        assert_eq!(
            settle(&trusted, &receipt("0x1", 100, 102, logs())).unwrap(),
            FinalityStatus::Final {
                reference: TX.to_ascii_lowercase()
            }
        );
        assert_eq!(
            settle(&trusted, &receipt("0x1", 100, 100, logs())).unwrap(),
            FinalityStatus::Pending {
                reason: "1 of 3 confirmations".to_owned()
            }
        );
        // A reverted settlement spent nothing; the payment may still land.
        assert!(matches!(
            settle(&trusted, &receipt("0x0", 100, 200, Vec::new())).unwrap(),
            FinalityStatus::Pending { .. }
        ));
    }

    #[test]
    fn a_receipt_must_show_this_payment() {
        let trusted = rail(true);
        let other = "0x857b06519e91e3a54538791bdbb0e22373e36b66";
        for logs in [
            vec![transfer(USDC, PAY_TO, 10_000)],
            vec![used(USDC)],
            vec![used(USDC), transfer(USDC, other, 10_000)],
            vec![used(USDC), transfer(USDC, PAY_TO, 9_999)],
            vec![used(other), transfer(other, PAY_TO, 10_000)],
        ] {
            assert!(
                matches!(
                    settle(&trusted, &receipt("0x1", 100, 102, logs.clone())),
                    Err(RailError::InvalidProof(_))
                ),
                "{logs:?}"
            );
        }
        let mut elsewhere = receipt(
            "0x1",
            100,
            102,
            vec![used(USDC), transfer(USDC, PAY_TO, 10_000)],
        );
        if let X402Finality::Receipt { network, .. } = &mut elsewhere {
            *network = "eip155:8453".to_owned();
        }
        assert!(matches!(
            settle(&trusted, &elsewhere),
            Err(RailError::InvalidProof(_))
        ));
        assert!(matches!(
            settle(&trusted, &receipt("0x1", 100, 99, Vec::new())),
            Err(RailError::InvalidProof(_))
        ));
    }
}
