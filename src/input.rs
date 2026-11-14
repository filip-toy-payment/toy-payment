//! Reads transactions from a CSV stream.
//!
//! Rows are read one at a time and never kept. Everything specific to CSV
//! stays in this module. The rest of the crate only sees [`Transaction`].

use std::io::{self, Read};

use serde::Deserialize;
use thiserror::Error;

use crate::amount::AmountError;
use crate::transaction::{ClientId, Kind, Transaction, TxId};

/// One row of the input.
#[derive(Debug)]
pub struct Row {
    /// The position of the row in the input. Counting starts at 1. A header
    /// counts as a record. Blank lines do not.
    pub record: u64,
    pub transaction: Result<Transaction, RowError>,
}

/// Why reading cannot start or continue.
#[derive(Debug, Error)]
pub enum InputError {
    #[error("cannot read input: {0}")]
    Io(#[from] io::Error),
    #[error("unexpected header: the columns must be type, client, tx, amount")]
    UnexpectedHeader,
}

/// Why a row is not a transaction. Later rows are unaffected.
#[derive(Debug, PartialEq, Eq, Error)]
pub enum RowError {
    #[error("{0}")]
    Malformed(String),
    #[error("unknown transaction type {0:?}")]
    UnknownKind(String),
    #[error("a deposit or withdrawal needs an amount")]
    MissingAmount,
    #[error("a deposit or withdrawal must not have data after its amount")]
    ExtraFields,
    #[error(transparent)]
    Amount(#[from] AmountError),
}

/// The column names of a header. The columns are read in this order.
const HEADER: [&str; 4] = ["type", "client", "tx", "amount"];

/// The columns of a row by position.
#[derive(Debug, Deserialize)]
struct CsvRow {
    kind: String,
    client: u16,
    tx: u32,
    // A dispute, resolve or chargeback row may leave the column out entirely.
    #[serde(default)]
    amount: Option<String>,
    /// Every field after the fourth.
    #[serde(default)]
    extra: Vec<String>,
}

impl TryFrom<CsvRow> for Transaction {
    type Error = RowError;

