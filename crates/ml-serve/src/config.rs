//! Configuration, from the environment, checked all at once.
//!
//! Every problem is reported together: one restart to fix a deployment,
//! not one per variable. A value that is a secret — the database URL, the
//! API keys, the evidence key — never appears in an error message.
//!
//! | Variable | Meaning | Default |
//! |---|---|---|
//! | `ML_LISTEN` | Address to bind | `127.0.0.1:8080` |
//! | `ML_DATABASE_URL` | PostgreSQL URL | required |
//! | `ML_DB_POOL` | Connections, 1–64; also the ledger calls in flight at once | `8` |
//! | `ML_API_KEYS` | Accepted keys, comma-separated, each at least 32 characters | required |
//! | `ML_TRUSTED_SIGNERS` | `principal=hex-public-key`, comma-separated | none: every mandate refused |
//! | `ML_MERCHANT_KEYS` | `key-id=hex-public-key`, comma-separated | none: only unsigned carts |
//! | `ML_RAIL_ID` | The rail's id, recorded with every payment | `mock` |
//! | `ML_RAIL_FINALITY` | Confirmations the rail needs, at least 1 | `1` |
//! | `ML_EVIDENCE_KEY` | Hex Ed25519 seed that signs exported evidence | none: bare bundles |
//! | `ML_REQUEST_TIMEOUT_SECS` | One ledger call, 1–300 | `30` |
//! | `ML_BODY_LIMIT` | Largest request body, bytes, 1 KiB–16 MiB | `262144` |
//! | `ML_SHUTDOWN_GRACE_SECS` | How long in-flight requests get after a stop signal, 0–600 | `20` |
//! | `ML_LOG` | A `tracing` filter | `info` |
//! | `ML_LOG_FORMAT` | `json` or `text` | `json` |

use crate::auth::ApiKeys;
use ml_core::{PrincipalId, Secret, SigningKey, VerifyingKey};
use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::str::FromStr;
use std::time::Duration;

/// What the service runs with. Its `Debug` form redacts the secrets.
#[derive(Debug)]
pub struct Config {
    /// Address to bind.
    pub listen: SocketAddr,
    /// The PostgreSQL connection. It may carry a password, so it is never
    /// displayed.
    pub database: Secret<postgres::Config>,
    /// Pool size, and the number of ledger calls in flight at once.
    pub db_pool: u32,
    /// Accepted API keys.
    pub api_keys: ApiKeys,
    /// Which keys may sign mandates for which principal.
    pub signers: Vec<(PrincipalId, VerifyingKey)>,
    /// Which merchant keys may sign carts, by key id.
    pub merchant_keys: Vec<(String, VerifyingKey)>,
    /// The rail's id.
    pub rail_id: String,
    /// Confirmations the rail needs to call a payment final.
    pub rail_finality: NonZeroU32,
    /// The key that signs exported evidence, if any.
    pub evidence_key: Option<Secret<SigningKey>>,
    /// How long one ledger call may take.
    pub request_timeout: Duration,
    /// The largest request body accepted.
    pub body_limit: usize,
    /// How long in-flight requests get after a stop signal.
    pub shutdown_grace: Duration,
    /// The `tracing` filter.
    pub log_filter: String,
    /// How log lines are written.
    pub log_format: LogFormat,
}

/// How log lines are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per line, for machines.
    Json,
    /// Human-readable lines, for a terminal.
    Text,
}

impl FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            _ => Err("must be json or text".to_owned()),
        }
    }
}

/// Every problem with the environment, together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    problems: Vec<String>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, problem) in self.problems.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            f.write_str(problem)?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

/// Collects problems while reading variables.
struct Reader<'a> {
    get: &'a dyn Fn(&str) -> Option<String>,
    problems: Vec<String>,
}

