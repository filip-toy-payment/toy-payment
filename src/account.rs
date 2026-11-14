//! A client account and the operations that move its funds.

use thiserror::Error;

use crate::amount::{Amount, ArithmeticError, Balance};

/// Why an account refused an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AccountError {
    #[error("account is locked")]
    Locked,
    #[error("insufficient available funds")]
    InsufficientFunds,
    #[error("held funds do not cover the amount")]
    InsufficientHeld,
    #[error(transparent)]
    Arithmetic(#[from] ArithmeticError),
}

/// The funds of one client.
///
/// Every operation either applies in full or leaves the account unchanged.
/// Held funds are never negative. Available funds may be negative. A dispute
/// holds the full deposit even when part of it was already withdrawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Account {
    available: Balance,
    held: Balance,
    locked: bool,
}

impl Account {
    /// Funds the client can withdraw.
    pub fn available(&self) -> Balance {
        self.available
    }

    /// Funds held back by open disputes.
    pub fn held(&self) -> Balance {
        self.held
    }

    /// Available plus held funds.
    ///
    /// The total is derived. It can never disagree with the other two.
    /// Every operation keeps it within the cap. This does not fail in
    /// practice.
    pub fn total(&self) -> Result<Balance, ArithmeticError> {
        self.available.checked_add(self.held)
    }

    /// Whether a chargeback has locked the account.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Credits the amount to the available funds.
    pub fn deposit(&mut self, amount: Amount) -> Result<(), AccountError> {
        if self.locked {
            return Err(AccountError::Locked);
        }
        let available = self.available.checked_add(amount.into())?;
        // The total must stay within the cap as well.
        available.checked_add(self.held)?;

        self.available = available;
        Ok(())
    }

    /// Debits the amount from the available funds.
    pub fn withdraw(&mut self, amount: Amount) -> Result<(), AccountError> {
        if self.locked {
            return Err(AccountError::Locked);
        }
        let amount = Balance::from(amount);
        if self.available < amount {
            return Err(AccountError::InsufficientFunds);
        }

        self.available = self.available.checked_sub(amount)?;
        Ok(())
    }

    /// Moves a disputed amount from available to held.
    ///
    /// A locked account still accepts this. Disputes on earlier deposits
    /// must be settled.
    pub fn hold(&mut self, amount: Amount) -> Result<(), AccountError> {
        let amount = Balance::from(amount);
        let available = self.available.checked_sub(amount)?;
        let held = self.held.checked_add(amount)?;

        self.available = available;
        self.held = held;
        Ok(())
    }

    /// Moves a held amount back to available.
    pub fn release(&mut self, amount: Amount) -> Result<(), AccountError> {
        let amount = Balance::from(amount);
        let held = self.held_without(amount)?;
        let available = self.available.checked_add(amount)?;

        self.available = available;
        self.held = held;
        Ok(())
    }

    /// Removes a held amount for good and locks the account.
    pub fn charge_back(&mut self, amount: Amount) -> Result<(), AccountError> {
        let held = self.held_without(amount.into())?;
        self.available.checked_add(held)?;

        self.held = held;
        self.locked = true;
        Ok(())
    }

