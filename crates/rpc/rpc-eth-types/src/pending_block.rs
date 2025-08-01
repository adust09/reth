//! Helper types for `reth_rpc_eth_api::EthApiServer` implementation.
//!
//! Types used in block building.

use std::{sync::Arc, time::Instant};

use alloy_consensus::BlockHeader;
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::B256;
use derive_more::Constructor;
use reth_ethereum_primitives::Receipt;
use reth_evm::EvmEnv;
use reth_primitives_traits::{Block, NodePrimitives, RecoveredBlock, SealedHeader};

/// Configured [`EvmEnv`] for a pending block.
#[derive(Debug, Clone, Constructor)]
pub struct PendingBlockEnv<B: Block, R, Spec> {
    /// Configured [`EvmEnv`] for the pending block.
    pub evm_env: EvmEnv<Spec>,
    /// Origin block for the config
    pub origin: PendingBlockEnvOrigin<B, R>,
}

/// The origin for a configured [`PendingBlockEnv`]
#[derive(Clone, Debug)]
pub enum PendingBlockEnvOrigin<B: Block = reth_ethereum_primitives::Block, R = Receipt> {
    /// The pending block as received from the CL.
    ActualPending(Arc<RecoveredBlock<B>>, Arc<Vec<R>>),
    /// The _modified_ header of the latest block.
    ///
    /// This derives the pending state based on the latest header by modifying:
    ///  - the timestamp
    ///  - the block number
    ///  - fees
    DerivedFromLatest(SealedHeader<B::Header>),
}

impl<B: Block, R> PendingBlockEnvOrigin<B, R> {
    /// Returns true if the origin is the actual pending block as received from the CL.
    pub const fn is_actual_pending(&self) -> bool {
        matches!(self, Self::ActualPending(_, _))
    }

    /// Consumes the type and returns the actual pending block.
    pub fn into_actual_pending(self) -> Option<Arc<RecoveredBlock<B>>> {
        match self {
            Self::ActualPending(block, _) => Some(block),
            _ => None,
        }
    }

    /// Returns the [`BlockId`] that represents the state of the block.
    ///
    /// If this is the actual pending block, the state is the "Pending" tag, otherwise we can safely
    /// identify the block by its hash (latest block).
    pub fn state_block_id(&self) -> BlockId {
        match self {
            Self::ActualPending(_, _) => BlockNumberOrTag::Pending.into(),
            Self::DerivedFromLatest(latest) => BlockId::Hash(latest.hash().into()),
        }
    }

    /// Returns the hash of the block the pending block should be built on.
    ///
    /// For the [`PendingBlockEnvOrigin::ActualPending`] this is the parent hash of the block.
    /// For the [`PendingBlockEnvOrigin::DerivedFromLatest`] this is the hash of the _latest_
    /// header.
    pub fn build_target_hash(&self) -> B256 {
        match self {
            Self::ActualPending(block, _) => block.header().parent_hash(),
            Self::DerivedFromLatest(latest) => latest.hash(),
        }
    }
}

/// Locally built pending block for `pending` tag.
#[derive(Debug, Constructor)]
pub struct PendingBlock<N: NodePrimitives> {
    /// Timestamp when the pending block is considered outdated.
    pub expires_at: Instant,
    /// The locally built pending block.
    pub block: Arc<RecoveredBlock<N::Block>>,
    /// The receipts for the pending block
    pub receipts: Arc<Vec<N::Receipt>>,
}
