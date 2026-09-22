//! `ml` — drive the mandate-ledger engine from a terminal.
//!
//! Every command follows the same conventions, and scripts rely on them:
//!
//! - **State is files and the database, nothing else.** Keys, mandates and
//!   carts are JSON files you can read and edit. Every command is one
//!   process; nothing is remembered between two invocations except what the
//!   store holds.
//! - **`--json`** prints one JSON object on stdout in place of the
//!   human-readable lines, with the same keys.
//! - **Exit codes mean something.** `0`: the step was allowed, or the command
//!   had nothing to decide. `2`: the ledger evaluated the step and refused it
//!   — a decision, not a failure; the report on stdout names the context, the
//!   stage, the code and the reason. `1`: the ledger could not decide, because
//!   a file, the store or the rail failed. `64`: the command line was wrong.
//!   Errors go to stderr, prefixed `error:`.

#![forbid(unsafe_code)]

mod authorize;
mod cart;
mod contexts;
mod engine;
mod files;
mod keys;
mod log;
mod mandate;
mod report;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// Drive the mandate-ledger engine from a terminal.
#[derive(Parser)]
#[command(name = "ml", version, about)]
struct Cli {
    /// Print one JSON object instead of human-readable lines.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Signing keys for the sandbox.
    Keys {
        #[command(subcommand)]
        command: keys::Command,
    },
    /// Mandates: what a principal allows an agent to spend.
    Mandate {
        #[command(subcommand)]
        command: mandate::Command,
    },
    /// Carts: what the agent wants to buy.
    Cart {
        #[command(subcommand)]
        command: cart::Command,
    },
    /// Check a cart against a mandate and reserve its total.
    Authorize(authorize::Cmd),
    /// Every event in every context, in order — refusals included.
    Log(log::Cmd),
    /// Where every context stands now.
    Contexts(contexts::Cmd),
}

/// What the shell sees. Scripts branch on these, so they are part of the
/// interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Exit {
    /// The step was allowed, or there was nothing to decide.
    Allowed = 0,
    /// The ledger could not decide: a file, the store or the rail failed.
    Undecided = 1,
    /// The ledger evaluated the step and refused it.
    Refused = 2,
    /// The command line was wrong. Distinct from a refusal on purpose: a
    /// script must never mistake a typo for a decision.
    Usage = 64,
}

impl Exit {
    const fn code(self) -> u8 {
        self as u8
    }
}

/// Why a command could not finish, and what the shell should see.
#[derive(Debug)]
pub struct Failure {
    message: String,
    exit: Exit,
}

impl Failure {
    /// The command could not reach a decision.
    fn undecided(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit: Exit::Undecided,
        }
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // `--help` and `--version` print and succeed; anything else is a
            // usage error.
            let _ = e.print();
            let exit = if e.use_stderr() {
                Exit::Usage
            } else {
                Exit::Allowed
            };
            return ExitCode::from(exit.code());
        }
    };

    let result = match cli.command {
        Command::Keys { command } => keys::run(command),
        Command::Mandate { command } => mandate::run(command),
        Command::Cart { command } => cart::run(command),
        Command::Authorize(cmd) => authorize::run(&cmd),
        Command::Log(cmd) => log::run(&cmd),
        Command::Contexts(cmd) => contexts::run(&cmd),
    };

    match result {
        Ok(report) => {
            print!(
                "{}",
                if cli.json {
                    report.json()
                } else {
                    report.human()
                }
            );
            ExitCode::from(report.exit().code())
        }
        Err(failure) => {
            eprintln!("error: {}", failure.message);
            ExitCode::from(failure.exit.code())
        }
    }
}
