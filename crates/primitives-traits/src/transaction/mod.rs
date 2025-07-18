//! Transaction abstraction
//!
//! This module provides traits for working with blockchain transactions:
//! - [`Transaction`] - Basic transaction interface
//! - [`signed::SignedTransaction`] - Transaction with signature and recovery methods
//! - [`FullTransaction`] - Transaction with database encoding support
//!
//! # Transaction Recovery
//!
//! Transaction senders are not stored directly but recovered from signatures.
//! Use `recover_signer` for post-EIP-2 transactions or `recover_signer_unchecked`
//! for historical transactions.

pub mod execute;
pub mod signature;
pub mod signed;

pub mod error;
pub mod recover;

pub use alloy_consensus::transaction::{SignerRecoverable, TransactionInfo, TransactionMeta};

use crate::{InMemorySize, MaybeCompact, MaybeSerde};
use core::{fmt, hash::Hash};

#[cfg(test)]
mod access_list;

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullTransaction: Transaction + MaybeCompact {}

impl<T> FullTransaction for T where T: Transaction + MaybeCompact {}

/// Abstraction of a transaction.
pub trait Transaction:
    Send
    + Sync
    + Unpin
    + Clone
    + fmt::Debug
    + Eq
    + PartialEq
    + Hash
    + alloy_consensus::Transaction
    + InMemorySize
    + MaybeSerde
{
}

impl<T> Transaction for T where
    T: Send
        + Sync
        + Unpin
        + Clone
        + fmt::Debug
        + Eq
        + PartialEq
        + Hash
        + alloy_consensus::Transaction
        + InMemorySize
        + MaybeSerde
{
}
