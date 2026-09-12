//! A plain JSON cart with optional merchant signature.
//!
//! Wire shape:
//!
//! ```json
//! {
//!   "merchant": "bigbasket.com",
//!   "total": { "amount": "128.00", "currency": "INR" },
//!   "category": "grocery",
//!   "items": [ ...anything... ],
//!   "attestation": { "key_id": "bb-2026", "signature": "<hex ed25519>" }
//! }
//! ```
//!
//! `items` is opaque: it is hashed, never interpreted. The signature covers
//! the canonical JSON of every field except `attestation.signature`.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use ml_core::{
    AdapterError, Attestation, Cart, CartAdapter, Category, MerchantId, Money, ScopeClaims,
    canonical_json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A merchant signature over a [`NativeCart`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeAttestation {
    /// Which merchant key signed. Looked up in [`NativeCartAdapter`].
    pub key_id: String,
    /// Ed25519 signature over [`NativeCart::signing_bytes`].
    #[serde(with = "hex")]
    pub signature: [u8; 64],
}

/// The native cart document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeCart {
    /// Merchant id (domain, DID…). Normalized to lower-case on ingest.
    pub merchant: String,
    /// Cart total.
    pub total: Money,
    /// Purchase category, if the merchant states one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Line items. Opaque to the engine.
    #[serde(default)]
    pub items: Vec<serde_json::Value>,
    /// Merchant signature, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<NativeAttestation>,
}

impl NativeCart {
    /// Canonical bytes the merchant signs: everything except the signature itself.
    pub fn signing_bytes(&self) -> Result<Vec<u8>, AdapterError> {
        let unsigned = NativeCart {
            attestation: None,
            ..self.clone()
        };
        Ok(canonical_json(&unsigned)?)
    }

    /// Sign as merchant `key_id` with `key`.
    pub fn sign(
        mut self,
        key_id: impl Into<String>,
        key: &SigningKey,
    ) -> Result<Self, AdapterError> {
        self.attestation = None;
        let signature = key.sign(&self.signing_bytes()?);
        self.attestation = Some(NativeAttestation {
            key_id: key_id.into(),
            signature: signature.to_bytes(),
        });
        Ok(self)
    }

    /// The canonical bytes of the full document — what the engine hashes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AdapterError> {
        Ok(canonical_json(self)?)
    }
}

/// Normalizes [`NativeCart`] JSON into a [`Cart`].
///
/// A cart with an `attestation` is accepted only if `key_id` is registered
/// and the signature verifies; it becomes [`Attestation::MerchantSigned`].
/// A cart without one becomes [`Attestation::AgentReported`]. A cart with a
/// bad or unknown signature is rejected outright — it is not downgraded.
#[derive(Debug, Default, Clone)]
pub struct NativeCartAdapter {
    merchant_keys: HashMap<String, VerifyingKey>,
}

impl NativeCartAdapter {
    /// An adapter that trusts no merchant keys (only unsigned carts normalize).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a merchant verifying key under `key_id`.
    #[must_use]
    pub fn with_merchant_key(mut self, key_id: impl Into<String>, key: VerifyingKey) -> Self {
        self.merchant_keys.insert(key_id.into(), key);
        self
    }

    /// Normalize an already-parsed document.
    pub fn normalize_cart(&self, cart: &NativeCart) -> Result<Cart, AdapterError> {
        let merchant = MerchantId::new(cart.merchant.as_str())?;
        let attestation = match &cart.attestation {
            None => Attestation::AgentReported,
            Some(a) => {
                let key = self.merchant_keys.get(&a.key_id).ok_or_else(|| {
                    AdapterError::SignatureInvalid(format!("unknown merchant key `{}`", a.key_id))
                })?;
                key.verify_strict(&cart.signing_bytes()?, &Signature::from_bytes(&a.signature))
                    .map_err(|_| {
                        AdapterError::SignatureInvalid(format!(
                            "bad signature for key `{}`",
                            a.key_id
                        ))
                    })?;
                Attestation::MerchantSigned {
                    key_id: a.key_id.clone(),
                }
            }
        };
        let category = cart.category.as_deref().map(Category::new).transpose()?;
        let line_count = u32::try_from(cart.items.len()).ok();
        let claims = ScopeClaims {
            merchant,
            total: cart.total.clone(),
            category,
            line_count,
        };
        Ok(Cart::new(cart.canonical_bytes()?, claims, attestation))
    }
}

impl CartAdapter for NativeCartAdapter {
    fn normalize(&self, raw: &[u8]) -> Result<Cart, AdapterError> {
        let cart: NativeCart =
            serde_json::from_slice(raw).map_err(|e| AdapterError::Malformed(e.to_string()))?;
        self.normalize_cart(&cart)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ml_core::AttestationLevel;

    fn cart() -> NativeCart {
        NativeCart {
            merchant: "BigBasket.com".into(),
            total: Money::parse("128.00", "INR").unwrap(),
            category: Some("Grocery".into()),
            items: vec![serde_json::json!({ "sku": "milk-1l", "qty": 2 })],
            attestation: None,
        }
    }

    #[test]
    fn unsigned_is_agent_reported_and_normalized() {
        let c = NativeCartAdapter::new().normalize_cart(&cart()).unwrap();
        assert_eq!(c.attestation().level(), AttestationLevel::AgentReported);
        assert_eq!(c.claims().merchant.as_str(), "bigbasket.com");
        assert_eq!(c.claims().category.as_ref().unwrap().as_str(), "grocery");
        assert_eq!(c.claims().line_count, Some(1));
    }

    #[test]
    fn hash_is_independent_of_wire_formatting() {
        let a = NativeCartAdapter::new();
        let compact = br#"{"merchant":"x.com","total":{"amount":"1","currency":"INR"}}"#;
        let spaced = b"{ \"total\" : { \"currency\" : \"INR\", \"amount\" : \"1\" }, \"merchant\" : \"x.com\" }";
        assert_eq!(
            a.normalize(compact).unwrap().hash(),
            a.normalize(spaced).unwrap().hash()
        );
    }

    #[test]
    fn signed_verifies_and_tamper_rejects() {
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let adapter = NativeCartAdapter::new().with_merchant_key("bb", key.verifying_key());
        let signed = cart().sign("bb", &key).unwrap();
        let c = adapter.normalize_cart(&signed).unwrap();
        assert_eq!(c.attestation().level(), AttestationLevel::MerchantSigned);

        let mut tampered = signed.clone();
        tampered.total = Money::parse("1.00", "INR").unwrap();
        assert!(matches!(
            adapter.normalize_cart(&tampered),
            Err(AdapterError::SignatureInvalid(_))
        ));

        let unknown = NativeCartAdapter::new();
        assert!(matches!(
            unknown.normalize_cart(&signed),
            Err(AdapterError::SignatureInvalid(_))
        ));
    }
}
