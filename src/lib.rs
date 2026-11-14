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
