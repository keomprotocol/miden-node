//! `verify_tx(tx)` requirements:
//!
//! Store-related requirements
//! VT1: `tx.initial_account_hash` must match the account hash in store
//! VT2: If store doesn't contain account, `verify_tx` should check that it is a new account ( TODO ) and succeed
//! VT3: If `tx` consumes an already-consumed note in the store, `verify_tx` must fail
//!
//! in-flight related requirements
//! VT4: In each block, at most 1 transaction is allowed to modify any given account
//! VT5: `verify_tx(tx)` must fail if a previous transaction, not yet in the block, consumed a note
//!      that `tx` is also consuming

use std::iter;

use miden_objects::transaction::{InputNotes, OutputNotes};
use tokio::task::JoinSet;

use super::*;
use crate::test_utils::MockStoreSuccessBuilder;

/// Tests the happy path where 3 transactions who modify different accounts and consume different
/// notes all verify successfully
#[tokio::test]
async fn test_verify_tx_happy_path() {
    let tx_gen = DummyProvenTxGenerator::new();
    let (txs, accounts): (Vec<ProvenTransaction>, Vec<MockPrivateAccount>) =
        get_txs_and_accounts(&tx_gen, 3).unzip();

    let store = Arc::new(
        MockStoreSuccessBuilder::new()
            .initial_accounts(
                accounts
                    .into_iter()
                    .map(|mock_account| (mock_account.id, mock_account.states[0])),
            )
            .build(),
    );

    let state_view = DefaultStateView::new(store);

    for tx in txs {
        state_view.verify_tx(&tx).await.unwrap();
    }
}

/// Tests the happy path where 3 transactions who modify different accounts and consume different
/// notes all verify successfully.
///
/// In this test, all calls to `verify_tx()` are concurrent
#[tokio::test]
async fn test_verify_tx_happy_path_concurrent() {
    let tx_gen = DummyProvenTxGenerator::new();
    let (txs, accounts): (Vec<ProvenTransaction>, Vec<MockPrivateAccount>) =
        get_txs_and_accounts(&tx_gen, 3).unzip();

    let store = Arc::new(
        MockStoreSuccessBuilder::new()
            .initial_accounts(
                accounts
                    .into_iter()
                    .map(|mock_account| (mock_account.id, mock_account.states[0])),
            )
            .build(),
    );

    let state_view = Arc::new(DefaultStateView::new(store));

    let mut set = JoinSet::new();

    for tx in txs {
        let state_view = state_view.clone();
        set.spawn(async move { state_view.verify_tx(&tx).await });
    }

    while let Some(res) = set.join_next().await {
        res.unwrap().unwrap();
    }
}

/// Verifies requirement VT1
#[tokio::test]
async fn test_verify_tx_vt1() {
    let tx_gen = DummyProvenTxGenerator::new();

    let account = MockPrivateAccount::<3>::from(0);

    let store = Arc::new(
        MockStoreSuccessBuilder::new()
            .initial_accounts(iter::once((account.id, account.states[0])))
            .build(),
    );

    // The transaction's initial account hash uses `account.states[1]`, where the store expects
    // `account.states[0]`
    let tx = tx_gen.dummy_proven_tx_with_params(
        account.id,
        account.states[1],
        account.states[2],
        InputNotes::new(vec![nullifier_by_index(0)]).unwrap(),
        OutputNotes::new(Vec::new()).unwrap(),
    );

    let state_view = DefaultStateView::new(store);

    let verify_tx_result = state_view.verify_tx(&tx).await;

    assert_eq!(
        verify_tx_result,
        Err(VerifyTxError::IncorrectAccountInitialHash {
            tx_initial_account_hash: account.states[1],
            store_account_hash: Some(account.states[0])
        })
    );
}

/// TODO: Add new_account checks from the ProvenTransaction
/// Verifies requirement VT2
#[tokio::test]
async fn test_verify_tx_vt2() {
    let tx_gen = DummyProvenTxGenerator::new();

    let account_not_in_store: MockPrivateAccount<3> = MockPrivateAccount::from(0);

    // Notice: account is not added to the store
    let store = Arc::new(MockStoreSuccessBuilder::new().build());

    let tx = tx_gen.dummy_proven_tx_with_params(
        account_not_in_store.id,
        account_not_in_store.states[0],
        account_not_in_store.states[1],
        InputNotes::new(vec![nullifier_by_index(0)]).unwrap(),
        OutputNotes::new(Vec::new()).unwrap(),
    );

    let state_view = DefaultStateView::new(store);

    let verify_tx_result = state_view.verify_tx(&tx).await;

    assert!(verify_tx_result.is_ok());
}

