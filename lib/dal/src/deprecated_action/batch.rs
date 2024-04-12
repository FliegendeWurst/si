//! This module contains [`ActionBatch`], which groups [`ActionRunners`](crate::ActionRunner)
//! and indicates whether or not all "actions" in the group have completed executing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use si_data_pg::PgError;
use si_events::{ulid::Ulid, ContentHash};
use si_layer_cache::LayerDbError;
use std::collections::HashMap;
use std::sync::Arc;
use telemetry::prelude::*;
use thiserror::Error;

use crate::change_set::ChangeSetError;
use crate::workspace_snapshot::content_address::{ContentAddress, ContentAddressDiscriminants};
use crate::workspace_snapshot::edge_weight::{
    EdgeWeightError, EdgeWeightKind, EdgeWeightKindDiscriminants,
};
use crate::workspace_snapshot::node_weight::category_node_weight::CategoryNodeKind;
use crate::workspace_snapshot::node_weight::{NodeWeight, NodeWeightError};
use crate::workspace_snapshot::WorkspaceSnapshotError;
use crate::DeprecatedActionRunnerId;
use crate::{
    func::binding::return_value::FuncBindingReturnValueError,
    implement_add_edge_to,
    layer_db_types::{DeprecatedActionBatchContent, DeprecatedActionBatchContentV1},
    pk, ActionCompletionStatus, ComponentError, DalContext, DeprecatedActionPrototypeError,
    DeprecatedActionRunner, DeprecatedActionRunnerError, FuncError, HelperError, HistoryEventError,
    SchemaError, Timestamp, TransactionsError, WsEvent, WsEventError, WsEventResult, WsPayload,
};

