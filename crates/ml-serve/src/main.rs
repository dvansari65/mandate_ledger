//! `ml-serve`: mandate-ledger as a service. Configuration comes from the
//! environment; see the `ml_serve::config` documentation for every
//! variable. Logs are JSON on stderr unless `ML_LOG_FORMAT=text`;
//! `ML_LOG` is a `tracing` filter, `info` by default.

#![forbid(unsafe_code)]

use ml_serve::{Config, LogFormat};
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
    // Validated by `Config`; the fallback is unreachable.
    let filter = EnvFilter::try_new(&config.log_filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    match config.log_format {
        LogFormat::Text => subscriber.init(),
        LogFormat::Json => subscriber.json().init(),
    }

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("error: cannot start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let app = match runtime.block_on(ml_serve::assemble(&config)) {
        Ok(app) => app,
        Err(e) => {
            tracing::error!(error = %e, "cannot start");
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let served = runtime.block_on(ml_serve::serve(&config, &app));
    // Ledger calls still on blocking threads get the grace once more; then
    // the runtime is gone, whatever they are doing, and so is the process.
    runtime.shutdown_timeout(config.shutdown_grace);
    // The last reference to the store goes here, on a thread no runtime
    // owns: its connections block to close.
    drop(app);
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "stopping");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
