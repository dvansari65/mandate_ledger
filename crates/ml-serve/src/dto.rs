//! The wire types of the service, and how they map to the engine's.
//!
//! Request bodies refuse unknown fields: a misspelt field is an error, not
//! a silently different request. [`CartBody`] mirrors the native cart
//! format field for field, and a test holds the two together. Identifiers
//! and prose a caller supplies are bounded before they reach the ledger.

use crate::error::ApiError;
use ml_adapters::sandbox::Reported;
use ml_adapters::{MockProof, NativeCart};
use ml_core::{
    Attestation, Authorized, ContextId, Delivered, DeliveryReceipt, Denied, Event, Hash32,
    MerchantId, Money, Paid, PaymentState, Record,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// The longest request key, reference, nonce or key id accepted.
pub const MAX_ID_LEN: usize = 256;
/// The longest reason or other prose accepted.
pub const MAX_TEXT_LEN: usize = 1024;

/// A field that must be present and no longer than `max` bytes.
fn bounded(field: &str, value: &str, max: usize) -> Result<(), ApiError> {
    if value.is_empty() {
        return Err(ApiError::bad_request(format!("{field}: must not be empty")));
    }
    if value.len() > max {
        return Err(ApiError::bad_request(format!(
            "{field}: longer than {max} bytes"
        )));
    }
    Ok(())
}

/// An amount of money.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MoneyBody {
    /// A decimal, as a string. Never a float.
    #[schema(example = "128.00")]
    pub amount: String,
    /// An ISO 4217 code or a token symbol.
    #[schema(example = "INR")]
    pub currency: String,
}

impl MoneyBody {
    /// The engine's money, or a `400` naming `field`.
    pub fn parse(&self, field: &str) -> Result<Money, ApiError> {
        Money::parse(&self.amount, &self.currency)
            .map_err(|e| ApiError::bad_request(format!("{field}: {e}")))
    }
}

impl From<&Money> for MoneyBody {
    fn from(money: &Money) -> Self {
        Self {
            amount: money.amount().to_string(),
            currency: money.currency().as_str().to_owned(),
        }
    }
}

/// A merchant's signature over a cart, under a key id the service knows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AttestationBody {
    /// Which merchant key signed.
    pub key_id: String,
    /// The Ed25519 signature, hex.
    pub signature: String,
}

/// A cart in the native format: what the agent wants to buy, from whom,
/// for how much.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CartBody {
    /// The merchant: a domain or a DID. Lower-cased on the way in.
    #[schema(example = "bigbasket.com")]
    pub merchant: String,
    /// The total.
    pub total: MoneyBody,
    /// A purchase category, if the merchant states one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Line items, any JSON. Hashed, never interpreted.
    #[serde(default)]
    pub items: Vec<serde_json::Value>,
    /// The merchant's signature over everything else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<AttestationBody>,
}

impl CartBody {
    /// The adapter's cart. The two types share a wire form, so this cannot
    /// fail for a body that deserialized — except on a signature that is
    /// not 64 bytes of hex, which is the caller's mistake.
    pub fn into_native(self) -> Result<NativeCart, ApiError> {
        serde_json::to_value(self)
            .and_then(serde_json::from_value)
            .map_err(|e| ApiError::bad_request(format!("cart: {e}")))
    }
}

/// A request to authorize a cart under a mandate.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorizeRequest {
    /// The signed mandate, exactly as the wallet issued it.
    #[schema(value_type = Object)]
    pub mandate: serde_json::Value,
    /// The cart.
    pub cart: CartBody,
    /// The caller's key for this purchase attempt. The same key with the
    /// same mandate and cart is the same purchase, and gets the same answer.
    #[schema(example = "order-8831")]
    pub request_key: String,
}

impl AuthorizeRequest {
    /// Check the bounds of what the engine would otherwise store verbatim,
    /// and of what the adapter would otherwise quote back.
    pub fn validate(&self) -> Result<(), ApiError> {
        bounded("request_key", &self.request_key, MAX_ID_LEN)?;
        if let Some(attestation) = &self.cart.attestation {
            bounded("cart.attestation.key_id", &attestation.key_id, MAX_ID_LEN)?;
        }
        Ok(())
    }
}

/// A payment proof for the sandbox rail. Every knob a real rail would
/// decide for itself is a field here, which is how the threat-model
/// scenarios present a replayed nonce, a swapped cart or a redirected payee.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProofBody {
    /// The rail's reference for the payment.
    pub reference: String,
    /// The amount the proof is for.
    pub amount: MoneyBody,
    /// The single-use nonce the rail guarantees. Defaults to the reference.
    #[serde(default)]
    pub nonce: Option<String>,
    /// A cart hash the proof is bound to, `sha256:…`.
    #[serde(default)]
    pub bound_cart: Option<String>,
    /// The merchant the proof pays.
    #[serde(default)]
    pub bound_merchant: Option<String>,
    /// Do not bind the proof to the context.
    #[serde(default)]
    pub unbound: bool,
    /// Whether the proof's signature verifies. Defaults to true.
    #[serde(default = "yes")]
    pub valid: bool,
}

