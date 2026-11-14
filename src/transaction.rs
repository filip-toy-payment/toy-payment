//! The transaction the engine accepts as input.
//!
//! These types carry no parsing logic. The input module builds them from
//! whatever source it reads.

use crate::amount::Amount;

/// Identifies a client and that client's single account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId(pub u16);

/// Identifies a transaction. It is unique across all clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TxId(pub u32);

/// What a transaction asks the engine to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

/// One transaction. The engine receives them in the order they occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transaction {
    pub kind: Kind,
    pub client: ClientId,
    /// For a deposit or withdrawal this is the transaction's own ID. For a
    /// dispute, resolve or chargeback it is the ID of the deposit referred to.
    pub tx: TxId,
    /// Required for a deposit or withdrawal. The engine rejects one without
    /// it. Ignored for every other kind.
    pub amount: Option<Amount>,
}
