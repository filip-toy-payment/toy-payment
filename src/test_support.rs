//! Helpers shared by the unit tests of several modules.

use std::io::{self, Read, Write};

use crate::account::Account;
use crate::amount::Amount;
use crate::transaction::{ClientId, Kind, Transaction, TxId};

/// The largest amount and balance.
pub(crate) const CAP: &str = "7922816251426433759354395.0335";

pub(crate) fn amount(text: &str) -> Amount {
    text.parse().unwrap()
}

pub(crate) fn transaction(kind: Kind, client: u16, tx: u32, amount: Option<&str>) -> Transaction {
    Transaction {
        kind,
        client: ClientId(client),
        tx: TxId(tx),
        amount: amount.map(self::amount),
    }
}

/// An account that has received one deposit.
pub(crate) fn account_with(deposit: &str) -> Account {
    let mut account = Account::default();
    account.deposit(amount(deposit)).unwrap();
    account
}

/// Available, held and total as text.
pub(crate) fn balances(account: &Account) -> [String; 3] {
    [
        account.available().to_string(),
        account.held().to_string(),
        account.total().unwrap().to_string(),
    ]
}

/// A reader and writer that always fails.
pub(crate) struct Broken;

impl Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("disk fault"))
    }
}

impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("pipe closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("pipe closed"))
    }
}
