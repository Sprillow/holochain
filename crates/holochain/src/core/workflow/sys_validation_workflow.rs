//! The workflow and queue consumer for sys validation
#![allow(deprecated)]

use super::*;
use crate::conductor::api::CellConductorApiT;
use crate::core::queue_consumer::OneshotWriter;
use crate::core::queue_consumer::TriggerSender;
use crate::core::queue_consumer::WorkComplete;
use crate::core::sys_validate::*;
use crate::core::validation::*;
use error::WorkflowError;
use error::WorkflowResult;
use fallible_iterator::FallibleIterator;
use holo_hash::DhtOpHash;
use holochain_cascade::Cascade;
use holochain_cascade::DbPair;
use holochain_cascade::DbPairMut;
use holochain_cascade2::test_utils::HolochainP2pCellT2;
use holochain_cascade2::Cascade as Cascade2;
use holochain_p2p::HolochainP2pCell;
use holochain_p2p::HolochainP2pCellT;
use holochain_sqlite::buffer::BufferedStore;
use holochain_sqlite::buffer::KvBufFresh;
use holochain_sqlite::fresh_reader;
use holochain_sqlite::prelude::*;

use holochain_sqlite::db::ReadManager;
use holochain_state::prelude::*;
use holochain_types::prelude::*;
use holochain_zome_types::Entry;
use holochain_zome_types::ValidationStatus;
use std::collections::BinaryHeap;
use std::convert::TryFrom;
use std::convert::TryInto;
use tracing::*;

use produce_dht_ops_workflow::dht_op_light::light_to_op;
use types::Outcome;

pub mod types;

mod sys_validation_query;

#[cfg(test)]
mod chain_test;
#[cfg(test)]
mod test_ideas;
#[cfg(test)]
mod tests;

#[instrument(skip(
    workspace,
    writer,
    trigger_app_validation,
    sys_validation_trigger,
    network,
    conductor_api
))]
pub async fn sys_validation_workflow(
    mut workspace: SysValidationWorkspace2,
    writer: OneshotWriter,
    mut trigger_app_validation: TriggerSender,
    sys_validation_trigger: TriggerSender,
    network: HolochainP2pCell,
    conductor_api: impl CellConductorApiT,
) -> WorkflowResult<WorkComplete> {
    let complete = sys_validation_workflow_inner(
        &mut workspace,
        network,
        conductor_api,
        sys_validation_trigger,
    )
    .await?;

    // --- END OF WORKFLOW, BEGIN FINISHER BOILERPLATE ---

    // commit the workspace
    writer.with_writer(|writer| Ok(workspace.flush_to_txn_ref(writer)?))?;

    // trigger other workflows
    trigger_app_validation.trigger();

    Ok(complete)
}

async fn sys_validation_workflow_inner(
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    conductor_api: impl CellConductorApiT,
    sys_validation_trigger: TriggerSender,
) -> WorkflowResult<WorkComplete> {
    let env = workspace.env.clone();
    let sorted_ops = sys_validation_query::get_ops_to_sys_validate(&env)?;

    // Process each op
    for so in sorted_ops {
        let (op, op_hash) = so.into_inner();

        // Create an incoming ops sender for any dependencies we find
        // that we are meant to be holding but aren't.
        // If we are not holding them they will be added to our incoming ops.
        let incoming_dht_ops_sender =
            IncomingDhtOpSender::new(workspace.env.clone().into(), sys_validation_trigger.clone());

        let outcome = validate_op(
            &op,
            workspace,
            network.clone(),
            &conductor_api,
            Some(incoming_dht_ops_sender),
        )
        .await?;

        match outcome {
            Outcome::Accepted => {
                workspace.put_validation_limbo(op_hash, ValidationLimboStatus::SysValidated)?;
            }
            Outcome::SkipAppValidation => {
                workspace.put_integration_limbo(op_hash, ValidationStatus::Valid)?;
            }
            Outcome::AwaitingOpDep(missing_dep) => {
                // TODO: Try and get this dependency to add to limbo
                //
                // I actually can't see how we can do this because there's no
                // way to get an DhtOpHash without either having the op or the full
                // header. We have neither that's why where here.
                //
                // We need to be holding the dependency because
                // we were meant to get a StoreElement or StoreEntry or
                // RegisterAgentActivity or RegisterAddLink.
                let status = ValidationLimboStatus::AwaitingSysDeps(missing_dep);
                workspace.put_validation_limbo(op_hash, status)?;
            }
            Outcome::MissingDhtDep => {
                // TODO: Not sure what missing dht dep is. Check if we need this.
                workspace.put_validation_limbo(op_hash, ValidationLimboStatus::Pending)?;
            }
            Outcome::Rejected => {
                workspace.put_integration_limbo(op_hash, ValidationStatus::Rejected)?;
            }
        }
    }
    Ok(WorkComplete::Complete)
}