impl Reader<'_> {
    /// The variable's value, trimmed, if it is set and not blank.
    fn raw(&self, name: &str) -> Option<String> {
        (self.get)(name)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    }

    fn problem(&mut self, name: &str, what: impl fmt::Display) {
        self.problems.push(format!("{name}: {what}"));
    }

    /// A parsed value, or the default. A value that does not parse is a
    /// problem, and the default stands in so that reading can go on.
    fn parsed<T: FromStr>(&mut self, name: &str, default: T) -> T
    where
        T::Err: fmt::Display,
    {
        match self.raw(name) {
            None => default,
            Some(raw) => match raw.parse() {
                Ok(value) => value,
                Err(e) => {
                    self.problem(name, e);
                    default
                }
            },
        }
    }

    /// A number within `range`.
    fn within(&mut self, name: &str, default: u64, range: std::ops::RangeInclusive<u64>) -> u64 {
        let value = self.parsed(name, default);
        if range.contains(&value) {
            value
        } else {
            self.problem(
                name,
                format!("must be between {} and {}", range.start(), range.end()),
            );
            default
        }
    }

    /// `name=hex-key` pairs, comma-separated, each key a valid Ed25519
    /// public key, each name checked by `check`.
    fn keyed<T>(
        &mut self,
        name: &str,
        check: impl Fn(&str) -> Result<T, String>,
    ) -> Vec<(T, VerifyingKey)> {
        let mut pairs = Vec::new();
        let Some(raw) = self.raw(name) else {
            return pairs;
        };
        for (i, entry) in raw.split(',').map(str::trim).enumerate() {
            let Some((id, key)) = entry.split_once('=') else {
                self.problem(name, format!("entry {} is not NAME=HEX", i + 1));
                continue;
            };
            let id = match check(id.trim()) {
                Ok(id) => id,
                Err(e) => {
                    self.problem(name, format!("entry {}: {e}", i + 1));
                    continue;
                }
            };
            match public_key(key.trim()) {
                Ok(key) => pairs.push((id, key)),
                Err(e) => self.problem(name, format!("entry {}: {e}", i + 1)),
            }
        }
        pairs
    }
}

/// 32 bytes of hex.
fn seed(hex: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex).map_err(|_| "not hex".to_owned())?;
    <[u8; 32]>::try_from(bytes).map_err(|b| format!("{} bytes, need 32", b.len()))
}

fn public_key(hex: &str) -> Result<VerifyingKey, String> {
    VerifyingKey::from_bytes(&seed(hex)?).map_err(|_| "not a valid Ed25519 public key".to_owned())
}

