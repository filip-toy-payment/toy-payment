//! The payments engine.
//!
//! The engine applies transactions to client accounts. It does no I/O and
//! knows nothing about where transactions come from.

use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::account::{Account, AccountError};
use crate::amount::Amount;
use crate::transaction::{ClientId, Kind, Transaction, TxId};

/// Why a transaction had no effect on any balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Rejection {
    #[error("a deposit or withdrawal needs an amount")]
    MissingAmount,
    #[error("an earlier transaction already uses this ID")]
    DuplicateTransaction,
    #[error("no deposit has this transaction ID")]
    UnknownDeposit,
    #[error("the deposit belongs to another client")]
    ClientMismatch,
    #[error("the deposit is already under dispute")]
    AlreadyDisputed,
    #[error("the deposit is not under dispute")]
    NotDisputed,
    #[error("the deposit was already charged back")]
    AlreadyChargedBack,
    #[error(transparent)]
    Account(#[from] AccountError),
}

/// Applies transactions and keeps the resulting state.
#[derive(Debug, Default)]
pub struct Engine {
    accounts: HashMap<ClientId, Account>,
    store: TransactionStore,
}

impl Engine {
    /// Applies one transaction.
    ///
    /// The transaction either applies in full or changes no balance. In both
    /// cases the client it names has an account afterwards.
    pub fn apply(&mut self, transaction: Transaction) -> Result<(), Rejection> {
        let Transaction {
            kind,
            client,
            tx,
            amount,
        } = transaction;
        let account = self.accounts.entry(client).or_default();
        let store = &mut self.store;

        match kind {
            Kind::Deposit => deposit(account, store, client, tx, amount),
            Kind::Withdrawal => withdraw(account, store, tx, amount),
            Kind::Dispute => dispute(account, store, client, tx),
            Kind::Resolve => resolve(account, store, client, tx),
            Kind::Chargeback => chargeback(account, store, client, tx),
        }
    }

    /// Every account. The order is unspecified.
    pub fn accounts(&self) -> impl Iterator<Item = (ClientId, &Account)> {
        self.accounts
            .iter()
            .map(|(client, account)| (*client, account))
    }
}

fn deposit(
    account: &mut Account,
    store: &mut TransactionStore,
    client: ClientId,
    tx: TxId,
    amount: Option<Amount>,
) -> Result<(), Rejection> {
    let amount = amount.ok_or(Rejection::MissingAmount)?;
    // A reused ID is a data fault. It is reported even on a locked account.
    if store.contains(tx) {
        return Err(Rejection::DuplicateTransaction);
    }

    account.deposit(amount)?;
    store.insert_deposit(tx, client, amount);
    Ok(())
}

fn withdraw(
    account: &mut Account,
    store: &mut TransactionStore,
    tx: TxId,
    amount: Option<Amount>,
) -> Result<(), Rejection> {
    let amount = amount.ok_or(Rejection::MissingAmount)?;
    if store.contains(tx) {
        return Err(Rejection::DuplicateTransaction);
    }

    account.withdraw(amount)?;
    store.insert_withdrawal(tx);
    Ok(())
}

fn dispute(
    account: &mut Account,
    store: &mut TransactionStore,
    client: ClientId,
    tx: TxId,
) -> Result<(), Rejection> {
    let deposit = store.deposit_owned_by(tx, client)?;
    match deposit.state {
        DepositState::Undisputed => {}
        DepositState::Disputed => return Err(Rejection::AlreadyDisputed),
        DepositState::ChargedBack => return Err(Rejection::AlreadyChargedBack),
    }

    account.hold(deposit.amount)?;
    deposit.state = DepositState::Disputed;
    Ok(())
}

fn resolve(
    account: &mut Account,
    store: &mut TransactionStore,
    client: ClientId,
    tx: TxId,
) -> Result<(), Rejection> {
    let deposit = store.deposit_owned_by(tx, client)?;
    if deposit.state != DepositState::Disputed {
        return Err(Rejection::NotDisputed);
    }

    account.release(deposit.amount)?;
    // The deposit can be disputed again.
    deposit.state = DepositState::Undisputed;
    Ok(())
}