/// Verifies requirement VT3
#[tokio::test]
async fn test_verify_tx_vt3() {
    let tx_gen = DummyProvenTxGenerator::new();

    let account: MockPrivateAccount<3> = MockPrivateAccount::from(0);

    let nullifier_in_store = nullifier_by_index(0);

    // Notice: `consumed_note_in_store` is added to the store
    let store = Arc::new(
        MockStoreSuccessBuilder::new()
            .initial_accounts(iter::once((account.id, account.states[0])))
            .initial_nullifiers(BTreeSet::from_iter(iter::once(nullifier_in_store.inner())))
            .build(),
    );

    let tx = tx_gen.dummy_proven_tx_with_params(
        account.id,
        account.states[0],
        account.states[1],
        InputNotes::new(vec![nullifier_in_store]).unwrap(),
        OutputNotes::new(Vec::new()).unwrap(),
    );

    let state_view = DefaultStateView::new(store);

    let verify_tx_result = state_view.verify_tx(&tx).await;

    assert_eq!(
        verify_tx_result,
        Err(VerifyTxError::InputNotesAlreadyConsumed(
            InputNotes::new(vec![nullifier_in_store]).unwrap()
        ))
    );
}

/// Verifies requirement VT4
#[tokio::test]
async fn test_verify_tx_vt4() {
    let tx_gen = DummyProvenTxGenerator::new();

    let account: MockPrivateAccount<3> = MockPrivateAccount::from(0);

    let store = Arc::new(
        MockStoreSuccessBuilder::new()
            .initial_accounts(iter::once((account.id, account.states[0])))
            .build(),
    );

    let tx1 = tx_gen.dummy_proven_tx_with_params(
        account.id,
        account.states[0],
        account.states[1],
        InputNotes::new(Vec::new()).unwrap(),
        OutputNotes::new(Vec::new()).unwrap(),
    );

    // Notice: tx2 modifies the same account as tx1, even though from a different initial state,
    // which is currently disallowed
    let tx2 = tx_gen.dummy_proven_tx_with_params(
        account.id,
        account.states[1],
        account.states[2],
        InputNotes::new(Vec::new()).unwrap(),
        OutputNotes::new(Vec::new()).unwrap(),
    );

    let state_view = DefaultStateView::new(store);

    let verify_tx1_result = state_view.verify_tx(&tx1).await;
    assert!(verify_tx1_result.is_ok());

    let verify_tx2_result = state_view.verify_tx(&tx2).await;
    assert_eq!(
        verify_tx2_result,
        Err(VerifyTxError::AccountAlreadyModifiedByOtherTx(account.id))
    );
}

/// Verifies requirement VT5
#[tokio::test]
async fn test_verify_tx_vt5() {
    let tx_gen = DummyProvenTxGenerator::new();

    let account_1: MockPrivateAccount<3> = MockPrivateAccount::from(0);
    let account_2: MockPrivateAccount<3> = MockPrivateAccount::from(1);
    let nullifier_in_both_txs = nullifier_by_index(0);

    // Notice: `consumed_note_in_both_txs` is NOT in the store
    let store = Arc::new(
        MockStoreSuccessBuilder::new()
            .initial_accounts(
                vec![account_1, account_2]
                    .into_iter()
                    .map(|account| (account.id, account.states[0])),
            )
            .build(),
    );

    let tx1 = tx_gen.dummy_proven_tx_with_params(
        account_1.id,
        account_1.states[0],
        account_1.states[1],
        InputNotes::new(vec![nullifier_in_both_txs]).unwrap(),
        OutputNotes::new(Vec::new()).unwrap(),
    );

    // Notice: tx2 modifies the same account as tx1, even though from a different initial state,
    // which is currently disallowed
    let tx2 = tx_gen.dummy_proven_tx_with_params(
        account_2.id,
        account_2.states[1],
        account_2.states[2],
        InputNotes::new(vec![nullifier_in_both_txs]).unwrap(),
        OutputNotes::new(Vec::new()).unwrap(),
    );

    let state_view = DefaultStateView::new(store);

    let verify_tx1_result = state_view.verify_tx(&tx1).await;
    assert!(verify_tx1_result.is_ok());

    let verify_tx2_result = state_view.verify_tx(&tx2).await;
    assert_eq!(
        verify_tx2_result,
        Err(VerifyTxError::InputNotesAlreadyConsumed(
            InputNotes::new(vec![nullifier_in_both_txs]).unwrap()
        ))
    );
}
