//! The offline commands, driven as a script would drive them.

mod common;

use common::*;
use std::path::Path;

#[test]
fn keys_new_writes_a_private_key_and_never_overwrites_it() {
    let dir = workdir("keys");
    let key = dir.join("user.key");

    let (code, out, _) = run(ml().args(["--json", "keys", "new", "--out"]).arg(&key));
    assert_eq!(code, 0);
    let report = json(&out);
    let public = report["public"].as_str().unwrap();
    assert_eq!(public.len(), 64);
    assert_eq!(report["file"], key.display().to_string());

    let file = json(&std::fs::read_to_string(&key).unwrap());
    assert_eq!(file["algorithm"], "ed25519");
    assert_eq!(file["public"], public);
    assert_eq!(file["secret"].as_str().unwrap().len(), 64);

    // The public half is a file of its own, safe to hand to anyone.
    let public_file = dir.join("user.key.pub");
    assert_eq!(report["public_file"], public_file.display().to_string());
    let shared = json(&std::fs::read_to_string(&public_file).unwrap());
    assert_eq!(shared["algorithm"], "ed25519");
    assert_eq!(shared["public"], public);
    assert!(
        shared.get("secret").is_none(),
        "the public file carries no secret"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&key).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a secret is readable by its owner alone"
        );
    }

    // Running again must not replace the key.
    let before = std::fs::read_to_string(&key).unwrap();
    let (code, _, err) = run(ml().args(["keys", "new", "--out"]).arg(&key));
    assert_eq!(code, 1, "an existing key is an error, not a refusal");
    assert!(err.starts_with("error: cannot create"), "{err}");
    assert_eq!(std::fs::read_to_string(&key).unwrap(), before);
}

#[test]
fn mandate_sign_produces_a_mandate_the_engine_verifies() {
    let dir = workdir("sign");
    let key = new_key(&dir, "user.key");
    let body = dir.join("body.json");
    std::fs::write(&body, BODY).unwrap();
    let out = dir.join("mandate.json");

    let (code, text, err) = run(ml()
        .args(["mandate", "sign"])
        .arg(&body)
        .arg("--key")
        .arg(&key)
        .arg("--out")
        .arg(&out));
    assert_eq!(code, 0, "{err}");
    assert!(text.contains("mandate    mnd-1\n"), "{text}");

    let mandate: ml_core::Mandate =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    mandate
        .verify()
        .expect("the engine accepts what the CLI signed");
    let key_file = json(&std::fs::read_to_string(&key).unwrap());
    assert_eq!(hex::encode(mandate.signer), key_file["public"]);
    assert_eq!(mandate.body.principal.as_str(), "user:alice");

    // The same, as a script would read it.
    let (code, text, _) = run(ml()
        .args(["--json", "mandate", "sign"])
        .arg(&body)
        .arg("--key")
        .arg(&key)
        .arg("--out")
        .arg(&out));
    assert_eq!(code, 0);
    let report = json(&text);
    assert_eq!(report["mandate"], "mnd-1");
    assert_eq!(report["principal"], "user:alice");
    assert_eq!(report["signer"], key_file["public"]);

    // Re-signing replaced the file in place, through a temporary that is gone.
    assert!(out.exists());
    assert!(!dir.join("mandate.json.tmp").exists());
}

#[test]
fn a_bad_scope_is_an_error_not_a_refusal() {
    let dir = workdir("bad-scope");
    let key = new_key(&dir, "user.key");
    let body = dir.join("body.json");
    std::fs::write(
        &body,
        BODY.replace("\"valid_until\": 1802592000", "\"valid_until\": 1"),
    )
    .unwrap();

    let (code, _, err) = run(ml()
        .args(["mandate", "sign"])
        .arg(&body)
        .args(["--key"])
        .arg(&key)
        .args(["--out"])
        .arg(dir.join("mandate.json")));
    assert_eq!(code, 1);
    assert!(err.contains("valid_from is after valid_until"), "{err}");
    assert!(
        !dir.join("mandate.json").exists(),
        "nothing is written on failure"
    );
}

#[test]
fn a_tampered_key_file_is_rejected() {
    let dir = workdir("tampered");
    let key = new_key(&dir, "user.key");
    let mut file = json(&std::fs::read_to_string(&key).unwrap());
    file["public"] = serde_json::Value::String("00".repeat(32));
    std::fs::write(&key, file.to_string()).unwrap();
    let body = dir.join("body.json");
    std::fs::write(&body, BODY).unwrap();

    let (code, _, err) = run(ml()
        .args(["mandate", "sign"])
        .arg(&body)
        .arg("--key")
        .arg(&key)
        .arg("--out")
        .arg(dir.join("mandate.json")));
    assert_eq!(code, 1);
    assert!(err.contains("public key does not match"), "{err}");
}

