//! The `otag` binary: command-line interface, client, and daemon entry point.
#![forbid(unsafe_code)]

mod agents;
mod attach;
mod cli;
mod client;
mod daemon;
mod git;
mod handoff;
mod hook;
mod install;
mod logging;
mod notify;
mod persist;
mod usage;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // One line per cause, so the user sees what happened and what to
            // do without a Rust backtrace in the way.
            eprintln!("otag: {error}");
            for cause in error.chain().skip(1) {
                eprintln!("  caused by: {cause}");
            }
            ExitCode::FAILURE
        }
    }
}
