use std::{
    collections::BTreeMap,
    fmt::{Display, Formatter},
};

use async_trait::async_trait;
use miden_node_proto::{
    account,
    conversion::convert,
    digest,
    domain::BlockInputs,
    requests::{ApplyBlockRequest, GetBlockInputsRequest, GetTransactionInputsRequest},
    store::api_client as store_client,
};
use miden_node_utils::formatting::{format_map, format_opt};
use miden_objects::{accounts::AccountId, Digest};
use tonic::transport::Channel;
use tracing::{debug, info, instrument};

pub use crate::errors::{ApplyBlockError, BlockInputsError, TxInputsError};
use crate::{block::Block, ProvenTransaction, COMPONENT};

// STORE TRAIT
// ================================================================================================

#[async_trait]
pub trait Store: ApplyBlock {
    /// TODO: add comments
    async fn get_tx_inputs(
        &self,
        proven_tx: &ProvenTransaction,
    ) -> Result<TxInputs, TxInputsError>;

    /// TODO: add comments
    async fn get_block_inputs(
        &self,
        updated_accounts: impl Iterator<Item = &AccountId> + Send,
        produced_nullifiers: impl Iterator<Item = &Digest> + Send,
    ) -> Result<BlockInputs, BlockInputsError>;
}

#[async_trait]
pub trait ApplyBlock: Send + Sync + 'static {
    async fn apply_block(
        &self,
        block: Block,
    ) -> Result<(), ApplyBlockError>;
}

/// Information needed from the store to verify a transaction.
#[derive(Debug)]
pub struct TxInputs {
    /// The account hash in the store corresponding to tx's account ID
    pub account_hash: Option<Digest>,

    /// Maps each consumed notes' nullifier to whether the note is already consumed
    pub nullifiers: BTreeMap<Digest, bool>,
}

impl Display for TxInputs {
    fn fmt(
        &self,
        f: &mut Formatter<'_>,
    ) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "{{ account_hash: {}, nullifiers: {} }}",
            format_opt(self.account_hash.as_ref()),
            format_map(&self.nullifiers)
        ))
    }
}

// DEFAULT STORE IMPLEMENTATION
// ================================================================================================

pub struct DefaultStore {
    store: store_client::ApiClient<Channel>,
}

impl DefaultStore {
    /// TODO: this should probably take store connection string and create a connection internally
    pub fn new(store: store_client::ApiClient<Channel>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ApplyBlock for DefaultStore {
    #[allow(clippy::blocks_in_conditions)] // Workaround of `instrument` issue
    #[instrument(target = "miden-block-producer", skip_all, err)]
    async fn apply_block(
        &self,
        block: Block,
    ) -> Result<(), ApplyBlockError> {
        let request = tonic::Request::new(ApplyBlockRequest {
            block: Some(block.header.into()),
            accounts: convert(block.updated_accounts),
            nullifiers: convert(block.produced_nullifiers),
            notes: convert(block.created_notes),
        });

        let _ = self
            .store
            .clone()
            .apply_block(request)
            .await
            .map_err(|status| ApplyBlockError::GrpcClientError(status.message().to_string()))?;

        Ok(())
    }
}

#[async_trait]
impl Store for DefaultStore {
    #[allow(clippy::blocks_in_conditions)] // Workaround of `instrument` issue
    #[instrument(target = "miden-block-producer", skip_all, err)]
    async fn get_tx_inputs(
        &self,
        proven_tx: &ProvenTransaction,
    ) -> Result<TxInputs, TxInputsError> {
        let message = GetTransactionInputsRequest {
            account_id: Some(proven_tx.account_id().into()),
            nullifiers: proven_tx
                .input_notes()
                .iter()
                .map(|nullifier| (*nullifier).into())
                .collect(),
        };

        info!(target: COMPONENT, tx_id = %proven_tx.id().to_hex());
        debug!(target: COMPONENT, ?message);

        let request = tonic::Request::new(message);
        let response = self
            .store
            .clone()
            .get_transaction_inputs(request)
            .await
            .map_err(|status| TxInputsError::GrpcClientError(status.message().to_string()))?
            .into_inner();

        debug!(target: COMPONENT, ?response);

        let account_hash = {
            let account_state = response
                .account_state
                .ok_or(TxInputsError::MalformedResponse("account_states empty".to_string()))?;

            let account_id_from_store: AccountId = account_state
                .account_id
                .clone()
                .ok_or(TxInputsError::MalformedResponse("empty account id".to_string()))?
                .try_into()?;

            if account_id_from_store != proven_tx.account_id() {
                return Err(TxInputsError::MalformedResponse(format!(
                    "incorrect account id returned from store. Got: {}, expected: {}",
                    account_id_from_store,
                    proven_tx.account_id()
                )));
            }

            account_state.account_hash.clone().map(Digest::try_from).transpose()?
        };

        let nullifiers = {
            let mut nullifiers = Vec::new();

            for nullifier_record in response.nullifiers {
                let nullifier = nullifier_record
                    .nullifier
                    .ok_or(TxInputsError::MalformedResponse(
                        "nullifier record contains empty nullifier".to_string(),
                    ))?
                    .try_into()?;

                // `block_num` is nonzero if already consumed; 0 otherwise
                nullifiers.push((nullifier, nullifier_record.block_num != 0))
            }

            nullifiers.into_iter().collect()
        };

        // We are matching the received account_hash from the Store here to check for different
        // cases:
        // 1. If the hash is equal to `Digest::default()`, it signifies that this is a new account
        //    which is not yet present in the Store.
        // 2. If the hash is not equal to `Digest::default()`, it signifies that it is an exiting
        //    account (i.e., known to the Store).
        // 3. If the hash is `None`, it means there has been an error in the processing of the
        //    account hash from the Store.
        let account_hash = match account_hash {
            Some(hash) if hash == Digest::default() => None,
            Some(hash) => Some(hash),
            None => {
                return Err(TxInputsError::MalformedResponse(
                    "incorrect account hash returned from the store. Got None".to_string(),
                ))
            },
        };

        let tx_inputs = TxInputs {
            account_hash,
            nullifiers,
        };

        debug!(target: COMPONENT, %tx_inputs);

        Ok(tx_inputs)
    }

    async fn get_block_inputs(
        &self,
        updated_accounts: impl Iterator<Item = &AccountId> + Send,
        produced_nullifiers: impl Iterator<Item = &Digest> + Send,
    ) -> Result<BlockInputs, BlockInputsError> {
        let request = tonic::Request::new(GetBlockInputsRequest {
            account_ids: updated_accounts
                .map(|&account_id| account::AccountId::from(account_id))
                .collect(),
            nullifiers: produced_nullifiers.map(digest::Digest::from).collect(),
        });

        let store_response = self
            .store
            .clone()
            .get_block_inputs(request)
            .await
            .map_err(|err| BlockInputsError::GrpcClientError(err.message().to_string()))?
            .into_inner();

        Ok(store_response.try_into()?)
    }
}
