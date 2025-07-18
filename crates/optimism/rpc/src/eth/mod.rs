//! OP-Reth `eth_` endpoint implementation.

pub mod ext;
pub mod receipt;
pub mod transaction;

mod block;
mod call;
mod pending_block;

use crate::{eth::transaction::OpTxInfoMapper, OpEthApiError, SequencerClient};
use alloy_primitives::U256;
use eyre::WrapErr;
use op_alloy_network::Optimism;
pub use receipt::{OpReceiptBuilder, OpReceiptFieldsBuilder};
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_node_api::{FullNodeComponents, NodePrimitives};
use reth_node_builder::rpc::{EthApiBuilder, EthApiCtx};
use reth_rpc::eth::{core::EthApiInner, DevSigner};
use reth_rpc_eth_api::{
    helpers::{
        spec::SignersForApi, AddDevSigners, EthApiSpec, EthFees, EthState, LoadBlock, LoadFee,
        LoadState, SpawnBlocking, Trace,
    },
    EthApiTypes, FromEvmError, FullEthApiServer, RpcConverter, RpcNodeCore, RpcNodeCoreExt,
    RpcTypes, SignableTxRequest,
};
use reth_rpc_eth_types::{EthStateCache, FeeHistoryCache, GasPriceOracle};
use reth_storage_api::{
    BlockNumReader, BlockReader, BlockReaderIdExt, ProviderBlock, ProviderHeader, ProviderReceipt,
    ProviderTx, StageCheckpointReader, StateProviderFactory,
};
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner,
};
use reth_transaction_pool::TransactionPool;
use std::{fmt, fmt::Formatter, marker::PhantomData, sync::Arc};

/// Adapter for [`EthApiInner`], which holds all the data required to serve core `eth_` API.
pub type EthApiNodeBackend<N, Rpc> = EthApiInner<
    <N as RpcNodeCore>::Provider,
    <N as RpcNodeCore>::Pool,
    <N as RpcNodeCore>::Network,
    <N as RpcNodeCore>::Evm,
    Rpc,
>;

/// A helper trait with requirements for [`RpcNodeCore`] to be used in [`OpEthApi`].
pub trait OpNodeCore: RpcNodeCore<Provider: BlockReader> {}
impl<T> OpNodeCore for T where T: RpcNodeCore<Provider: BlockReader> {}

/// OP-Reth `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
///
/// This wraps a default `Eth` implementation, and provides additional functionality where the
/// optimism spec deviates from the default (ethereum) spec, e.g. transaction forwarding to the
/// sequencer, receipts, additional RPC fields for transaction receipts.
///
/// This type implements the [`FullEthApi`](reth_rpc_eth_api::helpers::FullEthApi) by implemented
/// all the `Eth` helper traits and prerequisite traits.
pub struct OpEthApi<N: OpNodeCore, NetworkT: RpcTypes = Optimism> {
    /// Gateway to node's core components.
    inner: Arc<OpEthApiInner<N, NetworkT>>,
    /// Converter for RPC types.
    tx_resp_builder: RpcConverter<NetworkT, N::Evm, OpEthApiError, OpTxInfoMapper<N, NetworkT>>,
}

impl<N: OpNodeCore, NetworkT: RpcTypes> Clone for OpEthApi<N, NetworkT> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), tx_resp_builder: self.tx_resp_builder.clone() }
    }
}

impl<N: OpNodeCore, NetworkT: RpcTypes> OpEthApi<N, NetworkT> {
    /// Creates a new `OpEthApi`.
    pub fn new(
        eth_api: EthApiNodeBackend<N, NetworkT>,
        sequencer_client: Option<SequencerClient>,
        min_suggested_priority_fee: U256,
    ) -> Self {
        let inner =
            Arc::new(OpEthApiInner { eth_api, sequencer_client, min_suggested_priority_fee });
        Self {
            inner: inner.clone(),
            tx_resp_builder: RpcConverter::with_mapper(OpTxInfoMapper::new(inner)),
        }
    }