async fn validate_op(
    op: &DhtOp,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    conductor_api: &impl CellConductorApiT,
    incoming_dht_ops_sender: Option<IncomingDhtOpSender>,
) -> WorkflowResult<Outcome> {
    match validate_op_inner(
        op,
        workspace,
        network,
        conductor_api,
        incoming_dht_ops_sender,
    )
    .await
    {
        Ok(_) => match op {
            // TODO: Check strict mode where store element
            // is also run through app validation
            DhtOp::RegisterAgentActivity(_, _) => Ok(Outcome::SkipAppValidation),
            _ => Ok(Outcome::Accepted),
        },
        // Handle the errors that result in pending or awaiting deps
        Err(SysValidationError::ValidationOutcome(e)) => {
            warn!(
                agent = %which_agent(conductor_api.cell_id().agent_pubkey()),
                msg = "DhtOp has failed system validation",
                ?op,
                error = ?e,
                error_msg = %e
            );
            Ok(handle_failed(e))
        }
        Err(e) => Err(e.into()),
    }
}

/// For now errors result in an outcome but in the future
/// we might find it useful to include the reason something
/// was rejected etc.
/// This is why the errors contain data but is currently unread.
fn handle_failed(error: ValidationOutcome) -> Outcome {
    use Outcome::*;
    match error {
        ValidationOutcome::Counterfeit(_, _) => {
            unreachable!("Counterfeit ops are dropped before sys validation")
        }
        ValidationOutcome::DepMissingFromDht(_) => MissingDhtDep,
        ValidationOutcome::EntryDefId(_) => Rejected,
        ValidationOutcome::EntryHash => Rejected,
        ValidationOutcome::EntryTooLarge(_, _) => Rejected,
        ValidationOutcome::EntryType => Rejected,
        ValidationOutcome::EntryVisibility(_) => Rejected,
        ValidationOutcome::TagTooLarge(_, _) => Rejected,
        ValidationOutcome::NotCreateLink(_) => Rejected,
        ValidationOutcome::NotNewEntry(_) => Rejected,
        ValidationOutcome::NotHoldingDep(dep) => AwaitingOpDep(dep),
        ValidationOutcome::PrevHeaderError(PrevHeaderError::MissingMeta(dep)) => {
            AwaitingOpDep(dep.into())
        }
        ValidationOutcome::PrevHeaderError(_) => Rejected,
        ValidationOutcome::PrivateEntry => Rejected,
        ValidationOutcome::UpdateTypeMismatch(_, _) => Rejected,
        ValidationOutcome::VerifySignature(_, _) => Rejected,
        ValidationOutcome::ZomeId(_) => Rejected,
    }
}

