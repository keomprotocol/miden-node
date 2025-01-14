use miden_air::trace::chiplets::hasher::Digest;
use miden_crypto::merkle::MerkleError;
use miden_node_proto::errors::ParseError;
use miden_node_utils::formatting::format_opt;
use miden_objects::{
    accounts::AccountId,
    notes::Nullifier,
    transaction::{InputNotes, ProvenTransaction},
    TransactionInputError,
};
use miden_vm::ExecutionError;
use thiserror::Error;

use crate::{CREATED_NOTES_TREE_INSERTION_DEPTH, MAX_NUM_CREATED_NOTES_PER_BATCH};

// Transaction verification errors
// =================================================================================================

#[derive(Error, Debug, PartialEq)]
pub enum VerifyTxError {
    /// The account that the transaction modifies has already been modified and isn't yet committed
    /// to a block
    #[error("Account {0} was already modified by other transaction")]
    AccountAlreadyModifiedByOtherTx(AccountId),

    /// Another transaction already consumed the notes with given nullifiers
    #[error("Input notes with given nullifier were already consumed by another transaction")]
    InputNotesAlreadyConsumed(InputNotes<Nullifier>),

    /// The account's initial hash did not match the current account's hash
    #[error("Incorrect account's initial hash ({tx_initial_account_hash}, stored: {})", format_opt(.store_account_hash.as_ref()))]
    IncorrectAccountInitialHash {
        tx_initial_account_hash: Digest,
        store_account_hash: Option<Digest>,
    },

    /// Failed to retrieve transaction inputs from the store
    ///
    /// TODO: Make this an "internal error". Q: Should we have a single `InternalError` enum for all
    /// internal errors that can occur across the system?
    #[error("Failed to retrieve transaction inputs from the store: {0}")]
    StoreConnectionFailed(#[from] TxInputsError),

    #[error("Transaction input error: {0}")]
    TransactionInputError(#[from] TransactionInputError),
}

// Transaction adding errors
// =================================================================================================

#[derive(Error, Debug)]
pub enum AddTransactionError {
    #[error("Transaction verification failed: {0}")]
    VerificationFailed(#[from] VerifyTxError),
}

// Batch building errors
// =================================================================================================

/// Error that may happen while building a transaction batch.
///
/// These errors are returned from the batch builder to the transaction queue, instead of
/// dropping the transactions, they are included into the error values, so that the transaction
/// queue can re-queue them.
#[derive(Error, Debug)]
pub enum BuildBatchError {
    #[error(
        "Too many notes in the batch. Got: {0}, max: {}",
        MAX_NUM_CREATED_NOTES_PER_BATCH
    )]
    TooManyNotesCreated(usize, Vec<ProvenTransaction>),

    #[error("failed to create notes SMT: {0}")]
    NotesSmtError(MerkleError, Vec<ProvenTransaction>),
}

impl BuildBatchError {
    pub fn into_transactions(self) -> Vec<ProvenTransaction> {
        match self {
            BuildBatchError::TooManyNotesCreated(_, txs) => txs,
            BuildBatchError::NotesSmtError(_, txs) => txs,
        }
    }
}

// Block prover errors
// =================================================================================================

#[derive(Error, Debug, PartialEq)]
pub enum BlockProverError {
    #[error("Received invalid merkle path")]
    InvalidMerklePaths(MerkleError),
    #[error("program execution failed")]
    ProgramExecutionFailed(ExecutionError),
    #[error("failed to retrieve {0} root from stack outputs")]
    InvalidRootOutput(String),
}

// Block inputs errors
// =================================================================================================

#[derive(Debug, PartialEq, Error)]
pub enum BlockInputsError {
    #[error("failed to parse protobuf message: {0}")]
    ParseError(#[from] ParseError),
    #[error("gRPC client failed with error: {0}")]
    GrpcClientError(String),
}

// Block applying errors
// =================================================================================================

#[derive(Debug, PartialEq, Eq, Error)]
pub enum ApplyBlockError {
    #[error("gRPC client failed with error: {0}")]
    GrpcClientError(String),
}

// Block building errors
// =================================================================================================

#[derive(Debug, Error, PartialEq)]
pub enum BuildBlockError {
    #[error("failed to compute new block: {0}")]
    BlockProverFailed(#[from] BlockProverError),
    #[error("failed to apply block: {0}")]
    ApplyBlockFailed(#[from] ApplyBlockError),
    #[error("failed to get block inputs from store: {0}")]
    GetBlockInputsFailed(#[from] BlockInputsError),
    #[error("transaction batches and store don't modify the same account IDs. Offending accounts: {0:?}")]
    InconsistentAccountIds(Vec<AccountId>),
    #[error("transaction batches and store contain different hashes for some accounts. Offending accounts: {0:?}")]
    InconsistentAccountStates(Vec<AccountId>),
    #[error(
        "too many batches in block. Got: {0}, max: 2^{}",
        CREATED_NOTES_TREE_INSERTION_DEPTH
    )]
    TooManyBatchesInBlock(usize),
}

// Transaction inputs errors
// =================================================================================================

#[derive(Debug, PartialEq, Error)]
pub enum TxInputsError {
    #[error("gRPC client failed with error: {0}")]
    GrpcClientError(String),
    #[error("malformed response from store: {0}")]
    MalformedResponse(String),
    #[error("failed to parse protobuf message: {0}")]
    ParseError(#[from] ParseError),
    #[error("dummy")]
    Dummy,
}
