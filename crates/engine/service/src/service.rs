use futures::{Stream, StreamExt};
use pin_project::pin_project;
use reth_chainspec::EthChainSpec;
use reth_consensus::{ConsensusError, FullConsensus};
use reth_engine_primitives::{BeaconConsensusEngineEvent, BeaconEngineMessage};
use reth_engine_tree::{
    backfill::PipelineSync,
    download::BasicBlockDownloader,
    engine::{EngineApiKind, EngineApiRequest, EngineApiRequestHandler, EngineHandler},
    persistence::PersistenceHandle,
    tree::{EngineApiTreeHandler, EngineValidator, InvalidBlockHook, TreeConfig},
};
pub use reth_engine_tree::{
    chain::{ChainEvent, ChainOrchestrator},
    engine::EngineApiEvent,
};
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::ConfigureEvm;
use reth_network_p2p::BlockClient;
use reth_node_types::{BlockTy, NodeTypes};
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::{
    providers::{BlockchainProvider, ProviderNodeTypes},
    ProviderFactory,
};
use reth_prune::PrunerWithFactory;
use reth_stages_api::{MetricEventsSender, Pipeline};
use reth_tasks::TaskSpawner;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// Alias for consensus engine stream.
pub type EngineMessageStream<T> = Pin<Box<dyn Stream<Item = BeaconEngineMessage<T>> + Send + Sync>>;

/// Alias for chain orchestrator.
type EngineServiceType<N, Client> = ChainOrchestrator<
    EngineHandler<
        EngineApiRequestHandler<
            EngineApiRequest<<N as NodeTypes>::Payload, <N as NodeTypes>::Primitives>,
            <N as NodeTypes>::Primitives,
        >,
        EngineMessageStream<<N as NodeTypes>::Payload>,
        BasicBlockDownloader<Client, BlockTy<N>>,
    >,
    PipelineSync<N>,
>;

/// The type that drives the chain forward and communicates progress.
#[pin_project]
#[expect(missing_debug_implementations)]
// TODO(mattsse): remove hidden once fixed : <https://github.com/rust-lang/rust/issues/135363>
//  otherwise rustdoc fails to resolve the alias
#[doc(hidden)]
pub struct EngineService<N, Client>
where
    N: ProviderNodeTypes,
    Client: BlockClient<Block = BlockTy<N>> + 'static,
{
    orchestrator: EngineServiceType<N, Client>,
}

impl<N, Client> EngineService<N, Client>
where
    N: ProviderNodeTypes,
    Client: BlockClient<Block = BlockTy<N>> + 'static,
{
    /// Constructor for `EngineService`.
    #[expect(clippy::too_many_arguments)]
    pub fn new<V, C>(
        consensus: Arc<dyn FullConsensus<N::Primitives, Error = ConsensusError>>,
        chain_spec: Arc<N::ChainSpec>,
        client: Client,
        incoming_requests: EngineMessageStream<N::Payload>,
        pipeline: Pipeline<N>,
        pipeline_task_spawner: Box<dyn TaskSpawner>,
        provider: ProviderFactory<N>,
        blockchain_db: BlockchainProvider<N>,
        pruner: PrunerWithFactory<ProviderFactory<N>>,
        payload_builder: PayloadBuilderHandle<N::Payload>,
        payload_validator: V,
        tree_config: TreeConfig,
        invalid_block_hook: Box<dyn InvalidBlockHook<N::Primitives>>,
        sync_metrics_tx: MetricEventsSender,
        evm_config: C,
    ) -> Self
    where
        V: EngineValidator<N::Payload, Block = BlockTy<N>>,
        C: ConfigureEvm<Primitives = N::Primitives> + 'static,
    {
        let engine_kind =
            if chain_spec.is_optimism() { EngineApiKind::OpStack } else { EngineApiKind::Ethereum };

        let downloader = BasicBlockDownloader::new(client, consensus.clone());

        let persistence_handle =
            PersistenceHandle::<EthPrimitives>::spawn_service(provider, pruner, sync_metrics_tx);

        let canonical_in_memory_state = blockchain_db.canonical_in_memory_state();

        let (to_tree_tx, from_tree) = EngineApiTreeHandler::<N::Primitives, _, _, _, _>::spawn_new(
            blockchain_db,
            consensus,
            payload_validator,
            persistence_handle,
            payload_builder,
            canonical_in_memory_state,
            tree_config,
            invalid_block_hook,
            engine_kind,
            evm_config,
        );

        let engine_handler = EngineApiRequestHandler::new(to_tree_tx, from_tree);
        let handler = EngineHandler::new(engine_handler, downloader, incoming_requests);

        let backfill_sync = PipelineSync::new(pipeline, pipeline_task_spawner);

        Self { orchestrator: ChainOrchestrator::new(handler, backfill_sync) }
    }

    /// Returns a mutable reference to the orchestrator.
    pub fn orchestrator_mut(&mut self) -> &mut EngineServiceType<N, Client> {
        &mut self.orchestrator
    }
}

