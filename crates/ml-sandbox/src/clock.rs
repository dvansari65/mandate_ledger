//! `ml clock`: the sandbox's clock. Frozen, it is what every engine command
//! against this database reads as "now" — so a mandate can expire, and a
//! velocity window can pass, without waiting for a real day.
//!
//! It is global to the database: freezing it changes time for every process
//! using that database, including a test suite running alongside. Reset it
//! when you are done.

use crate::Failure;
use crate::engine::{self, DbArgs};
use crate::report::Report;
use crate::sandbox;
use clap::Subcommand;
use ml_core::{Clock as _, SystemClock, Timestamp};

#[derive(Subcommand)]
pub enum Command {
    /// Now, and whether it is wall time or frozen.
    Show {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Freeze the clock at this instant, in Unix seconds.
    Set {
        #[arg(value_name = "UNIX")]
        at: i64,
        #[command(flatten)]
        db: DbArgs,
    },
    /// Move the frozen clock forward. Freezes it at wall time first if it
    /// was not frozen.
    Advance {
        #[arg(value_name = "SECS")]
        secs: i64,
        #[command(flatten)]
        db: DbArgs,
    },
    /// Back to wall time.
    Reset {
        #[command(flatten)]
        db: DbArgs,
    },
}

pub fn run(command: &Command) -> Result<Report, Failure> {
    let db = match command {
        Command::Show { db }
        | Command::Set { db, .. }
        | Command::Advance { db, .. }
        | Command::Reset { db } => db,
    };
    let store = engine::store(db)?;
    let pool = store.pool();
    match command {
        Command::Set { at, .. } => {
            sandbox::set_clock(pool, Timestamp(*at)).map_err(Failure::undecided)?;
        }
        Command::Advance { secs, .. } => {
            sandbox::advance_clock(pool, *secs).map_err(Failure::undecided)?;
        }
        Command::Reset { .. } => {
            sandbox::reset_clock(pool).map_err(Failure::undecided)?;
        }
        Command::Show { .. } => {}
    }
    let frozen = sandbox::clock(pool).map_err(Failure::undecided)?;
    Ok(Report::new()
        .with("now", frozen.unwrap_or_else(|| SystemClock.now()).as_secs())
        .with("source", if frozen.is_some() { "frozen" } else { "wall" }))
}
