//! pecan CLI entrypoint.
//!
//! Subcommands exercise the domain against any agent directory (override
//! with `PECAN_AGENT_DIR`):
//!
//! ```text
//! pecan seed <dir>            fabricate a synthetic pi tree under <dir>
//! pecan seed-init             first-run project seeding against $PECAN_AGENT_DIR
//! pecan index [--all|--json]  print projects + sessions by open time
//! pecan add|remove <cwd>      curate the project allowlist
//! pecan settle|reopen <id>    fold/unfold a thread
//! pecan thread <id>           dump a parsed transcript
//! pecan serve [--port N]      run the local web UI
//! ```
#![allow(clippy::print_stdout, reason = "binary entrypoint CLI output")]
#![allow(clippy::print_stderr, reason = "binary entrypoint CLI diagnostics")]

mod cli;
mod server;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pecan: {error}");
            ExitCode::FAILURE
        }
    }
}