impl Config {
    /// Read the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(&|name| std::env::var(name).ok())
    }

    /// Read variables through `get`, so that a test can supply them.
    pub fn from_vars(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut r = Reader {
            get,
            problems: Vec::new(),
        };

        let listen = r.parsed("ML_LISTEN", SocketAddr::from(([127, 0, 0, 1], 8080)));
        let database = match r.raw("ML_DATABASE_URL") {
            None => {
                r.problem("ML_DATABASE_URL", "required");
                None
            }
            // The parse error is not shown: it can quote a character of
            // the string, and the string can carry a password.
            Some(url) => {
                let parsed = url.parse::<postgres::Config>().ok();
                if parsed.is_none() {
                    r.problem("ML_DATABASE_URL", "not a PostgreSQL connection string");
                }
                parsed
            }
        };
        let db_pool = u32::try_from(r.within("ML_DB_POOL", 8, 1..=64)).unwrap_or(8);
        let api_keys = match r.raw("ML_API_KEYS") {
            None => {
                r.problem("ML_API_KEYS", "required");
                None
            }
            Some(raw) => match ApiKeys::new(raw.split(',').map(str::trim)) {
                Ok(keys) => Some(keys),
                Err(e) => {
                    r.problem("ML_API_KEYS", e);
                    None
                }
            },
        };
        let signers = r.keyed("ML_TRUSTED_SIGNERS", |id| {
            PrincipalId::new(id).map_err(|e| e.to_string())
        });
        let merchant_keys = r.keyed("ML_MERCHANT_KEYS", |id| {
            if id.is_empty() {
                Err("empty key id".to_owned())
            } else {
                Ok(id.to_owned())
            }
        });
        let rail_id = r.raw("ML_RAIL_ID").unwrap_or_else(|| "mock".to_owned());
        if rail_id.len() > 64 {
            r.problem("ML_RAIL_ID", "longer than 64 bytes");
        }
        let rail_finality = r.parsed("ML_RAIL_FINALITY", NonZeroU32::MIN);
        let evidence_key = r.raw("ML_EVIDENCE_KEY").and_then(|hex| match seed(&hex) {
            Ok(seed) => Some(Secret::new(SigningKey::from_bytes(&seed))),
            Err(e) => {
                r.problem("ML_EVIDENCE_KEY", e);
                None
            }
        });
        let request_timeout = Duration::from_secs(r.within("ML_REQUEST_TIMEOUT_SECS", 30, 1..=300));
        let body_limit =
            usize::try_from(r.within("ML_BODY_LIMIT", 262_144, 1024..=16 * 1024 * 1024))
                .unwrap_or(262_144);
        let shutdown_grace = Duration::from_secs(r.within("ML_SHUTDOWN_GRACE_SECS", 20, 0..=600));
        let log_filter = r.raw("ML_LOG").unwrap_or_else(|| "info".to_owned());
        if tracing_subscriber::EnvFilter::try_new(&log_filter).is_err() {
            r.problem("ML_LOG", "not a tracing filter");
        }
        let log_format = r.parsed("ML_LOG_FORMAT", LogFormat::Json);

        match (r.problems.is_empty(), api_keys, database) {
            (true, Some(api_keys), Some(database)) => Ok(Self {
                listen,
                database: Secret::new(database),
                db_pool,
                api_keys,
                signers,
                merchant_keys,
                rail_id,
                rail_finality,
                evidence_key,
                request_timeout,
                body_limit,
                shutdown_grace,
                log_filter,
                log_format,
            }),
            _ => Err(ConfigError {
                problems: r.problems,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn read(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let vars = vars(pairs);
        Config::from_vars(&|name| vars.get(name).cloned())
    }

    const KEY: &str = "0123456789abcdef0123456789abcdef";
    const PUBLIC: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    #[test]
    fn the_minimum_is_a_database_and_a_key() {
        let config = read(&[("ML_DATABASE_URL", "postgres://x"), ("ML_API_KEYS", KEY)]).unwrap();
        assert_eq!(config.listen.port(), 8080);
        assert!(config.listen.ip().is_loopback());
        assert_eq!(config.db_pool, 8);
        assert_eq!(config.rail_id, "mock");
        assert_eq!(config.signers.len(), 0);
        assert!(config.evidence_key.is_none());
        assert_eq!(config.log_filter, "info");
        assert_eq!(config.log_format, LogFormat::Json);
    }

    #[test]
    fn every_problem_is_reported_at_once_and_no_secret_with_it() {
        let err = read(&[
            ("ML_API_KEYS", "short"),
            ("ML_DB_POOL", "0"),
            ("ML_TRUSTED_SIGNERS", "alice=nothex"),
            ("ML_EVIDENCE_KEY", "abcd"),
            ("ML_LISTEN", "nowhere"),
            ("ML_LOG", "info,,=bogus"),
            ("ML_LOG_FORMAT", "txt"),
        ])
        .unwrap_err();
        let text = err.to_string();
        for expected in [
            "ML_DATABASE_URL: required",
            "ML_API_KEYS: key 1 is shorter",
            "ML_DB_POOL: must be between 1 and 64",
            "ML_TRUSTED_SIGNERS: entry 1: not hex",
            "ML_EVIDENCE_KEY: 2 bytes, need 32",
            "ML_LISTEN:",
            "ML_LOG: not a tracing filter",
            "ML_LOG_FORMAT: must be json or text",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(
            !text.contains("short") || text.contains("shorter"),
            "{text}"
        );
        assert!(!text.contains("abcd"), "{text}");
    }

    #[test]
    fn a_database_url_that_is_not_one_is_refused_without_being_shown() {
        let err = read(&[
            (
                "ML_DATABASE_URL",
                "postgres://user:hunter2@host:notaport/db",
            ),
            ("ML_API_KEYS", KEY),
        ])
        .unwrap_err();
        let text = err.to_string();
        assert_eq!(text, "ML_DATABASE_URL: not a PostgreSQL connection string");
        assert!(!text.contains("hunter2"));
    }

    #[test]
    fn keys_are_parsed_and_validated() {
        let config = read(&[
            ("ML_DATABASE_URL", "postgres://x"),
            ("ML_API_KEYS", &format!("{KEY}, {KEY}x")),
            ("ML_TRUSTED_SIGNERS", &format!("user:alice={PUBLIC}")),
            ("ML_MERCHANT_KEYS", &format!("bb-2026={PUBLIC}")),
            ("ML_RAIL_FINALITY", "3"),
            ("ML_LOG_FORMAT", "text"),
        ])
        .unwrap();
        assert_eq!(config.signers.len(), 1);
        assert_eq!(config.signers[0].0.as_str(), "user:alice");
        assert_eq!(config.merchant_keys[0].0, "bb-2026");
        assert_eq!(config.rail_finality.get(), 3);
        assert_eq!(config.log_format, LogFormat::Text);
        assert!(
            config
                .api_keys
                .check(format!("{KEY}x").as_bytes())
                .is_some()
        );
    }

    #[test]
    fn zero_finality_is_refused() {
        let err = read(&[
            ("ML_DATABASE_URL", "postgres://x"),
            ("ML_API_KEYS", KEY),
            ("ML_RAIL_FINALITY", "0"),
        ])
        .unwrap_err();
        assert!(err.to_string().starts_with("ML_RAIL_FINALITY:"), "{err}");
    }
}
