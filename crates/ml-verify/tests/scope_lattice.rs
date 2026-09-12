//! Property: a child scope built by tightening never admits what its parent refuses.
//!
//! This is the invariant delegation rests on (`MandateBody::attenuate`).

use ml_core::*;
use proptest::prelude::*;
use rust_decimal::Decimal;

fn inr(n: i64) -> Money {
    Money::new(Decimal::from(n), Currency::new("INR").unwrap())
}

fn pattern(s: &str) -> MerchantPattern {
    MerchantPattern::parse(s).unwrap()
}

fn cat(s: &str) -> Category {
    Category::new(s).unwrap()
}

prop_compose! {
    fn arb_parent()(
        merchants in prop::collection::vec(prop::sample::select(vec!["*", "*.zepto.com", "bigbasket.com", "amazon.in"]), 1..=3),
        categories in prop::option::of(prop::collection::vec(prop::sample::select(vec!["grocery", "toys", "books"]), 1..=3)),
        per_txn in prop::option::of(100i64..5_000),
        total in prop::option::of(5_000i64..20_000),
        from in 0i64..100,
        len in 100i64..1_000,
        velocity in prop::option::of((1u32..10, 60i64..3_600)),
        strict in any::<bool>(),
    ) -> Scope {
        Scope {
            merchants: merchants.into_iter().map(pattern).collect(),
            categories: categories.map(|c| c.into_iter().map(cat).collect()),
            currency: Currency::new("INR").unwrap(),
            max_per_txn: per_txn.map(inr),
            max_total: total.map(inr),
            valid_from: Timestamp(from),
            valid_until: Timestamp(from + len),
            velocity: velocity.map(|(max_count, window_secs)| Velocity { max_count, window_secs }),
            min_attestation: if strict { AttestationLevel::MerchantSigned } else { AttestationLevel::AgentReported },
        }
    }
}

prop_compose! {
    fn arb_claims()(
        merchant in prop::sample::select(vec!["bigbasket.com", "api.zepto.com", "zepto.com", "amazon.in", "other.com"]),
        total in 0i64..25_000,
        category in prop::option::of(prop::sample::select(vec!["grocery", "toys", "books", "pets"])),
    ) -> ScopeClaims {
        ScopeClaims {
            merchant: MerchantId::new(merchant).unwrap(),
            total: inr(total),
            category: category.map(cat),
            line_count: None,
        }
    }
}

/// Deterministically narrow `parent` along every axis `seed` selects.
fn tighten(parent: &Scope, seed: u8) -> Scope {
    let mut s = parent.clone();
    if seed & 1 != 0 {
        s.merchants.truncate(1);
    }
    if seed & 2 != 0 {
        if let Some(c) = &mut s.categories {
            c.truncate(1);
        } else {
            s.categories = Some(vec![cat("grocery")]);
        }
    }
    if seed & 4 != 0 {
        s.max_per_txn = Some(s.max_per_txn.clone().map_or(inr(50), |m| {
            Money::new(m.amount() / Decimal::from(2), m.currency().clone())
        }));
    }
    if seed & 8 != 0 {
        s.max_total = Some(s.max_total.clone().map_or(inr(500), |m| {
            Money::new(m.amount() / Decimal::from(2), m.currency().clone())
        }));
    }
    if seed & 16 != 0 {
        let mid = (s.valid_from.0 + s.valid_until.0) / 2;
        s.valid_from = Timestamp(mid - 10);
        s.valid_until = Timestamp(mid + 10);
    }
    if seed & 32 != 0 {
        s.velocity = Some(s.velocity.map_or(
            Velocity {
                max_count: 1,
                window_secs: 86_400,
            },
            |v| Velocity {
                max_count: v.max_count,
                window_secs: v.window_secs * 2,
            },
        ));
    }
    if seed & 64 != 0 {
        s.min_attestation = AttestationLevel::MerchantSigned;
    }
    s
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn tightened_child_is_subset(parent in arb_parent(), seed in any::<u8>()) {
        let child = tighten(&parent, seed);
        prop_assert!(child.validate().is_ok());
        prop_assert!(child.is_subset_of(&parent), "child {child:?}\nparent {parent:?}");
    }

    #[test]
    fn child_admits_implies_parent_admits(
        parent in arb_parent(),
        seed in any::<u8>(),
        claims in arb_claims(),
        signed in any::<bool>(),
    ) {
        let child = tighten(&parent, seed);
        let attestation = if signed {
            Attestation::MerchantSigned { key_id: "k".into() }
        } else {
            Attestation::AgentReported
        };
        let now = Timestamp((child.valid_from.0 + child.valid_until.0) / 2);
        if child.admits(&claims, &attestation, now).is_ok() {
            prop_assert!(parent.admits(&claims, &attestation, now).is_ok());
        }
    }

    #[test]
    fn subset_is_reflexive_and_antisymmetric_on_tightening(parent in arb_parent(), seed in 1u8..) {
        prop_assert!(parent.is_subset_of(&parent));
        let child = tighten(&parent, seed);
        if child != parent {
            prop_assert!(!parent.is_subset_of(&child) || child.is_subset_of(&parent));
        }
    }
}
