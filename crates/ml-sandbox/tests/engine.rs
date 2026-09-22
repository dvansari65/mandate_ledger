//! The commands that decide against the database, driven as a script would.
//!
//! Skipped without `ML_TEST_DATABASE_URL`; fatal if `ML_REQUIRE_DATABASE` is
//! set without it, so the CI job that exists to run these cannot silently
//! pass. Every command is a separate process — that is the point.

mod common;

use common::*;
use std::path::{Path, PathBuf};

fn database() -> Option<String> {
    if let Some(url) = std::env::var_os("ML_TEST_DATABASE_URL") {
        return Some(url.to_string_lossy().into_owned());
    }
    assert!(
        std::env::var_os("ML_REQUIRE_DATABASE").is_none(),
        "ML_REQUIRE_DATABASE is set but ML_TEST_DATABASE_URL is not"
    );
    None
}

/// Keys, a mandate for a principal unique to this run, and the trust flags
/// that make the engine accept them.
struct Setup {
    dir: PathBuf,
    url: String,
    user_key: PathBuf,
    merchant_key: PathBuf,
    mandate: PathBuf,
    mandate_id: String,
    principal: String,
}

impl Setup {
    fn new(test: &str, url: String) -> Self {
        let dir = workdir(test);
        let user_key = new_key(&dir, "user.key");
        let merchant_key = new_key(&dir, "merchant.key");
        let id = unique();
        let mandate_id = format!("mnd-{id}");
        let principal = format!("user:{id}");
        let mandate = sign_mandate(&dir, &body(&mandate_id, &principal), &user_key);
        Self {
            dir,
            url,
            user_key,
            merchant_key,
            mandate,
            mandate_id,
            principal,
        }
    }

    fn cart(&self, name: &str, total: &str) -> PathBuf {
        sign_cart(&self.dir, name, &cart(total), &self.merchant_key, "bb")
    }

    /// `--database-url`, `--trust` and `--merchant-key` for this setup —
    /// trusting the public halves only, as an operator would.
    fn trust_args(&self) -> Vec<String> {
        vec![
            "--database-url".into(),
            self.url.clone(),
            "--trust".into(),
            format!("{}={}.pub", self.principal, self.user_key.display()),
            "--merchant-key".into(),
            format!("bb={}.pub", self.merchant_key.display()),
        ]
    }

    fn authorize(
        &self,
        cart: &Path,
        request_key: &str,
        extra: &[String],
    ) -> (i32, serde_json::Value, String) {
        let (code, out, err) = run(ml()
            .args([
                "--json",
                "authorize",
                "--request-key",
                request_key,
                "--mandate",
            ])
            .arg(&self.mandate)
            .arg("--cart")
            .arg(cart)
            .args(extra));
        let report = if out.is_empty() {
            serde_json::Value::Null
        } else {
            json(&out)
        };
        (code, report, err)
    }

    fn contexts(&self) -> Vec<serde_json::Value> {
        let (code, out, err) = run(ml().args([
            "--json",
            "contexts",
            "--mandate",
            &self.mandate_id,
            "--database-url",
            &self.url,
        ]));
        assert_eq!(code, 0, "{err}");
        json(&out)["contexts"].as_array().unwrap().clone()
    }

    fn chain(&self, ctx: &str) -> Vec<serde_json::Value> {
        let (code, out, err) = run(ml().args([
            "--json",
            "log",
            "--context",
            ctx,
            "--database-url",
            &self.url,
        ]));
        assert_eq!(code, 0, "{err}");
        json(&out)["events"].as_array().unwrap().clone()
    }
}