    /// Returns a reference to the [`EthApiNodeBackend`].
    pub fn eth_api(&self) -> &EthApiNodeBackend<N, NetworkT> {
        self.inner.eth_api()
    }
    /// Returns the configured sequencer client, if any.
    pub fn sequencer_client(&self) -> Option<&SequencerClient> {
        self.inner.sequencer_client()
    }

    /// Build a [`OpEthApi`] using [`OpEthApiBuilder`].
    pub const fn builder() -> OpEthApiBuilder<NetworkT> {
        OpEthApiBuilder::new()
    }
}

impl<N, NetworkT> EthApiTypes for OpEthApi<N, NetworkT>
where
    Self: Send + Sync + fmt::Debug,
    N: OpNodeCore,
    NetworkT: RpcTypes,
    <N as RpcNodeCore>::Evm: fmt::Debug,
    <N as RpcNodeCore>::Primitives: fmt::Debug,
{
    type Error = OpEthApiError;
    type NetworkTypes = NetworkT;
    type RpcConvert = RpcConverter<NetworkT, N::Evm, OpEthApiError, OpTxInfoMapper<N, NetworkT>>;

    fn tx_resp_builder(&self) -> &Self::RpcConvert {
        &self.tx_resp_builder
    }
}

impl<N, NetworkT> RpcNodeCore for OpEthApi<N, NetworkT>
where
    N: OpNodeCore,
    NetworkT: RpcTypes,
{
    type Primitives = N::Primitives;
    type Provider = N::Provider;
    type Pool = N::Pool;
    type Evm = <N as RpcNodeCore>::Evm;
    type Network = <N as RpcNodeCore>::Network;
    type PayloadBuilder = ();

    #[inline]
    fn pool(&self) -> &Self::Pool {
        self.inner.eth_api.pool()
    }

    #[inline]
    fn evm_config(&self) -> &Self::Evm {
        self.inner.eth_api.evm_config()
    }

    #[inline]
    fn network(&self) -> &Self::Network {
        self.inner.eth_api.network()
    }

    #[inline]
    fn payload_builder(&self) -> &Self::PayloadBuilder {
        &()
    }

    #[inline]
    fn provider(&self) -> &Self::Provider {
        self.inner.eth_api.provider()
    }
}

impl<N, NetworkT> RpcNodeCoreExt for OpEthApi<N, NetworkT>
where
    N: OpNodeCore,
    NetworkT: RpcTypes,
{
    #[inline]
    fn cache(&self) -> &EthStateCache<ProviderBlock<N::Provider>, ProviderReceipt<N::Provider>> {
        self.inner.eth_api.cache()
    }
}

impl<N, NetworkT> EthApiSpec for OpEthApi<N, NetworkT>
where
    N: OpNodeCore<
        Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>
                      + BlockNumReader
                      + StageCheckpointReader,
        Network: NetworkInfo,
    >,
    NetworkT: RpcTypes,
{
    type Transaction = ProviderTx<Self::Provider>;
    type Rpc = NetworkT;

    #[inline]
    fn starting_block(&self) -> U256 {
        self.inner.eth_api.starting_block()
    }

    #[inline]
    fn signers(&self) -> &SignersForApi<Self> {
        self.inner.eth_api.signers()
    }
}

impl<N, NetworkT> SpawnBlocking for OpEthApi<N, NetworkT>
where
    Self: Send + Sync + Clone + 'static,
    N: OpNodeCore,
    NetworkT: RpcTypes,
    <N as RpcNodeCore>::Evm: fmt::Debug,
    <N as RpcNodeCore>::Primitives: fmt::Debug,
{
    #[inline]
    fn io_task_spawner(&self) -> impl TaskSpawner {
        self.inner.eth_api.task_spawner()
    }

    #[inline]
    fn tracing_task_pool(&self) -> &BlockingTaskPool {
        self.inner.eth_api.blocking_task_pool()
    }

    #[inline]
    fn tracing_task_guard(&self) -> &BlockingTaskGuard {
        self.inner.eth_api.blocking_task_guard()
    }
}

