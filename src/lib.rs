//! A toy payments engine.
//!
//! It reads a stream of transactions, applies them to client accounts and
//! reports the final state of every account.

pub mod account;
pub mod amount;
pub mod engine;
pub mod input;
pub mod output;
pub mod transaction;

#[cfg(test)]
mod test_support;

use std::io::{self, Read, Write};

use thiserror::Error;

use crate::account::AccountError;
use crate::engine::{Engine, Rejection};
use crate::input::{InputError, Row};
use crate::output::OutputError;

/// Why a run ended without a complete result.
#[derive(Debug, Error)]
pub enum RunError {
    #[error(transparent)]
    Input(#[from] InputError),
    #[error("cannot write diagnostics: {0}")]
    Diagnostics(#[source] io::Error),
    #[error(transparent)]
    Output(#[from] OutputError),
    #[error("record {record}: the engine broke one of its own rules: {rejection}")]
    EngineFault { record: u64, rejection: Rejection },
}

/// Processes every transaction in `input` and writes the accounts to `output`.
///
/// A row that is skipped is explained on `diagnostics` with its record
/// number. Nothing is written to `output` unless all of `input` was read and
/// every explanation was written and flushed.
pub fn run<R, W, D>(input: R, output: W, mut diagnostics: D) -> Result<(), RunError>
where
    R: Read,
    W: Write,
    D: Write,
{
    let mut engine = Engine::default();

    for row in input::rows(input)? {
        let Row {
            record,
            transaction,
        } = row.map_err(InputError::Io)?;

        let logged = match transaction {
            Err(error) => writeln!(diagnostics, "record {record}: {error}"),
            Ok(transaction) => match engine.apply(transaction) {
                Ok(()) => Ok(()),
                Err(rejection) => match Severity::of(&rejection) {
                    Severity::Ordinary => Ok(()),
                    Severity::DataFault => writeln!(diagnostics, "record {record}: {rejection}"),
                    Severity::EngineFault => {
                        return Err(RunError::EngineFault { record, rejection });
                    }
                },
            },
        };
        logged.map_err(RunError::Diagnostics)?;
    }

    // Every log line must be out before any account is written. A buffered
    // writer reports its failure only here.
    diagnostics.flush().map_err(RunError::Diagnostics)?;

    output::write_accounts(output, engine.accounts())?;
    Ok(())
}

/// What a rejection means for the run.
#[derive(Debug, PartialEq, Eq)]
enum Severity {
    /// An outcome the specification says to ignore. The row passes quietly.
    Ordinary,
    /// The input data is wrong. The row is logged and the run continues.
    DataFault,
    /// No input can cause this. The engine has a bug and the run stops.
    EngineFault,
}

impl Severity {
    fn of(rejection: &Rejection) -> Self {
        // No wildcard arm. A new rejection must be sorted into one group here.
        match rejection {
            Rejection::UnknownDeposit
            | Rejection::ClientMismatch
            | Rejection::AlreadyDisputed
            | Rejection::NotDisputed
            | Rejection::AlreadyChargedBack
            | Rejection::Account(AccountError::Locked | AccountError::InsufficientFunds) => {
                Self::Ordinary
            }
            Rejection::MissingAmount
            | Rejection::DuplicateTransaction
            | Rejection::Account(AccountError::Arithmetic(_)) => Self::DataFault,
            // Held funds always cover a deposit under dispute. The engine
            // resolves or charges back only such a deposit.
            Rejection::Account(AccountError::InsufficientHeld) => Self::EngineFault,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Broken, CAP};
    use std::io::BufWriter;

    /// Output and diagnostics of a successful run.
    fn run_on(input: &str) -> (String, String) {
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        run(input.as_bytes(), &mut output, &mut diagnostics).unwrap();
        (
            String::from_utf8(output).unwrap(),
            String::from_utf8(diagnostics).unwrap(),
        )
    }

    #[test]
    fn processes_the_example_from_the_specification() {
        let (output, diagnostics) = run_on(
            "type, client, tx, amount\n\
             deposit, 1, 1, 1.0\n\
             deposit, 2, 2, 2.0\n\
             deposit, 1, 3, 2.0\n\
             withdrawal, 1, 4, 1.5\n\
             withdrawal, 2, 5, 3.0\n",
        );

        assert_eq!(
            output,
            "client,available,held,total,locked\n\
             1,1.5,0,1.5,false\n\
             2,2,0,2,false\n"
        );
        assert_eq!(diagnostics, "");
    }

    #[test]
    fn writes_only_the_header_for_empty_input() {
        let (output, diagnostics) = run_on("");

        assert_eq!(output, "client,available,held,total,locked\n");
        assert_eq!(diagnostics, "");
    }

    #[test]
    fn handles_a_dispute_that_ends_in_a_chargeback() {
        let (output, diagnostics) = run_on(
            "type,client,tx,amount\n\
             deposit,1,1,10\n\
             withdrawal,1,2,8\n\
             dispute,1,1\n\
             chargeback,1,1,\n\
             deposit,1,3,5\n",
        );

        assert_eq!(
            output,
            "client,available,held,total,locked\n\
             1,-8,0,-8,true\n"
        );
        assert_eq!(diagnostics, "");
    }

    #[test]
    fn skips_bad_rows_and_logs_them_with_their_record_number() {
        let (output, diagnostics) = run_on(
            "type,client,tx,amount\n\
             deposit,1,1,5\n\
             bogus,1,2,1\n\
             deposit,x,3,1\n\
             deposit,1,4,1.23456\n\
             deposit,1,5,2\n",
        );

        assert_eq!(
            output,
            "client,available,held,total,locked\n\
             1,7,0,7,false\n"
        );
        assert_eq!(
            diagnostics,
            "record 3: unknown transaction type \"bogus\"\n\
             record 4: client: invalid digit found in string\n\
             record 5: amount has more than four decimal places\n"
        );
    }

    #[test]
    fn a_bad_row_creates_no_account() {
        let (output, diagnostics) = run_on(
            "bogus,7,1,1\n\
             deposit,8,2,abc\n\
             deposit,9,3,\n\
             deposit,10,4,1,000.50\n",
        );

        assert_eq!(output, "client,available,held,total,locked\n");
        assert_eq!(
            diagnostics,
            "record 1: unknown transaction type \"bogus\"\n\
             record 2: amount is not a decimal number\n\
             record 3: a deposit or withdrawal needs an amount\n\
             record 4: a deposit or withdrawal must not have more than four fields\n"
        );
    }

    #[test]
    fn an_ignored_dispute_creates_no_account() {
        let (output, diagnostics) = run_on("dispute,65535,1\nresolve,7,1\nchargeback,8,1\n");

        assert_eq!(output, "client,available,held,total,locked\n");
        assert_eq!(diagnostics, "");
    }

    #[test]
    fn a_header_with_swapped_columns_ends_the_run_without_any_output() {
        let mut output = Vec::new();

        let result = run(
            "type,tx,client,amount\ndeposit,100,7,5\n".as_bytes(),
            &mut output,
            io::sink(),
        );

        assert!(matches!(
            result,
            Err(RunError::Input(InputError::UnexpectedHeader))
        ));
        assert!(output.is_empty());
    }

    #[test]
    fn logs_data_faults_reported_by_the_engine() {
        let input = format!(
            "deposit,1,1,5\n\
             deposit,1,1,5\n\
             withdrawal,1,1,1\n\
             deposit,3,3,{CAP}\n\
             deposit,3,4,0.0001\n"
        );
        let (_, diagnostics) = run_on(&input);

        assert_eq!(
            diagnostics,
            "record 2: an earlier transaction already uses this ID\n\
             record 3: an earlier transaction already uses this ID\n\
             record 5: balance would exceed the largest supported value\n"
        );
    }

    #[test]
    fn does_not_log_ordinary_rejections() {
        let (output, diagnostics) = run_on(
            "deposit,1,1,5\n\
             withdrawal,1,2,9\n\
             dispute,1,99\n\
             dispute,2,1\n\
             resolve,1,1\n\
             chargeback,1,1\n\
             dispute,1,1\n\
             dispute,1,1\n\
             chargeback,1,1\n\
             dispute,1,1\n\
             deposit,1,3,1\n\
             withdrawal,1,4,1\n",
        );

        assert_eq!(
            output,
            "client,available,held,total,locked\n\
             1,0,0,0,true\n"
        );
        assert_eq!(diagnostics, "");
    }

    #[test]
    fn held_funds_that_do_not_cover_a_dispute_are_an_engine_fault() {
        // No input reaches this state. Only the classification can be tested.
        let rejection = Rejection::Account(AccountError::InsufficientHeld);

        assert_eq!(Severity::of(&rejection), Severity::EngineFault);
    }

    #[test]
    fn a_read_failure_ends_the_run_without_any_output() {
        let input = "deposit,1,1,5\ndeposit,2,2,7\n".as_bytes().chain(Broken);
        let mut output = Vec::new();

        let result = run(input, &mut output, io::sink());

        assert!(matches!(result, Err(RunError::Input(_))));
        assert!(output.is_empty());
    }

    #[test]
    fn a_failing_output_is_reported() {
        let result = run("deposit,1,1,5\n".as_bytes(), Broken, io::sink());

        assert!(matches!(result, Err(RunError::Output(_))));
    }

    #[test]
    fn a_failing_diagnostics_writer_ends_the_run_without_any_output() {
        let mut output = Vec::new();

        let result = run("bogus,1,1,5\n".as_bytes(), &mut output, Broken);

        assert!(matches!(result, Err(RunError::Diagnostics(_))));
        assert!(output.is_empty());
    }

    #[test]
    fn a_diagnostics_writer_that_fails_on_flush_ends_the_run_without_any_output() {
        let mut output = Vec::new();
        // The buffer accepts the log line. The failure shows up at the flush.
        let diagnostics = BufWriter::new(Broken);

        let result = run("bogus,1,1,5\n".as_bytes(), &mut output, diagnostics);

        assert!(matches!(result, Err(RunError::Diagnostics(_))));
        assert!(output.is_empty());
    }
}
