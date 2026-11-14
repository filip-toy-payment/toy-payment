//! Command line entry point.
//!
//! Usage: `toy-payment <transactions.csv>`. The accounts are written to
//! standard output. Skipped rows and errors are reported on standard error.

use std::env;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

/// Exit code for a wrong invocation.
const USAGE_ERROR: u8 = 2;

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let (Some(path), None) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: toy-payment <transactions.csv>");
        return ExitCode::from(USAGE_ERROR);
    };

    let input = match File::open(&path) {
        Ok(input) => input,
        Err(error) => {
            eprintln!("error: cannot open {}: {error}", Path::new(&path).display());
            return ExitCode::FAILURE;
        }
    };

    // Buffered so that a file with many bad rows is not slowed down by logging.
    let mut diagnostics = BufWriter::new(io::stderr().lock());
    // `run` flushes the diagnostics itself and fails if it cannot.
    match toy_payment::run(input, io::stdout().lock(), &mut diagnostics) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The exit code already reports the failure. A broken standard
            // error cannot change it.
            let _ = writeln!(diagnostics, "error: {error}");
            let _ = diagnostics.flush();
            ExitCode::FAILURE
        }
    }
}
