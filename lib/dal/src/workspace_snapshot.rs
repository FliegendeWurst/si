//! Mostly everything is a node or an edge!

// #![warn(
//     missing_debug_implementations,
//     missing_docs,
//     unreachable_pub,
//     bad_style,
//     dead_code,
//     improper_ctypes,
//     non_shorthand_field_patterns,
//     no_mangle_generic_items,
//     overflowing_literals,
//     path_statements,
//     patterns_in_fns_without_body,
//     unconditional_recursion,
//     unused,
//     unused_allocation,
//     unused_comparisons,
//     unused_parens,
//     while_true,
//     clippy::missing_panics_doc
// )]

pub mod conflict;
pub mod content_address;
pub mod edge_weight;
pub mod graph;
pub mod lamport_clock;
pub mod node_weight;
pub mod update;
pub mod vector_clock;

use si_layer_cache::persister::PersistStatus;
use std::sync::Arc;
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use petgraph::prelude::*;
use si_data_pg::PgError;
use si_events::ContentHash;
use si_events::WorkspaceSnapshotAddress;
use telemetry::prelude::*;
use thiserror::Error;
use ulid::Ulid;

use crate::change_set_pointer::{ChangeSetId, ChangeSetPointer, ChangeSetPointerError};
use crate::workspace_snapshot::conflict::Conflict;
use crate::workspace_snapshot::edge_weight::{
    EdgeWeight, EdgeWeightError, EdgeWeightKind, EdgeWeightKindDiscriminants,
};
use crate::workspace_snapshot::node_weight::category_node_weight::CategoryNodeKind;
use crate::workspace_snapshot::node_weight::NodeWeight;
use crate::workspace_snapshot::update::Update;
use crate::workspace_snapshot::vector_clock::VectorClockId;
use crate::{
    workspace_snapshot::{graph::WorkspaceSnapshotGraphError, node_weight::NodeWeightError},
    DalContext, TransactionsError, WorkspaceSnapshotGraph,
};

use self::node_weight::{NodeWeightDiscriminants, OrderingNodeWeight};