async fn validate_op_inner(
    op: &DhtOp,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    conductor_api: &impl CellConductorApiT,
    incoming_dht_ops_sender: Option<IncomingDhtOpSender>,
) -> SysValidationResult<()> {
    match op {
        DhtOp::StoreElement(_, header, entry) => {
            store_element(header, workspace, network.clone()).await?;
            if let Some(entry) = entry {
                store_entry(
                    (header)
                        .try_into()
                        .map_err(|_| ValidationOutcome::NotNewEntry(header.clone()))?,
                    entry.as_ref(),
                    conductor_api,
                    workspace,
                    network,
                )
                .await?;
            }
            Ok(())
        }
        DhtOp::StoreEntry(_, header, entry) => {
            store_entry(
                (header).into(),
                entry.as_ref(),
                conductor_api,
                workspace,
                network.clone(),
            )
            .await?;

            let header = header.clone().into();
            store_element(&header, workspace, network).await?;
            Ok(())
        }
        DhtOp::RegisterAgentActivity(_, header) => {
            register_agent_activity(header, workspace, network.clone(), incoming_dht_ops_sender)
                .await?;
            store_element(header, workspace, network).await?;
            Ok(())
        }
        DhtOp::RegisterUpdatedContent(_, header, entry) => {
            register_updated_content(header, workspace, network.clone(), incoming_dht_ops_sender)
                .await?;
            if let Some(entry) = entry {
                store_entry(
                    NewEntryHeaderRef::Update(header),
                    entry.as_ref(),
                    conductor_api,
                    workspace,
                    network.clone(),
                )
                .await?;
            }

            Ok(())
        }
        DhtOp::RegisterUpdatedElement(_, header, entry) => {
            register_updated_element(header, workspace, network.clone(), incoming_dht_ops_sender)
                .await?;
            if let Some(entry) = entry {
                store_entry(
                    NewEntryHeaderRef::Update(header),
                    entry.as_ref(),
                    conductor_api,
                    workspace,
                    network.clone(),
                )
                .await?;
            }

            Ok(())
        }
        DhtOp::RegisterDeletedBy(_, header) => {
            register_deleted_by(header, workspace, network, incoming_dht_ops_sender).await?;
            Ok(())
        }
        DhtOp::RegisterDeletedEntryHeader(_, header) => {
            register_deleted_entry_header(header, workspace, network, incoming_dht_ops_sender)
                .await?;
            Ok(())
        }
        DhtOp::RegisterAddLink(_, header) => {
            register_add_link(header, workspace, network, incoming_dht_ops_sender).await?;
            Ok(())
        }
        DhtOp::RegisterRemoveLink(_, header) => {
            register_delete_link(header, workspace, network, incoming_dht_ops_sender).await?;
            Ok(())
        }
    }
}

#[instrument(skip(element, call_zome_workspace, network, conductor_api))]
/// Direct system validation call that takes
/// an Element instead of an op.
/// Does not require holding dependencies.
/// Will not await dependencies and instead returns
/// that outcome immediately.
pub async fn sys_validate_element(
    element: &Element,
    call_zome_workspace: &mut CallZomeWorkspace,
    network: HolochainP2pCell,
    conductor_api: &impl CellConductorApiT,
) -> SysValidationOutcome<()> {
    trace!(?element);
    // Create a SysValidationWorkspace with the scratches from the CallZomeWorkspace
    let workspace = SysValidationWorkspace::try_from(&*call_zome_workspace)?;
    // TODO: Remove this
    let mut workspace: SysValidationWorkspace2 = workspace.into();
    let result =
        match sys_validate_element_inner(element, &mut workspace, network, conductor_api).await {
            // Validation succeeded
            Ok(_) => Ok(()),
            // Validation failed so exit with that outcome
            Err(SysValidationError::ValidationOutcome(validation_outcome)) => {
                error!(msg = "Direct validation failed", ?element);
                validation_outcome.into_outcome()
            }
            // An error occurred so return it
            Err(e) => Err(OutcomeOrError::Err(e)),
        };

    // TODO: This is probably fine to remove because cache is now
    // a separate db but confirm that.
    // Set the call zome workspace to the updated
    // cache from the sys validation workspace
    // call_zome_workspace.meta_cache = workspace.meta_cache;
    // call_zome_workspace.element_cache = workspace.element_cache;

    result
}