#[test]
fn a_wrong_command_line_is_exit_64_and_help_is_exit_0() {
    let (code, _, err) = run(ml().args(["keys"]));
    assert_eq!(code, 64, "{err}");
    let (code, _, _) = run(ml().args(["keys", "new", "--nope"]));
    assert_eq!(code, 64);
    let (code, out, _) = run(ml().arg("--help"));
    assert_eq!(code, 0);
    assert!(out.contains("mandate-ledger"), "{out}");
}

#[test]
fn cart_sign_produces_a_cart_the_adapter_accepts() {
    let dir = workdir("cart-sign");
    let key = new_key(&dir, "merchant.key");
    let signed = sign_cart(&dir, "cart", &cart("128.00"), &key, "bb-2026");

    let key_file = json(&std::fs::read_to_string(&key).unwrap());
    let public: [u8; 32] = hex::decode(key_file["public"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let adapter = ml_adapters::NativeCartAdapter::new().with_merchant_key(
        "bb-2026",
        ml_core::VerifyingKey::from_bytes(&public).unwrap(),
    );
    let cart: ml_adapters::NativeCart =
        serde_json::from_str(&std::fs::read_to_string(&signed).unwrap()).unwrap();
    let normalized = adapter
        .normalize_cart(&cart)
        .expect("the adapter accepts the signature");
    assert_eq!(
        normalized.attestation().level(),
        ml_core::AttestationLevel::MerchantSigned
    );

    // Under a key id the adapter does not know, the same cart is rejected.
    assert!(
        ml_adapters::NativeCartAdapter::new()
            .normalize_cart(&cart)
            .is_err()
    );
}

#[test]
fn an_unreachable_database_fails_fast_and_names_the_cause() {
    let started = std::time::Instant::now();
    let (code, _, err) = run(ml().args([
        "contexts",
        "--database-url",
        "postgres://localhost:1/nothing",
    ]));
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("refused"),
        "the cause, not just a timeout: {err}"
    );
    assert!(
        started.elapsed().as_secs() < 20,
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn no_database_is_an_error_that_says_what_to_set() {
    let dir = workdir("no-db");
    let key = new_key(&dir, "user.key");
    let mandate = sign_mandate(&dir, BODY, &key);
    // An unsigned cart, so nothing local can fail before the database is needed.
    let cart_file = dir.join("cart.json");
    std::fs::write(&cart_file, cart("10.00")).unwrap();

    let (code, _, err) = run(ml()
        .args(["authorize", "--request-key", "k", "--mandate"])
        .arg(&mandate)
        .arg("--cart")
        .arg(&cart_file));
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("ML_DATABASE_URL"), "{err}");

    let (code, _, err) = run(ml().args(["contexts"]));
    assert_eq!(code, 1, "{err}");
}

/// A four-event chain built entirely in memory: what a bundle exported
/// elsewhere looks like to a machine that has never seen the database.
fn bundle_in_memory() -> ml_core::EvidenceBundle {
    use ml_adapters::{MockFinality, MockProof, MockRail, NativeCart, NativeCartAdapter};
    use ml_core::*;

    let key = SigningKey::from_bytes(&[7u8; 32]);
    let body = MandateBody {
        id: MandateId::new("mnd-1").unwrap(),
        principal: PrincipalId::new("user:alice").unwrap(),
        agent: AgentId::new("agent:shopper").unwrap(),
        scope: Scope {
            merchants: vec![MerchantPattern::parse("bigbasket.com").unwrap()],
            categories: None,
            currency: Currency::new("INR").unwrap(),
            max_per_txn: None,
            max_total: None,
            valid_from: Timestamp(0),
            valid_until: Timestamp(i64::MAX),
            velocity: None,
            min_attestation: AttestationLevel::AgentReported,
        },
        issued_at: Timestamp(0),
        parent: None,
    };
    let mandate = Mandate::sign(body, &key).unwrap();
    let cart = NativeCartAdapter::new()
        .normalize_cart(&NativeCart {
            merchant: "bigbasket.com".into(),
            total: Money::parse("50.00", "INR").unwrap(),
            category: None,
            items: vec![],
            attestation: None,
        })
        .unwrap();
    let ledger = Ledger::new(
        MemoryStore::new(),
        MockRail::new("mock", 1),
        AcceptAnySigner,
        FixedClock::at(1_700_000_000),
    );
    let a = ledger.authorize(&mandate, &cart, "o1").unwrap();
    let paid = ledger
        .record_payment(
            &a,
            &MockProof::bound_to(a.ctx(), "pay", Money::parse("50.00", "INR").unwrap()),
        )
        .unwrap();
    let Settlement::Settled(settled) = ledger
        .record_settlement(&paid, &MockFinality::confirmed("pay", 1))
        .unwrap()
    else {
        panic!("expected settled");
    };
    let receipt = DeliveryReceipt {
        reference: "r".into(),
        attestation: Attestation::AgentReported,
    };
    ledger.record_delivery(&settled, receipt).unwrap();
    ledger.evidence(a.ctx()).unwrap().unwrap()
}

fn verify(file: &Path, extra: &[&str]) -> (i32, serde_json::Value, String) {
    let (code, out, err) = run(ml().args(["--json", "verify"]).arg(file).args(extra));
    let report = if out.is_empty() {
        serde_json::Value::Null
    } else {
        json(&out)
    };
    (code, report, err)
}

#[test]
fn verify_checks_a_bundle_with_no_database_and_names_the_broken_event() {
    let dir = workdir("verify");
    let bundle = bundle_in_memory();
    let file = dir.join("bundle.json");
    std::fs::write(&file, serde_json::to_string_pretty(&bundle).unwrap()).unwrap();

    let (code, r, err) = verify(&file, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(r["verified"], true);
    assert_eq!(r["events"], 4);
    assert_eq!(r["final_state"], "delivered");
    assert_eq!(r["signed"], false);

    // An altered amount: the event's hash no longer matches its contents.
    let mut tampered: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    tampered["events"][1]["body"]["amount"]["amount"] = "5000.00".into();
    let altered = dir.join("altered.json");
    std::fs::write(&altered, tampered.to_string()).unwrap();
    let (code, r, _) = verify(&altered, &[]);
    assert_eq!(code, 2);
    assert_eq!(r["verified"], false);
    assert_eq!(r["refused"], "HASH_MISMATCH");
    assert_eq!(r["event"], 2);

    // A removed event: the next one's prev_hash points at nothing.
    let mut cut: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    cut["events"].as_array_mut().unwrap().remove(1);
    let shortened = dir.join("shortened.json");
    std::fs::write(&shortened, cut.to_string()).unwrap();
    let (code, r, _) = verify(&shortened, &[]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "BROKEN_CHAIN");
    assert_eq!(r["event"], 3);

    // A format this build does not know is refused, not checked.
    let mut future: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    future["version"] = 2.into();
    let unknown = dir.join("future.json");
    std::fs::write(&unknown, future.to_string()).unwrap();
    let (code, r, _) = verify(&unknown, &[]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "UNSUPPORTED_VERSION");

    // Not a bundle at all is an error, not a verdict — and the error says
    // what is missing.
    let junk = dir.join("junk.json");
    std::fs::write(&junk, "{\"hello\": 1}").unwrap();
    let (code, _, err) = verify(&junk, &[]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("missing field"), "{err}");
}

#[test]
fn verify_checks_who_signed_a_bundle() {
    let dir = workdir("verify-signed");
    let host = new_key(&dir, "host.key");
    let other = new_key(&dir, "other.key");
    let host_secret: [u8; 32] = hex::decode(
        json(&std::fs::read_to_string(&host).unwrap())["secret"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
    .try_into()
    .unwrap();
    let signed = bundle_in_memory()
        .sign(&ml_core::SigningKey::from_bytes(&host_secret))
        .unwrap();
    let file = dir.join("signed.json");
    std::fs::write(&file, serde_json::to_string_pretty(&signed).unwrap()).unwrap();

    let (code, r, err) = verify(&file, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(r["signed"], true);
    assert_eq!(r["signer"], hex::encode(signed.signer));

    // Requiring a signer of a bundle that has none is a refusal, not a pass.
    let bare = dir.join("bare.json");
    std::fs::write(&bare, serde_json::to_string(&signed.bundle).unwrap()).unwrap();
    let (code, r, _) = verify(&bare, &["--signer", &format!("{}.pub", host.display())]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "SIGNATURE_INVALID");
    assert!(r["detail"].as_str().unwrap().contains("not signed"));

    // The exporter you expect, from its public key alone.
    let host_pub = format!("{}.pub", host.display());
    let (code, r, err) = verify(&file, &["--signer", &host_pub]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(r["verified"], true);

    // A different expected exporter is refused, naming both keys.
    let other_pub = format!("{}.pub", other.display());
    let (code, r, _) = verify(&file, &["--signer", &other_pub]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "SIGNATURE_INVALID");
    assert!(r["detail"].as_str().unwrap().contains("expected"));

    // Tampering with a signed bundle fails the signature, not just the chain.
    let mut forged: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    forged["bundle"]["generated_at"] = 0.into();
    let forged_file = dir.join("forged.json");
    std::fs::write(&forged_file, forged.to_string()).unwrap();
    let (code, r, _) = verify(&forged_file, &[]);
    assert_eq!(code, 2);
    assert_eq!(r["refused"], "SIGNATURE_INVALID");
}
