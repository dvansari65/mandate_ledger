//! Driving the `ml` binary the way a script does, and the fixtures it needs.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn ml() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ml"));
    // Never let the developer's own database leak into a test.
    cmd.env_remove("ML_DATABASE_URL");
    cmd
}

/// A fresh directory per test, so tests never see each other's files.
pub fn workdir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ml-sandbox-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn run(cmd: &mut Command) -> (i32, String, String) {
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

pub fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON: {e}\n{text}"))
}

/// A suffix unique to this test process, so a durable database never
/// confuses two runs' mandates, budgets or request keys.
pub fn unique() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{}-{nanos:x}", std::process::id())
}

/// A mandate body for `principal`, allowing grocery at bigbasket.com up to
/// 2,000 per purchase and 8,000 in total, merchant-signed carts only. Valid
/// from 2023 until 2100: the engine commands run on the wall clock.
pub fn body(id: &str, principal: &str) -> String {
    format!(
        r#"{{
  "id": "{id}",
  "principal": "{principal}",
  "agent": "agent:shopper",
  "scope": {{
    "merchants": ["bigbasket.com", "*.zepto.com"],
    "categories": ["grocery"],
    "currency": "INR",
    "max_per_txn": {{ "amount": "2000.00", "currency": "INR" }},
    "max_total": {{ "amount": "8000.00", "currency": "INR" }},
    "valid_from": 1700000000,
    "valid_until": 4102444800,
    "velocity": {{ "max_count": 5, "window_secs": 86400 }},
    "min_attestation": "merchant_signed"
  }},
  "issued_at": 1700000000
}}"#
    )
}

pub const BODY: &str = r#"{
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

/// A grocery cart at bigbasket.com for `total` INR.
pub fn cart(total: &str) -> String {
    format!(
        r#"{{ "merchant": "bigbasket.com", "total": {{ "amount": "{total}", "currency": "INR" }},
  "category": "grocery", "items": [ {{ "sku": "milk-1l", "qty": 2 }} ] }}"#
    )
}

pub fn new_key(dir: &Path, name: &str) -> PathBuf {
    let key = dir.join(name);
    let (code, _, err) = run(ml().args(["keys", "new", "--out"]).arg(&key));
    assert_eq!(code, 0, "{err}");
    key
}

pub fn sign_mandate(dir: &Path, body_text: &str, key: &Path) -> PathBuf {
    let body_file = dir.join("body.json");
    std::fs::write(&body_file, body_text).unwrap();
    let out = dir.join("mandate.json");
    let (code, _, err) = run(ml()
        .args(["mandate", "sign"])
        .arg(&body_file)
        .arg("--key")
        .arg(key)
        .arg("--out")
        .arg(&out));
    assert_eq!(code, 0, "{err}");
    out
}

pub fn sign_cart(dir: &Path, name: &str, cart_text: &str, key: &Path, key_id: &str) -> PathBuf {
    let raw = dir.join(format!("{name}.raw.json"));
    std::fs::write(&raw, cart_text).unwrap();
    let out = dir.join(format!("{name}.json"));
    let (code, _, err) = run(ml()
        .args(["cart", "sign"])
        .arg(&raw)
        .arg("--key")
        .arg(key)
        .args(["--key-id", key_id, "--out"])
        .arg(&out));
    assert_eq!(code, 0, "{err}");
    out
}
