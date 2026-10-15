//! `ml-serve`: mandate-ledger as a service. Configuration comes from the
//! environment; see the `ml_serve::config` documentation for every
//! variable. Logs are JSON on stderr unless `ML_LOG_FORMAT=text`;
//! `ML_LOG` is a `tracing` filter, `info` by default.

#![forbid(unsafe_code)]

use ml_serve::Config;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: the configuration is not usable:\n{e}");
            return ExitCode::from(64);
        }
    };
    let filter = EnvFilter::try_from_env("ML_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let text = std::env::var("ML_LOG_FORMAT").is_ok_and(|f| f.eq_ignore_ascii_case("text"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    if text {
        subscriber.init();
    } else {
        subscriber.json().init();
    }

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("error: cannot start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(ml_serve::run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "stopping");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