#[remain::sorted]
#[derive(Error, Debug)]
pub enum WorkspaceSnapshotError {
    #[error("change set pointer error: {0}")]
    ChangeSetPointer(#[from] ChangeSetPointerError),
    #[error("change set pointer {0} has no workspace snapshot address")]
    ChangeSetPointerMissingWorkspaceSnapshotAddress(ChangeSetId),
    #[error("edge weight error: {0}")]
    EdgeWeight(#[from] EdgeWeightError),
    #[error("layer db error: {0}")]
    LayerDb(#[from] si_layer_cache::LayerDbError),
    #[error("missing content from store for id: {0}")]
    MissingContentFromStore(Ulid),
    #[error("monotonic error: {0}")]
    Monotonic(#[from] ulid::MonotonicError),
    #[error("NodeWeight error: {0}")]
    NodeWeight(#[from] NodeWeightError),
    #[error("si_data_pg error: {0}")]
    Pg(#[from] PgError),
    #[error("postcard error: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("serde json error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("transactions error: {0}")]
    Transactions(#[from] TransactionsError),
    #[error("could not acquire lock: {0}")]
    TryLock(#[from] tokio::sync::TryLockError),
    #[error("Unexpected edge source {0} for target {1} and edge weight type {0:?}")]
    UnexpectedEdgeSource(Ulid, Ulid, EdgeWeightKindDiscriminants),
    #[error("Unexpected edge target {0} for source {1} and edge weight type {0:?}")]
    UnexpectedEdgeTarget(Ulid, Ulid, EdgeWeightKindDiscriminants),
    #[error("Unexpected number of incoming edges of type {0:?} for node type {1:?} with id {2}")]
    UnexpectedNumberOfIncomingEdges(EdgeWeightKindDiscriminants, NodeWeightDiscriminants, Ulid),
    #[error("WorkspaceSnapshotGraph error: {0}")]
    WorkspaceSnapshotGraph(#[from] WorkspaceSnapshotGraphError),
    #[error("workspace snapshot graph missing at address: {0}")]
    WorkspaceSnapshotGraphMissing(WorkspaceSnapshotAddress),
    #[error("no workspace snapshot was fetched for this dal context")]
    WorkspaceSnapshotNotFetched,
}

pub type WorkspaceSnapshotResult<T> = Result<T, WorkspaceSnapshotError>;

#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    address: Arc<RwLock<WorkspaceSnapshotAddress>>,
    //    _created_at: Arc<RwLock<DateTime<Utc>>>,
    /// When the snapshot is fetched from the layer cache (hopefully from memory), it comes back
    /// wrapped in an Arc to prevent cloning the graph (which can get quite large). Graph
    /// operations that never modify the graph will use this read-only copy *until* the graph is
    /// modified.
    read_only_graph: Arc<WorkspaceSnapshotGraph>,

    /// Before the graph is modified, the read_only_graph is copied into this RwLock, and all
    /// subsequent graph operations (both read and write) will need to acquire this lock in order
    /// to read or write to the graph. See the SnapshotReadGuard and SnapshotWriteGuard
    /// implemenations of Deref and DerefMut, and their construction in
    /// working_copy()/working_copy_mut()
    working_copy: Arc<RwLock<Option<WorkspaceSnapshotGraph>>>,
}

struct SnapshotReadGuard<'a> {
    read_only_graph: Arc<WorkspaceSnapshotGraph>,
    working_copy_read_guard: RwLockReadGuard<'a, Option<WorkspaceSnapshotGraph>>,
}

struct SnapshotWriteGuard<'a> {
    working_copy_write_guard: RwLockWriteGuard<'a, Option<WorkspaceSnapshotGraph>>,
}

impl<'a> std::ops::Deref for SnapshotReadGuard<'a> {
    type Target = WorkspaceSnapshotGraph;

    fn deref(&self) -> &Self::Target {
        if self.working_copy_read_guard.is_some() {
            let option = &*self.working_copy_read_guard;
            option.as_ref().expect("we confirmed it was some above")
        } else {
            &self.read_only_graph
        }
    }
}

impl<'a> std::ops::Deref for SnapshotWriteGuard<'a> {
    type Target = WorkspaceSnapshotGraph;

    fn deref(&self) -> &Self::Target {
        let option = &*self.working_copy_write_guard;
        option.as_ref().expect(
            "attempted to deref snapshot without copying contents into the mutable working copy",
        )
    }
}

impl<'a> std::ops::DerefMut for SnapshotWriteGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let option = &mut *self.working_copy_write_guard;
        &mut *option.as_mut().expect("attempted to DerefMut a snapshot without copying contents into the mutable working copy")
    }
}

#[allow(dead_code)]
pub(crate) fn serde_value_to_string_type(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Null => "null",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::Object(_) => "object",
        serde_json::Value::String(_) => "string",
    }
    .into()
}

impl WorkspaceSnapshot {
    #[instrument(level = "debug", skip_all)]
    pub async fn initial(
        ctx: &DalContext,
        change_set: &ChangeSetPointer,
    ) -> WorkspaceSnapshotResult<Self> {
        let mut graph: WorkspaceSnapshotGraph = WorkspaceSnapshotGraph::new(change_set)?;

        // Create the category nodes under root.
        let component_node_index =
            graph.add_category_node(change_set, CategoryNodeKind::Component)?;
        let func_node_index = graph.add_category_node(change_set, CategoryNodeKind::Func)?;
        let action_batch_node_index =
            graph.add_category_node(change_set, CategoryNodeKind::ActionBatch)?;
        let schema_node_index = graph.add_category_node(change_set, CategoryNodeKind::Schema)?;
        let secret_node_index = graph.add_category_node(change_set, CategoryNodeKind::Secret)?;

        // Connect them to root.
        graph.add_edge(
            graph.root(),
            EdgeWeight::new(change_set, EdgeWeightKind::Use)?,
            action_batch_node_index,
        )?;
        graph.add_edge(
            graph.root(),
            EdgeWeight::new(change_set, EdgeWeightKind::Use)?,
            component_node_index,
        )?;
        graph.add_edge(
            graph.root(),
            EdgeWeight::new(change_set, EdgeWeightKind::Use)?,
            func_node_index,
        )?;
        graph.add_edge(
            graph.root(),
            EdgeWeight::new(change_set, EdgeWeightKind::Use)?,
            schema_node_index,
        )?;
        graph.add_edge(
            graph.root(),
            EdgeWeight::new(change_set, EdgeWeightKind::Use)?,
            secret_node_index,
        )?;

        // We do not care about any field other than "working_copy" because "write" will populate
        // them using the assigned working copy.
        let initial = Self {
            address: Arc::new(RwLock::new(WorkspaceSnapshotAddress::nil())),
            read_only_graph: Arc::new(graph),
            working_copy: Arc::new(RwLock::new(None)),
        };

        initial.write(ctx, change_set.vector_clock_id()).await?;

        Ok(initial)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn write(
        &self,
        ctx: &DalContext,
        vector_clock_id: VectorClockId,
    ) -> WorkspaceSnapshotResult<WorkspaceSnapshotAddress> {
        // Pull out the working copy and clean it up.
        let new_address = {
            let mut working_copy = self.working_copy_mut().await;
            working_copy.cleanup();

            // Mark everything left as seen.
            working_copy.mark_graph_seen(vector_clock_id)?;

            let (new_address, status_reader) = ctx
                .layer_db()
                .workspace_snapshot()
                .write(
                    Arc::new(working_copy.clone()),
                    None,
                    ctx.events_tenancy(),
                    ctx.events_actor(),
                )
                .await?;

            if let PersistStatus::Error(e) = status_reader.get_status().await? {
                return Err(e)?;
            }

            new_address
        };

        // Note, we continue to use the working copy after this, even for reads, since otherwise
        // we'd have to replace the read_only_graph, which would require another thread-safe
        // interior mutability type to store the read only graph in.

        *self.address.write().await = new_address;

        Ok(new_address)
    }

    pub async fn id(&self) -> WorkspaceSnapshotAddress {
        *self.address.read().await
    }

    pub async fn root(&self) -> WorkspaceSnapshotResult<NodeIndex> {
        Ok(self.working_copy().await.root())
    }

    #[instrument(level = "debug", skip_all)]
    async fn working_copy(&self) -> SnapshotReadGuard<'_> {
        SnapshotReadGuard {
            read_only_graph: self.read_only_graph.clone(),
            working_copy_read_guard: self.working_copy.read().await,
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn working_copy_mut(&self) -> SnapshotWriteGuard<'_> {
        if self.working_copy.read().await.is_none() {
            // Make a copy of the read only graph as our new working copy
            *self.working_copy.write().await = Some(self.read_only_graph.as_ref().clone());
        }

        SnapshotWriteGuard {
            working_copy_write_guard: self.working_copy.write().await,
        }
    }

    pub async fn add_node(&self, node: NodeWeight) -> WorkspaceSnapshotResult<NodeIndex> {
        let new_node_index = self.working_copy_mut().await.add_node(node)?;
        Ok(new_node_index)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn add_ordered_node(
        &self,
        change_set: &ChangeSetPointer,
        node: NodeWeight,
    ) -> WorkspaceSnapshotResult<NodeIndex> {
        let new_node_index = self
            .working_copy_mut()
            .await
            .add_ordered_node(change_set, node)?;
        Ok(new_node_index)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn update_content(
        &self,
        change_set: &ChangeSetPointer,
        id: Ulid,
        new_content_hash: ContentHash,
    ) -> WorkspaceSnapshotResult<()> {
        Ok(self
            .working_copy_mut()
            .await
            .update_content(change_set, id, new_content_hash)?)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn add_edge(
        &self,
        from_node_id: impl Into<Ulid>,
        edge_weight: EdgeWeight,
        to_node_id: impl Into<Ulid>,
    ) -> WorkspaceSnapshotResult<EdgeIndex> {
        let from_node_index = self
            .working_copy()
            .await
            .get_node_index_by_id(from_node_id)?;
        let to_node_index = self.working_copy().await.get_node_index_by_id(to_node_id)?;
        Ok(self
            .working_copy_mut()
            .await
            .add_edge(from_node_index, edge_weight, to_node_index)?)
    }

    // NOTE(nick): this should only be used by the rebaser and in specific scenarios where the
    // indices are definitely correct.
    #[instrument(level = "debug", skip_all)]
    pub async fn add_edge_unchecked(
        &self,
        from_node_index: NodeIndex,
        edge_weight: EdgeWeight,
        to_node_index: NodeIndex,
    ) -> WorkspaceSnapshotResult<EdgeIndex> {
        Ok(self
            .working_copy_mut()
            .await
            .add_edge(from_node_index, edge_weight, to_node_index)?)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn add_ordered_edge(
        &self,
        change_set: &ChangeSetPointer,
        from_node_id: impl Into<Ulid>,
        edge_weight: EdgeWeight,
        to_node_id: impl Into<Ulid>,
    ) -> WorkspaceSnapshotResult<EdgeIndex> {
        let from_node_index = self
            .working_copy()
            .await
            .get_node_index_by_id(from_node_id)?;
        let to_node_index = self.working_copy().await.get_node_index_by_id(to_node_id)?;
        let (edge_index, _) = self.working_copy_mut().await.add_ordered_edge(
            change_set,
            from_node_index,
            edge_weight,
            to_node_index,
        )?;
        Ok(edge_index)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn detect_conflicts_and_updates(
        &self,
        to_rebase_vector_clock_id: VectorClockId,
        onto_workspace_snapshot: &WorkspaceSnapshot,
        onto_vector_clock_id: VectorClockId,
    ) -> WorkspaceSnapshotResult<(Vec<Conflict>, Vec<Update>)> {
        Ok(self.working_copy().await.detect_conflicts_and_updates(
            to_rebase_vector_clock_id,
            &*onto_workspace_snapshot.working_copy().await,
            onto_vector_clock_id,
        )?)
    }

    // NOTE(nick): this should only be used by the rebaser.
    #[instrument(level = "debug", skip_all)]
    pub async fn edge_endpoints(
        &self,
        edge_index: EdgeIndex,
    ) -> WorkspaceSnapshotResult<(NodeIndex, NodeIndex)> {
        Ok(self.working_copy_mut().await.edge_endpoints(edge_index)?)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn import_subgraph(
        &self,
        other: &mut Self,
        root_index: NodeIndex,
    ) -> WorkspaceSnapshotResult<()> {
        Ok(self
            .working_copy_mut()
            .await
            .import_subgraph(&*other.working_copy().await, root_index)?)
    }

    /// Calls [`WorkspaceSnapshotGraph::replace_references()`]
    #[instrument(level = "debug", skip_all)]
    pub async fn replace_references(
        &self,
        original_node_index: NodeIndex,
    ) -> WorkspaceSnapshotResult<()> {
        Ok(self
            .working_copy_mut()
            .await
            .replace_references(original_node_index)?)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn get_node_weight_by_id(
        &self,
        id: impl Into<Ulid>,
    ) -> WorkspaceSnapshotResult<NodeWeight> {
        let node_idx = self.get_node_index_by_id(id).await?;
        Ok(self
            .working_copy()
            .await
            .get_node_weight(node_idx)?
            .to_owned())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn get_node_weight(
        &self,
        node_index: NodeIndex,
    ) -> WorkspaceSnapshotResult<NodeWeight> {
        Ok(self
            .working_copy()
            .await
            .get_node_weight(node_index)?
            .to_owned())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn find_equivalent_node(
        &self,
        id: Ulid,
        lineage_id: Ulid,
    ) -> WorkspaceSnapshotResult<Option<NodeIndex>> {
        Ok(self
            .working_copy()
            .await
            .find_equivalent_node(id, lineage_id)?)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn cleanup(&self) -> WorkspaceSnapshotResult<()> {
        self.working_copy_mut().await.cleanup();
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn nodes(&self) -> WorkspaceSnapshotResult<Vec<(NodeWeight, NodeIndex)>> {
        Ok(self
            .working_copy()
            .await
            .nodes()
            .map(|(weight, index)| (weight.to_owned(), index))
            .collect())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn edges(&self) -> WorkspaceSnapshotResult<Vec<(EdgeWeight, NodeIndex, NodeIndex)>> {
        Ok(self
            .working_copy()
            .await
            .edges()
            .map(|(weight, from, to)| (weight.to_owned(), from, to))
            .collect())
    }

    pub async fn dot(&self) {
        self.working_copy().await.dot();
    }

    pub async fn tiny_dot_to_file(&self, suffix: Option<&str>) {
        self.working_copy().await.tiny_dot_to_file(suffix);
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn get_node_index_by_id(
        &self,
        id: impl Into<Ulid>,
    ) -> WorkspaceSnapshotResult<NodeIndex> {
        Ok(self.working_copy().await.get_node_index_by_id(id)?)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn get_latest_node_index(
        &self,
        node_index: NodeIndex,
    ) -> WorkspaceSnapshotResult<NodeIndex> {
        Ok(self.working_copy().await.get_latest_node_idx(node_index)?)
    }

    #[instrument(skip_all)]
    pub async fn find(
        ctx: &DalContext,
        workspace_snapshot_addr: WorkspaceSnapshotAddress,
    ) -> WorkspaceSnapshotResult<Self> {
        let start = tokio::time::Instant::now();

        let snapshot = ctx
            .layer_db()
            .workspace_snapshot()
            .read(&workspace_snapshot_addr)
            .await?
            .ok_or(WorkspaceSnapshotError::WorkspaceSnapshotGraphMissing(
                workspace_snapshot_addr,
            ))?;

        info!("snapshot fetch took: {:?}", start.elapsed());

        Ok(Self {
            address: Arc::new(RwLock::new(workspace_snapshot_addr)),
            read_only_graph: snapshot,
            working_copy: Arc::new(RwLock::new(None)),
        })
    }

    pub async fn find_for_change_set(
        ctx: &DalContext,
        change_set_pointer_id: ChangeSetId,
    ) -> WorkspaceSnapshotResult<Self> {
        let row = ctx
            .txns()
            .await?
            .pg()
            .query_opt(
                "SELECT workspace_snapshot_address FROM change_set_pointers WHERE id = $1",
                &[&change_set_pointer_id],
            )
            .await?
            .ok_or(
                WorkspaceSnapshotError::ChangeSetPointerMissingWorkspaceSnapshotAddress(
                    change_set_pointer_id,
                ),
            )?;

        let address: WorkspaceSnapshotAddress = row.try_get("workspace_snapshot_address")?;

        Self::find(ctx, address).await
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn get_category_node(
        &self,
        source: Option<Ulid>,
        kind: CategoryNodeKind,
    ) -> WorkspaceSnapshotResult<Ulid> {
        let (category_node_id, _) = self.working_copy().await.get_category_node(source, kind)?;
        Ok(category_node_id)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn edges_directed(
        &self,
        id: impl Into<Ulid>,
        direction: Direction,
    ) -> WorkspaceSnapshotResult<Vec<(EdgeWeight, NodeIndex, NodeIndex)>> {
        let node_index = self.working_copy().await.get_node_index_by_id(id)?;
        Ok(self
            .working_copy()
            .await
            .edges_directed(node_index, direction)
            .map(|edge_ref| {
                (
                    edge_ref.weight().to_owned(),
                    edge_ref.source(),
                    edge_ref.target(),
                )
            })
            .collect())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn edges_directed_for_edge_weight_kind(
        &self,
        id: impl Into<Ulid>,
        direction: Direction,
        edge_kind: EdgeWeightKindDiscriminants,
    ) -> WorkspaceSnapshotResult<Vec<(EdgeWeight, NodeIndex, NodeIndex)>> {
        let node_index = self.working_copy().await.get_node_index_by_id(id)?;

        Ok(self
            .working_copy()
            .await
            .edges_directed_for_edge_weight_kind(node_index, direction, edge_kind))
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn edges_directed_by_index(
        &self,
        node_index: NodeIndex,
        direction: Direction,
    ) -> WorkspaceSnapshotResult<Vec<(EdgeWeight, NodeIndex, NodeIndex)>> {
        Ok(self
            .working_copy()
            .await
            .edges_directed(node_index, direction)
            .map(|edge_ref| {
                (
                    edge_ref.weight().to_owned(),
                    edge_ref.source(),
                    edge_ref.target(),
                )
            })
            .collect())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn remove_all_edges(
        &self,
        change_set: &ChangeSetPointer,
        id: impl Into<Ulid>,
    ) -> WorkspaceSnapshotResult<()> {
        let id = id.into();
        for (edge_weight, source, target) in self.edges_directed(id, Direction::Outgoing).await? {
            self.remove_edge(change_set, source, target, edge_weight.kind().into())
                .await?;
        }
        for (edge_weight, source, target) in self.edges_directed(id, Direction::Incoming).await? {
            self.remove_edge(change_set, source, target, edge_weight.kind().into())
                .await?;
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn incoming_sources_for_edge_weight_kind(
        &self,
        id: impl Into<Ulid>,
        edge_weight_kind_discrim: EdgeWeightKindDiscriminants,
    ) -> WorkspaceSnapshotResult<Vec<NodeIndex>> {
        Ok(self
            .edges_directed(id.into(), Direction::Incoming)
            .await?
            .into_iter()
            .filter_map(|(edge_weight, source_idx, _)| {
                if edge_weight_kind_discrim == edge_weight.kind().into() {
                    Some(source_idx)
                } else {
                    None
                }
            })
            .collect())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn outgoing_targets_for_edge_weight_kind(
        &self,
        id: impl Into<Ulid>,
        edge_weight_kind_discrim: EdgeWeightKindDiscriminants,
    ) -> WorkspaceSnapshotResult<Vec<NodeIndex>> {
        let id = id.into();
        Ok(self
            .edges_directed(id, Direction::Outgoing)
            .await?
            .into_iter()
            .filter_map(|(edge_weight, _, target_idx)| {
                if edge_weight_kind_discrim == edge_weight.kind().into() {
                    Some(target_idx)
                } else {
                    None
                }
            })
            .collect())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn outgoing_targets_for_edge_weight_kind_by_index(
        &self,
        node_index: NodeIndex,
        edge_weight_kind_discrim: EdgeWeightKindDiscriminants,
    ) -> WorkspaceSnapshotResult<Vec<NodeIndex>> {
        Ok(self
            .edges_directed_by_index(node_index, Direction::Outgoing)
            .await?
            .into_iter()
            .filter_map(|(edge_weight, _, target_idx)| {
                if edge_weight_kind_discrim == edge_weight.kind().into() {
                    Some(target_idx)
                } else {
                    None
                }
            })
            .collect())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn all_outgoing_targets(
        &self,
        id: impl Into<Ulid>,
    ) -> WorkspaceSnapshotResult<Vec<NodeWeight>> {
        let mut result = vec![];
        let target_idxs: Vec<NodeIndex> = self
            .edges_directed(id, Direction::Outgoing)
            .await?
            .into_iter()
            .map(|(_, _, target_idx)| target_idx)
            .collect();

        for target_idx in target_idxs {
            let node_weight = self.get_node_weight(target_idx).await?;
            result.push(node_weight.to_owned());
        }

        Ok(result)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn all_incoming_sources(
        &self,
        id: impl Into<Ulid>,
    ) -> WorkspaceSnapshotResult<Vec<NodeWeight>> {
        let mut result = vec![];
        let source_idxs: Vec<NodeIndex> = self
            .edges_directed(id, Direction::Incoming)
            .await?
            .into_iter()
            .map(|(_, source_idx, _)| source_idx)
            .collect();

        for source_idx in source_idxs {
            let node_weight = self.get_node_weight(source_idx).await?;
            result.push(node_weight.to_owned());
        }

        Ok(result)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn remove_incoming_edges_of_kind(
        &self,
        change_set: &ChangeSetPointer,
        target_id: impl Into<Ulid>,
        kind: EdgeWeightKindDiscriminants,
    ) -> WorkspaceSnapshotResult<()> {
        let target_id = target_id.into();

        let sources = self
            .incoming_sources_for_edge_weight_kind(target_id, kind)
            .await?;
        for source_node_idx in sources {
            let target_node_idx = self.get_node_index_by_id(target_id).await?;
            self.remove_edge(change_set, source_node_idx, target_node_idx, kind)
                .await?;
        }

        Ok(())
    }

    pub async fn get_edges_between_nodes(
        &self,
        from_node_id: Ulid,
        to_node_id: Ulid,
    ) -> WorkspaceSnapshotResult<Vec<EdgeWeight>> {
        let from_node_index = self.get_node_index_by_id(from_node_id).await?;

        let to_node_index = self.get_node_index_by_id(to_node_id).await?;
        let edges = self
            .edges()
            .await?
            .into_iter()
            .filter_map(|(edge, node_from, node_to)| {
                if node_from == from_node_index && node_to == to_node_index {
                    Some(edge)
                } else {
                    None
                }
            })
            .collect();
        Ok(edges)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn remove_node_by_id(
        &self,
        change_set: &ChangeSetPointer,
        id: impl Into<Ulid>,
    ) -> WorkspaceSnapshotResult<()> {
        let id: Ulid = id.into();
        let node_idx = self.get_node_index_by_id(id).await?;
        self.remove_all_edges(change_set, id).await?;
        self.working_copy_mut().await.remove_node(node_idx);
        self.working_copy_mut().await.remove_node_id(id);

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn remove_edge(
        &self,
        change_set: &ChangeSetPointer,
        source_node_index: NodeIndex,
        target_node_index: NodeIndex,
        edge_kind: EdgeWeightKindDiscriminants,
    ) -> WorkspaceSnapshotResult<()> {
        Ok(self.working_copy_mut().await.remove_edge(
            change_set,
            source_node_index,
            target_node_index,
            edge_kind,
        )?)
    }

    /// Perform [`Updates`](Update) using [`self`](WorkspaceSnapshot) as the "to rebase" graph and
    /// another [`snapshot`](WorkspaceSnapshot) as the "onto" graph.
    #[instrument(level = "debug", skip_all)]
    pub async fn perform_updates(
        &self,
        to_rebase_change_set: &ChangeSetPointer,
        onto: &WorkspaceSnapshot,
        updates: &[Update],
    ) -> WorkspaceSnapshotResult<()> {
        Ok(self.working_copy_mut().await.perform_updates(
            to_rebase_change_set,
            &*onto.working_copy().await,
            updates,
        )?)
    }

    /// Mark whether a prop can be used as an input to a function. Props below
    /// Maps and Arrays are not valid inputs. Must only be used when
    /// "finalizing" a schema variant!
    #[instrument(level = "debug", skip_all)]
    pub async fn mark_prop_as_able_to_be_used_as_prototype_arg(
        &self,
        node_index: NodeIndex,
    ) -> WorkspaceSnapshotResult<()> {
        self.working_copy_mut()
            .await
            .update_node_weight(node_index, |node_weight| match node_weight {
                NodeWeight::Prop(prop_inner) => {
                    prop_inner.set_can_be_used_as_prototype_arg(true);
                    Ok(())
                }
                _ => Err(WorkspaceSnapshotGraphError::IncompatibleNodeTypes)?,
            })?;

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn ordering_node_for_container(
        &self,
        id: impl Into<Ulid>,
    ) -> WorkspaceSnapshotResult<Option<OrderingNodeWeight>> {
        let idx = self.get_node_index_by_id(id).await?;
        Ok(self.working_copy().await.ordering_node_for_container(idx)?)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn ordered_children_for_node(
        &self,
        id: impl Into<Ulid>,
    ) -> WorkspaceSnapshotResult<Option<Vec<Ulid>>> {
        let idx = self.get_node_index_by_id(id.into()).await?;
        let mut result = vec![];
        Ok(
            if let Some(idxs) = self.working_copy().await.ordered_children_for_node(idx)? {
                for idx in idxs {
                    let id = self.get_node_weight(idx).await?.id();
                    result.push(id);
                }
                Some(result)
            } else {
                None
            },
        )
    }
}
