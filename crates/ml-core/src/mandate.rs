//! Mandates: what a principal allowed an agent to spend, signed.
//!
//! A [`Mandate`] is a [`MandateBody`] plus an Ed25519 signature over its
//! canonical JSON. The body's [`Scope`] is written purely in terms of
//! [`ScopeClaims`] — never line items — so any protocol that can produce
//! those claims can be governed by the same mandate.
//!
//! Scopes form a lattice: [`Scope::is_subset_of`] decides whether a child
//! scope is no wider than its parent, which is what makes delegation safe
//! ([`MandateBody::attenuate`]).

use crate::cart::{Attestation, AttestationLevel, ScopeClaims};
use crate::error::DenyReason;
use crate::hash::{CanonicalizeError, canonical_json};
use crate::ids::{AgentId, Category, MandateId, MerchantId, PrincipalId};
use crate::money::{Currency, Money};
use crate::time::Timestamp;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

/// Which merchants a scope admits. Serializes as a string:
/// `"*"` (any), `"bigbasket.com"` (exact), `"*.zepto.com"` (sub-domains).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MerchantPattern {
    /// Any merchant.
    Any,
    /// Exactly this merchant id (lower-case).
    Exact(String),
    /// Any id ending in this suffix, which starts with `.` (lower-case).
    Suffix(String),
}

impl MerchantPattern {
    /// Parse a pattern string.
    pub fn parse(pattern: &str) -> Result<Self, MandateError> {
        let s = pattern.trim().to_ascii_lowercase();
        let invalid = || MandateError::InvalidMerchantPattern(pattern.to_owned());
        if s.is_empty() {
            return Err(invalid());
        }
        if s == "*" {
            return Ok(Self::Any);
        }
        if let Some(rest) = s.strip_prefix("*.") {
            if rest.is_empty() || rest.contains('*') {
                return Err(invalid());
            }
            return Ok(Self::Suffix(format!(".{rest}")));
        }
        if s.contains('*') {
            return Err(invalid());
        }
        Ok(Self::Exact(s))
    }

    /// Whether `merchant` is admitted by this pattern.
    #[must_use]
    pub fn matches(&self, merchant: &MerchantId) -> bool {
        let m = merchant.as_str();
        match self {
            Self::Any => true,
            Self::Exact(e) => e == m,
            Self::Suffix(suffix) => m.len() > suffix.len() && m.ends_with(suffix.as_str()),
        }
    }

    /// Whether every merchant admitted by `self` is admitted by `other`.
    #[must_use]
    pub fn is_subset_of(&self, other: &Self) -> bool {
        match (self, other) {
            (_, Self::Any) => true,
            (Self::Any, _) | (Self::Suffix(_), Self::Exact(_)) => false,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Exact(a), Self::Suffix(s)) => a.len() > s.len() && a.ends_with(s.as_str()),
            (Self::Suffix(a), Self::Suffix(b)) => a.ends_with(b.as_str()),
        }
    }
}

impl fmt::Display for MerchantPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => f.write_str("*"),
            Self::Exact(e) => f.write_str(e),
            Self::Suffix(s) => write!(f, "*{s}"),
        }
    }
}

impl Serialize for MerchantPattern {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MerchantPattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// At most `max_count` authorizations in any `window_secs`-long window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Velocity {
    /// Maximum authorizations in the window.
    pub max_count: u32,
    /// Window length in seconds.
    pub window_secs: i64,
}

/// What a mandate permits. Every field constrains [`ScopeClaims`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    /// Admitted merchants. Empty admits none (fail closed); use `["*"]` for any.
    pub merchants: Vec<MerchantPattern>,
    /// Admitted categories. `None` = unconstrained. `Some` requires the cart
    /// to carry a category — a cart without one is denied `UNVERIFIABLE_SCOPE`.
    pub categories: Option<Vec<Category>>,
    /// The single currency this mandate operates in.
    pub currency: Currency,
    /// Maximum per authorization.
    pub max_per_txn: Option<Money>,
    /// Maximum outstanding across all live authorizations.
    pub max_total: Option<Money>,
    /// Inclusive start of validity.
    pub valid_from: Timestamp,
    /// Inclusive end of validity.
    pub valid_until: Timestamp,
    /// Rate limit on authorizations.
    pub velocity: Option<Velocity>,
    /// Weakest cart attestation accepted.
    pub min_attestation: AttestationLevel,
}