const fn yes() -> bool {
    true
}

impl ProofBody {
    /// The rail's proof, bound to `ctx` unless told not to be.
    pub fn into_proof(self, ctx: &ContextId) -> Result<MockProof, ApiError> {
        bounded("proof.reference", &self.reference, MAX_ID_LEN)?;
        if let Some(nonce) = &self.nonce {
            bounded("proof.nonce", nonce, MAX_ID_LEN)?;
        }
        let amount = self.amount.parse("proof.amount")?;
        let bound_cart = self
            .bound_cart
            .as_deref()
            .map(str::parse::<Hash32>)
            .transpose()
            .map_err(|e| ApiError::bad_request(format!("proof.bound_cart: {e}")))?;
        let bound_merchant = self
            .bound_merchant
            .as_deref()
            .map(MerchantId::new)
            .transpose()
            .map_err(|e| ApiError::bad_request(format!("proof.bound_merchant: {e}")))?;
        Ok(MockProof {
            nonce: self.nonce.unwrap_or_else(|| self.reference.clone()),
            reference: self.reference,
            amount,
            bound_ctx: (!self.unbound).then(|| ctx.clone()),
            bound_cart,
            bound_merchant,
            valid: self.valid,
        })
    }
}

/// A request to record a payment.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PaymentRequest {
    /// The proof.
    pub proof: ProofBody,
}

/// Finality evidence for the sandbox rail: which payment, and, if the
/// caller speaks for the rail, what it reports. With neither
/// `confirmations` nor `failed`, the rail answers from the reference:
/// `…-ok` is final, `…-fail` failed, `…-reorg` gains a confirmation per
/// check and is then dropped.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FinalityBody {
    /// The payment the evidence is about. Defaults to the one recorded.
    #[serde(default)]
    pub reference: Option<String>,
    /// The rail reports this many confirmations.
    #[serde(default)]
    pub confirmations: Option<u32>,
    /// The rail reports the payment failed, for this reason.
    #[serde(default)]
    pub failed: Option<String>,
}

impl FinalityBody {
    /// The reference, if one was given, and what the rail is told to report.
    pub fn validate(self) -> Result<(Option<String>, Option<Reported>), ApiError> {
        if let Some(reference) = &self.reference {
            bounded("finality.reference", reference, MAX_ID_LEN)?;
        }
        let reported = match (self.confirmations, self.failed) {
            (Some(_), Some(_)) => {
                return Err(ApiError::bad_request(
                    "finality: confirmations and failed are exclusive",
                ));
            }
            (Some(n), None) => Some(Reported::Confirmations(n)),
            (None, Some(reason)) => {
                bounded("finality.failed", &reason, MAX_TEXT_LEN)?;
                Some(Reported::Failed(reason))
            }
            (None, None) => None,
        };
        Ok((self.reference, reported))
    }
}

/// A request to record settlement.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SettlementRequest {
    /// The finality evidence.
    pub finality: FinalityBody,
}

/// A request to record compensation.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CompensationRequest {
    /// The host's reference for the compensation — a refund id.
    #[serde(default)]
    pub reference: Option<String>,
}

impl CompensationRequest {
    /// The reference, bounded.
    pub fn validate(&self) -> Result<(), ApiError> {
        match &self.reference {
            Some(reference) => bounded("reference", reference, MAX_ID_LEN),
            None => Ok(()),
        }
    }
}

/// A delivery receipt.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptBody {
    /// The merchant's fulfilment reference — an order id, a tracking number.
    pub reference: String,
    /// The merchant key id that vouches for the receipt. Without it, the
    /// receipt is agent-reported.
    #[serde(default)]
    pub signed_by: Option<String>,
}

impl ReceiptBody {
    /// The engine's receipt.
    pub fn into_receipt(self) -> Result<DeliveryReceipt, ApiError> {
        bounded("receipt.reference", &self.reference, MAX_ID_LEN)?;
        let attestation = match self.signed_by {
            Some(key_id) => {
                bounded("receipt.signed_by", &key_id, MAX_ID_LEN)?;
                Attestation::MerchantSigned { key_id }
            }
            None => Attestation::AgentReported,
        };
        Ok(DeliveryReceipt {
            reference: self.reference,
            attestation,
        })
    }
}

/// A request to record delivery.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRequest {
    /// The receipt.
    pub receipt: ReceiptBody,
}

