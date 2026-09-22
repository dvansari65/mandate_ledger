//! The offline commands, driven as a script would drive them.

mod common;

use common::*;

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