impl Scope {
    /// Check internal consistency (currencies agree, window is sane, …).
    pub fn validate(&self) -> Result<(), MandateError> {
        let bad = |m: &str| MandateError::InvalidScope(m.to_owned());
        if self.valid_from > self.valid_until {
            return Err(bad("valid_from is after valid_until"));
        }
        for (name, cap) in [
            ("max_per_txn", &self.max_per_txn),
            ("max_total", &self.max_total),
        ] {
            if let Some(cap) = cap {
                if cap.currency() != &self.currency {
                    return Err(bad(&format!("{name} currency differs from scope currency")));
                }
                if cap.is_negative() {
                    return Err(bad(&format!("{name} is negative")));
                }
            }
        }
        if let Some(v) = self.velocity {
            if v.max_count == 0 || v.window_secs <= 0 {
                return Err(bad("velocity must have max_count > 0 and window_secs > 0"));
            }
        }
        Ok(())
    }

    /// Whether this scope admits `claims` with `attestation` at time `now`.
    ///
    /// Budget (`max_total`) and `velocity` are not checked here — they depend
    /// on store state and are enforced atomically by the store on append.
    pub fn admits(
        &self,
        claims: &ScopeClaims,
        attestation: &Attestation,
        now: Timestamp,
    ) -> Result<(), (DenyReason, String)> {
        if now < self.valid_from {
            return Err((
                DenyReason::MandateNotYetValid,
                format!("valid from {}, now {}", self.valid_from.0, now.0),
            ));
        }
        if now > self.valid_until {
            return Err((
                DenyReason::MandateExpired,
                format!("valid until {}, now {}", self.valid_until.0, now.0),
            ));
        }
        if attestation.level() < self.min_attestation {
            return Err((
                DenyReason::AttestationInsufficient,
                format!(
                    "cart is {:?}, mandate requires {:?}",
                    attestation.level(),
                    self.min_attestation
                ),
            ));
        }
        if !self.merchants.iter().any(|p| p.matches(&claims.merchant)) {
            return Err((
                DenyReason::ScopeMerchantMismatch,
                format!("merchant `{}` not in mandate scope", claims.merchant),
            ));
        }
        if let Some(allowed) = &self.categories {
            match &claims.category {
                None => {
                    return Err((
                        DenyReason::UnverifiableScope,
                        "mandate constrains category but cart carries none".to_owned(),
                    ));
                }
                Some(c) if !allowed.contains(c) => {
                    return Err((
                        DenyReason::ScopeCategoryMismatch,
                        format!("category `{c}` not in mandate scope"),
                    ));
                }
                Some(_) => {}
            }
        }
        if claims.total.currency() != &self.currency {
            return Err((
                DenyReason::CurrencyMismatch,
                format!(
                    "cart is {}, mandate is {}",
                    claims.total.currency(),
                    self.currency
                ),
            ));
        }
        if claims.total.is_negative() {
            return Err((
                DenyReason::AmountInvalid,
                format!("total {} is negative", claims.total),
            ));
        }
        if let Some(cap) = &self.max_per_txn {
            let over = claims
                .total
                .cmp_same_currency(cap)
                .map_err(|e| (DenyReason::CurrencyMismatch, e.to_string()))?
                == Ordering::Greater;
            if over {
                return Err((
                    DenyReason::ScopePerTxnExceeded,
                    format!("total {} exceeds per-transaction cap {}", claims.total, cap),
                ));
            }
        }
        Ok(())
    }

