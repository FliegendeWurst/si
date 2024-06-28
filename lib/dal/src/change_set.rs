//! The sequel to [`ChangeSets`](crate::ChangeSet). Coming to an SI instance near you!

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use si_layer_cache::LayerDbError;
use thiserror::Error;
use ulid::Generator;

use si_data_pg::{PgError, PgRow};
use si_events::{ulid::Ulid, WorkspaceSnapshotAddress};
use telemetry::prelude::*;

use crate::context::{Conflicts, RebaseRequest};
use crate::workspace_snapshot::vector_clock::VectorClockId;
use crate::{
    action::{ActionError, ActionId},
    id, ChangeSetStatus, ComponentError, DalContext, HistoryActor, HistoryEvent, HistoryEventError,
    TransactionsError, User, UserError, UserPk, Workspace, WorkspacePk, WorkspaceSnapshot,
    WorkspaceSnapshotError, WsEvent, WsEventError,
};

pub mod event;
pub mod status;
pub mod view;

/// The primary error type for this module.
#[remain::sorted]
#[derive(Debug, Error)]
pub enum ChangeSetError {
    #[error("could not find default change set: {0}")]
    DefaultChangeSetNotFound(ChangeSetId),
    #[error("default change set {0} has no workspace snapshot pointer")]
    DefaultChangeSetNoWorkspaceSnapshotPointer(ChangeSetId),
    #[error("enum parse error: {0}")]
    EnumParse(#[from] strum::ParseError),
    #[error("history event error: {0}")]
    HistoryEvent(#[from] HistoryEventError),
    #[error("invalid user actor pk")]
    InvalidActor(UserPk),
    #[error("invalid user system init")]
    InvalidUserSystemInit,
    #[error("layer db error: {0}")]
    LayerDb(#[from] LayerDbError),
    #[error("ulid monotonic error: {0}")]
    Monotonic(#[from] ulid::MonotonicError),
    #[error("mutex error: {0}")]
    Mutex(String),
    #[error("Changeset {0} does not have a base change set")]
    NoBaseChangeSet(ChangeSetId),
    #[error("no tenancy set in context")]
    NoTenancySet,
    #[error("Changeset {0} does not have a workspace snapshot")]
    NoWorkspaceSnapshot(ChangeSetId),
    #[error("pg error: {0}")]
    Pg(#[from] PgError),
    #[error("serde json error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("transactions error: {0}")]
    Transactions(#[from] TransactionsError),
    #[error("found an unexpected number of open change sets matching default change set (should be one, found {0:?})")]
    UnexpectedNumberOfOpenChangeSetsMatchingDefaultChangeSet(Vec<ChangeSetId>),
    #[error("user error: {0}")]
    User(#[from] UserError),
    #[error("workspace error: {0}")]
    Workspace(String),
    #[error("workspace not found: {0}")]
    WorkspaceNotFound(WorkspacePk),
    #[error("workspace snapshot error: {0}")]
    WorkspaceSnapshot(#[from] Box<WorkspaceSnapshotError>),
    #[error("ws event error: {0}")]
    WsEvent(#[from] Box<WsEventError>),
}

impl From<WsEventError> for ChangeSetError {
    fn from(value: WsEventError) -> Self {
        Self::WsEvent(Box::new(value))
    }
}

/// The primary result type for this module.
pub type ChangeSetResult<T> = Result<T, ChangeSetError>;

/// A superset of [`ChangeSetError`] used when performing apply logic.
#[remain::sorted]
#[derive(Debug, Error)]
pub enum ChangeSetApplyError {
    #[error("action error: {0}")]
    Action(#[from] ActionError),
    #[error("action prototype not found for id: {0}")]
    ActionPrototypeNotFound(ActionId),
    #[error("change set error: {0}")]
    ChangeSet(#[from] ChangeSetError),
    #[error("change set not found by id: {0}")]
    ChangeSetNotFound(ChangeSetId),
    #[error("component error: {0}")]
    Component(#[from] ComponentError),
    #[error("could not apply to head because of merge conflicts")]
    ConflictsOnApply(Conflicts),
    #[error("invalid user: {0}")]
    InvalidUser(UserPk),
    #[error("invalid user system init")]
    InvalidUserSystemInit,
    #[error("change set ({0}) does not have a base change set")]
    NoBaseChangeSet(ChangeSetId),
    #[error("transactions error: {0}")]
    Transactions(#[from] TransactionsError),
    #[error("user error: {0}")]
    User(#[from] UserError),
}

/// A superset of [`ChangeSetResult`] used when performing apply logic.
pub type ChangeSetApplyResult<T> = Result<T, ChangeSetApplyError>;

id!(ChangeSetId);

impl From<ChangeSetId> for si_events::ChangeSetId {
    fn from(value: ChangeSetId) -> Self {
        let id: ulid::Ulid = value.into();
        id.into()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ChangeSet {
    pub id: ChangeSetId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    pub name: String,
    pub status: ChangeSetStatus,
    pub base_change_set_id: Option<ChangeSetId>,
    pub workspace_snapshot_address: Option<WorkspaceSnapshotAddress>,
    pub workspace_id: Option<WorkspacePk>,
    pub merge_requested_by_user_id: Option<UserPk>,

    #[serde(skip)]
    pub generator: Arc<Mutex<Generator>>,
}

impl TryFrom<PgRow> for ChangeSet {
    type Error = ChangeSetError;

    fn try_from(value: PgRow) -> Result<Self, Self::Error> {
        let status_string: String = value.try_get("status")?;
        let status = ChangeSetStatus::try_from(status_string.as_str())?;
        Ok(Self {
            id: value.try_get("id")?,
            created_at: value.try_get("created_at")?,
            updated_at: value.try_get("updated_at")?,
            name: value.try_get("name")?,
            status,
            base_change_set_id: value.try_get("base_change_set_id")?,
            workspace_snapshot_address: value.try_get("workspace_snapshot_address")?,
            workspace_id: value.try_get("workspace_id")?,
            merge_requested_by_user_id: value.try_get("merge_requested_by_user_id")?,
            generator: Arc::new(Mutex::new(Default::default())),
        })
    }
}

impl ChangeSet {
    pub fn new_local() -> ChangeSetResult<Self> {
        let mut generator = Generator::new();
        let id: Ulid = generator.generate()?.into();

        Ok(Self {
            id: id.into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            generator: Arc::new(Mutex::new(generator)),
            base_change_set_id: None,
            workspace_snapshot_address: None,
            workspace_id: None,
            name: "".to_string(),
            status: ChangeSetStatus::Open,
            merge_requested_by_user_id: None,
        })
    }

    pub fn editing_changeset(&self) -> ChangeSetResult<Self> {
        let mut new_local = Self::new_local()?;
        new_local.base_change_set_id = self.base_change_set_id;
        new_local.workspace_snapshot_address = self.workspace_snapshot_address;
        new_local.workspace_id = self.workspace_id;
        self.name.clone_into(&mut new_local.name);
        self.status.clone_into(&mut new_local.status);
        Ok(new_local)
    }

    pub async fn new(
        ctx: &DalContext,
        name: impl AsRef<str>,
        base_change_set_id: Option<ChangeSetId>,
        workspace_snapshot_address: WorkspaceSnapshotAddress,
    ) -> ChangeSetResult<Self> {
        let id: ChangeSetId = Ulid::new().into();

        let workspace_snapshot = WorkspaceSnapshot::find(ctx, workspace_snapshot_address)
            .await
            .map_err(Box::new)?;
        // The workspace snapshot needs to be marked as seen by this new
        // changeset, so that edit sessions are able to know what is net new in
        // the edit session vs what the changeset already contained. The "onto"
        // changeset needs to have seen the "to_rebase" or we will treat them as
        // completely disjoint changesets.
        let workspace_snapshot_address = workspace_snapshot
            .write(ctx, id.into_inner().into())
            .await
            .map_err(Box::new)?;

        let workspace_id = ctx.tenancy().workspace_pk();
        let name = name.as_ref();
        let row = ctx
            .txns()
            .await?
            .pg()
            .query_one(
                "INSERT INTO change_set_pointers (id, name, base_change_set_id, status, workspace_id, workspace_snapshot_address) VALUES ($1, $2, $3, $4, $5, $6) RETURNING *",
                &[&id, &name, &base_change_set_id, &ChangeSetStatus::Open.to_string(), &workspace_id, &workspace_snapshot_address],
            )
            .await?;
        let change_set = Self::try_from(row)?;
        let _history_event = HistoryEvent::new(
            ctx,
            "change_set.create",
            "Change Set created",
            &serde_json::to_value(&change_set)?,
        )
        .await?;
        Ok(change_set)
    }

    pub async fn fork_head(ctx: &DalContext, name: impl AsRef<str>) -> ChangeSetResult<Self> {
        let workspace_pk = ctx
            .tenancy()
            .workspace_pk()
            .ok_or(ChangeSetError::NoTenancySet)?;

        let workspace = Workspace::get_by_pk(ctx, &workspace_pk)
            .await
            .map_err(|err| ChangeSetError::Workspace(err.to_string()))?
            .ok_or(ChangeSetError::WorkspaceNotFound(workspace_pk))?;

        let base_change_set = ChangeSet::find(ctx, workspace.default_change_set_id())
            .await?
            .ok_or(ChangeSetError::DefaultChangeSetNotFound(
                workspace.default_change_set_id(),
            ))?;

        let workspace_snapshot_address = base_change_set.workspace_snapshot_address.ok_or(
            ChangeSetError::DefaultChangeSetNoWorkspaceSnapshotPointer(
                workspace.default_change_set_id(),
            ),
        )?;
        let change_set = ChangeSet::new(
            ctx,
            name,
            Some(workspace.default_change_set_id()),
            workspace_snapshot_address,
        )
        .await?;

        Ok(change_set)
    }

    /// Create a [`VectorClockId`] from the [`ChangeSet`].
    pub fn vector_clock_id(&self) -> VectorClockId {
        VectorClockId::from(Ulid::from(self.id))
    }

    pub fn generate_ulid(&self) -> ChangeSetResult<Ulid> {
        self.generator
            .lock()
            .map_err(|e| ChangeSetError::Mutex(e.to_string()))?
            .generate()
            .map(Into::into)
            .map_err(Into::into)
    }

    pub async fn update_workspace_id(
        &mut self,
        ctx: &DalContext,
        workspace_id: WorkspacePk,
    ) -> ChangeSetResult<()> {
        ctx.txns()
            .await?
            .pg()
            .query_none(
                "UPDATE change_set_pointers SET workspace_id = $2 WHERE id = $1",
                &[&self.id, &workspace_id],
            )
            .await?;

        self.workspace_id = Some(workspace_id);

        Ok(())
    }

    pub async fn update_pointer(
        &mut self,
        ctx: &DalContext,
        workspace_snapshot_address: WorkspaceSnapshotAddress,
    ) -> ChangeSetResult<()> {
        ctx.txns()
            .await?
            .pg()
            .query_none(
                "UPDATE change_set_pointers SET workspace_snapshot_address = $2 WHERE id = $1",
                &[&self.id, &workspace_snapshot_address],
            )
            .await?;

        self.workspace_snapshot_address = Some(workspace_snapshot_address);

        Ok(())
    }

    pub async fn update_status(
        &mut self,
        ctx: &DalContext,
        status: ChangeSetStatus,
    ) -> ChangeSetResult<()> {
        ctx.txns()
            .await?
            .pg()
            .query_none(
                "UPDATE change_set_pointers SET status = $2 WHERE id = $1",
                &[&self.id, &status.to_string()],
            )
            .await?;

        self.status = status;

        Ok(())
    }

    pub async fn update_merge_requested_by_user_id(
        &mut self,
        ctx: &DalContext,
        user_pk: UserPk,
    ) -> ChangeSetResult<()> {
        ctx.txns()
            .await?
            .pg()
            .query_none(
                "UPDATE change_set_pointers SET merge_requested_by_user_id = $2 WHERE id = $1",
                &[&self.id, &user_pk],
            )
            .await?;

        self.merge_requested_by_user_id = Some(user_pk);

        Ok(())
    }

    #[instrument(
        name = "change_set.find",
        level = "debug",
        skip_all,
        fields(
            si.change_set.id = %change_set_id,
            si.workspace.pk = Empty,
        ),
    )]
    pub async fn find(
        ctx: &DalContext,
        change_set_id: ChangeSetId,
    ) -> ChangeSetResult<Option<Self>> {
        let row = ctx
            .txns()
            .await?
            .pg()
            .query_opt(
                "SELECT * FROM change_set_pointers WHERE id = $1",
                &[&change_set_id],
            )
            .await?;

        match row {
            Some(row) => {
                let span = Span::current();

                let change_set = Self::try_from(row)?;

                if let Some(workspace_id) = change_set.workspace_id {
                    span.record("si.workspace.pk", workspace_id.to_string());
                }
                Ok(Some(change_set))
            }
            None => Ok(None),
        }
    }

    pub async fn migrate_change_set_snapshot(
        ctx: &DalContext,
        change_set_id: ChangeSetId,
    ) -> ChangeSetResult<()> {
        let mut change_set = ChangeSet::find(ctx, change_set_id)
            .await?
            .ok_or(TransactionsError::ChangeSetNotFound(change_set_id))?;

        info!("migrating change set {} to updated graph", change_set_id);

        let snapshot_addr = change_set
            .workspace_snapshot_address
            .ok_or(TransactionsError::ChangeSetNotFound(change_set_id))?;

        let snapshot_bytes = ctx
            .layer_db()
            .workspace_snapshot()
            .read_bytes_from_durable_storage(&snapshot_addr)
            .await?
            .ok_or(WorkspaceSnapshotError::WorkspaceSnapshotGraphMissing(
                snapshot_addr,
            ))
            .map_err(Box::new)?;

        let migrated_snapshot = WorkspaceSnapshot::try_migrate_snapshot_bytes(snapshot_bytes)
            .await
            .map_err(Box::new)?;

        let (migrated_address, _) = ctx
            .layer_db()
            .workspace_snapshot()
            .write(
                migrated_snapshot.clone(),
                None,
                ctx.events_tenancy(),
                ctx.events_actor(),
            )
            .await?;

        change_set.update_pointer(ctx, migrated_address).await?;

        info!("migration of change set {} finished", change_set_id);

        Ok(())
    }

    pub async fn list_open(ctx: &DalContext) -> ChangeSetResult<Vec<Self>> {
        let mut result = vec![];
        let rows = ctx
            .txns()
            .await?
            .pg()
            .query(
                "SELECT * from change_set_pointers WHERE workspace_id = $1 AND status IN ($2, $3, $4)",
                &[
                    &ctx.tenancy().workspace_pk(),
                    &ChangeSetStatus::Open.to_string(),
                    &ChangeSetStatus::NeedsApproval.to_string(),
                    &ChangeSetStatus::NeedsAbandonApproval.to_string(),
                ],
            )
            .await?;

        for row in rows {
            result.push(Self::try_from(row)?);
        }

        Ok(result)
    }

    /// Applies the current [`ChangeSet`] in the provided [`DalContext`]. [`Actions`](Action)
    /// are enqueued as needed and only done so if the base [`ChangeSet`] is "HEAD" (i.e.
    /// the default [`ChangeSet`] of the [`Workspace`]).
    #[instrument(level = "info", skip_all)]
    pub async fn apply_to_base_change_set(ctx: &mut DalContext) -> ChangeSetApplyResult<ChangeSet> {
        // Apply to the base change with the current change set (non-editing) and commit.
        let mut change_set_to_be_applied = Self::find(ctx, ctx.change_set_id())
            .await?
            .ok_or(ChangeSetApplyError::ChangeSetNotFound(ctx.change_set_id()))?;
        ctx.update_visibility_and_snapshot_to_visibility_no_editing_change_set(ctx.change_set_id())
            .await?;
        change_set_to_be_applied
            .apply_to_base_change_set_inner(ctx)
            .await?;

        // do we need this commit?
        if let Some(conflicts) = ctx.blocking_commit().await? {
            error!("Conflicts when commiting again:{:?}", conflicts);

            return Err(ChangeSetApplyError::ConflictsOnApply(conflicts));
        }

        let change_set_that_was_applied = change_set_to_be_applied;

        Ok(change_set_that_was_applied)
    }

    /// Applies the current [`ChangeSet`] in the provided [`DalContext`] to its base
    /// [`ChangeSet`]. This involves performing a rebase request and updating the status
    /// of the [`ChangeSet`] accordingly.
    ///
    /// This function neither changes the visibility nor the snapshot after performing the
    /// aforementioned actions.
    async fn apply_to_base_change_set_inner(&mut self, ctx: &DalContext) -> ChangeSetResult<()> {
        let to_rebase_change_set_id = self
            .base_change_set_id
            .ok_or(ChangeSetError::NoBaseChangeSet(self.id))?;
        let onto_workspace_snapshot_address = self
            .workspace_snapshot_address
            .ok_or(ChangeSetError::NoWorkspaceSnapshot(self.id))?;
        let rebase_request = RebaseRequest {
            onto_workspace_snapshot_address,
            onto_vector_clock_id: self.vector_clock_id(),
            to_rebase_change_set_id,
        };
        ctx.do_rebase_request(rebase_request).await?;

        self.update_status(ctx, ChangeSetStatus::Applied).await?;
        let user = Self::extract_userid_from_context(ctx).await;
        WsEvent::change_set_applied(ctx, self.id, to_rebase_change_set_id, user)
            .await?
            .publish_on_commit(ctx)
            .await?;

        Ok(())
    }

    /// Returns a new [`ChangeSetId`](ChangeSet) if a new [`ChangeSet`] was created.
    pub async fn force_new(ctx: &mut DalContext) -> ChangeSetResult<Option<ChangeSetId>> {
        let maybe_fake_pk =
            if ctx.change_set_id() == ctx.get_workspace_default_change_set_id().await? {
                let change_set = Self::fork_head(ctx, Self::generate_name()).await?;
                ctx.update_visibility_and_snapshot_to_visibility(change_set.id)
                    .await?;

                WsEvent::change_set_created(ctx, change_set.id)
                    .await?
                    .publish_on_commit(ctx)
                    .await?;

                Some(change_set.id)
            } else {
                None
            };
        Ok(maybe_fake_pk)
    }

    fn generate_name() -> String {
        Utc::now().format("%Y-%m-%d-%H:%M").to_string()
    }

    pub async fn merge_vote(&mut self, ctx: &DalContext, vote: String) -> ChangeSetResult<()> {
        let user_id = Self::extract_userid_from_context(ctx).await;
        WsEvent::change_set_merge_vote(ctx, self.id, user_id, vote)
            .await?
            .publish_on_commit(ctx)
            .await?;

        Ok(())
    }
    pub async fn abandon_vote(&mut self, ctx: &DalContext, vote: String) -> ChangeSetResult<()> {
        let user_id = Self::extract_userid_from_context(ctx).await;
        WsEvent::change_set_abandon_vote(ctx, self.id, user_id, vote)
            .await?
            .publish_on_commit(ctx)
            .await?;

        Ok(())
    }

    pub async fn cancel_abandon_approval_flow(&mut self, ctx: &DalContext) -> ChangeSetResult<()> {
        self.update_status(ctx, ChangeSetStatus::Open).await?;
        let user_id = Self::extract_userid_from_context(ctx).await;
        WsEvent::change_set_cancel_abandon_approval_process(ctx, self.id, user_id)
            .await?
            .publish_on_commit(ctx)
            .await?;

        Ok(())
    }
    pub async fn begin_abandon_approval_flow(&mut self, ctx: &DalContext) -> ChangeSetResult<()> {
        self.update_status(ctx, ChangeSetStatus::NeedsAbandonApproval)
            .await?;
        let user_id = Self::extract_userid_from_context(ctx).await;
        WsEvent::change_set_begin_abandon_approval_process(ctx, self.id, user_id)
            .await?
            .publish_on_commit(ctx)
            .await?;
        WsEvent::change_set_abandon_vote(
            ctx,
            ctx.visibility().change_set_id,
            user_id,
            "Approve".to_string(),
        )
        .await?
        .publish_on_commit(ctx)
        .await?;
        Ok(())
    }

    pub async fn begin_approval_flow(&mut self, ctx: &DalContext) -> ChangeSetResult<()> {
        self.update_status(ctx, ChangeSetStatus::NeedsApproval)
            .await?;
        let user_id = Self::extract_userid_from_context(ctx).await;
        if let Some(user_pk) = user_id {
            self.update_merge_requested_by_user_id(ctx, user_pk).await?;
        }
        WsEvent::change_set_begin_approval_process(ctx, self.id, user_id)
            .await?
            .publish_on_commit(ctx)
            .await?;
        WsEvent::change_set_merge_vote(
            ctx,
            ctx.visibility().change_set_id,
            user_id,
            "Approve".to_string(),
        )
        .await?
        .publish_on_commit(ctx)
        .await?;
        Ok(())
    }

    pub async fn cancel_approval_flow(&mut self, ctx: &DalContext) -> ChangeSetResult<()> {
        self.update_status(ctx, ChangeSetStatus::Open).await?;
        let user_id = Self::extract_userid_from_context(ctx).await;
        WsEvent::change_set_cancel_approval_process(ctx, self.id, user_id)
            .await?
            .publish_on_commit(ctx)
            .await?;
        Ok(())
    }

    pub async fn abandon(&mut self, ctx: &DalContext) -> ChangeSetResult<()> {
        self.update_status(ctx, ChangeSetStatus::Abandoned).await?;
        let user_id = Self::extract_userid_from_context(ctx).await;
        WsEvent::change_set_abandoned(ctx, self.id, user_id)
            .await?
            .publish_on_commit(ctx)
            .await?;
        Ok(())
    }

    pub async fn extract_userid_from_context(ctx: &DalContext) -> Option<UserPk> {
        let user_id = match ctx.history_actor() {
            HistoryActor::User(user_pk) => {
                let maybe_user = User::get_by_pk(ctx, *user_pk).await;
                match maybe_user {
                    Ok(user_option) => user_option.map(|user| user.pk()),
                    Err(_) => None,
                }
            }
            HistoryActor::SystemInit => None,
        };
        user_id
    }

    #[instrument(
        name = "change_set.workspace_snapshot_in_use",
        level = "debug",
        skip_all,
        fields(
            si.workspace_snapshot_address = %workspace_snapshot_address,
            si.workspace.pk = Empty,
        ),
    )]
    pub async fn workspace_snapshot_address_in_use(
        ctx: &DalContext,
        workspace_snapshot_address: &WorkspaceSnapshotAddress,
    ) -> ChangeSetResult<bool> {
        let row = ctx
            .txns()
            .await?
            .pg()
            .query_one(
                "SELECT count(id) AS count FROM change_set_pointers WHERE workspace_snapshot_address = $1",
                &[&workspace_snapshot_address],
            )
            .await?;

        let count: i64 = row.get("count");
        if count > 0 {
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl std::fmt::Debug for ChangeSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangeSet")
            .field("id", &self.id.to_string())
            .field(
                "base_change_set_id",
                &self.base_change_set_id.map(|bcsid| bcsid.to_string()),
            )
            .field(
                "workspace_snapshot_address",
                &self
                    .workspace_snapshot_address
                    .map(|wsaddr| wsaddr.to_string()),
            )
            .field(
                "merge_requested_by_user_id",
                &self
                    .merge_requested_by_user_id
                    .map(|user_pk| user_pk.to_string()),
            )
            .finish()
    }
}