    fn try_from(row: CsvRow) -> Result<Self, Self::Error> {
        let kind = parse_kind(&row.kind).ok_or(RowError::UnknownKind(row.kind))?;
        let amount = match kind {
            Kind::Deposit | Kind::Withdrawal => {
                // Text after the amount makes the amount ambiguous. `1,000.50`
                // would otherwise be read as 1. Empty fields carry no data.
                if row.extra.iter().any(|field| !field.is_empty()) {
                    return Err(RowError::ExtraFields);
                }
                let text = row.amount.ok_or(RowError::MissingAmount)?;
                Some(text.parse()?)
            }
            // These kinds need no amount. Whatever the row holds after the
            // third field is not examined.
            Kind::Dispute | Kind::Resolve | Kind::Chargeback => None,
        };

        Ok(Self {
            kind,
            client: ClientId(row.client),
            tx: TxId(row.tx),
            amount,
        })
    }
}

fn parse_kind(text: &str) -> Option<Kind> {
    const KINDS: [(&str, Kind); 5] = [
        ("deposit", Kind::Deposit),
        ("withdrawal", Kind::Withdrawal),
        ("dispute", Kind::Dispute),
        ("resolve", Kind::Resolve),
        ("chargeback", Kind::Chargeback),
    ];

    KINDS
        .iter()
        .find(|(name, _)| text.eq_ignore_ascii_case(name))
        .map(|(_, kind)| *kind)
}

/// Starts reading rows from a CSV stream.
///
/// A header line is optional. Each item is a row or an I/O error. Reading
/// cannot continue after an I/O error.
pub fn rows<R: Read>(input: R) -> Result<impl Iterator<Item = io::Result<Row>>, InputError> {
    let mut reader = csv::ReaderBuilder::new()
        // The header is detected below. A file without one loses no row.
        .has_headers(false)
        .trim(csv::Trim::All)
        // Rows may have three fields or more than four.
        .flexible(true)
        .from_reader(input);

    // `headers` returns the first record without consuming it.
    let has_header = match reader.headers() {
        Ok(first) => is_header(first)?,
        Err(error) if error.is_io_error() => return Err(io::Error::from(error).into()),
        // The first record is not valid text. It is reported as a bad row.
        Err(_) => false,
    };

    let rows = reader
        .into_deserialize()
        .zip(1..)
        .skip(usize::from(has_header))
        .map(|(result, record)| to_row(record, result));
    Ok(rows)
}

/// Whether the first record is a header.
///
/// A record that starts with `type` is one. Its names must then be the
/// expected ones in the expected order. Columns are read by position. A
/// header with swapped columns would otherwise be misread without any error.
fn is_header(first: &csv::StringRecord) -> Result<bool, InputError> {
    let names_match = |record: &csv::StringRecord, count: usize| {
        record.len() >= count
            && HEADER
                .iter()
                .zip(record)
                .take(count)
                .all(|(name, field)| field.eq_ignore_ascii_case(name))
    };

    if !names_match(first, 1) {
        return Ok(false);
    }
    if !names_match(first, HEADER.len()) {
        return Err(InputError::UnexpectedHeader);
    }
    Ok(true)
}

fn to_row(record: u64, result: csv::Result<CsvRow>) -> io::Result<Row> {
    let transaction = match result {
        Ok(row) => Transaction::try_from(row),
        Err(error) if error.is_io_error() => return Err(error.into()),
        Err(error) => Err(RowError::Malformed(describe(&error))),
    };
    Ok(Row {
        record,
        transaction,
    })
}

/// Explains a row the `csv` crate could not turn into a [`CsvRow`].
fn describe(error: &csv::Error) -> String {
    match error.kind() {
        csv::ErrorKind::Deserialize { err, .. } => match err.field() {
            Some(1) => format!("client: {}", err.kind()),
            Some(2) => format!("tx: {}", err.kind()),
            _ => err.kind().to_string(),
        },
        csv::ErrorKind::Utf8 { .. } => "row is not valid UTF-8".to_owned(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::transaction;

    fn read(input: &str) -> Vec<Row> {
        read_bytes(input.as_bytes())
    }

    fn read_bytes(input: &[u8]) -> Vec<Row> {
        rows(input).unwrap().map(Result::unwrap).collect()
    }

    /// The single transaction in the input.
    fn one(input: &str) -> Transaction {
        let mut rows = read(input);
        assert_eq!(rows.len(), 1, "{input:?}");
        rows.remove(0).transaction.unwrap()
    }

    /// The error of the single row in the input.
    fn one_error(input: &str) -> RowError {
        let mut rows = read(input);
        assert_eq!(rows.len(), 1, "{input:?}");
        rows.remove(0).transaction.unwrap_err()
    }

    /// A reader that fails once the given number of bytes has been read.
    struct FailsAfter<'a> {
        data: &'a [u8],
        remaining: usize,
    }

    impl Read for FailsAfter<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("disk fault"));
            }
            let count = buffer.len().min(self.remaining).min(self.data.len());
            buffer[..count].copy_from_slice(&self.data[..count]);
            self.data = &self.data[count..];
            self.remaining = self.remaining.saturating_sub(count);
            Ok(count)
        }
    }

    #[test]
    fn reads_the_example_from_the_specification() {
        let rows = read(
            "type, client, tx, amount\n\
             deposit, 1, 1, 1.0\n\
             deposit, 2, 2, 2.0\n\
             deposit, 1, 3, 2.0\n\
             withdrawal, 1, 4, 1.5\n\
             withdrawal, 2, 5, 3.0\n",
        );

        let transactions: Vec<_> = rows
            .into_iter()
            .map(|row| row.transaction.unwrap())
            .collect();
        assert_eq!(
            transactions,
            [
                transaction(Kind::Deposit, 1, 1, Some("1.0")),
                transaction(Kind::Deposit, 2, 2, Some("2.0")),
                transaction(Kind::Deposit, 1, 3, Some("2.0")),
                transaction(Kind::Withdrawal, 1, 4, Some("1.5")),
                transaction(Kind::Withdrawal, 2, 5, Some("3.0")),
            ]
        );
    }

    #[test]
    fn skips_a_header_whatever_its_case_or_spacing() {
        for header in [
            "type,client,tx,amount",
            "Type,Client,Tx,Amount",
            " TYPE , client, tx, amount",
        ] {
            let rows = read(&format!("{header}\ndeposit,1,1,1.0\n"));
            assert_eq!(rows.len(), 1, "{header}");
        }
    }

    #[test]
    fn a_header_with_other_columns_stops_the_run() {
        for header in [
            "type,tx,client,amount",
            "type,client,amount,tx",
            "type,client,tx",
            "type,client,tx,value",
            "type",
        ] {
            let input = format!("{header}\ndeposit,1,1,1.0\n");
            let result = rows(input.as_bytes());
            assert!(
                matches!(result, Err(InputError::UnexpectedHeader)),
                "{header}"
            );
        }
    }

    #[test]
    fn a_header_may_name_more_than_four_columns() {
        let rows = read("type,client,tx,amount,note\ndispute,1,1,,late\n");

        assert_eq!(rows.len(), 1);
        assert!(rows[0].transaction.is_ok());
    }

    #[test]
    fn reads_the_first_line_as_a_row_when_there_is_no_header() {
        let rows = read("deposit,1,1,1.0\nwithdrawal,1,2,0.5\n");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].record, 1);
        assert_eq!(rows[1].record, 2);
        assert_eq!(
            rows[0].transaction,
            Ok(transaction(Kind::Deposit, 1, 1, Some("1.0")))
        );
    }

    #[test]
    fn yields_nothing_for_empty_input_or_a_lone_header() {
        assert!(read("").is_empty());
        assert!(read("type,client,tx,amount\n").is_empty());
        assert!(read("type,client,tx,amount").is_empty());
    }

    fn record_numbers(input: &str) -> Vec<u64> {
        read(input).iter().map(|row| row.record).collect()
    }

    #[test]
    fn numbers_every_record_from_one_and_counts_the_header() {
        let with_header =
            "type,client,tx,amount\ndeposit,1,1,1.0\nbogus,1,2,1.0\ndeposit,x,3,1.0\n";
        let without_header = "deposit,1,1,1.0\nbogus,1,2,1.0\ndeposit,x,3,1.0\n";

        assert_eq!(record_numbers(with_header), [2, 3, 4]);
        assert_eq!(record_numbers(without_header), [1, 2, 3]);
    }

    #[test]
    fn blank_lines_are_not_counted_as_records() {
        let input = "type,client,tx,amount\n\ndeposit,1,1,1.0\n\n\ndeposit,1,2,1.0\n";

        assert_eq!(record_numbers(input), [2, 3]);
    }

    #[test]
    fn matches_the_type_without_regard_to_case() {
        for kind in ["deposit", "Deposit", "DEPOSIT", "dEpOsIt"] {
            assert_eq!(
                one(&format!("{kind},1,1,1.0\n")).kind,
                Kind::Deposit,
                "{kind}"
            );
        }
        assert_eq!(one("Withdrawal,1,1,1.0\n").kind, Kind::Withdrawal);
        assert_eq!(one("DISPUTE,1,1\n").kind, Kind::Dispute);
        assert_eq!(one("Resolve,1,1\n").kind, Kind::Resolve);
        assert_eq!(one("ChargeBack,1,1\n").kind, Kind::Chargeback);
    }

    #[test]
    fn rejects_an_unknown_type() {
        assert_eq!(
            one_error("bogus,1,1,1.0\n"),
            RowError::UnknownKind("bogus".to_owned())
        );
        assert_eq!(
            one_error(",1,1,1.0\n"),
            RowError::UnknownKind(String::new())
        );
    }

    #[test]
    fn trims_whitespace_around_every_field() {
        assert_eq!(
            one("  deposit ,\t7 ,  9,   1.2500  \n"),
            transaction(Kind::Deposit, 7, 9, Some("1.25"))
        );
    }

    #[test]
    fn reads_quoted_fields() {
        assert_eq!(
            one("\"deposit\",\"1\",\"2\",\"3.5\"\n"),
            transaction(Kind::Deposit, 1, 2, Some("3.5"))
        );
        assert_eq!(
            read("\"type\",\"client\",\"tx\",\"amount\"\n\"dispute\",\"1\",\"2\"\n").len(),
            1
        );
    }

    #[test]
    fn a_comma_inside_a_quoted_amount_is_part_of_the_amount() {
        assert_eq!(
            one_error("deposit,1,1,\"1,000.50\"\n"),
            RowError::Amount(AmountError::Malformed)
        );
    }

    #[test]
    fn skips_blank_lines() {
        let rows = read("type,client,tx,amount\n\ndeposit,1,1,1.0\n\n\ndeposit,1,2,1.0\n\n");

        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn a_dispute_row_may_have_three_fields_or_an_empty_fourth() {
        let expected = transaction(Kind::Dispute, 1, 5, None);

        assert_eq!(one("dispute,1,5\n"), expected);
        assert_eq!(one("dispute,1,5,\n"), expected);
        assert_eq!(one("dispute, 1, 5, \n"), expected);
    }

    #[test]
    fn the_amount_column_is_never_examined_on_dispute_rows() {
        for kind in ["dispute", "resolve", "chargeback"] {
            for stray in ["5.0", "abc", "-5", "0", "1.23456"] {
                let row = one(&format!("{kind},1,5,{stray}\n"));
                assert_eq!(row.amount, None, "{kind} {stray}");
            }
        }
    }

    #[test]
    fn rejects_a_deposit_or_withdrawal_without_an_amount() {
        for row in ["deposit,1,1,", "deposit,1,1", "withdrawal,1,1,  "] {
            assert_eq!(
                one_error(&format!("{row}\n")),
                RowError::MissingAmount,
                "{row}"
            );
        }
    }

    #[test]
    fn rejects_a_deposit_or_withdrawal_with_an_invalid_amount() {
        assert_eq!(
            one_error("deposit,1,1,abc\n"),
            RowError::Amount(AmountError::Malformed)
        );
        assert_eq!(
            one_error("deposit,1,1,1e3\n"),
            RowError::Amount(AmountError::Malformed)
        );
        assert_eq!(
            one_error("deposit,1,1,1.23456\n"),
            RowError::Amount(AmountError::TooPrecise)
        );
        assert_eq!(
            one_error("withdrawal,1,1,-1\n"),
            RowError::Amount(AmountError::NotPositive)
        );
        assert_eq!(
            one_error("withdrawal,1,1,0\n"),
            RowError::Amount(AmountError::NotPositive)
        );
    }

    #[test]
    fn accepts_trailing_zeros_past_four_decimal_places() {
        assert_eq!(
            one("deposit,1,1,1.50000\n"),
            transaction(Kind::Deposit, 1, 1, Some("1.5"))
        );
    }

    #[test]
    fn rejects_a_client_that_is_not_a_u16() {
        for client in ["x", "", "-1", "1.5", "65536"] {
            let error = one_error(&format!("deposit,{client},1,1.0\n"));
            assert!(
                matches!(&error, RowError::Malformed(reason) if reason.starts_with("client: ")),
                "{client:?}: {error}"
            );
        }
        assert_eq!(one("deposit,65535,1,1.0\n").client, ClientId(u16::MAX));
    }

    #[test]
    fn rejects_a_tx_that_is_not_a_u32() {
        for tx in ["x", "", "-1", "1.5", "4294967296"] {
            let error = one_error(&format!("deposit,1,{tx},1.0\n"));
            assert!(
                matches!(&error, RowError::Malformed(reason) if reason.starts_with("tx: ")),
                "{tx:?}: {error}"
            );
        }
        assert_eq!(one("deposit,1,4294967295,1.0\n").tx, TxId(u32::MAX));
    }

    #[test]
    fn rejects_a_row_with_fewer_than_three_fields() {
        assert!(matches!(one_error("deposit,1\n"), RowError::Malformed(_)));
        assert!(matches!(one_error("deposit\n"), RowError::Malformed(_)));
    }

    #[test]
    fn rejects_a_deposit_or_withdrawal_with_more_than_four_fields() {
        for row in [
            "deposit,1,1,2.5,extra",
            "deposit,1,1,2.5,extra,more",
            "withdrawal,1,1,2.5,extra",
            "deposit,1,1,2.5,,extra",
            "deposit,1,1,2.5,,,,extra",
            "deposit,1,1,2.5, ,extra",
            "withdrawal,1,1,2.5,,extra,",
        ] {
            assert_eq!(
                one_error(&format!("{row}\n")),
                RowError::ExtraFields,
                "{row}"
            );
        }
    }

    #[test]
    fn a_comma_inside_an_amount_is_not_read_as_a_smaller_amount() {
        assert_eq!(one_error("deposit,1,1,1,000.50\n"), RowError::ExtraFields);
    }

    #[test]
    fn empty_fields_after_the_amount_are_tolerated() {
        for row in [
            "deposit,1,1,2.5,",
            "deposit,1,1,2.5,,",
            "deposit,1,1,2.5, ,  ,",
        ] {
            assert_eq!(
                one(&format!("{row}\n")),
                transaction(Kind::Deposit, 1, 1, Some("2.5")),
                "{row}"
            );
        }
    }

    #[test]
    fn extra_fields_on_a_dispute_row_are_dropped() {
        for kind in ["dispute", "resolve", "chargeback"] {
            let row = one(&format!("{kind},1,5,,extra,more\n"));
            assert_eq!(row.amount, None, "{kind}");
        }
    }

    #[test]
    fn a_bad_row_does_not_stop_the_rows_after_it() {
        let rows = read(
            "deposit,1,1,1.0\nbogus,1,2,1.0\ndeposit,x,3,1.0\ndeposit,1,4,abc\ndeposit,1,5,2.0\n",
        );

        let outcomes: Vec<_> = rows.iter().map(|row| row.transaction.is_ok()).collect();
        assert_eq!(outcomes, [true, false, false, false, true]);
    }

    #[test]
    fn a_row_with_invalid_utf8_is_a_bad_row() {
        let mut input = b"deposit,1,1,1.0\ndep".to_vec();
        input.extend_from_slice(&[0xff, 0xfe]);
        input.extend_from_slice(b"sit,1,2,1.0\ndeposit,1,3,3.0\n");

        let rows = read_bytes(&input);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[1].transaction,
            Err(RowError::Malformed("row is not valid UTF-8".to_owned()))
        );
        assert!(rows[2].transaction.is_ok());
    }

    #[test]
    fn a_read_failure_at_the_start_is_an_io_error() {
        let reader = FailsAfter {
            data: b"type,client,tx,amount\n",
            remaining: 0,
        };

        assert!(matches!(rows(reader), Err(InputError::Io(_))));
    }

    #[test]
    fn a_read_failure_midway_is_an_io_error_and_not_a_bad_row() {
        let data = b"type,client,tx,amount\ndeposit,1,1,1.0\ndeposit,1,2,2.0\ndeposit,1,3,3.0\n";
        let reader = FailsAfter {
            data,
            remaining: 45,
        };

        let results: Vec<_> = rows(reader).unwrap().take(3).collect();
        assert!(matches!(
            results[0],
            Ok(Row {
                record: 2,
                transaction: Ok(_)
            })
        ));
        assert!(results[1].is_err());
    }
}
