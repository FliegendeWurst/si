use chrono::{DateTime, Utc};
use module_index_client::{ModuleDetailsResponse, ModuleIndexClient};
use petgraph::Direction;
use serde::{Deserialize, Serialize};
use si_data_nats::NatsError;
use si_data_pg::{PgError, PgRow};
use si_events::{ContentHash, VectorClockId};
use si_layer_cache::db::serialize;
use si_layer_cache::LayerDbError;
use si_pkg::{
    SiPkg, SiPkgError, WorkspaceExport, WorkspaceExportChangeSetV0, WorkspaceExportContentV0,
    WorkspaceExportMetadataV0,
};
use std::collections::{HashMap, VecDeque};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use telemetry::prelude::*;
use thiserror::Error;
use tokio::task::{JoinError, JoinSet};
use tokio::time::{self, Instant};
use ulid::Ulid;

use crate::change_set::{ChangeSet, ChangeSetError, ChangeSetId};
use crate::feature_flags::FeatureFlag;
use crate::layer_db_types::ContentTypes;
use crate::pkg::{import_pkg_from_pkg, ImportOptions, PkgError};
use crate::workspace_snapshot::graph::WorkspaceSnapshotGraphDiscriminants;
use crate::workspace_snapshot::WorkspaceSnapshotError;
use crate::{
    builtins, pk, standard_model, standard_model_accessor_ro, DalContext, HistoryActor,
    HistoryEvent, HistoryEventError, KeyPairError, StandardModelError, Tenancy, Timestamp,
    TransactionsError, User, UserError, UserPk, WorkspaceSnapshot,
};

const WORKSPACE_GET_BY_PK: &str = include_str!("queries/workspace/get_by_pk.sql");
const WORKSPACE_LIST_FOR_USER: &str = include_str!("queries/workspace/list_for_user.sql");

const DEFAULT_BUILTIN_WORKSPACE_NAME: &str = "builtin";
const DEFAULT_BUILTIN_WORKSPACE_TOKEN: &str = "builtin";
const DEFAULT_CHANGE_SET_NAME: &str = "HEAD";