    // Guards the rule that held funds are never negative. No caller can break it.
    fn held_without(&self, amount: Balance) -> Result<Balance, AccountError> {
        if self.held < amount {
            return Err(AccountError::InsufficientHeld);
        }
        Ok(self.held.checked_sub(amount)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: &str = "7922816251426433759354395.0335";

    fn amount(text: &str) -> Amount {
        text.parse().unwrap()
    }

    /// Available, held and total as text.
    fn balances(account: &Account) -> [String; 3] {
        [
            account.available().to_string(),
            account.held().to_string(),
            account.total().unwrap().to_string(),
        ]
    }

    fn account_with(deposit: &str) -> Account {
        let mut account = Account::default();
        account.deposit(amount(deposit)).unwrap();
        account
    }

    #[test]
    fn a_new_account_is_empty_and_unlocked() {
        let account = Account::default();

        assert_eq!(balances(&account), ["0", "0", "0"]);
        assert!(!account.is_locked());
    }

    #[test]
    fn deposit_increases_available_and_total() {
        let mut account = account_with("1.5");
        account.deposit(amount("2.25")).unwrap();

        assert_eq!(balances(&account), ["3.75", "0", "3.75"]);
    }

    #[test]
    fn withdraw_decreases_available_and_total() {
        let mut account = account_with("3");
        account.withdraw(amount("1.5")).unwrap();

        assert_eq!(balances(&account), ["1.5", "0", "1.5"]);
    }

    #[test]
    fn withdraw_can_empty_the_account() {
        let mut account = account_with("3");
        account.withdraw(amount("3")).unwrap();

        assert_eq!(balances(&account), ["0", "0", "0"]);
    }

    #[test]
    fn withdraw_fails_without_sufficient_available_funds() {
        let mut account = account_with("2");
        let before = account;

        assert_eq!(
            account.withdraw(amount("2.0001")),
            Err(AccountError::InsufficientFunds)
        );
        assert_eq!(account, before);
    }

    #[test]
    fn withdraw_cannot_use_held_funds() {
        let mut account = account_with("5");
        account.hold(amount("5")).unwrap();

        assert_eq!(
            account.withdraw(amount("1")),
            Err(AccountError::InsufficientFunds)
        );
        assert_eq!(balances(&account), ["0", "5", "5"]);
    }

    #[test]
    fn hold_moves_funds_from_available_to_held() {
        let mut account = account_with("10");
        account.hold(amount("4")).unwrap();

        assert_eq!(balances(&account), ["6", "4", "10"]);
    }

    #[test]
    fn hold_can_push_available_below_zero() {
        let mut account = account_with("10");
        account.withdraw(amount("8")).unwrap();
        account.hold(amount("10")).unwrap();

        assert_eq!(balances(&account), ["-8", "10", "2"]);
    }

    #[test]
    fn release_moves_funds_from_held_back_to_available() {
        let mut account = account_with("10");
        account.hold(amount("4")).unwrap();
        account.release(amount("4")).unwrap();

        assert_eq!(balances(&account), ["10", "0", "10"]);
        assert!(!account.is_locked());
    }

    #[test]
    fn charge_back_removes_held_funds_and_locks_the_account() {
        let mut account = account_with("10");
        account.hold(amount("4")).unwrap();
        account.charge_back(amount("4")).unwrap();

        assert_eq!(balances(&account), ["6", "0", "6"]);
        assert!(account.is_locked());
    }

    #[test]
    fn charge_back_can_leave_a_negative_total() {
        let mut account = account_with("10");
        account.withdraw(amount("8")).unwrap();
        account.hold(amount("10")).unwrap();
        account.charge_back(amount("10")).unwrap();

        assert_eq!(balances(&account), ["-8", "0", "-8"]);
    }

    #[test]
    fn a_locked_account_refuses_deposits_and_withdrawals() {
        let mut account = account_with("10");
        account.hold(amount("4")).unwrap();
        account.charge_back(amount("4")).unwrap();
        let before = account;

        assert_eq!(account.deposit(amount("1")), Err(AccountError::Locked));
        assert_eq!(account.withdraw(amount("1")), Err(AccountError::Locked));
        assert_eq!(account, before);
    }

    #[test]
    fn a_locked_account_still_settles_disputes() {
        let mut account = account_with("10");
        account.hold(amount("1")).unwrap();
        account.charge_back(amount("1")).unwrap();

        account.hold(amount("2")).unwrap();
        account.release(amount("2")).unwrap();
        account.hold(amount("3")).unwrap();
        account.charge_back(amount("3")).unwrap();

        assert_eq!(balances(&account), ["6", "0", "6"]);
        assert!(account.is_locked());
    }

    #[test]
    fn release_and_charge_back_never_take_more_than_is_held() {
        let mut account = account_with("10");
        account.hold(amount("4")).unwrap();
        let before = account;

        assert_eq!(
            account.release(amount("4.0001")),
            Err(AccountError::InsufficientHeld)
        );
        assert_eq!(
            account.charge_back(amount("4.0001")),
            Err(AccountError::InsufficientHeld)
        );
        assert_eq!(account, before);
    }

    #[test]
    fn deposit_fails_when_available_would_exceed_the_cap() {
        let mut account = account_with(CAP);
        let before = account;

        assert_eq!(
            account.deposit(amount("0.0001")),
            Err(AccountError::Arithmetic(ArithmeticError))
        );
        assert_eq!(account, before);
    }

    #[test]
    fn deposit_fails_when_only_the_total_would_exceed_the_cap() {
        let mut account = account_with(CAP);
        account.hold(amount(CAP)).unwrap();
        let before = account;

        // Available is zero here. The deposit alone would fit.
        assert_eq!(
            account.deposit(amount("0.0001")),
            Err(AccountError::Arithmetic(ArithmeticError))
        );
        assert_eq!(account, before);
    }

    #[test]
    fn hold_fails_when_available_would_fall_below_the_negative_cap() {
        let mut account = account_with(CAP);
        account.withdraw(amount(CAP)).unwrap();
        account.hold(amount(CAP)).unwrap();
        let before = account;

        assert_eq!(
            account.hold(amount("0.0001")),
            Err(AccountError::Arithmetic(ArithmeticError))
        );
        assert_eq!(account, before);
    }
}