fn chargeback(
    account: &mut Account,
    store: &mut TransactionStore,
    client: ClientId,
    tx: TxId,
) -> Result<(), Rejection> {
    let deposit = store.deposit_owned_by(tx, client)?;
    if deposit.state != DepositState::Disputed {
        return Err(Rejection::NotDisputed);
    }

    account.charge_back(deposit.amount)?;
    deposit.state = DepositState::ChargedBack;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DepositState {
    /// Not under dispute. A resolved deposit returns to this state.
    Undisputed,
    Disputed,
    /// Final. The deposit can never be disputed again.
    ChargedBack,
}

#[derive(Debug)]
struct Deposit {
    client: ClientId,
    amount: Amount,
    state: DepositState,
}

/// Every applied deposit and withdrawal.
///
/// A deposit is kept in full so that it can be disputed later. A withdrawal
/// cannot be disputed. Only its ID is kept. That is enough to detect a reused
/// ID.
///
/// This is the only state that grows with the input. It lives in memory. The
/// engine uses just these four methods. A store on disk could replace it
/// without touching the rules above.
#[derive(Debug, Default)]
struct TransactionStore {
    deposits: HashMap<TxId, Deposit>,
    withdrawals: HashSet<TxId>,
}

impl TransactionStore {
    /// Whether an applied deposit or withdrawal already uses this ID.
    fn contains(&self, tx: TxId) -> bool {
        self.deposits.contains_key(&tx) || self.withdrawals.contains(&tx)
    }

    fn insert_deposit(&mut self, tx: TxId, client: ClientId, amount: Amount) {
        let deposit = Deposit {
            client,
            amount,
            state: DepositState::Undisputed,
        };
        self.deposits.insert(tx, deposit);
    }

    fn insert_withdrawal(&mut self, tx: TxId) {
        self.withdrawals.insert(tx);
    }

    /// Finds a deposit and checks that it belongs to the given client.
    fn deposit_owned_by(&mut self, tx: TxId, client: ClientId) -> Result<&mut Deposit, Rejection> {
        let deposit = self
            .deposits
            .get_mut(&tx)
            .ok_or(Rejection::UnknownDeposit)?;
        if deposit.client != client {
            return Err(Rejection::ClientMismatch);
        }
        Ok(deposit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::ArithmeticError;

    const CAP: &str = "7922816251426433759354395.0335";
    const LOCKED: Rejection = Rejection::Account(AccountError::Locked);
    const INSUFFICIENT_FUNDS: Rejection = Rejection::Account(AccountError::InsufficientFunds);
    const DUPLICATE: Rejection = Rejection::DuplicateTransaction;

    fn transaction(kind: Kind, client: u16, tx: u32, amount: Option<&str>) -> Transaction {
        Transaction {
            kind,
            client: ClientId(client),
            tx: TxId(tx),
            amount: amount.map(|text| text.parse().unwrap()),
        }
    }

    fn deposit(client: u16, tx: u32, amount: &str) -> Transaction {
        transaction(Kind::Deposit, client, tx, Some(amount))
    }

    fn withdrawal(client: u16, tx: u32, amount: &str) -> Transaction {
        transaction(Kind::Withdrawal, client, tx, Some(amount))
    }

    fn dispute(client: u16, tx: u32) -> Transaction {
        transaction(Kind::Dispute, client, tx, None)
    }

    fn resolve(client: u16, tx: u32) -> Transaction {
        transaction(Kind::Resolve, client, tx, None)
    }

    fn chargeback(client: u16, tx: u32) -> Transaction {
        transaction(Kind::Chargeback, client, tx, None)
    }

    /// An engine that has applied the given transactions. Each must succeed.
    fn engine_after(transactions: &[Transaction]) -> Engine {
        let mut engine = Engine::default();
        for transaction in transactions {
            engine.apply(*transaction).unwrap();
        }
        engine
    }

    fn account(engine: &Engine, client: u16) -> Option<&Account> {
        engine.accounts.get(&ClientId(client))
    }

    /// Available, held and total of a client as text.
    fn balances(engine: &Engine, client: u16) -> [String; 3] {
        let account = account(engine, client).unwrap();
        [
            account.available().to_string(),
            account.held().to_string(),
            account.total().unwrap().to_string(),
        ]
    }

    fn is_locked(engine: &Engine, client: u16) -> bool {
        account(engine, client).unwrap().is_locked()
    }

    #[test]
    fn processes_the_example_from_the_specification() {
        let mut engine = engine_after(&[
            deposit(1, 1, "1.0"),
            deposit(2, 2, "2.0"),
            deposit(1, 3, "2.0"),
            withdrawal(1, 4, "1.5"),
        ]);
        assert_eq!(
            engine.apply(withdrawal(2, 5, "3.0")),
            Err(INSUFFICIENT_FUNDS)
        );

        assert_eq!(balances(&engine, 1), ["1.5", "0", "1.5"]);
        assert_eq!(balances(&engine, 2), ["2", "0", "2"]);
        assert!(!is_locked(&engine, 1));
        assert!(!is_locked(&engine, 2));
    }

    #[test]
    fn keeps_clients_separate() {
        let engine = engine_after(&[deposit(1, 1, "5"), deposit(2, 2, "7"), dispute(2, 2)]);

        assert_eq!(balances(&engine, 1), ["5", "0", "5"]);
        assert_eq!(balances(&engine, 2), ["0", "7", "7"]);
        assert_eq!(engine.accounts().count(), 2);
    }

    #[test]
    fn a_rejected_transaction_still_opens_the_account() {
        let mut engine = Engine::default();

        assert_eq!(engine.apply(withdrawal(7, 1, "1")), Err(INSUFFICIENT_FUNDS));
        assert_eq!(engine.apply(dispute(8, 99)), Err(Rejection::UnknownDeposit));

        assert_eq!(balances(&engine, 7), ["0", "0", "0"]);
        assert_eq!(balances(&engine, 8), ["0", "0", "0"]);
    }

    #[test]
    fn a_deposit_or_withdrawal_without_an_amount_is_rejected() {
        let mut engine = engine_after(&[deposit(1, 1, "5")]);
        let no_amount_deposit = transaction(Kind::Deposit, 1, 2, None);
        let no_amount_withdrawal = transaction(Kind::Withdrawal, 1, 3, None);

        assert_eq!(
            engine.apply(no_amount_deposit),
            Err(Rejection::MissingAmount)
        );
        assert_eq!(
            engine.apply(no_amount_withdrawal),
            Err(Rejection::MissingAmount)
        );
        assert_eq!(balances(&engine, 1), ["5", "0", "5"]);
    }

    #[test]
    fn a_deposit_without_an_amount_still_opens_the_account() {
        let mut engine = Engine::default();

        assert_eq!(
            engine.apply(transaction(Kind::Deposit, 7, 1, None)),
            Err(Rejection::MissingAmount)
        );
        assert_eq!(balances(&engine, 7), ["0", "0", "0"]);
    }

    #[test]
    fn a_deposit_that_reuses_a_deposit_id_is_rejected() {
        let mut engine = engine_after(&[deposit(1, 1, "5")]);

        assert_eq!(engine.apply(deposit(1, 1, "9")), Err(DUPLICATE));
        assert_eq!(engine.apply(deposit(2, 1, "9")), Err(DUPLICATE));

        assert_eq!(balances(&engine, 1), ["5", "0", "5"]);
        assert_eq!(balances(&engine, 2), ["0", "0", "0"]);
        // The stored deposit is still the first one.
        engine.apply(dispute(1, 1)).unwrap();
        assert_eq!(balances(&engine, 1), ["0", "5", "5"]);
    }

    #[test]
    fn a_withdrawal_that_reuses_a_withdrawal_id_is_rejected() {
        let mut engine = engine_after(&[deposit(1, 1, "5"), withdrawal(1, 2, "1")]);

        assert_eq!(engine.apply(withdrawal(1, 2, "1")), Err(DUPLICATE));
        assert_eq!(engine.apply(withdrawal(2, 2, "1")), Err(DUPLICATE));
        assert_eq!(balances(&engine, 1), ["4", "0", "4"]);
    }

    #[test]
    fn deposits_and_withdrawals_share_one_space_of_ids() {
        let mut engine = engine_after(&[deposit(1, 1, "5"), withdrawal(1, 2, "1")]);

        assert_eq!(engine.apply(withdrawal(1, 1, "1")), Err(DUPLICATE));
        assert_eq!(engine.apply(deposit(1, 2, "3")), Err(DUPLICATE));
        assert_eq!(balances(&engine, 1), ["4", "0", "4"]);
    }

    #[test]
    fn a_rejected_deposit_or_withdrawal_does_not_use_up_its_id() {
        let mut engine = engine_after(&[deposit(1, 1, "5")]);
        assert_eq!(engine.apply(withdrawal(1, 2, "9")), Err(INSUFFICIENT_FUNDS));
        assert_eq!(
            engine.apply(transaction(Kind::Deposit, 1, 3, None)),
            Err(Rejection::MissingAmount)
        );

        engine.apply(withdrawal(1, 2, "1")).unwrap();
        engine.apply(deposit(1, 3, "2")).unwrap();
        assert_eq!(balances(&engine, 1), ["6", "0", "6"]);
    }

    #[test]
    fn a_withdrawal_cannot_be_disputed() {
        let mut engine = engine_after(&[deposit(1, 1, "5"), withdrawal(1, 2, "1")]);

        assert_eq!(engine.apply(dispute(1, 2)), Err(Rejection::UnknownDeposit));
        assert_eq!(balances(&engine, 1), ["4", "0", "4"]);
    }

    #[test]
    fn a_dispute_holds_the_deposit_amount() {
        let engine = engine_after(&[deposit(1, 1, "5"), deposit(1, 2, "3"), dispute(1, 1)]);

        assert_eq!(balances(&engine, 1), ["3", "5", "8"]);
        assert!(!is_locked(&engine, 1));
    }

    #[test]
    fn a_dispute_can_push_available_below_zero() {
        let engine = engine_after(&[deposit(1, 1, "10"), withdrawal(1, 2, "8"), dispute(1, 1)]);

        assert_eq!(balances(&engine, 1), ["-8", "10", "2"]);
    }

    #[test]
    fn a_dispute_ignores_an_amount_it_carries() {
        let mut engine = engine_after(&[deposit(1, 1, "5")]);

        engine
            .apply(transaction(Kind::Dispute, 1, 1, Some("999")))
            .unwrap();
        assert_eq!(balances(&engine, 1), ["0", "5", "5"]);
    }

    #[test]
    fn a_dispute_for_an_unknown_deposit_is_rejected() {
        let mut engine = engine_after(&[deposit(1, 1, "5")]);

        assert_eq!(engine.apply(dispute(1, 2)), Err(Rejection::UnknownDeposit));
        assert_eq!(engine.apply(resolve(1, 2)), Err(Rejection::UnknownDeposit));
        assert_eq!(
            engine.apply(chargeback(1, 2)),
            Err(Rejection::UnknownDeposit)
        );
        assert_eq!(balances(&engine, 1), ["5", "0", "5"]);
    }

    #[test]
    fn a_client_cannot_touch_another_clients_deposit() {
        let mut engine = engine_after(&[deposit(1, 1, "5")]);

        assert_eq!(engine.apply(dispute(2, 1)), Err(Rejection::ClientMismatch));
        assert_eq!(balances(&engine, 1), ["5", "0", "5"]);

        engine.apply(dispute(1, 1)).unwrap();
        assert_eq!(engine.apply(resolve(2, 1)), Err(Rejection::ClientMismatch));
        assert_eq!(
            engine.apply(chargeback(2, 1)),
            Err(Rejection::ClientMismatch)
        );
        assert_eq!(balances(&engine, 1), ["0", "5", "5"]);
        assert!(!is_locked(&engine, 1));

        // The client named on the rejected rows has an empty account.
        assert_eq!(balances(&engine, 2), ["0", "0", "0"]);
        assert!(!is_locked(&engine, 2));
    }

    #[test]
    fn a_deposit_under_dispute_cannot_be_disputed_again() {
        let mut engine = engine_after(&[deposit(1, 1, "5"), dispute(1, 1)]);

        assert_eq!(engine.apply(dispute(1, 1)), Err(Rejection::AlreadyDisputed));
        assert_eq!(balances(&engine, 1), ["0", "5", "5"]);
    }

    #[test]
    fn a_resolve_releases_the_held_amount() {
        let engine = engine_after(&[deposit(1, 1, "5"), dispute(1, 1), resolve(1, 1)]);

        assert_eq!(balances(&engine, 1), ["5", "0", "5"]);
        assert!(!is_locked(&engine, 1));
    }

    #[test]
    fn a_resolved_deposit_can_be_disputed_again() {
        let engine = engine_after(&[
            deposit(1, 1, "5"),
            dispute(1, 1),
            resolve(1, 1),
            dispute(1, 1),
        ]);

        assert_eq!(balances(&engine, 1), ["0", "5", "5"]);
    }

    #[test]
    fn a_resolve_or_chargeback_needs_an_open_dispute() {
        let mut engine = engine_after(&[deposit(1, 1, "5")]);

        assert_eq!(engine.apply(resolve(1, 1)), Err(Rejection::NotDisputed));
        assert_eq!(engine.apply(chargeback(1, 1)), Err(Rejection::NotDisputed));
        assert_eq!(balances(&engine, 1), ["5", "0", "5"]);
        assert!(!is_locked(&engine, 1));
    }

    #[test]
    fn a_chargeback_removes_the_held_amount_and_locks_the_account() {
        let engine = engine_after(&[
            deposit(1, 1, "5"),
            deposit(1, 2, "3"),
            dispute(1, 1),
            chargeback(1, 1),
        ]);

        assert_eq!(balances(&engine, 1), ["3", "0", "3"]);
        assert!(is_locked(&engine, 1));
    }

    #[test]
    fn a_chargeback_after_a_withdrawal_leaves_a_negative_balance() {
        let engine = engine_after(&[
            deposit(1, 1, "10"),
            withdrawal(1, 2, "8"),
            dispute(1, 1),
            chargeback(1, 1),
        ]);

        assert_eq!(balances(&engine, 1), ["-8", "0", "-8"]);
        assert!(is_locked(&engine, 1));
    }

    #[test]
    fn a_charged_back_deposit_is_final() {
        let mut engine = engine_after(&[deposit(1, 1, "5"), dispute(1, 1), chargeback(1, 1)]);

        assert_eq!(
            engine.apply(dispute(1, 1)),
            Err(Rejection::AlreadyChargedBack)
        );
        assert_eq!(engine.apply(resolve(1, 1)), Err(Rejection::NotDisputed));
        assert_eq!(engine.apply(chargeback(1, 1)), Err(Rejection::NotDisputed));
        assert_eq!(balances(&engine, 1), ["0", "0", "0"]);
    }

    #[test]
    fn a_locked_account_rejects_deposits_and_withdrawals() {
        let mut engine = engine_after(&[
            deposit(1, 1, "5"),
            deposit(1, 2, "3"),
            dispute(1, 1),
            chargeback(1, 1),
        ]);

        assert_eq!(engine.apply(deposit(1, 3, "1")), Err(LOCKED));
        assert_eq!(engine.apply(withdrawal(1, 4, "1")), Err(LOCKED));
        assert_eq!(balances(&engine, 1), ["3", "0", "3"]);
    }

    #[test]
    fn a_deposit_rejected_by_a_locked_account_is_not_stored() {
        let mut engine = engine_after(&[deposit(1, 1, "5"), dispute(1, 1), chargeback(1, 1)]);
        assert_eq!(engine.apply(deposit(1, 2, "4")), Err(LOCKED));

        assert_eq!(engine.apply(dispute(1, 2)), Err(Rejection::UnknownDeposit));
    }

    #[test]
    fn a_locked_account_still_settles_disputes_on_earlier_deposits() {
        let mut engine = engine_after(&[
            deposit(1, 1, "5"),
            deposit(1, 2, "3"),
            deposit(1, 3, "2"),
            dispute(1, 1),
            chargeback(1, 1),
        ]);
        assert!(is_locked(&engine, 1));

        engine.apply(dispute(1, 2)).unwrap();
        assert_eq!(balances(&engine, 1), ["2", "3", "5"]);
        engine.apply(resolve(1, 2)).unwrap();
        assert_eq!(balances(&engine, 1), ["5", "0", "5"]);

        engine.apply(dispute(1, 3)).unwrap();
        engine.apply(chargeback(1, 3)).unwrap();
        assert_eq!(balances(&engine, 1), ["3", "0", "3"]);
        assert!(is_locked(&engine, 1));
    }

    #[test]
    fn a_reused_id_is_reported_even_on_a_locked_account() {
        let mut engine = engine_after(&[
            deposit(1, 1, "5"),
            withdrawal(1, 2, "1"),
            dispute(1, 1),
            chargeback(1, 1),
        ]);
        assert!(is_locked(&engine, 1));

        assert_eq!(engine.apply(deposit(1, 1, "5")), Err(DUPLICATE));
        assert_eq!(engine.apply(withdrawal(1, 2, "1")), Err(DUPLICATE));
    }

    #[test]
    fn a_deposit_that_would_exceed_the_cap_is_rejected_and_not_stored() {
        let mut engine = engine_after(&[deposit(1, 1, CAP)]);
        let over_cap = Rejection::Account(AccountError::Arithmetic(ArithmeticError));

        assert_eq!(engine.apply(deposit(1, 2, "0.0001")), Err(over_cap));
        assert_eq!(balances(&engine, 1), [CAP, "0", CAP]);
        assert_eq!(engine.apply(dispute(1, 2)), Err(Rejection::UnknownDeposit));
    }

    #[test]
    fn a_dispute_that_would_exceed_the_cap_leaves_the_deposit_undisputed() {
        let mut engine = engine_after(&[
            deposit(1, 1, CAP),
            withdrawal(1, 2, CAP),
            deposit(1, 3, CAP),
            withdrawal(1, 4, CAP),
            dispute(1, 1),
        ]);
        let over_cap = Rejection::Account(AccountError::Arithmetic(ArithmeticError));

        assert_eq!(engine.apply(dispute(1, 3)), Err(over_cap));
        // Deposit 3 is not under dispute. It cannot be resolved.
        assert_eq!(engine.apply(resolve(1, 3)), Err(Rejection::NotDisputed));
    }
}