#[remain::sorted]
#[derive(Error, Debug)]
pub enum WorkspaceError {
    #[error("migrating builtin functions failed")]
    BuiltinMigrationsFailed,
    #[error("builtin workspace not found")]
    BuiltinWorkspaceNotFound,
    #[error("change set error: {0}")]
    ChangeSet(#[from] ChangeSetError),
    #[error("change set not found by id: {0}")]
    ChangeSetNotFound(ChangeSetId),
    #[error("Trying to export from system actor. This can only be done by a user actor")]
    ExportingFromSystemActor,
    #[error(transparent)]
    HistoryEvent(#[from] HistoryEventError),
    #[error("Trying to import a changeset that does not have a valid base: {0}")]
    ImportingOrphanChangeset(ChangeSetId),
    #[error("invalid user {0}")]
    InvalidUser(UserPk),
    #[error(transparent)]
    Join(#[from] JoinError),
    #[error(transparent)]
    KeyPair(#[from] KeyPairError),
    #[error("LayerDb error: {0}")]
    LayerDb(#[from] LayerDbError),
    #[error("Module index: {0}")]
    ModuleIndex(#[from] module_index_client::ModuleIndexClientError),
    #[error("Module index url not set")]
    ModuleIndexNotSet,
    #[error(transparent)]
    Nats(#[from] NatsError),
    #[error("no user in context")]
    NoUserInContext,
    #[error(transparent)]
    Pg(#[from] PgError),
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
    #[error(transparent)]
    SiPkg(#[from] SiPkgError),
    #[error(transparent)]
    StandardModel(#[from] StandardModelError),
    #[error("strum parse error: {0}")]
    StrumParse(#[from] strum::ParseError),
    #[error(transparent)]
    Transactions(#[from] TransactionsError),
    #[error("Unable to parse URL: {0}")]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    User(#[from] UserError),
    #[error("workspace not found: {0}")]
    WorkspaceNotFound(WorkspacePk),
    #[error("workspace snapshot error: {0}")]
    WorkspaceSnapshot(#[from] WorkspaceSnapshotError),
}

pub type WorkspaceResult<T, E = WorkspaceError> = std::result::Result<T, E>;

pk!(WorkspacePk);
pk!(WorkspaceId);

impl From<WorkspacePk> for si_events::WorkspacePk {
    fn from(value: WorkspacePk) -> Self {
        let id: ulid::Ulid = value.into();
        id.into()
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pk: WorkspacePk,
    name: String,
    default_change_set_id: ChangeSetId,
    uses_actions_v2: bool,
    #[serde(flatten)]
    timestamp: Timestamp,
    token: Option<String>,
    snapshot_version: WorkspaceSnapshotGraphDiscriminants,
}

impl TryFrom<PgRow> for Workspace {
    type Error = WorkspaceError;

    fn try_from(row: PgRow) -> Result<Self, Self::Error> {
        let created_at: DateTime<Utc> = row.try_get("created_at")?;
        let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
        let snapshot_version: String = row.try_get("snapshot_version")?;
        Ok(Self {
            pk: row.try_get("pk")?,
            name: row.try_get("name")?,
            default_change_set_id: row.try_get("default_change_set_id")?,
            uses_actions_v2: row.try_get("uses_actions_v2")?,
            timestamp: Timestamp::assemble(created_at, updated_at),
            token: row.try_get("token")?,
            snapshot_version: WorkspaceSnapshotGraphDiscriminants::from_str(&snapshot_version)?,
        })
    }
}

impl Workspace {
    pub fn pk(&self) -> &WorkspacePk {
        &self.pk
    }

    pub fn default_change_set_id(&self) -> ChangeSetId {
        self.default_change_set_id
    }

    pub fn uses_actions_v2(&self) -> bool {
        self.uses_actions_v2
    }

    pub fn token(&self) -> Option<String> {
        self.token.clone()
    }

    pub fn snapshot_version(&self) -> WorkspaceSnapshotGraphDiscriminants {
        self.snapshot_version
    }

    pub async fn set_token(&mut self, ctx: &DalContext, token: String) -> WorkspaceResult<()> {
        ctx.txns()
            .await?
            .pg()
            .query_none(
                "UPDATE workspaces SET token = $2 WHERE pk = $1",
                &[&self.pk, &token],
            )
            .await?;
        self.token = Some(token);

        Ok(())
    }

    pub async fn update_default_change_set_id(
        &mut self,
        ctx: &DalContext,
        change_set_id: ChangeSetId,
    ) -> WorkspaceResult<()> {
        ctx.txns()
            .await?
            .pg()
            .query_none(
                "UPDATE workspaces SET default_change_set_id = $2 WHERE pk = $1",
                &[&self.pk, &change_set_id],
            )
            .await?;

        self.default_change_set_id = change_set_id;

        Ok(())
    }

    // Find or create the builtin [`Workspace`].
    #[instrument(skip_all)]
    pub async fn setup_builtin(ctx: &mut DalContext) -> WorkspaceResult<()> {
        // Check if the builtin already exists. If so, update our tenancy and visibility using it.
        if let Some(found_builtin) = Self::find_builtin(ctx).await? {
            ctx.update_tenancy(Tenancy::new(*found_builtin.pk()));
            ctx.update_visibility_and_snapshot_to_visibility(found_builtin.default_change_set_id)
                .await?;

            return Ok(());
        }

        let initial_vector_clock_id = VectorClockId::new(
            WorkspaceId::NONE.into_inner(),
            WorkspaceId::NONE.into_inner(),
        );
        let workspace_snapshot = WorkspaceSnapshot::initial(ctx, initial_vector_clock_id).await?;

        // If not, create the builtin workspace with a corresponding base change set and initial
        // workspace snapshot.
        let mut change_set = ChangeSet::new(
            ctx,
            DEFAULT_CHANGE_SET_NAME,
            None,
            workspace_snapshot.id().await,
        )
        .await?;
        let change_set_id = change_set.id;

        let head_pk = WorkspaceId::NONE;

        let uses_actions_v2 = ctx
            .services_context()
            .feature_flags_service()
            .feature_is_enabled(&FeatureFlag::ActionsV2);

        let row = ctx
            .txns()
            .await?
            .pg()
            .query_one(
                "INSERT INTO workspaces (pk, name, default_change_set_id, uses_actions_v2, token) VALUES ($1, $2, $3, $4, $5) RETURNING *",
                &[&head_pk, &DEFAULT_BUILTIN_WORKSPACE_NAME, &change_set_id, &uses_actions_v2, &DEFAULT_BUILTIN_WORKSPACE_TOKEN],
            )
            .await?;

        let workspace = Self::try_from(row)?;
        let workspace_pk = *workspace.pk();

        change_set.update_workspace_id(ctx, workspace_pk).await?;

        // Update our tenancy and visibility once it has been created.
        ctx.update_tenancy(Tenancy::new(workspace_pk));
        ctx.update_visibility_and_snapshot_to_visibility(change_set.id)
            .await?;

        Ok(())
    }

    /// This method attempts to find the builtin [`Workspace`].
    #[instrument(skip_all)]
    pub async fn find_builtin(ctx: &DalContext) -> WorkspaceResult<Option<Self>> {
        let head_pk = WorkspaceId::NONE;
        let maybe_row = ctx
            .txns()
            .await?
            .pg()
            .query_opt("SELECT * FROM workspaces WHERE pk = $1", &[&head_pk])
            .await?;
        let maybe_builtin = match maybe_row {
            Some(found) => Some(Self::try_from(found)?),
            None => None,
        };
        Ok(maybe_builtin)
    }

    pub async fn list_for_user(ctx: &DalContext) -> WorkspaceResult<Vec<Self>> {
        let user_pk = match ctx.history_actor() {
            HistoryActor::User(user_pk) => *user_pk,
            _ => return Err(WorkspaceError::NoUserInContext),
        };
        let rows = ctx
            .txns()
            .await?
            .pg()
            .query(WORKSPACE_LIST_FOR_USER, &[&user_pk])
            .await?;

        Ok(standard_model::objects_from_rows(rows)?)
    }

    pub async fn find_first_user_workspace(ctx: &DalContext) -> WorkspaceResult<Option<Self>> {
        let maybe_row = ctx.txns().await?.pg().query_opt(
            "SELECT row_to_json(w.*) AS object FROM workspaces AS w WHERE pk != $1 ORDER BY created_at ASC LIMIT 1", &[&WorkspacePk::NONE],
        ).await?;
        let maybe_workspace = match maybe_row {
            Some(found) => Some(Self::try_from(found)?),
            None => None,
        };
        Ok(maybe_workspace)
    }

    pub async fn new(
        ctx: &mut DalContext,
        pk: WorkspacePk,
        name: impl AsRef<str>,
    ) -> WorkspaceResult<Self> {
        let initial_vector_clock_id = VectorClockId::new(pk.into_inner(), pk.into_inner());
        let workspace_snapshot = WorkspaceSnapshot::initial(ctx, initial_vector_clock_id).await?;

        let mut change_set = ChangeSet::new(
            ctx,
            DEFAULT_CHANGE_SET_NAME,
            None,
            workspace_snapshot.id().await,
        )
        .await?;
        let change_set_id = change_set.id;

        let uses_actions_v2 = ctx
            .services_context()
            .feature_flags_service()
            .feature_is_enabled(&FeatureFlag::ActionsV2);

        let name = name.as_ref();
        let row = ctx
            .txns()
            .await?
            .pg()
            .query_one(
                "INSERT INTO workspaces (pk, name, default_change_set_id, uses_actions_v2) VALUES ($1, $2, $3, $4) RETURNING *",
                &[&pk, &name, &change_set_id, &uses_actions_v2],
            )
            .await?;
        let new_workspace = Self::try_from(row)?;

        change_set
            .update_workspace_id(ctx, *new_workspace.pk())
            .await?;

        ctx.update_tenancy(Tenancy::new(new_workspace.pk));

        // TODO(nick,zack,jacob): convert visibility (or get rid of it?) to use our the new change set id.
        // should set_change_set and set_workspace_snapshot happen in update_visibility?
        ctx.update_visibility_and_snapshot_to_visibility(change_set.id)
            .await?;

        let _history_event = HistoryEvent::new(
            ctx,
            "workspace.create".to_owned(),
            "Workspace created".to_owned(),
            &serde_json::json![{ "visibility": ctx.visibility() }],
        )
        .await?;

        Self::migrate_workspace(ctx).await?;

        Ok(new_workspace)
    }

    async fn migrate_workspace(ctx: &DalContext) -> WorkspaceResult<()> {
        info!("migrating intrinsic functions");
        let _ = match builtins::func::migrate_intrinsics(ctx).await {
            Err(_) => Err(WorkspaceError::BuiltinMigrationsFailed),
            _ => Ok(()),
        };

        info!("migrating builtins");
        let module_index_url = ctx
            .module_index_url()
            .ok_or(WorkspaceError::ModuleIndexNotSet)?;

        let mut interval = time::interval(Duration::from_secs(5));
        let instant = Instant::now();

        let module_index_client =
            ModuleIndexClient::unauthenticated_client(module_index_url.try_into()?);
        let install_builtins = Self::install_latest_builtins(ctx, module_index_client);
        tokio::pin!(install_builtins);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    info!(elapsed = instant.elapsed().as_secs_f32(), "migrating");
                }
                result = &mut install_builtins  => match result {
                    Ok(_) => {
                        info!(elapsed = instant.elapsed().as_secs_f32(), "migrating completed");
                        break;
                    }
                    Err(err) => return Err(err),
                }
            }
        }

        Ok(())
    }

    async fn install_latest_builtins(
        ctx: &DalContext,
        module_index_client: ModuleIndexClient,
    ) -> WorkspaceResult<()> {
        let module_list = module_index_client.list_builtins().await?;
        let modules = module_list.modules;

        let total = modules.len();

        let mut join_set = JoinSet::new();
        for module in modules {
            let module = module.clone();
            let client = module_index_client.clone();
            join_set.spawn(async move {
                (
                    module.name.to_owned(),
                    (
                        module.to_owned(),
                        Self::fetch_builtin(&module, &client).await,
                    ),
                )
            });
        }

        let mut count: usize = 0;
        while let Some(res) = join_set.join_next().await {
            let (pkg_name, (module, res)) = res?;
            match res {
                Ok(pkg) => {
                    let instant = Instant::now();

                    match import_pkg_from_pkg(
                        ctx,
                        &pkg,
                        Some(ImportOptions {
                            is_builtin: true,
                            schema_id: module.schema_id().map(Into::into),
                            past_module_hashes: module.past_hashes,
                            ..Default::default()
                        }),
                    )
                    .await
                    {
                        Ok(_) => {
                            count += 1;
                            let elapsed = instant.elapsed().as_secs_f32();
                            info!(
                                    "pkg {pkg_name} install finished successfully and took {elapsed:.2} seconds ({count} of {total} installed)",
                                );
                        }
                        Err(PkgError::PackageAlreadyInstalled(hash)) => {
                            count += 1;
                            warn!(%hash, "pkg {pkg_name} already installed ({count} of {total} installed)");
                        }
                        Err(err) => error!(?err, "pkg {pkg_name} install failed"),
                    }
                }
                Err(err) => {
                    error!(?err, "pkg {pkg_name} install failed with server error");
                }
            }
        }

        let mut ctx = ctx.clone();
        ctx.commit().await?;
        ctx.update_snapshot_to_visibility().await?;

        Ok(())
    }

    async fn fetch_builtin(
        module: &ModuleDetailsResponse,
        module_index_client: &ModuleIndexClient,
    ) -> WorkspaceResult<SiPkg> {
        let module = module_index_client
            .get_builtin(Ulid::from_string(&module.id).unwrap_or_default())
            .await?;

        Ok(SiPkg::load_from_bytes(module)?)
    }

    pub async fn clear(&self, ctx: &DalContext) -> WorkspaceResult<()> {
        let tenancy = Tenancy::new(self.pk);

        ctx.txns()
            .await?
            .pg()
            .execute("SELECT clear_workspace_v1($1)", &[&tenancy])
            .await?;

        Ok(())
    }

    pub async fn clear_or_create_workspace(
        ctx: &mut DalContext,
        workspace_pk: WorkspacePk,
        workspace_name: impl AsRef<str>,
    ) -> WorkspaceResult<Self> {
        let workspace = match Workspace::get_by_pk(ctx, &workspace_pk).await? {
            Some(existing_workspace) => {
                existing_workspace.clear(ctx).await?;
                existing_workspace
            }
            None => Workspace::new(ctx, workspace_pk, workspace_name).await?,
        };

        Ok(workspace)
    }

    pub async fn get_by_pk(
        ctx: &DalContext,
        pk: &WorkspacePk,
    ) -> WorkspaceResult<Option<Workspace>> {
        let row = ctx
            .txns()
            .await?
            .pg()
            .query_opt(WORKSPACE_GET_BY_PK, &[&pk])
            .await?;
        if let Some(row) = row {
            let json: serde_json::Value = row.try_get("object")?;
            Ok(serde_json::from_value(json)?)
        } else {
            Ok(None)
        }
    }

    pub async fn get_by_pk_or_error(
        ctx: &DalContext,
        pk: &WorkspacePk,
    ) -> WorkspaceResult<Workspace> {
        Self::get_by_pk(ctx, pk)
            .await?
            .ok_or(WorkspaceError::WorkspaceNotFound(*pk))
    }

    pub async fn generate_export_data(
        &self,
        ctx: &DalContext,
        workspace_version: &str,
    ) -> WorkspaceResult<WorkspaceExport> {
        let mut content_hashes = vec![];
        let mut change_sets: HashMap<Ulid, Vec<WorkspaceExportChangeSetV0>> = HashMap::new();
        let mut default_change_set_base = Ulid::nil();
        for change_set in ChangeSet::list_open(ctx).await? {
            let snap = WorkspaceSnapshot::find_for_change_set(ctx, change_set.id).await?;

            // From root, get every value from every node, store with hash
            let mut queue = VecDeque::from([snap.root().await?]);

            while let Some(this_node_idx) = queue.pop_front() {
                // Queue contents
                content_hashes.extend(
                    snap.get_node_weight(this_node_idx)
                        .await?
                        .content_store_hashes(),
                );

                let children = snap
                    .edges_directed_by_index(this_node_idx, Direction::Outgoing)
                    .await?
                    .into_iter()
                    .map(|(_, _, target)| target)
                    .collect::<VecDeque<_>>();

                queue.extend(children)
            }

            let base_changeset = change_set
                .base_change_set_id
                .map(|id| id.into_inner())
                .unwrap_or(Ulid::nil());

            if change_set.id == self.default_change_set_id() {
                default_change_set_base = base_changeset
            }

            change_sets
                .entry(base_changeset)
                .or_default()
                .push(WorkspaceExportChangeSetV0 {
                    id: change_set.id.into_inner(),
                    name: change_set.name.clone(),
                    base_change_set_id: change_set.base_change_set_id.map(|id| id.into_inner()),
                    workspace_snapshot_serialized_data: snap.serialized().await?,
                })
        }

        let store_values_map = ctx
            .layer_db()
            .cas()
            .read_many(content_hashes.as_ref())
            .await?
            .into_iter()
            .map(|(hash, content)| (hash, (content, "postcard".to_string())))
            .collect::<HashMap<_, _>>();

        let content_store_values = serialize::to_vec(&store_values_map)?;

        let created_by = if let HistoryActor::User(user_pk) = ctx.history_actor() {
            let user = User::get_by_pk(ctx, *user_pk)
                .await?
                .ok_or(WorkspaceError::InvalidUser(*user_pk))?;

            user.email().clone()
        } else {
            "SystemInit".to_string()
        };

        let metadata = WorkspaceExportMetadataV0 {
            name: self.name().clone(),
            version: workspace_version.to_string(),
            description: "Workspace Backup".to_string(), // TODO Get this from the user
            created_at: Default::default(),
            created_by,
            default_change_set: self.default_change_set_id().into_inner(),
            default_change_set_base,
            workspace_pk: self.pk().into_inner(),
            workspace_name: self.name().clone(),
        };

        Ok(WorkspaceExport::new(WorkspaceExportContentV0 {
            change_sets,
            content_store_values,
            metadata,
        }))
    }

    pub async fn import(
        &mut self,
        ctx: &DalContext,
        workspace_data: WorkspaceExport,
    ) -> WorkspaceResult<()> {
        let WorkspaceExportContentV0 {
            change_sets,
            content_store_values,
            metadata,
        } = workspace_data.into_latest();

        // ABANDON PREVIOUS CHANGESETS
        for mut change_set in ChangeSet::list_open(ctx).await? {
            change_set.abandon(ctx).await?;
        }

        let base_changeset_for_default = {
            let changeset_id = self.default_change_set_id();

            let changeset = ChangeSet::find(ctx, changeset_id)
                .await?
                .ok_or(WorkspaceError::ChangeSetNotFound(changeset_id))?;

            changeset.base_change_set_id
        };

        // Go from head changeset to children, creating new changesets and updating base references
        let mut base_change_set_queue = VecDeque::from([metadata.default_change_set_base]);
        let mut change_set_id_map = HashMap::new();
        while let Some(base_change_set_ulid) = base_change_set_queue.pop_front() {
            let Some(change_sets) = change_sets.get(&base_change_set_ulid) else {
                continue;
            };

            for change_set_data in change_sets {
                let imported_snapshot = WorkspaceSnapshot::from_bytes(
                    &change_set_data.workspace_snapshot_serialized_data,
                )
                .await?;

                // If base_change_set is default_change_set_base, it pointed to the builtin workspace
                // originally, so this change set needs to be the new default for the workspace - HEAD
                let mut is_new_default = false;
                let actual_base_changeset: Option<ChangeSetId> =
                    if base_change_set_ulid == metadata.default_change_set_base {
                        is_new_default = true;
                        base_changeset_for_default
                    } else {
                        Some(*change_set_id_map.get(&base_change_set_ulid).ok_or(
                            WorkspaceError::ImportingOrphanChangeset(base_change_set_ulid.into()),
                        )?)
                    };

                // XXX: fake vector clock here. Figure out the right one
                let vector_clock_id = VectorClockId::new(Ulid::new(), Ulid::new());
                let new_snap_address = imported_snapshot.write(ctx, vector_clock_id).await?;

                let new_change_set = ChangeSet::new(
                    ctx,
                    change_set_data.name.clone(),
                    actual_base_changeset,
                    new_snap_address,
                )
                .await?;

                change_set_id_map.insert(change_set_data.id, new_change_set.id);

                // Set new default changeset for workspace
                if is_new_default {
                    self.update_default_change_set_id(ctx, new_change_set.id)
                        .await?;
                }

                base_change_set_queue.push_back(change_set_data.id)
            }
        }

        let cas_values: HashMap<ContentHash, (Arc<ContentTypes>, String)> =
            serialize::from_bytes(&content_store_values)?;

        let layer_db = ctx.layer_db();

        // TODO use the serialization format to ensure we're hashing the data correctly, if we change the format
        for (_, (content, _serialization_format)) in cas_values {
            layer_db
                .cas()
                .write(content, None, ctx.events_tenancy(), ctx.events_actor())
                .await?;
        }

        Ok(())
    }

    standard_model_accessor_ro!(name, String);

    pub async fn has_change_set(
        ctx: &DalContext,
        change_set_id: ChangeSetId,
    ) -> WorkspaceResult<bool> {
        let row = ctx
            .txns()
            .await?
            .pg()
            .query_one(
                "SELECT count(*) > 0 AS has_change_set FROM change_set_pointers WHERE workspace_id = $1 AND id = $2",
                &[&ctx.tenancy().workspace_pk(), &change_set_id],
            )
            .await?;
        let has_change_set: bool = row.try_get("has_change_set")?;

        Ok(has_change_set)
    }

    /// Mark all workspaces in the database with a given snapshot version. Use
    /// only if you know you have migrated the snapshots for these workspaces to
    /// this version!
    pub async fn set_snapshot_version_for_all_workspaces(
        ctx: &DalContext,
        snapshot_version: WorkspaceSnapshotGraphDiscriminants,
    ) -> WorkspaceResult<()> {
        let version_string = snapshot_version.to_string();

        ctx.txns()
            .await?
            .pg()
            .query(
                "UPDATE workspaces SET snapshot_version = $1",
                &[&version_string],
            )
            .await?;

        Ok(())
    }
}
