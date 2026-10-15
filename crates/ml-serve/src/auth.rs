//! API keys: who may ask the ledger anything.
//!
//! A key is a bearer token in the `Authorization` header, and a key is full
//! access — there is one tenant per deployment, and the key is the
//! operator's. The service keeps only SHA-256 digests of the keys it
//! accepts, and compares a presented key against every one of them in
//! constant time, so neither the length of a key nor which key matched
//! leaks through timing. A key never appears in a log; its fingerprint does.

use crate::app::App;
use crate::error::ApiError;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::header::AUTHORIZATION;
use sha2::{Digest, Sha256};
use std::fmt;
use subtle::{Choice, ConstantTimeEq};

/// The shortest key accepted. Thirty-two characters of a random string are
/// well beyond brute force; a shorter key is refused at startup.
pub const MIN_KEY_LEN: usize = 32;

/// The accepted keys, as digests.
#[derive(Clone)]
pub struct ApiKeys {
    digests: Vec<[u8; 32]>,
}

impl fmt::Debug for ApiKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ApiKeys({} keys)", self.digests.len())
    }
}

/// A key as the logs name it: the first eight hex characters of its digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyFingerprint([u8; 4]);

impl fmt::Display for KeyFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl ApiKeys {
    /// Accept exactly these keys. A key shorter than [`MIN_KEY_LEN`] is an
    /// error that names its position, never its content.
    pub fn new<'a>(keys: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        let mut digests = Vec::new();
        for (i, key) in keys.into_iter().enumerate() {
            if key.len() < MIN_KEY_LEN {
                return Err(format!(
                    "key {} is shorter than {MIN_KEY_LEN} characters",
                    i + 1
                ));
            }
            digests.push(Sha256::digest(key.as_bytes()).into());
        }
        if digests.is_empty() {
            return Err("no keys".to_owned());
        }
        Ok(Self { digests })
    }

    /// Whether `presented` is an accepted key. The work done is the same
    /// whichever key it is, and whether or not it is one.
    #[must_use]
    pub fn check(&self, presented: &[u8]) -> Option<KeyFingerprint> {
        let digest: [u8; 32] = Sha256::digest(presented).into();
        let mut matched = Choice::from(0u8);
        for known in &self.digests {
            matched |= known.ct_eq(&digest);
        }
        bool::from(matched).then(|| KeyFingerprint([digest[0], digest[1], digest[2], digest[3]]))
    }
}

/// The bearer token in an `Authorization` header, if the header is one.
fn bearer(request: &Request) -> Option<&[u8]> {
    let value = request.headers().get(AUTHORIZATION)?.as_bytes();
    let (scheme, token) = value.split_at_checked(7)?;
    scheme.eq_ignore_ascii_case(b"bearer ").then_some(token)
}

/// Refuse the request unless it carries an accepted key. On success the
/// key's fingerprint is in the request extensions and on the request span.
pub async fn require_key(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let Some(fingerprint) = bearer(&request).and_then(|token| app.keys().check(token)) else {
        return ApiError::Unauthorized.into_response();
    };
    tracing::Span::current().record("key", tracing::field::display(fingerprint));
    request.extensions_mut().insert(fingerprint);
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_checked_by_digest() {
        let keys = ApiKeys::new(["a".repeat(32).as_str(), "b".repeat(40).as_str()]).unwrap();
        assert!(keys.check("a".repeat(32).as_bytes()).is_some());
        assert!(keys.check("b".repeat(40).as_bytes()).is_some());
        assert!(keys.check("a".repeat(31).as_bytes()).is_none());
        assert!(keys.check(b"").is_none());
    }

    #[test]
    fn short_or_missing_keys_are_refused_without_being_named() {
        let err = ApiKeys::new(["ok".repeat(16).as_str(), "short"]).unwrap_err();
        assert_eq!(err, "key 2 is shorter than 32 characters");
        assert_eq!(ApiKeys::new([]).unwrap_err(), "no keys");
    }
}
