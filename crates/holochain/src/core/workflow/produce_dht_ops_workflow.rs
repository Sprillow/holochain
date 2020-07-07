use super::{error::WorkflowResult, InvokeZomeWorkspace};
use crate::core::queue_consumer::{OneshotWriter, TriggerSender, WorkComplete};
use crate::core::state::{
    dht_op_integration::{AuthoredDhtOpsStore, IntegrationQueueStore, IntegrationQueueValue},
    workspace::{Workspace, WorkspaceResult},
};
use holochain_state::{
    buffer::KvBuf,
    db::{AUTHORED_DHT_OPS, INTEGRATION_QUEUE},
    prelude::{BufferedStore, GetDb, Reader, Writer},
};
use holochain_types::{dht_op::DhtOpHashed, validate::ValidationStatus, TimestampKey};
use tracing::*;

pub mod dht_op_light;

// TODO: #[instrument]
pub async fn produce_dht_ops_workflow(
    mut workspace: ProduceDhtOpsWorkspace<'_>,
    writer: OneshotWriter,
    trigger_integration: &mut TriggerSender,
) -> WorkflowResult<WorkComplete> {
    let complete = produce_dht_ops_workflow_inner(&mut workspace).await?;

    // --- END OF WORKFLOW, BEGIN FINISHER BOILERPLATE ---

    // commit the workspace
    writer
        .with_writer(|writer| workspace.flush_to_txn(writer).expect("TODO"))
        .await?;

    // trigger other workflows
    trigger_integration.trigger();

    Ok(complete)
}

async fn produce_dht_ops_workflow_inner(
    workspace: &mut ProduceDhtOpsWorkspace<'_>,
) -> WorkflowResult<WorkComplete> {
    debug!("Starting dht op workflow");
    let invoke_zome_workspace = &mut workspace.invoke_zome_workspace;
    let all_ops = invoke_zome_workspace
        .source_chain
        .get_incomplete_dht_ops()
        .await?;

    for (index, ops) in all_ops {
        for op in ops {
            let (op, hash) = DhtOpHashed::with_data(op).await.into();
            debug!(?hash);
            workspace.integration_queue.put(
                (TimestampKey::now(), hash.clone()).into(),
                IntegrationQueueValue {
                    validation_status: ValidationStatus::Valid,
                    op,
                },
            )?;
            workspace.authored_dht_ops.put(hash, 0)?;
        }
        // Mark the dht op as complete
        invoke_zome_workspace.source_chain.complete_dht_op(index)?;
    }

    Ok(WorkComplete::Complete)
}

pub struct ProduceDhtOpsWorkspace<'env> {
    pub invoke_zome_workspace: InvokeZomeWorkspace<'env>,
    pub authored_dht_ops: AuthoredDhtOpsStore<'env>,
    pub integration_queue: IntegrationQueueStore<'env>,
}