impl<N, NetworkT> LoadFee for OpEthApi<N, NetworkT>
where
    Self: LoadBlock<Provider = N::Provider>,
    N: OpNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
    >,
    NetworkT: RpcTypes,
{
    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.eth_api.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache<ProviderHeader<N::Provider>> {
        self.inner.eth_api.fee_history_cache()
    }

    async fn suggested_priority_fee(&self) -> Result<U256, Self::Error> {
        let min_tip = U256::from(self.inner.min_suggested_priority_fee);
        self.inner.eth_api.gas_oracle().op_suggest_tip_cap(min_tip).await.map_err(Into::into)
    }
}

impl<N, NetworkT> LoadState for OpEthApi<N, NetworkT>
where
    N: OpNodeCore<
        Provider: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks>,
        Pool: TransactionPool,
    >,
    NetworkT: RpcTypes,
    <N as RpcNodeCore>::Evm: fmt::Debug,
    <N as RpcNodeCore>::Primitives: fmt::Debug,
{
}

impl<N, NetworkT> EthState for OpEthApi<N, NetworkT>
where
    Self: LoadState + SpawnBlocking,
    N: OpNodeCore,
    NetworkT: RpcTypes,
{
    #[inline]
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_api.eth_proof_window()
    }
}

impl<N, NetworkT> EthFees for OpEthApi<N, NetworkT>
where
    Self: LoadFee<
        Provider: ChainSpecProvider<
            ChainSpec: EthChainSpec<Header = ProviderHeader<Self::Provider>>,
        >,
    >,
    N: OpNodeCore,
    NetworkT: RpcTypes,
{
}

impl<N, NetworkT> Trace for OpEthApi<N, NetworkT>
where
    Self: RpcNodeCore<Provider: BlockReader>
        + LoadState<
            Evm: ConfigureEvm<
                Primitives: NodePrimitives<
                    BlockHeader = ProviderHeader<Self::Provider>,
                    SignedTx = ProviderTx<Self::Provider>,
                >,
            >,
            Error: FromEvmError<Self::Evm>,
        >,
    N: OpNodeCore,
    NetworkT: RpcTypes,
{
}

impl<N, NetworkT> AddDevSigners for OpEthApi<N, NetworkT>
where
    N: OpNodeCore,
    NetworkT: RpcTypes<TransactionRequest: SignableTxRequest<ProviderTx<N::Provider>>>,
{
    fn with_dev_accounts(&self) {
        *self.inner.eth_api.signers().write() = DevSigner::random_signers(20)
    }
}

impl<N: OpNodeCore, NetworkT: RpcTypes> fmt::Debug for OpEthApi<N, NetworkT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpEthApi").finish_non_exhaustive()
    }
}

/// Container type `OpEthApi`
pub struct OpEthApiInner<N: OpNodeCore, Rpc: RpcTypes> {
    /// Gateway to node's core components.
    eth_api: EthApiNodeBackend<N, Rpc>,
    /// Sequencer client, configured to forward submitted transactions to sequencer of given OP
    /// network.
    sequencer_client: Option<SequencerClient>,
    /// Minimum priority fee enforced by OP-specific logic.
    ///
    /// See also <https://github.com/ethereum-optimism/op-geth/blob/d4e0fe9bb0c2075a9bff269fb975464dd8498f75/eth/gasprice/optimism-gasprice.go#L38-L38>
    min_suggested_priority_fee: U256,
}

impl<N: OpNodeCore, Rpc: RpcTypes> fmt::Debug for OpEthApiInner<N, Rpc> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpEthApiInner").finish()
    }
}