#[test]
fn authorize_then_read_it_back_from_fresh_processes() {
    let Some(url) = database() else { return };
    let s = Setup::new("authorize", url);
    let cart = s.cart("cart", "128.00");

    // Where the log ends now, before this test writes anything.
    let (code, out, err) =
        run(ml().args(["--json", "log", "--limit", "0", "--database-url", &s.url]));
    assert_eq!(code, 0, "{err}");
    let now = json(&out)["next"].as_u64().unwrap();

    let (code, report, err) = s.authorize(&cart, "order-1", &s.trust_args());
    assert_eq!(code, 0, "{err}");
    assert_eq!(report["state"], "authorized");
    assert_eq!(report["amount"], "128.00 INR");
    assert_eq!(report["mandate"], s.mandate_id);
    let ctx = report["context"].as_str().unwrap().to_owned();

    // A new process sees the reservation and the event.
    let contexts = s.contexts();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["context"], ctx);
    assert_eq!(contexts[0]["state"], "authorized");
    let chain = s.chain(&ctx);
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0]["event"], "authorized");
    assert!(
        chain[0]["detail"]
            .as_str()
            .unwrap()
            .starts_with("128.00 INR")
    );

    // The same request key from another process is the same purchase.
    let (code, again, err) = s.authorize(&cart, "order-1", &s.trust_args());
    assert_eq!(code, 0, "{err}");
    assert_eq!(again["context"], ctx);
    assert_eq!(s.contexts().len(), 1, "a replay reserves nothing twice");

    // Everything after `now` includes our event, and so does the tail.
    let (code, out, _) = run(ml().args([
        "--json",
        "log",
        "--after",
        &now.to_string(),
        "--limit",
        "1000",
        "--database-url",
        &s.url,
    ]));
    assert_eq!(code, 0);
    let since = json(&out);
    assert!(
        since["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["context"] == ctx)
    );
    assert!(since["next"].as_u64().unwrap() > now);
    let (code, out, _) =
        run(ml().args(["--json", "log", "--limit", "1000", "--database-url", &s.url]));
    assert_eq!(code, 0);
    assert!(
        json(&out)["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["context"] == ctx)
    );
}

#[test]
fn a_refusal_is_a_decision_with_exit_2_and_a_record() {
    let Some(url) = database() else { return };
    let s = Setup::new("refusal", url);
    let too_much = s.cart("cart", "3000.00"); // over the 2,000 per-purchase cap

    let (code, report, err) = s.authorize(&too_much, "order-1", &s.trust_args());
    assert_eq!(code, 2, "{err}");
    assert_eq!(report["refused"], "SCOPE_PER_TXN_EXCEEDED");
    assert_eq!(report["stage"], "authorize");
    assert!(report["detail"].as_str().unwrap().contains("2000.00 INR"));
    let ctx = report["context"].as_str().unwrap().to_owned();

    // The refusal is in the chain; nothing was reserved.
    let chain = s.chain(&ctx);
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0]["event"], "denied");
    assert_eq!(chain[0]["code"], "SCOPE_PER_TXN_EXCEEDED");
    assert!(
        s.contexts().is_empty(),
        "a refused-only context has no record"
    );
}

#[test]
fn an_untrusted_signer_is_refused() {
    let Some(url) = database() else { return };
    let s = Setup::new("untrusted", url);
    let cart = s.cart("cart", "10.00");

    // No --trust: the mandate's own key vouches for nothing.
    let args = vec![
        "--database-url".to_owned(),
        s.url.clone(),
        "--merchant-key".to_owned(),
        format!("bb={}", s.merchant_key.display()),
    ];
    let (code, report, err) = s.authorize(&cart, "order-1", &args);
    assert_eq!(code, 2, "{err}");
    assert_eq!(report["refused"], "MANDATE_SIGNER_UNTRUSTED");
}

#[test]
fn an_unsigned_cart_is_refused_when_the_mandate_requires_a_signature() {
    let Some(url) = database() else { return };
    let s = Setup::new("unsigned", url);
    let unsigned = s.dir.join("unsigned.json");
    std::fs::write(&unsigned, cart("10.00")).unwrap();

    let (code, report, err) = s.authorize(&unsigned, "order-1", &s.trust_args());
    assert_eq!(code, 2, "{err}");
    assert_eq!(report["refused"], "ATTESTATION_INSUFFICIENT");
}

#[test]
fn a_cart_with_an_unknown_merchant_key_never_reaches_the_ledger() {
    let Some(url) = database() else { return };
    let s = Setup::new("unknown-key", url);
    let cart = s.cart("cart", "10.00");

    // Trust the signer, but not the merchant key: the adapter rejects the
    // cart before the ledger sees it — an error, not a recorded refusal.
    let args = vec![
        "--database-url".to_owned(),
        s.url.clone(),
        "--trust".to_owned(),
        format!("{}={}", s.principal, s.user_key.display()),
    ];
    let (code, _, err) = s.authorize(&cart, "order-1", &args);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("unknown merchant key"), "{err}");
    assert!(s.contexts().is_empty());
}