    /// Whether `self` admits nothing `parent` would refuse.
    ///
    /// This is the delegation rule: a child mandate may only *narrow*.
    #[must_use]
    pub fn is_subset_of(&self, parent: &Scope) -> bool {
        fn cap_within(child: Option<&Money>, parent: Option<&Money>) -> bool {
            match (child, parent) {
                (_, None) => true,
                (None, Some(_)) => false,
                (Some(c), Some(p)) => c.cmp_same_currency(p).is_ok_and(|o| o != Ordering::Greater),
            }
        }

        if self.currency != parent.currency {
            return false;
        }
        let merchants_ok = self
            .merchants
            .iter()
            .all(|c| parent.merchants.iter().any(|p| c.is_subset_of(p)));
        let categories_ok = match (&self.categories, &parent.categories) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(c), Some(p)) => c.iter().all(|x| p.contains(x)),
        };
        let velocity_ok = match (self.velocity, parent.velocity) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(c), Some(p)) => c.max_count <= p.max_count && c.window_secs >= p.window_secs,
        };
        merchants_ok
            && categories_ok
            && cap_within(self.max_per_txn.as_ref(), parent.max_per_txn.as_ref())
            && cap_within(self.max_total.as_ref(), parent.max_total.as_ref())
            && self.valid_from >= parent.valid_from
            && self.valid_until <= parent.valid_until
            && velocity_ok
            && self.min_attestation >= parent.min_attestation
    }
}

/// The signed part of a mandate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MandateBody {
    /// Unique id.
    pub id: MandateId,
    /// Who granted it.
    pub principal: PrincipalId,
    /// Who may act under it.
    pub agent: AgentId,
    /// What it permits.
    pub scope: Scope,
    /// When it was issued.
    pub issued_at: Timestamp,
    /// The mandate this one was attenuated from, if any.
    pub parent: Option<MandateId>,
}

impl MandateBody {
    /// Derive a narrower mandate for delegation. Fails unless `scope ⊆ self.scope`.
    pub fn attenuate(
        &self,
        id: MandateId,
        agent: AgentId,
        scope: Scope,
        issued_at: Timestamp,
    ) -> Result<MandateBody, MandateError> {
        scope.validate()?;
        if !scope.is_subset_of(&self.scope) {
            return Err(MandateError::NotSubset(id));
        }
        Ok(MandateBody {
            id,
            principal: self.principal.clone(),
            agent,
            scope,
            issued_at,
            parent: Some(self.id.clone()),
        })
    }

    /// The bytes that are signed.
    pub fn signing_bytes(&self) -> Result<Vec<u8>, CanonicalizeError> {
        canonical_json(self)
    }
}

/// A signed mandate. Serializes with `signer` and `signature` as hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mandate {
    /// The signed content.
    pub body: MandateBody,
    /// Ed25519 public key that signed `body`.
    #[serde(with = "hex")]
    pub signer: [u8; 32],
    /// Ed25519 signature over `body.signing_bytes()`.
    #[serde(with = "hex")]
    pub signature: [u8; 64],
}

impl Mandate {
    /// Sign `body` with `key`. Validates the scope first.
    pub fn sign(body: MandateBody, key: &SigningKey) -> Result<Self, MandateError> {
        body.scope.validate()?;
        let bytes = body.signing_bytes()?;
        let signature = key.sign(&bytes);
        Ok(Self {
            body,
            signer: key.verifying_key().to_bytes(),
            signature: signature.to_bytes(),
        })
    }

    /// Verify the signature and scope consistency. Does **not** decide whether
    /// `signer` is allowed to sign for `body.principal` — see [`SignerPolicy`].
    pub fn verify(&self) -> Result<(), MandateError> {
        self.body.scope.validate()?;
        let key = VerifyingKey::from_bytes(&self.signer).map_err(|_| MandateError::BadSignerKey)?;
        let signature = Signature::from_bytes(&self.signature);
        let bytes = self.body.signing_bytes()?;
        key.verify_strict(&bytes, &signature)
            .map_err(|_| MandateError::SignatureInvalid)
    }