async fn sys_validate_element_inner(
    element: &Element,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    conductor_api: &impl CellConductorApiT,
) -> SysValidationResult<()> {
    let signature = element.signature();
    let header = element.header();
    let entry = element.entry().as_option();
    let incoming_dht_ops_sender = None;
    if !counterfeit_check(signature, header).await? {
        return Err(ValidationOutcome::Counterfeit(signature.clone(), header.clone()).into());
    }
    store_element(header, workspace, network.clone()).await?;
    if let Some((entry, EntryVisibility::Public)) =
        &entry.and_then(|e| header.entry_type().map(|et| (e, et.visibility())))
    {
        store_entry(
            (header)
                .try_into()
                .map_err(|_| ValidationOutcome::NotNewEntry(header.clone()))?,
            entry,
            conductor_api,
            workspace,
            network.clone(),
        )
        .await?;
    }
    match header {
        Header::Update(header) => {
            register_updated_content(header, workspace, network, incoming_dht_ops_sender).await?;
        }
        Header::Delete(header) => {
            register_deleted_entry_header(header, workspace, network, incoming_dht_ops_sender)
                .await?;
        }
        Header::CreateLink(header) => {
            register_add_link(header, workspace, network, incoming_dht_ops_sender).await?;
        }
        Header::DeleteLink(header) => {
            register_delete_link(header, workspace, network, incoming_dht_ops_sender).await?;
        }
        _ => {}
    }
    Ok(())
}

/// Check if the op has valid signature and author.
/// Ops that fail this check should be dropped.
pub async fn counterfeit_check(
    signature: &Signature,
    header: &Header,
) -> SysValidationResult<bool> {
    Ok(verify_header_signature(&signature, &header).await?
        && author_key_is_valid(header.author()).await?)
}

async fn register_agent_activity(
    header: &Header,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    incoming_dht_ops_sender: Option<IncomingDhtOpSender>,
) -> SysValidationResult<()> {
    // Get data ready to validate
    let prev_header_hash = header.prev_header();

    // Checks
    check_prev_header(&header)?;
    check_valid_if_dna(&header, &workspace).await?;
    if let Some(prev_header_hash) = prev_header_hash {
        check_and_hold_register_agent_activity(
            prev_header_hash,
            workspace,
            network,
            incoming_dht_ops_sender,
            |_| Ok(()),
        )
        .await?;
    }
    check_chain_rollback(&header, &workspace).await?;
    Ok(())
}

async fn store_element(
    header: &Header,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
) -> SysValidationResult<()> {
    // Get data ready to validate
    let prev_header_hash = header.prev_header();

    // Checks
    check_prev_header(header)?;
    if let Some(prev_header_hash) = prev_header_hash {
        let mut cascade = workspace.full_cascade(network);
        let prev_header = cascade
            .retrieve_header(prev_header_hash.clone(), Default::default())
            .await?
            .ok_or_else(|| ValidationOutcome::DepMissingFromDht(prev_header_hash.clone().into()))?;
        check_prev_timestamp(&header, prev_header.header())?;
        check_prev_seq(&header, prev_header.header())?;
    }
    Ok(())
}

async fn store_entry(
    header: NewEntryHeaderRef<'_>,
    entry: &Entry,
    conductor_api: &impl CellConductorApiT,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
) -> SysValidationResult<()> {
    // Get data ready to validate
    let entry_type = header.entry_type();
    let entry_hash = header.entry_hash();

    // Checks
    check_entry_type(entry_type, entry)?;
    if let EntryType::App(app_entry_type) = entry_type {
        let entry_def = check_app_entry_type(app_entry_type, conductor_api).await?;
        check_not_private(&entry_def)?;
    }
    check_entry_hash(entry_hash, entry).await?;
    check_entry_size(entry)?;

    // Additional checks if this is an Update
    if let NewEntryHeaderRef::Update(entry_update) = header {
        let original_header_address = &entry_update.original_header_address;
        let mut cascade = workspace.full_cascade(network);
        let original_header = cascade
            .retrieve_header(original_header_address.clone(), Default::default())
            .await?
            .ok_or_else(|| {
                ValidationOutcome::DepMissingFromDht(original_header_address.clone().into())
            })?;
        update_check(entry_update, original_header.header())?;
    }
    Ok(())
}

async fn register_updated_content(
    entry_update: &Update,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    incoming_dht_ops_sender: Option<IncomingDhtOpSender>,
) -> SysValidationResult<()> {
    // Get data ready to validate
    let original_header_address = &entry_update.original_header_address;

    let dependency_check =
        |original_element: &Element| update_check(entry_update, original_element.header());

    check_and_hold_store_entry(
        original_header_address,
        workspace,
        network,
        incoming_dht_ops_sender,
        dependency_check,
    )
    .await?;
    Ok(())
}