impl<N, Client> Stream for EngineService<N, Client>
where
    N: ProviderNodeTypes,
    Client: BlockClient<Block = BlockTy<N>> + 'static,
{
    type Item = ChainEvent<BeaconConsensusEngineEvent<N::Primitives>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut orchestrator = self.project().orchestrator;
        StreamExt::poll_next_unpin(&mut orchestrator, cx)
    }
}

/// Potential error returned by `EngineService`.
#[derive(Debug, thiserror::Error)]
#[error("Engine service error.")]
pub struct EngineServiceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_engine_primitives::BeaconEngineMessage;
    use reth_engine_tree::{test_utils::TestPipelineBuilder, tree::NoopInvalidBlockHook};
    use reth_ethereum_consensus::EthBeaconConsensus;
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_exex_types::FinishedExExHeight;
    use reth_network_p2p::test_utils::TestFullBlockClient;
    use reth_node_ethereum::EthereumEngineValidator;
    use reth_primitives_traits::SealedHeader;
    use reth_provider::{
        providers::BlockchainProvider, test_utils::create_test_provider_factory_with_chain_spec,
    };
    use reth_prune::Pruner;
    use reth_tasks::TokioTaskExecutor;
    use std::sync::Arc;
    use tokio::sync::{mpsc::unbounded_channel, watch};
    use tokio_stream::wrappers::UnboundedReceiverStream;

    #[test]
    fn eth_chain_orchestrator_build() {
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );
        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));

        let client = TestFullBlockClient::default();

        let (_tx, rx) = unbounded_channel::<BeaconEngineMessage<EthEngineTypes>>();
        let incoming_requests = UnboundedReceiverStream::new(rx);

        let pipeline = TestPipelineBuilder::new().build(chain_spec.clone());
        let pipeline_task_spawner = Box::<TokioTaskExecutor>::default();
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());

        let blockchain_db =
            BlockchainProvider::with_latest(provider_factory.clone(), SealedHeader::default())
                .unwrap();
        let engine_payload_validator = EthereumEngineValidator::new(chain_spec.clone());
        let (_tx, rx) = watch::channel(FinishedExExHeight::NoExExs);
        let pruner = Pruner::new_with_factory(provider_factory.clone(), vec![], 0, 0, None, rx);
        let evm_config = EthEvmConfig::new(chain_spec.clone());

        let (sync_metrics_tx, _sync_metrics_rx) = unbounded_channel();
        let (tx, _rx) = unbounded_channel();
        let _eth_service = EngineService::new(
            consensus,
            chain_spec,
            client,
            Box::pin(incoming_requests),
            pipeline,
            pipeline_task_spawner,
            provider_factory,
            blockchain_db,
            pruner,
            PayloadBuilderHandle::new(tx),
            engine_payload_validator,
            TreeConfig::default(),
            Box::new(NoopInvalidBlockHook::default()),
            sync_metrics_tx,
            evm_config,
        );
    }
}