    /// The mandate id.
    #[must_use]
    pub const fn id(&self) -> &MandateId {
        &self.body.id
    }
}

/// Decides whether a signing key may issue mandates for a principal.
///
/// This binding is security-critical: without it anyone could mint a
/// mandate naming someone else's principal. Production hosts implement this
/// against their user key directory; [`TrustedSigners`] is a static version.
pub trait SignerPolicy: Send + Sync {
    /// Whether `signer` is trusted to sign for `principal`.
    fn is_trusted(&self, principal: &PrincipalId, signer: &[u8; 32]) -> bool;
}

/// A static allow-list of `principal → signing keys`.
#[derive(Debug, Default, Clone)]
pub struct TrustedSigners {
    keys: HashMap<PrincipalId, Vec<[u8; 32]>>,
}

impl TrustedSigners {
    /// An empty allow-list (trusts nobody).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Trust `key` for `principal`.
    #[must_use]
    pub fn allow(mut self, principal: PrincipalId, key: [u8; 32]) -> Self {
        self.keys.entry(principal).or_default().push(key);
        self
    }
}

impl SignerPolicy for TrustedSigners {
    fn is_trusted(&self, principal: &PrincipalId, signer: &[u8; 32]) -> bool {
        self.keys
            .get(principal)
            .is_some_and(|keys| keys.contains(signer))
    }
}

/// **INSECURE.** Trusts every key for every principal. For tests only.
#[derive(Debug, Default, Clone, Copy)]
pub struct AcceptAnySigner;

impl SignerPolicy for AcceptAnySigner {
    fn is_trusted(&self, _: &PrincipalId, _: &[u8; 32]) -> bool {
        true
    }
}

/// Errors from mandate construction and verification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MandateError {
    /// A merchant pattern string was not `*`, `exact`, or `*.suffix`.
    #[error("invalid merchant pattern `{0}`")]
    InvalidMerchantPattern(String),
    /// The scope is internally inconsistent.
    #[error("invalid scope: {0}")]
    InvalidScope(String),
    /// An attenuated scope was wider than its parent.
    #[error("scope of `{0}` is not a subset of its parent")]
    NotSubset(MandateId),
    /// `signer` is not a valid Ed25519 public key.
    #[error("signer is not a valid Ed25519 public key")]
    BadSignerKey,
    /// The signature did not verify.
    #[error("signature invalid")]
    SignatureInvalid,
    /// Canonicalization failed.
    #[error(transparent)]
    Canonicalize(#[from] CanonicalizeError),
}