async fn register_updated_element(
    entry_update: &Update,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    incoming_dht_ops_sender: Option<IncomingDhtOpSender>,
) -> SysValidationResult<()> {
    // Get data ready to validate
    let original_header_address = &entry_update.original_header_address;

    let dependency_check =
        |original_element: &Element| update_check(entry_update, original_element.header());

    check_and_hold_store_element(
        original_header_address,
        workspace,
        network,
        incoming_dht_ops_sender,
        dependency_check,
    )
    .await?;
    Ok(())
}

async fn register_deleted_by(
    element_delete: &Delete,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    incoming_dht_ops_sender: Option<IncomingDhtOpSender>,
) -> SysValidationResult<()> {
    // Get data ready to validate
    let removed_header_address = &element_delete.deletes_address;

    // Checks
    let dependency_check =
        |removed_header: &Element| check_new_entry_header(removed_header.header());

    check_and_hold_store_element(
        removed_header_address,
        workspace,
        network,
        incoming_dht_ops_sender,
        dependency_check,
    )
    .await?;
    Ok(())
}

async fn register_deleted_entry_header(
    element_delete: &Delete,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    incoming_dht_ops_sender: Option<IncomingDhtOpSender>,
) -> SysValidationResult<()> {
    // Get data ready to validate
    let removed_header_address = &element_delete.deletes_address;

    // Checks
    let dependency_check =
        |removed_header: &Element| check_new_entry_header(removed_header.header());

    check_and_hold_store_entry(
        removed_header_address,
        workspace,
        network,
        incoming_dht_ops_sender,
        dependency_check,
    )
    .await?;
    Ok(())
}

async fn register_add_link(
    link_add: &CreateLink,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    incoming_dht_ops_sender: Option<IncomingDhtOpSender>,
) -> SysValidationResult<()> {
    // Get data ready to validate
    let base_entry_address = &link_add.base_address;
    let target_entry_address = &link_add.target_address;

    // Checks
    check_and_hold_any_store_entry(
        base_entry_address,
        workspace,
        network.clone(),
        incoming_dht_ops_sender,
        |_| Ok(()),
    )
    .await?;

    let mut cascade = workspace.full_cascade(network);
    cascade
        .retrieve_entry(target_entry_address.clone(), Default::default())
        .await?
        .ok_or_else(|| ValidationOutcome::DepMissingFromDht(target_entry_address.clone().into()))?;

    check_tag_size(&link_add.tag)?;
    Ok(())
}

async fn register_delete_link(
    link_remove: &DeleteLink,
    workspace: &mut SysValidationWorkspace2,
    network: HolochainP2pCell,
    incoming_dht_ops_sender: Option<IncomingDhtOpSender>,
) -> SysValidationResult<()> {
    // Get data ready to validate
    let link_add_address = &link_remove.link_add_address;

    // Checks
    check_and_hold_register_add_link(
        link_add_address,
        workspace,
        network,
        incoming_dht_ops_sender,
        |_| Ok(()),
    )
    .await?;
    Ok(())
}

fn update_check(entry_update: &Update, original_header: &Header) -> SysValidationResult<()> {
    check_new_entry_header(original_header)?;
    let original_header: NewEntryHeaderRef = original_header
        .try_into()
        .expect("This can't fail due to the above check_new_entry_header");
    check_update_reference(entry_update, &original_header)?;
    Ok(())
}

pub struct SysValidationWorkspace2 {
    env: EnvRead,
    cache: EnvRead,
}

