//! Writes the final state of every account as CSV.

use std::io::{self, Write};

use thiserror::Error;

use crate::account::Account;
use crate::amount::ArithmeticError;
use crate::transaction::ClientId;

const HEADER: [&str; 5] = ["client", "available", "held", "total", "locked"];

/// Why the accounts could not be written.
#[derive(Debug, Error)]
pub enum OutputError {
    #[error("cannot write output: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Arithmetic(#[from] ArithmeticError),
}

/// Writes one row per account. Rows are sorted by client ID.
///
/// The header is written even when there are no accounts.
pub fn write_accounts<'a, W, I>(output: W, accounts: I) -> Result<(), OutputError>
where
    W: Write,
    I: IntoIterator<Item = (ClientId, &'a Account)>,
{
    // Sorting makes the output the same on every run.
    let mut accounts: Vec<_> = accounts.into_iter().collect();
    accounts.sort_unstable_by_key(|(client, _)| *client);

    let mut writer = csv::Writer::from_writer(output);
    writer.write_record(HEADER).map_err(io::Error::from)?;
    for (client, account) in accounts {
        let row = [
            client.0.to_string(),
            account.available().to_string(),
            account.held().to_string(),
            account.total()?.to_string(),
            account.is_locked().to_string(),
        ];
        writer.write_record(row).map_err(io::Error::from)?;
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Broken, account_with, amount};

    fn written(accounts: &[(u16, Account)]) -> String {
        let mut output = Vec::new();
        let accounts = accounts
            .iter()
            .map(|(client, account)| (ClientId(*client), account));
        write_accounts(&mut output, accounts).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn writes_only_the_header_when_there_are_no_accounts() {
        assert_eq!(written(&[]), "client,available,held,total,locked\n");
    }

    #[test]
    fn writes_the_example_from_the_specification() {
        let accounts = [(1, account_with("1.5")), (2, account_with("2.0"))];

        assert_eq!(
            written(&accounts),
            "client,available,held,total,locked\n\
             1,1.5,0,1.5,false\n\
             2,2,0,2,false\n"
        );
    }

    #[test]
    fn sorts_rows_by_client_id() {
        let accounts = [
            (300, account_with("3")),
            (2, account_with("2")),
            (65535, account_with("5")),
            (0, account_with("1")),
        ];

        let clients: Vec<_> = written(&accounts)
            .lines()
            .skip(1)
            .map(|line| line.split(',').next().unwrap().to_owned())
            .collect();
        assert_eq!(clients, ["0", "2", "300", "65535"]);
    }

    #[test]
    fn prints_up_to_four_decimal_places_without_trailing_zeros() {
        let accounts = [
            (1, account_with("0.1234")),
            (2, account_with("10.5000")),
            (3, account_with("7.0000")),
        ];

        assert_eq!(
            written(&accounts),
            "client,available,held,total,locked\n\
             1,0.1234,0,0.1234,false\n\
             2,10.5,0,10.5,false\n\
             3,7,0,7,false\n"
        );
    }

    #[test]
    fn writes_held_funds_a_negative_balance_and_the_locked_flag() {
        let mut disputed = account_with("10");
        disputed.withdraw(amount("8")).unwrap();
        disputed.hold(amount("10")).unwrap();

        let mut charged_back = disputed;
        charged_back.charge_back(amount("10")).unwrap();

        assert_eq!(
            written(&[(1, disputed), (2, charged_back)]),
            "client,available,held,total,locked\n\
             1,-8,10,2,false\n\
             2,-8,0,-8,true\n"
        );
    }

    #[test]
    fn reports_a_failing_writer() {
        let accounts = [(ClientId(1), account_with("1"))];
        let accounts = accounts.iter().map(|(client, account)| (*client, account));

        let result = write_accounts(Broken, accounts);
        assert!(matches!(result, Err(OutputError::Io(_))));
    }
}
