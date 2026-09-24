//! The commands that decide against the database, driven as a script would.
//!
//! Skipped without `ML_TEST_DATABASE_URL`; fatal if `ML_REQUIRE_DATABASE` is
//! set without it, so the CI job that exists to run these cannot silently
//! pass. Every command is a separate process — that is the point.

mod common;

use common::cart as cart_json;
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
        authorize_under(&self.mandate, cart, request_key, extra)
    }

    /// Run `ml --json <args> --database-url URL` and parse the report.
    fn step(&self, args: &[&str]) -> (i32, serde_json::Value, String) {
        let (code, out, err) = run(ml()
            .arg("--json")
            .args(args)
            .args(["--database-url", &self.url]));
        let report = if out.is_empty() {
            serde_json::Value::Null
        } else {
            json(&out)
        };
        (code, report, err)
    }

    /// A payment reference unique to this run. Nonces are spent forever per
    /// rail and the database is durable, so a fixed reference would replay.
    fn reference(&self, tag: &str) -> String {
        format!("{tag}-{}", self.principal.trim_start_matches("user:"))
    }

    /// Authorize `cart`, asserting it is allowed, and return the context id.
    fn authorized(&self, cart: &Path, request_key: &str) -> String {
        let (code, report, err) = self.authorize(cart, request_key, &self.trust_args());
        assert_eq!(code, 0, "{err}");
        report["context"].as_str().unwrap().to_owned()
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

/// Authorize under a specific mandate file.
fn authorize_under(
    mandate: &Path,
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
        .arg(mandate)
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

fn events(chain: &[serde_json::Value]) -> Vec<String> {
    chain
        .iter()
        .map(|e| e["event"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_whole_lifecycle_across_processes() {
    let Some(url) = database() else { return };
    let s = Setup::new("lifecycle", url);
    let (pay1, pay2) = (s.reference("pay-1"), s.reference("pay-2"));
    let ctx = s.authorized(&s.cart("cart", "128.00"), "order-1");

    let (code, r, err) = s.step(&["pay", &ctx, "--reference", &pay1, "--amount", "128.00 INR"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(r["state"], "paid");
    assert_eq!(r["rail"], "mock");
    assert_eq!(r["reference"], pay1);

    // Not final yet: nothing is recorded; ask again later.
    let (code, r, _) = s.step(&["settle", &ctx, "--confirmations", "0"]);
    assert_eq!(code, 0);
    assert_eq!(r["state"], "pending");
    assert_eq!(s.chain(&ctx).len(), 2);

    let (code, r, _) = s.step(&["settle", &ctx, "--confirmations", "1"]);
    assert_eq!(code, 0);
    assert_eq!(r["state"], "settled");
    assert_eq!(r["reference"], format!("{pay1}@1"));

    let (code, r, _) = s.step(&["deliver", &ctx, "--receipt", "BB-1", "--signed-by", "bb"]);
    assert_eq!(code, 0);
    assert_eq!(r["state"], "delivered");
    assert_eq!(r["receipt"], "BB-1");

    assert_eq!(
        events(&s.chain(&ctx)),
        ["authorized", "paid", "settled", "delivered"]
    );
    assert_eq!(s.contexts()[0]["state"], "delivered");

    // Every step replays from a fresh process and adds nothing to the chain.
    let (code, r, _) = s.step(&["pay", &ctx, "--reference", &pay1, "--amount", "128.00 INR"]);
    assert_eq!((code, r["state"].as_str()), (0, Some("paid")));
    let (code, r, _) = s.step(&["settle", &ctx, "--confirmations", "1"]);
    assert_eq!((code, r["state"].as_str()), (0, Some("settled")));
    let (code, r, _) = s.step(&["deliver", &ctx, "--receipt", "BB-1"]);
    assert_eq!((code, r["state"].as_str()), (0, Some("delivered")));
    assert_eq!(s.chain(&ctx).len(), 4);

    // A *different* payment against a paid context is the engine's refusal, recorded.
    let (code, r, _) = s.step(&["pay", &ctx, "--reference", &pay2, "--amount", "128.00 INR"]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "INVALID_STATE");
    assert_eq!(r["recorded"], true);
    assert_eq!(s.chain(&ctx).len(), 5);
}

#[test]
fn the_failure_path_ends_in_compensation() {
    let Some(url) = database() else { return };
    let s = Setup::new("failure", url);
    let pay1 = s.reference("pay-1");
    let ctx = s.authorized(&s.cart("cart", "500.00"), "order-1");
    let (code, _, err) = s.step(&["pay", &ctx, "--reference", &pay1, "--amount", "500.00 INR"]);
    assert_eq!(code, 0, "{err}");

    let (code, r, _) = s.step(&["settle", &ctx, "--failed", "reverted"]);
    assert_eq!(code, 0);
    assert_eq!(r["state"], "settlement_failed");
    assert_eq!(r["reason"], "reverted");
    assert_eq!(s.contexts()[0]["state"], "settlement_failed");

    // Delivery is unreachable: the type system's refusal, so nothing is recorded.
    let (code, r, _) = s.step(&["deliver", &ctx, "--receipt", "x"]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "INVALID_STATE");
    assert_eq!(r["recorded"], false);

    let (code, r, _) = s.step(&["compensate", &ctx, "--reference", "refund-1"]);
    assert_eq!(code, 0);
    assert_eq!(r["state"], "compensated");

    // Replays, from fresh processes.
    let (code, r, _) = s.step(&["compensate", &ctx]);
    assert_eq!((code, r["state"].as_str()), (0, Some("compensated")));
    let (code, r, _) = s.step(&["settle", &ctx, "--failed", "reverted"]);
    assert_eq!((code, r["state"].as_str()), (0, Some("settlement_failed")));
    assert_eq!(
        events(&s.chain(&ctx)),
        ["authorized", "paid", "settlement_failed", "compensated"]
    );
}

#[test]
fn delivery_before_finality_is_unreachable() {
    let Some(url) = database() else { return };
    let s = Setup::new("early-delivery", url);
    let pay1 = s.reference("pay-1");
    let ctx = s.authorized(&s.cart("cart", "10.00"), "order-1");

    let (code, r, _) = s.step(&["deliver", &ctx, "--receipt", "x"]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "INVALID_STATE");
    assert_eq!(r["recorded"], false);
    assert!(r["detail"].as_str().unwrap().contains("authorized"));

    let (code, _, err) = s.step(&["pay", &ctx, "--reference", &pay1, "--amount", "10.00 INR"]);
    assert_eq!(code, 0, "{err}");
    let (code, r, _) = s.step(&["settle", &ctx, "--confirmations", "0"]);
    assert_eq!((code, r["state"].as_str()), (0, Some("pending")));

    // Paid but not final: still unreachable.
    let (code, r, _) = s.step(&["deliver", &ctx, "--receipt", "x"]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "INVALID_STATE");
    assert!(r["detail"].as_str().unwrap().contains("paid"));
    assert_eq!(events(&s.chain(&ctx)), ["authorized", "paid"]);
}

#[test]
fn pay_refuses_replayed_swapped_unbound_and_invalid_proofs() {
    let Some(url) = database() else { return };
    let s = Setup::new("proofs", url);
    let (good, other_ref) = (s.reference("p"), s.reference("p2"));
    let cart = s.cart("cart", "10.00");
    let a = s.authorized(&cart, "o1");
    let b = s.authorized(&cart, "o2");
    let refused = |args: &[&str]| {
        let (code, r, err) = s.step(args);
        assert_eq!(code, 2, "{err}");
        assert_eq!(r["recorded"], true, "{r}");
        r["refused"].as_str().unwrap().to_owned()
    };

    assert_eq!(
        refused(&["pay", &a, "--reference", &good, "--amount", "9.00 INR"]),
        "AMOUNT_MISMATCH"
    );
    assert_eq!(
        refused(&[
            "pay",
            &a,
            "--reference",
            &good,
            "--amount",
            "10.00 INR",
            "--invalid"
        ]),
        "PROOF_INVALID"
    );
    assert_eq!(
        refused(&[
            "pay",
            &a,
            "--reference",
            &good,
            "--amount",
            "10.00 INR",
            "--unbound"
        ]),
        "UNBOUND_PROOF"
    );

    // The hash of another cart: a swap.
    let raw = s.dir.join("other.raw.json");
    std::fs::write(&raw, cart_json("20.00")).unwrap();
    let (code, out, err) = run(ml()
        .args(["--json", "cart", "sign"])
        .arg(&raw)
        .arg("--key")
        .arg(&s.merchant_key)
        .args(["--key-id", "bb", "--out"])
        .arg(s.dir.join("other.json")));
    assert_eq!(code, 0, "{err}");
    let other = json(&out)["hash"].as_str().unwrap().to_owned();
    assert_eq!(
        refused(&[
            "pay",
            &a,
            "--reference",
            &good,
            "--amount",
            "10.00 INR",
            "--bound-cart",
            &other
        ]),
        "CART_BINDING_MISMATCH"
    );

    // A good proof pays a; the same nonce cannot pay b.
    let (code, _, err) = s.step(&["pay", &a, "--reference", &good, "--amount", "10.00 INR"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        refused(&[
            "pay",
            &b,
            "--reference",
            &other_ref,
            "--amount",
            "10.00 INR",
            "--nonce",
            &good
        ]),
        "NONCE_ALREADY_USED"
    );

    // Every refusal is in the chain it was made against, as a code.
    let codes: Vec<_> = s
        .chain(&a)
        .iter()
        .filter_map(|e| e["code"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        codes,
        [
            "AMOUNT_MISMATCH",
            "PROOF_INVALID",
            "UNBOUND_PROOF",
            "CART_BINDING_MISMATCH"
        ]
    );
}

#[test]
fn expire_and_revoke() {
    let Some(url) = database() else { return };
    let s = Setup::new("expire-revoke", url);
    let p = s.reference("p");
    let cart = s.cart("cart", "10.00");
    let ctx = s.authorized(&cart, "o1");

    let (code, r, err) = s.step(&["expire", &ctx]);
    assert_eq!((code, r["state"].as_str()), (0, Some("expired")), "{err}");
    let (code, r, _) = s.step(&["expire", &ctx]);
    assert_eq!(
        (code, r["state"].as_str()),
        (0, Some("expired")),
        "idempotent"
    );
    assert_eq!(s.contexts()[0]["state"], "expired");

    // Paying an expired context is the engine's refusal, recorded.
    let (code, r, _) = s.step(&["pay", &ctx, "--reference", &p, "--amount", "10.00 INR"]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "INVALID_STATE");
    assert_eq!(r["recorded"], true);

    let (code, r, err) = s.step(&["revoke", &s.mandate_id]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(r["revoked"], true);
    let (code, r, _) = s.authorize(&cart, "o2", &s.trust_args());
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "MANDATE_REVOKED");
}

#[test]
fn a_step_on_an_unknown_context_is_refused_without_a_record() {
    let Some(url) = database() else { return };
    let s = Setup::new("unknown", url);
    let p = s.reference("p");
    let (code, r, _) = s.step(&["pay", "ctx_nope", "--reference", &p, "--amount", "1.00 INR"]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "CONTEXT_NOT_FOUND");
    assert_eq!(r["recorded"], false);
    assert!(s.chain("ctx_nope").is_empty());
}

#[test]
fn evidence_exported_from_the_ledger_verifies_without_it() {
    let Some(url) = database() else { return };
    let s = Setup::new("evidence", url);
    let pay1 = s.reference("pay-1");
    let ctx = s.authorized(&s.cart("cart", "10.00"), "o1");
    for step in [
        vec!["pay", &ctx, "--reference", &pay1, "--amount", "10.00 INR"],
        vec!["settle", &ctx, "--confirmations", "1"],
        vec!["deliver", &ctx, "--receipt", "R"],
    ] {
        let (code, _, err) = s.step(&step);
        assert_eq!(code, 0, "{err}");
    }

    let bundle = s.dir.join("bundle.json");
    let (code, r, err) = s.step(&["evidence", &ctx, "--out", bundle.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(r["events"], 4);
    assert_eq!(r["final_state"], "delivered");

    // Signed by the host: the file names who exported it.
    let host = new_key(&s.dir, "host.key");
    let signed = s.dir.join("signed.json");
    let (code, r, err) = s.step(&[
        "evidence",
        &ctx,
        "--out",
        signed.to_str().unwrap(),
        "--sign",
        host.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(r["signed_by"].as_str().unwrap().len(), 64);

    // Verification takes no database flags at all.
    for (file, extra) in [
        (&bundle, vec![]),
        (
            &signed,
            vec!["--signer", s.dir.join("host.key.pub").to_str().unwrap()],
        ),
    ] {
        let (code, out, err) = run(ml().args(["--json", "verify"]).arg(file).args(&extra));
        assert_eq!(code, 0, "{err}");
        let r = json(&out);
        assert_eq!(r["verified"], true);
        assert_eq!(r["context"], ctx);
        assert_eq!(r["events"], 4);
    }

    // No such context: an error, not a refusal — there is no decision here.
    let (code, _, err) = s.step(&[
        "evidence",
        "ctx_nope",
        "--out",
        s.dir.join("x.json").to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("no such context"), "{err}");
}

#[test]
fn the_rail_answers_from_the_reference() {
    let Some(url) = database() else { return };
    let s = Setup::new("rail-magic", url);

    // `-ok`: final on the first ask, without being told anything.
    let ctx = s.authorized(&s.cart("ok", "10.00"), "ok");
    let ok = format!("{}-ok", s.reference("pay"));
    let (code, _, err) = s.step(&["pay", &ctx, "--reference", &ok, "--amount", "10.00 INR"]);
    assert_eq!(code, 0, "{err}");
    let (code, r, err) = s.step(&["settle", &ctx]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(r["state"], "settled");
    assert_eq!(r["reference"], format!("{ok}@1"));

    // `-fail`: the rail declined it.
    let ctx = s.authorized(&s.cart("fail", "10.00"), "fail");
    let fail = format!("{}-fail", s.reference("pay"));
    let (code, _, err) = s.step(&["pay", &ctx, "--reference", &fail, "--amount", "10.00 INR"]);
    assert_eq!(code, 0, "{err}");
    let (code, r, _) = s.step(&["settle", &ctx]);
    assert_eq!(code, 0);
    assert_eq!(r["state"], "settlement_failed");
    assert_eq!(r["reason"], "declined by the rail");

    // `-reorg` at finality 3: a confirmation per check, dropped on the third
    // — and the count survives across processes.
    let ctx = s.authorized(&s.cart("reorg", "10.00"), "reorg");
    let reorg = format!("{}-reorg", s.reference("pay"));
    let (code, _, err) = s.step(&["pay", &ctx, "--reference", &reorg, "--amount", "10.00 INR"]);
    assert_eq!(code, 0, "{err}");
    for have in 1..3 {
        let (code, r, _) = s.step(&["settle", &ctx, "--finality", "3"]);
        assert_eq!(code, 0);
        assert_eq!(r["state"], "pending");
        assert!(
            r["reason"]
                .as_str()
                .unwrap()
                .ends_with(&format!("have {have}")),
            "{r}"
        );
    }
    let (code, r, _) = s.step(&["settle", &ctx, "--finality", "3"]);
    assert_eq!(code, 0);
    assert_eq!(r["state"], "settlement_failed");
    assert_eq!(r["reason"], "reorganized: dropped after 2 confirmations");
    let (code, r, _) = s.step(&["compensate", &ctx]);
    assert_eq!((code, r["state"].as_str()), (0, Some("compensated")));

    // A plain reference has to be told: the rail has no report of its own.
    let ctx = s.authorized(&s.cart("plain", "10.00"), "plain");
    let plain = s.reference("pay");
    let (code, _, err) = s.step(&["pay", &ctx, "--reference", &plain, "--amount", "10.00 INR"]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = s.step(&["settle", &ctx]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("no report"), "{err}");
    let (code, r, _) = s.step(&["settle", &ctx, "--confirmations", "1"]);
    assert_eq!((code, r["state"].as_str()), (0, Some("settled")));
}

#[test]
fn finality_from_another_rail_is_refused() {
    let Some(url) = database() else { return };
    let s = Setup::new("rail-mismatch", url);
    let ctx = s.authorized(&s.cart("cart", "10.00"), "o1");
    let pay = s.reference("pay");
    let (code, _, err) = s.step(&["pay", &ctx, "--reference", &pay, "--amount", "10.00 INR"]);
    assert_eq!(code, 0, "{err}");

    let (code, r, _) = s.step(&["settle", &ctx, "--confirmations", "1", "--rail", "other"]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "RAIL_MISMATCH");
    assert_eq!(r["recorded"], true);

    // From the rail that took the payment, it settles.
    let (code, r, _) = s.step(&["settle", &ctx, "--confirmations", "1"]);
    assert_eq!((code, r["state"].as_str()), (0, Some("settled")));
}

/// Puts the sandbox clock back to wall time when dropped, even if the test
/// panicked: the clock is global to the database, and a frozen one would
/// change "now" for every other test running against it.
struct WallClock<'a>(&'a Setup);

impl Drop for WallClock<'_> {
    fn drop(&mut self) {
        let _ = run(ml().args(["clock", "reset", "--database-url", &self.0.url]));
    }
}

#[test]
fn the_clock_can_be_frozen_advanced_and_reset() {
    let Some(url) = database() else { return };
    let s = Setup::new("clock", url);
    let _wall = WallClock(&s);

    let (code, r, err) = s.step(&["clock", "show"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(r["source"], "wall");

    // Frozen, the engine reads that instant, and the events say so.
    let at: i64 = 1_800_000_000; // inside every fixture mandate's window
    let (code, r, _) = s.step(&["clock", "set", &at.to_string()]);
    assert_eq!(code, 0);
    assert_eq!(r["source"], "frozen");
    assert_eq!(r["now"], at);
    let ctx = s.authorized(&s.cart("c0", "10.00"), "o0");
    assert_eq!(s.chain(&ctx)[0]["at"], at);

    // Expiry: a mandate that ends a minute after `at`.
    let short = sign_mandate_as(
        &s.dir,
        "short-mandate.json",
        &body_valid(
            &format!("{}-short", s.mandate_id),
            &s.principal,
            at,
            at + 60,
        ),
        &s.user_key,
    );
    let (code, _, err) = authorize_under(&short, &s.cart("short", "10.00"), "s1", &s.trust_args());
    assert_eq!(code, 0, "{err}");
    let (code, _, _) = s.step(&["clock", "set", &(at + 120).to_string()]);
    assert_eq!(code, 0);
    let (code, r, _) = authorize_under(&short, &s.cart("short", "10.00"), "s2", &s.trust_args());
    assert_eq!(
        (code, r["refused"].as_str()),
        (2, Some("MANDATE_EXPIRED")),
        "{r}"
    );

    // Velocity: five a day. Back at `at`, four more fill the window; the
    // sixth is refused — until a day passes.
    let (code, _, _) = s.step(&["clock", "set", &at.to_string()]);
    assert_eq!(code, 0);
    for i in 1..=4 {
        s.authorized(&s.cart(&format!("c{i}"), "10.00"), &format!("o{i}"));
    }
    let (code, r, _) = s.authorize(&s.cart("c5", "10.00"), "o5", &s.trust_args());
    assert_eq!(
        (code, r["refused"].as_str()),
        (2, Some("VELOCITY_EXCEEDED"))
    );
    let (code, r, _) = s.step(&["clock", "advance", "86401"]);
    assert_eq!(code, 0);
    assert_eq!(r["now"], at + 86401);
    s.authorized(&s.cart("c6", "10.00"), "o6");

    let (code, r, _) = s.step(&["clock", "reset"]);
    assert_eq!(code, 0);
    assert_eq!(r["source"], "wall");
}
