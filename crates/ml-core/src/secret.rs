//! A wrapper that keeps sensitive values out of logs (property P16).

use std::fmt;

/// Holds a value whose `Debug` output is always `[REDACTED]`.
///
/// `Secret` deliberately does not implement `Serialize`, `Display`, or
/// `Clone` for `T: !Clone`, so a payment credential cannot drift into a
/// log line or an evidence bundle by accident. Call [`Secret::expose`] at
/// the one place the raw value is genuinely needed.
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wrap a sensitive value.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Borrow the raw value. Keep the scope of this call as small as possible.
    pub const fn expose(&self) -> &T {
        &self.0
    }

    /// Consume the wrapper and return the raw value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl<T: Clone> Clone for Secret<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let s = Secret::new("pay_token_123");
        assert_eq!(format!("{s:?}"), "[REDACTED]");
        assert_eq!(*s.expose(), "pay_token_123");
    }
}