#[remain::sorted]
#[derive(Error, Debug)]
pub enum DeprecatedActionBatchError {
    #[error(transparent)]
    ActionPrototype(#[from] DeprecatedActionPrototypeError),
    #[error("cannot stamp batch as started since it already finished")]
    AlreadyFinished,
    #[error("cannot stamp batch as started since it already started")]
    AlreadyStarted,
    #[error(transparent)]
    ChangeSet(#[from] ChangeSetError),
    #[error(transparent)]
    Component(#[from] ComponentError),
    #[error(transparent)]
    DeprecatedActionRunner(#[from] DeprecatedActionRunnerError),
    #[error("edge weight error: {0}")]
    EdgeWeight(#[from] EdgeWeightError),
    #[error("completion status is empty")]
    EmptyCompletionStatus,
    #[error(transparent)]
    Func(#[from] FuncError),
    #[error(transparent)]
    FuncBindingReturnValue(#[from] FuncBindingReturnValueError),
    #[error("helper error: {0}")]
    Helper(#[from] HelperError),
    #[error(transparent)]
    HistoryEvent(#[from] HistoryEventError),
    #[error("layer db error: {0}")]
    LayerDb(#[from] LayerDbError),
    #[error("no action runners in batch: action batch is empty")]
    NoActionRunnersInBatch(DeprecatedActionBatchId),
    #[error("node weight error: {0}")]
    NodeWeight(#[from] NodeWeightError),
    #[error("cannot stamp batch as finished since it has not yet been started")]
    NotYetStarted,
    #[error(transparent)]
    Pg(#[from] PgError),
    #[error(transparent)]
    Schema(#[from] SchemaError),
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
    #[error(transparent)]
    Transactions(#[from] TransactionsError),
    #[error("could not acquire lock: {0}")]
    TryLock(#[from] tokio::sync::TryLockError),
    #[error("workspace snapshot error: {0}")]
    WorkspaceSnapshot(#[from] WorkspaceSnapshotError),
    #[error(transparent)]
    WsEvent(#[from] WsEventError),
}

pub type DeprecatedActionBatchResult<T, E = DeprecatedActionBatchError> = std::result::Result<T, E>;

/// A batch of [`ActionRunners`](crate::ActionRunner). Every [`ActionRunner`](crate::ActionRunner)
/// must belong at one and only one [`batch`](Self).
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct DeprecatedActionBatch {
    pub id: DeprecatedActionBatchId,
    pub timestamp: Timestamp,

    // TODO(nick): automate with the logged in user.
    pub author: String,

    // This is a comma separated list of people involved in the ChangeSet
    pub actors: String,

    /// Indicates when the [`DeprecatedActionBatch`] started execution when populated.
    pub started_at: Option<DateTime<Utc>>,
    /// Indicates when the [`DeprecatedActionBatch`] finished execution when populated.
    pub finished_at: Option<DateTime<Utc>>,
    /// Indicates the state of the [`DeprecatedActionBatch`] when finished.
    pub completion_status: Option<ActionCompletionStatus>,
}

impl From<DeprecatedActionBatch> for DeprecatedActionBatchContentV1 {
    fn from(batch: DeprecatedActionBatch) -> Self {
        Self {
            author: batch.author,
            actors: batch.actors,
            started_at: batch.started_at,
            finished_at: batch.finished_at,
            completion_status: batch.completion_status,
            timestamp: batch.timestamp,
        }
    }
}

impl DeprecatedActionBatch {
    pub fn assemble(id: DeprecatedActionBatchId, content: DeprecatedActionBatchContentV1) -> Self {
        Self {
            id,
            author: content.author,
            actors: content.actors,
            started_at: content.started_at,
            finished_at: content.finished_at,
            completion_status: content.completion_status,
            timestamp: content.timestamp,
        }
    }

    implement_add_edge_to!(
        source_id: DeprecatedActionBatchId,
        destination_id: DeprecatedActionRunnerId,
        add_fn: add_edge_to_runner,
        discriminant: EdgeWeightKindDiscriminants::Use,
        result: DeprecatedActionBatchResult,
    );

    implement_add_edge_to!(
        source_id: Ulid,
        destination_id: DeprecatedActionBatchId,
        add_fn: add_category_edge,
        discriminant: EdgeWeightKindDiscriminants::Use,
        result: DeprecatedActionBatchResult,
    );

    pub async fn new(
        ctx: &DalContext,
        author: impl AsRef<str>,
        actors: &str,
    ) -> DeprecatedActionBatchResult<Self> {
        let timestamp = Timestamp::now();

        let content = DeprecatedActionBatchContentV1 {
            author: author.as_ref().to_owned(),
            actors: actors.to_owned(),
            started_at: None,
            finished_at: None,
            completion_status: None,
            timestamp,
        };

        let (hash, _) = ctx
            .layer_db()
            .cas()
            .write(
                Arc::new(DeprecatedActionBatchContent::V1(content.clone()).into()),
                None,
                ctx.events_tenancy(),
                ctx.events_actor(),
            )
            .await?;

        let change_set = ctx.change_set()?;
        let id = change_set.generate_ulid()?;
        let node_weight =
            NodeWeight::new_content(change_set, id, ContentAddress::DeprecatedActionBatch(hash))?;

        let workspace_snapshot = ctx.workspace_snapshot()?;

        workspace_snapshot.add_node(node_weight.to_owned()).await?;

        // Root --> ActionBatch Category --> Component (this)
        let category_id = workspace_snapshot
            .get_category_node(None, CategoryNodeKind::DeprecatedActionBatch)
            .await?;
        Self::add_category_edge(ctx, category_id, id.into(), EdgeWeightKind::new_use()).await?;

        Ok(Self::assemble(id.into(), content))
    }

    pub async fn get_by_id(
        ctx: &DalContext,
        id: DeprecatedActionBatchId,
    ) -> DeprecatedActionBatchResult<Self> {
        let workspace_snapshot = ctx.workspace_snapshot()?;
        let node_index = workspace_snapshot.get_node_index_by_id(id).await?;
        let node_weight = workspace_snapshot.get_node_weight(node_index).await?;
        let hash = node_weight.content_hash();

        let content: DeprecatedActionBatchContent = ctx
            .layer_db()
            .cas()
            .try_read_as(&hash)
            .await?
            .ok_or_else(|| WorkspaceSnapshotError::MissingContentFromStore(id.into()))?;

        // NOTE(nick,jacob,zack): if we had a v2, then there would be migration logic here.
        let DeprecatedActionBatchContent::V1(inner) = content;

        Ok(Self::assemble(id, inner))
    }

    pub async fn list(ctx: &DalContext) -> DeprecatedActionBatchResult<Vec<Self>> {
        let workspace_snapshot = ctx.workspace_snapshot()?;

        let mut action_batches = vec![];
        let action_batch_category_node_id = workspace_snapshot
            .get_category_node(None, CategoryNodeKind::DeprecatedActionBatch)
            .await?;

        let action_batch_node_indices = workspace_snapshot
            .outgoing_targets_for_edge_weight_kind(
                action_batch_category_node_id,
                EdgeWeightKindDiscriminants::Use,
            )
            .await?;

        let mut node_weights = vec![];
        let mut hashes = vec![];
        for index in action_batch_node_indices {
            let node_weight = workspace_snapshot
                .get_node_weight(index)
                .await?
                .get_content_node_weight_of_kind(
                    ContentAddressDiscriminants::DeprecatedActionBatch,
                )?;
            hashes.push(node_weight.content_hash());
            node_weights.push(node_weight);
        }

        let contents: HashMap<ContentHash, DeprecatedActionBatchContent> = ctx
            .layer_db()
            .cas()
            .try_read_many_as(hashes.as_slice())
            .await?;

        for node_weight in node_weights {
            match contents.get(&node_weight.content_hash()) {
                Some(content) => {
                    // NOTE(nick,jacob,zack): if we had a v2, then there would be migration logic here.
                    let DeprecatedActionBatchContent::V1(inner) = content;

                    action_batches.push(Self::assemble(node_weight.id().into(), inner.to_owned()));
                }
                None => Err(WorkspaceSnapshotError::MissingContentFromStore(
                    node_weight.id(),
                ))?,
            }
        }
        action_batches.sort_by(|a, b| b.id.cmp(&a.id));

        Ok(action_batches)
    }

    pub async fn runners(
        &self,
        ctx: &DalContext,
    ) -> DeprecatedActionBatchResult<Vec<DeprecatedActionRunner>> {
        Ok(DeprecatedActionRunner::for_batch(ctx, self.id).await?)
    }

    pub async fn set_completion_status(
        &mut self,
        ctx: &DalContext,
        status: Option<ActionCompletionStatus>,
    ) -> DeprecatedActionBatchResult<()> {
        self.completion_status = status;
        self.update_content(ctx).await
    }

    async fn update_content(&self, ctx: &DalContext) -> DeprecatedActionBatchResult<()> {
        let content = DeprecatedActionBatchContentV1::from(self.clone());

        let (hash, _) = ctx
            .layer_db()
            .cas()
            .write(
                Arc::new(DeprecatedActionBatchContent::V1(content).into()),
                None,
                ctx.events_tenancy(),
                ctx.events_actor(),
            )
            .await?;

        ctx.workspace_snapshot()?
            .update_content(ctx.change_set()?, self.id.into(), hash)
            .await?;

        Ok(())
    }

    pub async fn set_started_at(&mut self, ctx: &DalContext) -> DeprecatedActionBatchResult<()> {
        self.started_at = Some(Utc::now());
        self.update_content(ctx).await
    }

    pub async fn set_finished_at(&mut self, ctx: &DalContext) -> DeprecatedActionBatchResult<()> {
        self.finished_at = Some(Utc::now());
        self.update_content(ctx).await
    }

    /// A safe wrapper around setting the finished and completion status columns.
    pub async fn stamp_finished(
        &mut self,
        ctx: &DalContext,
    ) -> DeprecatedActionBatchResult<ActionCompletionStatus> {
        if self.started_at.is_some() {
            self.set_finished_at(ctx).await?;

            // TODO(nick): getting what the batch completion status should be can be a query.
            let mut batch_completion_status = ActionCompletionStatus::Success;
            for runner in self.runners(ctx).await? {
                match runner
                    .completion_status
                    .ok_or(DeprecatedActionBatchError::EmptyCompletionStatus)?
                {
                    ActionCompletionStatus::Success => {}
                    ActionCompletionStatus::Failure => {
                        // If we see failures, we should still continue to see if there's an error.
                        batch_completion_status = ActionCompletionStatus::Failure
                    }
                    ActionCompletionStatus::Error | ActionCompletionStatus::Unstarted => {
                        // Only break on an error since errors take precedence over failures.
                        batch_completion_status = ActionCompletionStatus::Error;
                        break;
                    }
                }
            }

            self.set_completion_status(ctx, Some(batch_completion_status))
                .await?;
            Ok(batch_completion_status)
        } else {
            Err(DeprecatedActionBatchError::NotYetStarted)
        }
    }

    /// A safe wrapper around setting the started column.
    pub async fn stamp_started(&mut self, ctx: &DalContext) -> DeprecatedActionBatchResult<()> {
        if self.started_at.is_some() {
            Err(DeprecatedActionBatchError::AlreadyStarted)
        } else if self.finished_at.is_some() {
            Err(DeprecatedActionBatchError::AlreadyFinished)
        } else if self.runners(ctx).await?.is_empty() {
            Err(DeprecatedActionBatchError::NoActionRunnersInBatch(self.id))
        } else {
            self.set_started_at(ctx).await?;
            Ok(())
        }
    }

    pub fn author(&self) -> String {
        self.author.clone()
    }

    pub fn actors(&self) -> String {
        self.actors.clone()
    }
}

pk!(DeprecatedActionBatchId);

#[derive(Clone, Deserialize, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeprecatedActionBatchReturn {
    id: DeprecatedActionBatchId,
    status: ActionCompletionStatus,
}

impl WsEvent {
    pub async fn action_batch_return(
        ctx: &DalContext,
        id: DeprecatedActionBatchId,
        status: ActionCompletionStatus,
    ) -> WsEventResult<Self> {
        WsEvent::new(
            ctx,
            WsPayload::DeprecatedActionBatchReturn(DeprecatedActionBatchReturn { id, status }),
        )
        .await
    }
}