impl SysValidationWorkspace2 {
    pub fn put_validation_limbo(
        &self,
        hash: DhtOpHash,
        status: ValidationLimboStatus,
    ) -> WorkflowResult<()> {
        self.env.conn()?.with_commit(|txn| {
            set_validation_stage(txn, hash, status)?;
            WorkflowResult::Ok(())
        })?;
        Ok(())
    }
    pub fn put_integration_limbo(
        &self,
        hash: DhtOpHash,
        status: ValidationStatus,
    ) -> WorkflowResult<()> {
        self.env.conn()?.with_commit(|txn| {
            set_validation_status(txn, hash.clone(), status)?;
            set_validation_stage(txn, hash, ValidationLimboStatus::AwaitingIntegration)?;
            WorkflowResult::Ok(())
        })?;
        Ok(())
    }
    pub fn is_chain_empty(&self, author: &AgentPubKey) -> DatabaseResult<bool> {
        let chain_not_empty = self.env.conn()?.with_reader(|txn| {
            let mut stmt = txn.prepare(
                "
            SELECT 
            *
            FROM Header
            WHERE
            Header.author = :author
            ",
            )?;
            DatabaseResult::Ok(stmt.exists(named_params! {
                ":author": author,
            })?)
        })?;
        Ok(!chain_not_empty)
    }
    /// Create a cascade with local data only
    pub fn local_cascade(&mut self) -> Cascade2 {
        Cascade2::empty()
            .with_vault(self.env.clone())
            // TODO: Does the cache count as local?
            .with_cache(self.cache.clone().into())
    }
    pub fn full_cascade<Network: HolochainP2pCellT2 + Clone + 'static + Send>(
        &mut self,
        network: Network,
    ) -> Cascade2<Network> {
        Cascade2::<Network>::empty()
            .with_vault(self.env.clone())
            .with_network(network, self.cache.clone().into())
    }
}

impl Workspace for SysValidationWorkspace2 {
    fn flush_to_txn_ref(&mut self, writer: &mut Writer) -> WorkspaceResult<()> {
        todo!("Flush scratch");
        Ok(())
    }
}

impl From<SysValidationWorkspace> for SysValidationWorkspace2 {
    fn from(old: SysValidationWorkspace) -> Self {
        SysValidationWorkspace2 {
            env: old.env,
            cache: todo!("Make cache env"),
        }
    }
}

#[deprecated = "Remove when updating sys validation tests for sql"]
pub struct SysValidationWorkspace {
    pub integration_limbo: IntegrationLimboStore,
    pub validation_limbo: ValidationLimboStore,
    /// Integrated data
    pub element_vault: ElementBuf,
    pub meta_vault: MetadataBuf,
    /// Data pending validation
    pub element_pending: ElementBuf<PendingPrefix>,
    pub meta_pending: MetadataBuf<PendingPrefix>,
    /// Read only rejected store for finding dependency data
    pub element_rejected: ElementBuf<RejectedPrefix>,
    pub meta_rejected: MetadataBuf<RejectedPrefix>,
    // Read only authored store for finding dependency data
    pub element_authored: ElementBuf<AuthoredPrefix>,
    pub meta_authored: MetadataBuf<AuthoredPrefix>,
    /// Cached data
    pub element_cache: ElementBuf,
    pub meta_cache: MetadataBuf,
    pub env: EnvRead,
}

impl<'a> SysValidationWorkspace {
    pub fn cascade<Network: HolochainP2pCellT + Clone + Send + 'static>(
        &'a mut self,
        network: Network,
        keystore: KeystoreSender,
    ) -> Cascade<'a, Network> {
        Cascade::new(
            EnvRead::from_parts(self.validation_limbo.env().clone(), keystore),
            &self.element_authored,
            &self.meta_authored,
            &self.element_vault,
            &self.meta_vault,
            &self.element_rejected,
            &self.meta_rejected,
            &mut self.element_cache,
            &mut self.meta_cache,
            network,
        )
    }
}

impl SysValidationWorkspace {
    pub fn new(env: EnvRead) -> WorkspaceResult<Self> {
        let db = env.get_table(TableName::IntegrationLimbo)?;
        let integration_limbo = KvBufFresh::new(env.clone(), db);

        let validation_limbo = ValidationLimboStore::new(env.clone())?;

        let element_vault = ElementBuf::vault(env.clone(), false)?;
        let meta_vault = MetadataBuf::vault(env.clone())?;
        let element_cache = ElementBuf::cache(env.clone())?;
        let meta_cache = MetadataBuf::cache(env.clone())?;

        let element_pending = ElementBuf::pending(env.clone())?;
        let meta_pending = MetadataBuf::pending(env.clone())?;

        // READ ONLY
        let element_authored = ElementBuf::authored(env.clone(), false)?;
        let meta_authored = MetadataBuf::authored(env.clone())?;
        let element_rejected = ElementBuf::rejected(env.clone())?;
        let meta_rejected = MetadataBuf::rejected(env.clone())?;

        Ok(Self {
            integration_limbo,
            validation_limbo,
            element_vault,
            meta_vault,
            element_pending,
            meta_pending,
            element_rejected,
            meta_rejected,
            element_authored,
            meta_authored,
            element_cache,
            meta_cache,
            env,
        })
    }