impl<N: OpNodeCore, Rpc: RpcTypes> OpEthApiInner<N, Rpc> {
    /// Returns a reference to the [`EthApiNodeBackend`].
    const fn eth_api(&self) -> &EthApiNodeBackend<N, Rpc> {
        &self.eth_api
    }

    /// Returns the configured sequencer client, if any.
    const fn sequencer_client(&self) -> Option<&SequencerClient> {
        self.sequencer_client.as_ref()
    }
}

/// Builds [`OpEthApi`] for Optimism.
#[derive(Debug)]
pub struct OpEthApiBuilder<NetworkT = Optimism> {
    /// Sequencer client, configured to forward submitted transactions to sequencer of given OP
    /// network.
    sequencer_url: Option<String>,
    /// Headers to use for the sequencer client requests.
    sequencer_headers: Vec<String>,
    /// Minimum suggested priority fee (tip)
    min_suggested_priority_fee: u64,
    /// Marker for network types.
    _nt: PhantomData<NetworkT>,
}

impl<NetworkT> Default for OpEthApiBuilder<NetworkT> {
    fn default() -> Self {
        Self {
            sequencer_url: None,
            sequencer_headers: Vec::new(),
            min_suggested_priority_fee: 1_000_000,
            _nt: PhantomData,
        }
    }
}

impl<NetworkT> OpEthApiBuilder<NetworkT> {
    /// Creates a [`OpEthApiBuilder`] instance from core components.
    pub const fn new() -> Self {
        Self {
            sequencer_url: None,
            sequencer_headers: Vec::new(),
            min_suggested_priority_fee: 1_000_000,
            _nt: PhantomData,
        }
    }

    /// With a [`SequencerClient`].
    pub fn with_sequencer(mut self, sequencer_url: Option<String>) -> Self {
        self.sequencer_url = sequencer_url;
        self
    }

    /// With headers to use for the sequencer client requests.
    pub fn with_sequencer_headers(mut self, sequencer_headers: Vec<String>) -> Self {
        self.sequencer_headers = sequencer_headers;
        self
    }

    /// With minimum suggested priority fee (tip)
    pub const fn with_min_suggested_priority_fee(mut self, min: u64) -> Self {
        self.min_suggested_priority_fee = min;
        self
    }
}

impl<N, NetworkT> EthApiBuilder<N> for OpEthApiBuilder<NetworkT>
where
    N: FullNodeComponents,
    NetworkT: RpcTypes,
    OpEthApi<N, NetworkT>: FullEthApiServer<Provider = N::Provider, Pool = N::Pool> + AddDevSigners,
{
    type EthApi = OpEthApi<N, NetworkT>;

    async fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> eyre::Result<Self::EthApi> {
        let Self { sequencer_url, sequencer_headers, min_suggested_priority_fee, .. } = self;
        let eth_api = reth_rpc::EthApiBuilder::new(
            ctx.components.provider().clone(),
            ctx.components.pool().clone(),
            ctx.components.network().clone(),
            ctx.components.evm_config().clone(),
        )
        .eth_cache(ctx.cache)
        .task_spawner(ctx.components.task_executor().clone())
        .gas_cap(ctx.config.rpc_gas_cap.into())
        .max_simulate_blocks(ctx.config.rpc_max_simulate_blocks)
        .eth_proof_window(ctx.config.eth_proof_window)
        .fee_history_cache_config(ctx.config.fee_history_cache)
        .proof_permits(ctx.config.proof_permits)
        .gas_oracle_config(ctx.config.gas_oracle)
        .build_inner();

        let sequencer_client = if let Some(url) = sequencer_url {
            Some(
                SequencerClient::new_with_headers(&url, sequencer_headers)
                    .await
                    .wrap_err_with(|| "Failed to init sequencer client with: {url}")?,
            )
        } else {
            None
        };

        Ok(OpEthApi::new(eth_api, sequencer_client, U256::from(min_suggested_priority_fee)))
    }
}