impl<P: SignerPolicy + ?Sized> SignerPolicy for std::sync::Arc<P> {
    fn is_trusted(&self, principal: &PrincipalId, signer: &[u8; 32]) -> bool {
        (**self).is_trusted(principal, signer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(merchants: &[&str], cats: Option<&[&str]>) -> Scope {
        Scope {
            merchants: merchants
                .iter()
                .map(|m| MerchantPattern::parse(m).unwrap())
                .collect(),
            categories: cats.map(|c| c.iter().map(|x| Category::new(*x).unwrap()).collect()),
            currency: Currency::new("INR").unwrap(),
            max_per_txn: Some(Money::parse("2000", "INR").unwrap()),
            max_total: Some(Money::parse("8000", "INR").unwrap()),
            valid_from: Timestamp(0),
            valid_until: Timestamp(1_000),
            velocity: Some(Velocity {
                max_count: 5,
                window_secs: 86_400,
            }),
            min_attestation: AttestationLevel::AgentReported,
        }
    }

    fn claims(merchant: &str, total: &str, cat: Option<&str>) -> ScopeClaims {
        ScopeClaims {
            merchant: MerchantId::new(merchant).unwrap(),
            total: Money::parse(total, "INR").unwrap(),
            category: cat.map(|c| Category::new(c).unwrap()),
            line_count: None,
        }
    }

    #[test]
    fn merchant_patterns() {
        let any = MerchantPattern::parse("*").unwrap();
        let exact = MerchantPattern::parse("BigBasket.com").unwrap();
        let suffix = MerchantPattern::parse("*.zepto.com").unwrap();
        let bb = MerchantId::new("bigbasket.com").unwrap();
        let z = MerchantId::new("api.zepto.com").unwrap();
        let root = MerchantId::new("zepto.com").unwrap();
        assert!(any.matches(&bb) && exact.matches(&bb) && !suffix.matches(&bb));
        assert!(suffix.matches(&z) && !suffix.matches(&root));
        assert!(exact.is_subset_of(&any) && !any.is_subset_of(&exact));
        assert!(
            MerchantPattern::parse("*.x.zepto.com")
                .unwrap()
                .is_subset_of(&suffix)
        );
        assert!(MerchantPattern::parse("a*b").is_err());
        assert!(MerchantPattern::parse("*.").is_err());
    }

    #[test]
    fn admits_and_denies() {
        let s = scope(&["bigbasket.com"], Some(&["grocery"]));
        let ok = claims("bigbasket.com", "100", Some("grocery"));
        assert!(
            s.admits(&ok, &Attestation::AgentReported, Timestamp(10))
                .is_ok()
        );

        let cases = [
            (
                claims("amazon.in", "100", Some("grocery")),
                DenyReason::ScopeMerchantMismatch,
            ),
            (
                claims("bigbasket.com", "100", None),
                DenyReason::UnverifiableScope,
            ),
            (
                claims("bigbasket.com", "100", Some("toys")),
                DenyReason::ScopeCategoryMismatch,
            ),
            (
                claims("bigbasket.com", "2001", Some("grocery")),
                DenyReason::ScopePerTxnExceeded,
            ),
            (
                claims("bigbasket.com", "-1", Some("grocery")),
                DenyReason::AmountInvalid,
            ),
        ];
        for (c, reason) in cases {
            let err = s
                .admits(&c, &Attestation::AgentReported, Timestamp(10))
                .unwrap_err();
            assert_eq!(err.0, reason);
        }
        assert_eq!(
            s.admits(&ok, &Attestation::AgentReported, Timestamp(5_000))
                .unwrap_err()
                .0,
            DenyReason::MandateExpired
        );
        let strict = Scope {
            min_attestation: AttestationLevel::MerchantSigned,
            ..s
        };
        assert_eq!(
            strict
                .admits(&ok, &Attestation::AgentReported, Timestamp(10))
                .unwrap_err()
                .0,
            DenyReason::AttestationInsufficient
        );
    }

    #[test]
    fn subset_lattice() {
        let parent = scope(&["*.zepto.com", "bigbasket.com"], None);
        let mut child = scope(&["api.zepto.com"], Some(&["grocery"]));
        assert!(child.is_subset_of(&parent));
        child.max_total = Some(Money::parse("9000", "INR").unwrap());
        assert!(!child.is_subset_of(&parent));
        child.max_total = None; // unbounded child under bounded parent
        assert!(!child.is_subset_of(&parent));
    }

    #[test]
    fn sign_verify_tamper() {
        let key = SigningKey::from_bytes(&[1u8; 32]);
        let body = MandateBody {
            id: MandateId::new("m1").unwrap(),
            principal: PrincipalId::new("user:a").unwrap(),
            agent: AgentId::new("agent:x").unwrap(),
            scope: scope(&["*"], None),
            issued_at: Timestamp(1),
            parent: None,
        };
        let m = Mandate::sign(body, &key).unwrap();
        assert!(m.verify().is_ok());
        let json = serde_json::to_string(&m).unwrap();
        let back: Mandate = serde_json::from_str(&json).unwrap();
        assert!(back.verify().is_ok());
        let mut tampered = m.clone();
        tampered.body.scope.max_total = None;
        assert_eq!(
            tampered.verify().unwrap_err(),
            MandateError::SignatureInvalid
        );
    }
}