    fn put_val_limbo(
        &mut self,
        hash: DhtOpHash,
        mut vlv: ValidationLimboValue,
    ) -> WorkflowResult<()> {
        vlv.last_try = Some(timestamp::now());
        vlv.num_tries += 1;
        self.validation_limbo.put(hash, vlv)?;
        Ok(())
    }

    #[tracing::instrument(skip(self, hash))]
    fn put_int_limbo(&mut self, hash: DhtOpHash, iv: IntegrationLimboValue) -> WorkflowResult<()> {
        self.integration_limbo.put(hash, iv)?;
        Ok(())
    }

    pub fn network_only_cascade<Network: HolochainP2pCellT + Clone + Send + 'static>(
        &mut self,
        network: Network,
    ) -> Cascade<'_, Network> {
        let cache_data = DbPairMut {
            element: &mut self.element_cache,
            meta: &mut self.meta_cache,
        };
        Cascade::empty()
            .with_network(network)
            .with_cache(cache_data)
    }

    /// Create a cascade with local data only
    pub fn local_cascade(&mut self) -> Cascade<'_> {
        let integrated_data = DbPair {
            element: &self.element_vault,
            meta: &self.meta_vault,
        };
        let authored_data = DbPair {
            element: &self.element_authored,
            meta: &self.meta_authored,
        };
        let pending_data = DbPair {
            element: &self.element_pending,
            meta: &self.meta_pending,
        };
        let rejected_data = DbPair {
            element: &self.element_rejected,
            meta: &self.meta_rejected,
        };
        let cache_data = DbPairMut {
            element: &mut self.element_cache,
            meta: &mut self.meta_cache,
        };
        Cascade::empty()
            .with_integrated(integrated_data)
            .with_authored(authored_data)
            .with_pending(pending_data)
            .with_cache(cache_data)
            .with_rejected(rejected_data)
    }

    /// Get a cascade over all local databases and the network
    pub fn full_cascade<Network: HolochainP2pCellT + Clone>(
        &mut self,
        network: Network,
    ) -> Cascade<'_, Network> {
        self.local_cascade().with_network(network)
    }
}

impl Workspace for SysValidationWorkspace {
    fn flush_to_txn_ref(&mut self, writer: &mut Writer) -> WorkspaceResult<()> {
        self.validation_limbo.0.flush_to_txn_ref(writer)?;
        self.integration_limbo.flush_to_txn_ref(writer)?;
        // Flush for cascade
        self.element_cache.flush_to_txn_ref(writer)?;
        self.meta_cache.flush_to_txn_ref(writer)?;

        self.element_pending.flush_to_txn_ref(writer)?;
        self.meta_pending.flush_to_txn_ref(writer)?;
        Ok(())
    }
}

/// Create a new SysValidationWorkspace with the scratches from the CallZomeWorkspace
impl TryFrom<&CallZomeWorkspace> for SysValidationWorkspace {
    type Error = WorkspaceError;

    fn try_from(call_zome: &CallZomeWorkspace) -> Result<Self, Self::Error> {
        let CallZomeWorkspace {
            source_chain,
            meta_authored,
            element_integrated,
            meta_integrated,
            element_rejected,
            meta_rejected,
            element_cache,
            meta_cache,
        } = call_zome;
        let mut sys_val = Self::new(call_zome.env().clone())?;
        sys_val.element_authored = source_chain.elements().into();
        sys_val.meta_authored = meta_authored.into();
        sys_val.element_vault = element_integrated.into();
        sys_val.meta_vault = meta_integrated.into();
        sys_val.element_rejected = element_rejected.into();
        sys_val.meta_rejected = meta_rejected.into();
        sys_val.element_cache = element_cache.into();
        sys_val.meta_cache = meta_cache.into();
        Ok(sys_val)
    }
}