impl<'env> Workspace<'env> for ProduceDhtOpsWorkspace<'env> {
    fn new(reader: &'env Reader<'env>, db: &impl GetDb) -> WorkspaceResult<Self> {
        let authored_dht_ops = db.get_db(&*AUTHORED_DHT_OPS)?;
        let integration_queue = db.get_db(&*INTEGRATION_QUEUE)?;
        Ok(Self {
            invoke_zome_workspace: InvokeZomeWorkspace::new(reader, db)?,
            authored_dht_ops: KvBuf::new(reader, authored_dht_ops)?,
            integration_queue: KvBuf::new(reader, integration_queue)?,
        })
    }

    fn flush_to_txn(self, writer: &mut Writer) -> WorkspaceResult<()> {
        self.invoke_zome_workspace.flush_to_txn(writer)?;
        self.authored_dht_ops.flush_to_txn(writer)?;
        self.integration_queue.flush_to_txn(writer)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::genesis_workflow::tests::fake_genesis;
    use super::*;
    use crate::core::state::{dht_op_integration::IntegrationQueueKey, source_chain::SourceChain};

    use fallible_iterator::FallibleIterator;
    use fixt::prelude::*;
    use holo_hash::{DhtOpHash, Hashable, Hashed, HoloHashBaseExt};

    use holochain_state::{
        env::{ReadManager, WriteManager},
        test_utils::test_cell_env,
    };
    use holochain_types::{
        dht_op::{ops_from_element, DhtOp, DhtOpHashed},
        header::{builder, EntryType},
        observability, Entry, EntryHashed,
    };
    use holochain_zome_types::entry_def::EntryVisibility;
    use matches::assert_matches;

    struct TestData {
        app_entry: Box<dyn Iterator<Item = Entry>>,
    }

    impl TestData {
        fn new() -> Self {
            let app_entry =
                Box::new(SerializedBytesFixturator::new(Unpredictable).map(|b| Entry::App(b)));
            Self { app_entry }
        }

        async fn put_fix_entry(
            &mut self,
            source_chain: &mut SourceChain<'_>,
            visibility: EntryVisibility,
        ) -> Vec<DhtOp> {
            let app_entry = self.app_entry.next().unwrap();
            let (app_entry, entry_hash) = EntryHashed::with_data(app_entry).await.unwrap().into();
            let app_entry_type = holochain_types::fixt::AppEntryTypeFixturator::new(visibility)
                .next()
                .unwrap();
            source_chain
                .put(
                    builder::EntryCreate {
                        entry_type: EntryType::App(app_entry_type),
                        entry_hash,
                    },
                    Some(app_entry),
                )
                .await
                .unwrap();
            let element = source_chain
                .get_element(source_chain.chain_head().unwrap())
                .await
                .unwrap()
                .unwrap();
            ops_from_element(&element).unwrap()
        }
    }

    #[tokio::test(threaded_scheduler)]
    async fn elements_produce_ops() {
        observability::test_run().ok();
        let env = test_cell_env();
        let dbs = env.dbs().await;
        let env_ref = env.guard().await;

        // Setup the database and expected data
        let expected: Vec<_> = {
            let reader = env_ref.reader().unwrap();
            let mut td = TestData::new();
            let mut source_chain = ProduceDhtOpsWorkspace::new(&reader, &dbs)
                .unwrap()
                .invoke_zome_workspace
                .source_chain;

            // Add genesis so we can use the source chain
            fake_genesis(&mut source_chain).await.unwrap();
            let headers: Vec<_> = source_chain.iter_back().collect().unwrap();
            // The ops will be created from start to end of the chain
            let headers: Vec<_> = headers.into_iter().rev().collect();
            let mut all_ops = Vec::new();
            // Collect the ops from genesis
            for h in headers {
                let ops = ops_from_element(
                    &source_chain
                        .get_element(h.as_hash())
                        .await
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
                all_ops.push(ops);
            }

            // Add some entries and collect the expected ops
            for _ in 0..10 as u8 {
                all_ops.push(
                    td.put_fix_entry(&mut source_chain, EntryVisibility::Public)
                        .await,
                );
                all_ops.push(
                    td.put_fix_entry(&mut source_chain, EntryVisibility::Private)
                        .await,
                );
            }

            env_ref
                .with_commit(|writer| source_chain.flush_to_txn(writer))
                .unwrap();

            all_ops.into_iter().flatten().collect()
        };

        // Run the workflow and commit it
        {
            let reader = env_ref.reader().unwrap();
            let mut workspace = ProduceDhtOpsWorkspace::new(&reader, &dbs).unwrap();
            let complete = produce_dht_ops_workflow_inner(&mut workspace)
                .await
                .unwrap();
            assert_matches!(complete, WorkComplete::Complete);
            env_ref
                .with_commit(|writer| workspace.flush_to_txn(writer))
                .unwrap();
        }

        // Pull out the results and check them
        let last_count = {
            let reader = env_ref.reader().unwrap();
            let workspace = ProduceDhtOpsWorkspace::new(&reader, &dbs).unwrap();
            let mut times = Vec::new();
            let results = workspace
                .integration_queue
                .iter()
                .unwrap()
                .map(|(k, v)| {
                    let s = debug_span!("times");
                    let _g = s.enter();
                    let t: (TimestampKey, DhtOpHash) = IntegrationQueueKey::from(k).into();
                    debug!(time = ?t.0);
                    debug!(hash = ?t.1);
                    times.push(t.0);
                    // Check the status is Valid
                    assert_matches!(v.validation_status, ValidationStatus::Valid);
                    Ok(v.op)
                })
                .collect::<Vec<_>>()
                .unwrap();

            // Check that the integration queue is ordered by time
            times.into_iter().fold(None, |last, time| {
                if let Some(lt) = last {
                    // Check they are ordered by time
                    assert!(lt <= time);
                }
                Some(time)
            });

            // Get the authored ops
            let mut authored_results = workspace
                .authored_dht_ops
                .iter()
                .unwrap()
                .map(|(k, v)| {
                    assert_eq!(v, 0);
                    Ok(DhtOpHash::with_pre_hashed(k.to_vec()))
                })
                .collect::<Vec<_>>()
                .unwrap();

            // Check we got all the hashes
            assert_eq!(results, expected);

            // Hash the results
            let mut results_hashed = Vec::new();
            for op in results {
                let (_, hash) = DhtOpHashed::with_data(op).await.into();
                results_hashed.push(hash);
            }

            // authored are in a different order so need to sort
            results_hashed.sort();
            authored_results.sort();
            // Check authored are all there
            assert_eq!(results_hashed, authored_results);
            results_hashed.len()
        };

        // Call the workflow again now the queue should be the same length as last time
        // because no new ops should hav been added
        {
            let reader = env_ref.reader().unwrap();
            let mut workspace = ProduceDhtOpsWorkspace::new(&reader, &dbs).unwrap();
            let complete = produce_dht_ops_workflow_inner(&mut workspace)
                .await
                .unwrap();
            assert_matches!(complete, WorkComplete::Complete);
            env_ref
                .with_commit(|writer| workspace.flush_to_txn(writer))
                .unwrap();
        }

        // Check the lengths are unchanged
        {
            let reader = env_ref.reader().unwrap();
            let workspace = ProduceDhtOpsWorkspace::new(&reader, &dbs).unwrap();
            let count = workspace.integration_queue.iter().unwrap().count().unwrap();
            let authored_count = workspace.authored_dht_ops.iter().unwrap().count().unwrap();

            assert_eq!(last_count, count);
            assert_eq!(last_count, authored_count);
        }
    }
}