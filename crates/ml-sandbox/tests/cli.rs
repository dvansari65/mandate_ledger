//! The `ml` binary, driven as a script would drive it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn ml() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ml"))
}

/// A fresh directory per test, so tests never see each other's files.
fn workdir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ml-sandbox-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(cmd: &mut Command) -> (i32, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = cmd.output().unwrap();
    (
        status.code().unwrap(),
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON: {e}\n{text}"))
}

const BODY: &str = r#"{
  "id": "mnd-1",
  "principal": "user:alice",
  "agent": "agent:shopper",
  "scope": {
    "merchants": ["bigbasket.com", "*.zepto.com"],
    "categories": ["grocery"],
    "currency": "INR",
    "max_per_txn": { "amount": "2000.00", "currency": "INR" },
    "max_total": { "amount": "8000.00", "currency": "INR" },
    "valid_from": 1800000000,
    "valid_until": 1802592000,
    "velocity": { "max_count": 5, "window_secs": 86400 },
    "min_attestation": "merchant_signed"
  },
  "issued_at": 1800000000
}"#;

fn new_key(dir: &Path) -> PathBuf {
    let key = dir.join("user.key");
    let (code, _, err) = run(ml().args(["keys", "new", "--out"]).arg(&key));
    assert_eq!(code, 0, "{err}");
    key
}

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
    let key = new_key(&dir);
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

    let mandate: ml_core::Mandate = json(&std::fs::read_to_string(&out).unwrap())
        .pipe(serde_json::from_value)
        .unwrap();
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
    let key = new_key(&dir);
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
    let key = new_key(&dir);
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

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}
