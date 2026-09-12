//! Time as an explicit dependency.
//!
//! The engine never reads the wall clock directly; it asks a [`Clock`].
//! Production uses [`SystemClock`]; tests use [`FixedClock`] so expiry and
//! velocity rules are deterministic.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// Construct from Unix seconds.
    #[must_use]
    pub const fn unix(secs: i64) -> Self {
        Self(secs)
    }

    /// Unix seconds.
    #[must_use]
    pub const fn as_secs(self) -> i64 {
        self.0
    }

    /// `self - secs`, saturating at `i64::MIN`.
    #[must_use]
    pub const fn saturating_sub_secs(self, secs: i64) -> Self {
        Self(self.0.saturating_sub(secs))
    }
}

/// Source of the current time.
pub trait Clock: Send + Sync {
    /// The current time.
    fn now(&self) -> Timestamp;
}

/// Wall-clock time.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        Timestamp(secs)
    }
}

/// A clock that only moves when told to. For tests.
#[derive(Debug)]
pub struct FixedClock(AtomicI64);

impl FixedClock {
    /// A clock frozen at `secs`.
    #[must_use]
    pub const fn at(secs: i64) -> Self {
        Self(AtomicI64::new(secs))
    }

    /// Set the time.
    pub fn set(&self, secs: i64) {
        self.0.store(secs, Ordering::SeqCst);
    }

    /// Move the time forward by `secs`.
    pub fn advance(&self, secs: i64) {
        self.0.fetch_add(secs, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.0.load(Ordering::SeqCst))
    }
}

impl<C: Clock + ?Sized> Clock for std::sync::Arc<C> {
    fn now(&self) -> Timestamp {
        (**self).now()
    }
}