/// Where a step left the context. `pending` is only ever a settlement's
/// answer: not final yet, nothing changed, ask again later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// Scope and budget checks passed; the total is reserved.
    Authorized,
    /// A proof was verified and bound.
    Paid,
    /// The rail reported finality; delivery may proceed.
    Settled,
    /// Not final yet.
    Pending,
    /// The rail reported that settlement will not happen.
    SettlementFailed,
    /// The host undid its side effects.
    Compensated,
    /// The merchant recorded fulfilment.
    Delivered,
    /// The authorization lapsed; the reservation is released.
    Expired,
}

impl From<PaymentState> for StepState {
    fn from(state: PaymentState) -> Self {
        match state {
            PaymentState::Authorized => Self::Authorized,
            PaymentState::Paid => Self::Paid,
            PaymentState::Settled => Self::Settled,
            PaymentState::SettlementFailed => Self::SettlementFailed,
            PaymentState::Compensated => Self::Compensated,
            PaymentState::Delivered => Self::Delivered,
            PaymentState::Expired => Self::Expired,
        }
    }
}

/// The `Authorized` token's facts.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AuthorizedResponse {
    /// The context id: the handle for every later step.
    pub ctx: String,
    /// `authorized`.
    pub state: StepState,
    /// The mandate the context was authorized under.
    pub mandate: String,
    /// The merchant that will be paid.
    pub merchant: String,
    /// The amount reserved.
    pub amount: MoneyBody,
    /// The hash of the cart as authorized.
    pub cart_hash: String,
}

impl From<&Authorized> for AuthorizedResponse {
    fn from(auth: &Authorized) -> Self {
        Self {
            ctx: auth.ctx().as_str().to_owned(),
            state: StepState::Authorized,
            mandate: auth.mandate_id().as_str().to_owned(),
            merchant: auth.merchant().as_str().to_owned(),
            amount: auth.amount().into(),
            cart_hash: auth.cart_hash().to_string(),
        }
    }
}

/// The `Paid` token's facts.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct PaidResponse {
    /// The context.
    pub ctx: String,
    /// `paid`.
    pub state: StepState,
    /// The rail that verified the proof.
    pub rail: String,
    /// The rail's reference for the payment.
    pub reference: String,
    /// `H(ctx ‖ rail ‖ nonce)`: the key under which this payment is unique.
    pub idempotency_key: String,
}

impl From<&Paid> for PaidResponse {
    fn from(paid: &Paid) -> Self {
        Self {
            ctx: paid.ctx().as_str().to_owned(),
            state: StepState::Paid,
            rail: paid.rail().to_owned(),
            reference: paid.reference().to_owned(),
            idempotency_key: paid.idempotency_key().to_string(),
        }
    }
}

/// What the rail said about finality.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct SettlementResponse {
    /// The context.
    pub ctx: String,
    /// `settled`, `pending` or `settlement_failed`.
    pub state: StepState,
    /// The settlement reference, once settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// What is outstanding, or why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The `Compensated` token's facts.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct CompensatedResponse {
    /// The context.
    pub ctx: String,
    /// `compensated`.
    pub state: StepState,
}

/// The `Delivered` token's facts.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct DeliveredResponse {
    /// The context.
    pub ctx: String,
    /// `delivered`.
    pub state: StepState,
    /// The receipt reference as recorded.
    pub receipt: String,
}

impl From<&Delivered> for DeliveredResponse {
    fn from(delivered: &Delivered) -> Self {
        Self {
            ctx: delivered.ctx().as_str().to_owned(),
            state: StepState::Delivered,
            receipt: delivered.receipt_reference().to_owned(),
        }
    }
}

/// An expired authorization.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ExpiredResponse {
    /// The context.
    pub ctx: String,
    /// `expired`.
    pub state: StepState,
}

/// A revoked mandate.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RevokedResponse {
    /// The mandate.
    pub mandate: String,
    /// Always true: every later authorization under it is refused.
    pub revoked: bool,
}

/// Where a context stands now.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ContextResponse {
    /// The context.
    pub ctx: String,
    /// Its state.
    pub state: StepState,
    /// The mandate it was authorized under.
    pub mandate: String,
    /// The cart hash bound at authorization.
    pub cart_hash: String,
    /// The merchant bound at authorization.
    pub merchant: String,
    /// The amount reserved at authorization.
    pub amount: MoneyBody,
    /// The rail that recorded payment, once paid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rail: Option<String>,
    /// The rail's payment reference, once paid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_reference: Option<String>,
    /// The payment's idempotency key, once paid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// The settlement reference, once settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_reference: Option<String>,
    /// Why settlement failed, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// The receipt reference, once delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_reference: Option<String>,
}

impl From<Record> for ContextResponse {
    fn from(record: Record) -> Self {
        Self {
            ctx: record.ctx.as_str().to_owned(),
            state: record.state.into(),
            mandate: record.mandate_id.as_str().to_owned(),
            cart_hash: record.cart_hash.to_string(),
            merchant: record.merchant.as_str().to_owned(),
            amount: (&record.amount).into(),
            rail: record.rail,
            payment_reference: record.payment_reference,
            idempotency_key: record.idempotency_key.map(|h| h.to_string()),
            settlement_reference: record.settlement_reference,
            failure_reason: record.failure_reason,
            receipt_reference: record.receipt_reference,
        }
    }
}

/// Which contexts to list.
#[derive(Clone, Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ContextsQuery {
    /// Only contexts authorized under this mandate.
    pub mandate: Option<String>,
    /// Only contexts in this state.
    pub state: Option<String>,
    /// Start just after this context id. The response's `next` is the
    /// value to pass to read the contexts after it.
    pub after: Option<String>,
    /// At most this many, in byte order of their ids. Default 50, at most 500.
    pub limit: Option<usize>,
}

/// A page of contexts.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ContextsResponse {
    /// The contexts, in byte order of their ids.
    pub contexts: Vec<ContextResponse>,
    /// The cursor for the page after this one, or null when nothing was listed.
    pub next: Option<String>,
}

/// Which events to read.
#[derive(Clone, Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct EventsQuery {
    /// Start just after this sequence number. Without it, the most recent
    /// events. The response's `next` is the value to pass to read what
    /// came after; `limit=0` gives `next` alone, which is where "now" is.
    pub after: Option<u64>,
    /// At most this many events. Default 50, at most 500.
    pub limit: Option<usize>,
}

/// A page of the log.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct EventsResponse {
    /// Events in commit order, each as it appears in an evidence bundle.
    #[schema(value_type = Vec<Object>)]
    pub events: Vec<Event>,
    /// The cursor for the page after this one.
    pub next: u64,
}

/// A refusal: the ledger's decision, with its code.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct Refusal {
    /// The machine-readable code. Stable; see `docs/stability.md`.
    #[schema(example = "SCOPE_MERCHANT_MISMATCH")]
    pub code: String,
    /// Which step refused.
    #[schema(example = "authorize")]
    pub stage: String,
    /// Why, for people. Never parse it.
    pub detail: String,
    /// The context the refusal was decided under.
    pub ctx: String,
    /// Whether the refusal was written to the context's chain. False once
    /// the chain holds its cap of refusals; the decision is the same.
    pub recorded: bool,
}

impl From<Denied> for Refusal {
    fn from(denied: Denied) -> Self {
        Self {
            code: denied.reason.code().to_owned(),
            stage: denied.stage.to_string(),
            detail: denied.detail,
            ctx: denied.ctx.as_str().to_owned(),
            recorded: denied.recorded,
        }
    }
}

/// Anything that went wrong other than a refusal.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct Problem {
    /// What went wrong, for people.
    pub error: String,
}

impl Problem {
    /// A problem described by `error`.
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
        }
    }
}

/// Liveness or readiness.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct Health {
    /// `ok`, or `not ready`.
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ml_adapters::NativeAttestation;
    use serde_json::json;

    /// The two cart types share a wire form, in both directions: every
    /// field of one is a field of the other.
    #[test]
    fn the_cart_body_is_the_native_cart() {
        let native = NativeCart {
            merchant: "bigbasket.com".to_owned(),
            total: Money::parse("128.00", "INR").unwrap(),
            category: Some("grocery".to_owned()),
            items: vec![json!({"sku": "a"}), json!("b")],
            attestation: Some(NativeAttestation {
                key_id: "bb-2026".to_owned(),
                signature: [7u8; 64],
            }),
        };
        let as_body: CartBody = serde_json::from_value(serde_json::to_value(&native).unwrap())
            .expect("every native field is a body field");
        let back = as_body
            .clone()
            .into_native()
            .expect("every body field is a native field");
        assert_eq!(back, native);
        assert_eq!(
            serde_json::to_value(&as_body).unwrap(),
            serde_json::to_value(&native).unwrap()
        );
    }

    #[test]
    fn a_proof_defaults_its_nonce_and_binding() {
        let ctx = ContextId::new("ctx-1").unwrap();
        let proof: ProofBody = serde_json::from_value(json!({
            "reference": "pay-1",
            "amount": {"amount": "10.00", "currency": "INR"}
        }))
        .unwrap();
        let proof = proof.into_proof(&ctx).unwrap();
        assert_eq!(proof.nonce, "pay-1");
        assert_eq!(proof.bound_ctx, Some(ctx));
        assert!(proof.valid);
    }

    #[test]
    fn finality_cannot_both_confirm_and_fail() {
        let body = FinalityBody {
            reference: None,
            confirmations: Some(1),
            failed: Some("x".to_owned()),
        };
        assert!(matches!(
            body.validate(),
            Err(ApiError::Rejected { status, .. }) if status == http::StatusCode::BAD_REQUEST
        ));
    }
}
