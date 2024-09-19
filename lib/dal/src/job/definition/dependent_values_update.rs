use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    convert::TryFrom,
    sync::Arc,
};
use telemetry_utils::metric;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use si_events::FuncRunValue;
use telemetry::prelude::*;
use thiserror::Error;
use tokio::{
    sync::RwLock,
    task::{JoinError, JoinSet},
};
use ulid::Ulid;

use crate::{
    attribute::value::{dependent_value_graph::DependentValueGraph, AttributeValueError},
    component::inferred_connection_graph::InferredConnectionGraph,
    job::{
        consumer::{
            JobCompletionState, JobConsumer, JobConsumerError, JobConsumerMetadata,
            JobConsumerResult, JobInfo,
        },
        producer::{JobProducer, JobProducerResult},
    },
    prop::PropError,
    status::{StatusMessageState, StatusUpdate, StatusUpdateError},
    workspace_snapshot::DependentValueRoot,
    AccessBuilder, AttributeValue, AttributeValueId, ComponentError, ComponentId, DalContext, Func,
    TransactionsError, Visibility, WorkspacePk, WorkspaceSnapshotError, WsEvent, WsEventError,
};

#[remain::sorted]
#[derive(Debug, Error)]
pub enum DependentValueUpdateError {
    #[error("attribute value error: {0}")]
    AttributeValue(#[from] AttributeValueError),
    #[error("component error: {0}")]
    Component(#[from] ComponentError),
    #[error("prop error: {0}")]
    Prop(#[from] PropError),
    #[error("status update error: {0}")]
    StatusUpdate(#[from] StatusUpdateError),
    #[error(transparent)]
    TokioTask(#[from] JoinError),
    #[error(transparent)]
    Transactions(#[from] TransactionsError),
    #[error("workspace snapshot error: {0}")]
    WorkspaceSnapshot(#[from] WorkspaceSnapshotError),
    #[error("ws event error: {0}")]
    WsEvent(#[from] WsEventError),
}

pub type DependentValueUpdateResult<T> = Result<T, DependentValueUpdateError>;

#[derive(Debug, Deserialize, Serialize)]
struct DependentValuesUpdateArgs;

impl From<DependentValuesUpdate> for DependentValuesUpdateArgs {
    fn from(_value: DependentValuesUpdate) -> Self {
        Self
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DependentValuesUpdate {
    access_builder: AccessBuilder,
    visibility: Visibility,
    job: Option<JobInfo>,
    #[serde(skip)]
    set_value_lock: Arc<RwLock<()>>,
}

impl DependentValuesUpdate {
    pub fn new(access_builder: AccessBuilder, visibility: Visibility) -> Box<Self> {
        Box::new(Self {
            access_builder,
            visibility,
            job: None,
            set_value_lock: Arc::new(RwLock::new(())),
        })
    }
}

impl JobProducer for DependentValuesUpdate {
    fn arg(&self) -> JobProducerResult<serde_json::Value> {
        Ok(serde_json::to_value(DependentValuesUpdateArgs::from(
            self.clone(),
        ))?)
    }
}

impl JobConsumerMetadata for DependentValuesUpdate {
    fn type_name(&self) -> String {
        "DependentValuesUpdate".to_string()
    }

    fn access_builder(&self) -> AccessBuilder {
        self.access_builder
    }

    fn visibility(&self) -> Visibility {
        self.visibility
    }
}

#[async_trait]
impl JobConsumer for DependentValuesUpdate {
    #[instrument(
        level="info",
        name = "dependent_values_update.run",
        skip_all,
        fields(
                si.change_set.id = Empty,
                si.workspace.id = Empty,
            ),
        )]
    async fn run(&self, ctx: &mut DalContext) -> JobConsumerResult<JobCompletionState> {
        let span = Span::current();
        span.record("si.change_set.id", ctx.change_set_id().to_string());
        span.record(
            "si.workspace.id",
            ctx.tenancy()
                .workspace_pk_opt()
                .unwrap_or(WorkspacePk::NONE)
                .to_string(),
        );
        Ok(self.inner_run(ctx).await?)
    }
}

struct StatusUpdateTracker {
    values_by_component: HashMap<ComponentId, HashSet<AttributeValueId>>,
    components_by_value: HashMap<AttributeValueId, ComponentId>,
    active_components: HashSet<ComponentId>,
}

impl StatusUpdateTracker {
    async fn new_for_values(
        ctx: &DalContext,
        value_ids: Vec<AttributeValueId>,
    ) -> DependentValueUpdateResult<Self> {
        let mut tracker = Self {
            values_by_component: HashMap::new(),
            components_by_value: HashMap::new(),
            active_components: HashSet::new(),
        };

        for value_id in value_ids {
            let component_id = AttributeValue::component_id(ctx, value_id).await?;
            tracker
                .values_by_component
                .entry(component_id)
                .and_modify(|values: &mut HashSet<AttributeValueId>| {
                    values.insert(value_id);
                })
                .or_default();
            tracker.components_by_value.insert(value_id, component_id);
        }

        Ok(tracker)
    }

    fn active_components_count(&self) -> usize {
        self.active_components.len()
    }

    fn would_start_component(&self, value_id: AttributeValueId) -> bool {
        self.components_by_value
            .get(&value_id)
            .is_some_and(|component_id| !self.active_components.contains(component_id))
    }

    fn start_value(&mut self, value_id: AttributeValueId) -> Option<ComponentId> {
        self.components_by_value
            .get(&value_id)
            .and_then(|component_id| {
                self.active_components
                    .insert(*component_id)
                    .then_some(*component_id)
            })
    }

    fn finish_value(&mut self, value_id: AttributeValueId) -> Option<ComponentId> {
        self.components_by_value
            .get(&value_id)
            .and_then(
                |component_id| match self.values_by_component.entry(*component_id) {
                    Entry::Occupied(mut values_entry) => {
                        let values = values_entry.get_mut();
                        values.remove(&value_id);
                        values.is_empty().then_some(*component_id)
                    }
                    Entry::Vacant(_) => None,
                },
            )
    }

    fn finish_remaining(&self) -> Vec<StatusUpdate> {
        self.values_by_component
            .iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(component_id, _)| {
                StatusUpdate::new_dvu(StatusMessageState::StatusFinished, *component_id)
            })
            .collect()
    }

    fn get_status_update(
        &mut self,
        state: StatusMessageState,
        value_id: AttributeValueId,
    ) -> Option<StatusUpdate> {
        match state {
            StatusMessageState::StatusFinished => self.finish_value(value_id),
            StatusMessageState::StatusStarted => self.start_value(value_id),
        }
        .map(|component_id| StatusUpdate::new_dvu(state, component_id))
    }
}

impl DependentValuesUpdate {
    async fn inner_run(
        &self,
        ctx: &mut DalContext,
    ) -> DependentValueUpdateResult<JobCompletionState> {
        let start = tokio::time::Instant::now();
        let span = Span::current();
        metric!(counter.dvu_concurrency_count = 1);
        let roots = ctx.workspace_snapshot()?.take_dependent_values().await?;

        // Calculate the inferred connection graph up front so we reuse it throughout the job and don't rebuild each time
        let inferred_connection_graph = InferredConnectionGraph::for_workspace(ctx).await?;
        ctx.workspace_snapshot()?
            .set_cached_inferred_connection_graph(Some(inferred_connection_graph))
            .await;

        let concurrency_limit = ctx.get_workspace().await?.component_concurrency_limit() as usize;

        let mut dependency_graph = DependentValueGraph::new(ctx, roots).await?;

        debug!(
            "DependentValueGraph calculation took: {:?}",
            start.elapsed()
        );

        // Remove the first set of independent_values since they should already have had their functions executed
        for value in dependency_graph.independent_values() {
            if !dependency_graph.values_needs_to_execute_from_prototype_function(value) {
                dependency_graph.remove_value(value);
            }
        }
        let all_value_ids = dependency_graph.all_value_ids();
        metric!(counter.dvu.values_to_run = all_value_ids.len());

        let mut tracker = StatusUpdateTracker::new_for_values(ctx, all_value_ids).await?;

        let mut spawned_ids = HashSet::new();
        let mut task_id_to_av_id = HashMap::new();
        let mut update_join_set = JoinSet::new();
        let mut independent_value_ids: HashSet<AttributeValueId> =
            dependency_graph.independent_values().into_iter().collect();
        let mut would_start_ids = HashSet::new();

        loop {
            if independent_value_ids.is_empty() && task_id_to_av_id.is_empty() {
                break;
            }

            if independent_value_ids
                .difference(&would_start_ids)
                .next()
                .is_none()
            {
                if task_id_to_av_id.is_empty() {
                    break;
                }
            } else {
                for attribute_value_id in &independent_value_ids {
                    let attribute_value_id = *attribute_value_id;
                    let parent_span = span.clone();
                    if !spawned_ids.contains(&attribute_value_id)
                        && !would_start_ids.contains(&attribute_value_id)
                    {
                        let id = Ulid::new();

                        if tracker.would_start_component(attribute_value_id)
                            && tracker.active_components_count() >= concurrency_limit
                        {
                            would_start_ids.insert(attribute_value_id);
                            continue;
                        }

                        let status_update = tracker.get_status_update(
                            StatusMessageState::StatusStarted,
                            attribute_value_id,
                        );

                        update_join_set.spawn(
                                values_from_prototype_function_execution(
                                    id,
                                    ctx.clone(),
                                    attribute_value_id,
                                    self.set_value_lock.clone(),
                                    status_update,
                                )
                                .instrument(info_span!(parent: parent_span, "dependent_values_update.values_from_prototype_function_execution",
                                    attribute_value.id = %attribute_value_id,
                                )),
                            );
                        task_id_to_av_id.insert(id, attribute_value_id);
                        spawned_ids.insert(attribute_value_id);
                    }
                }
            }

            // Wait for a task to finish
            if let Some(join_result) = update_join_set.join_next().await {
                let (task_id, execution_result) = join_result?;
                metric!(counter.dvu.values_to_run = -1);

                metric!(counter.dvu.function_execution = -1);
                if let Some(finished_value_id) = task_id_to_av_id.remove(&task_id) {
                    match execution_result {
                        Ok((execution_values, func)) => {
                            // Lock the graph for writing inside this job. The
                            // lock will be released when this guard is dropped
                            // at the end of the scope.
                            let write_guard = self.set_value_lock.write().await;

                            // Only set values if their functions are actually
                            // "dependent". Other values may have been
                            // introduced to the attribute value graph because
                            // of child-parent prop dependencies, but these
                            // values themselves do not need to change (they are
                            // always Objects, Maps, or Arrays set by
                            // setObject/setArray/setMap and are not updated in
                            // the dependent value execution). If we forced
                            // these container values to update here, we might
                            // touch child properties unnecessarily.
                            match AttributeValue::is_set_by_dependent_function(
                                ctx,
                                finished_value_id,
                            )
                            .await
                            {
                                Ok(true) => match AttributeValue::set_values_from_func_run_value(
                                    ctx,
                                    finished_value_id,
                                    execution_values,
                                    func,
                                )
                                .await
                                {
                                    Ok(_) => {
                                        // Remove the value, so that any values that depend on it will
                                        // become independent values (once all other dependencies are removed)
                                        dependency_graph.remove_value(finished_value_id);
                                        drop(write_guard);
                                    }
                                    Err(err) => {
                                        execution_error(ctx, err.to_string(), finished_value_id)
                                            .await;
                                        dependency_graph.cycle_on_self(finished_value_id);
                                    }
                                },
                                Ok(false) => {
                                    dependency_graph.remove_value(finished_value_id);
                                }
                                Err(err) => {
                                    execution_error(ctx, err.to_string(), finished_value_id).await;
                                    dependency_graph.cycle_on_self(finished_value_id);
                                }
                            }
                        }
                        Err(err) => {
                            // By adding an outgoing edge from the failed node to itself it will
                            // never appear in the `independent_values` call above since that looks for
                            // nodes *without* outgoing edges. Thus we will never try to re-execute
                            // the function for this value, nor will we execute anything in the
                            // dependency graph connected to this value
                            let read_guard = self.set_value_lock.read().await;
                            execution_error(ctx, err.to_string(), finished_value_id).await;
                            drop(read_guard);
                            dependency_graph.cycle_on_self(finished_value_id);
                        }
                    }

                    if let Some(status_update) = tracker
                        .get_status_update(StatusMessageState::StatusFinished, finished_value_id)
                    {
                        if let Err(err) = send_status_update(ctx, status_update).await {
                            error!(si.error.message = ?err, "status update finished event send failed for AttributeValue {finished_value_id}");
                        }
                    }
                }
            }

            independent_value_ids = dependency_graph.independent_values().into_iter().collect();
        }

        let snap = ctx.workspace_snapshot()?;
        for value_id in &independent_value_ids {
            if spawned_ids.contains(value_id) {
                snap.add_dependent_value_root(DependentValueRoot::Finished(value_id.into()))
                    .await?;
            } else {
                snap.add_dependent_value_root(DependentValueRoot::Unfinished(value_id.into()))
                    .await?;
            }
        }

        // If we enouncter a failure when executing the values above, we may
        // not process the downstream attributes and thus will fail to send the
        // "finish" update. So we send the "finish" update here to ensure the
        // frontend can continue to work on the snapshot.
        if independent_value_ids.is_empty() {
            for status_update in tracker.finish_remaining() {
                if let Err(err) = send_status_update(ctx, status_update).await {
                    error!(si.error.message = ?err, "status update finished event send for leftover component failed");
                }
            }
        }

        debug!("DependentValuesUpdate took: {:?}", start.elapsed());

        ctx.commit().await?;
        metric!(counter.dvu_concurrency_count = -1);
        Ok(JobCompletionState::Done)
    }
}

async fn execution_error(
    ctx: &DalContext,
    err_string: String,
    attribute_value_id: AttributeValueId,
) {
    let fallback = format!(
        "error executing prototype function for AttributeValue {attribute_value_id}: {err_string}"
    );
    let error_message = if let Ok(detail) = execution_error_detail(ctx, attribute_value_id).await {
        format!("{detail}: {err_string}")
    } else {
        fallback
    };

    error!(si.error.message = error_message, %attribute_value_id);
}

async fn execution_error_detail(
    ctx: &DalContext,
    id: AttributeValueId,
) -> DependentValueUpdateResult<String> {
    let is_for = AttributeValue::is_for(ctx, id)
        .await?
        .debug_info(ctx)
        .await?;
    let prototype_func = AttributeValue::prototype_func(ctx, id).await?.name;

    Ok(format!(
        "error executing prototype function \"{prototype_func}\" to set the value of {is_for} ({id})"
    ))
}

/// Wrapper around `AttributeValue.values_from_prototype_function_execution(&ctx)` to get it to
/// play more nicely with being spawned into a `JoinSet`.
async fn values_from_prototype_function_execution(
    task_id: Ulid,
    ctx: DalContext,
    attribute_value_id: AttributeValueId,
    set_value_lock: Arc<RwLock<()>>,
    status_update: Option<StatusUpdate>,
) -> (Ulid, DependentValueUpdateResult<(FuncRunValue, Func)>) {
    metric!(counter.dvu.function_execution = 1);

    if let Some(status_update) = status_update {
        if let Err(err) = send_status_update(&ctx, status_update).await {
            return (task_id, Err(err));
        }
    }

    let parent_span = Span::current();
    let result =
        AttributeValue::execute_prototype_function(&ctx, attribute_value_id, set_value_lock)
            .instrument(info_span!(parent:parent_span, "value.execute_prototype_function", attribute_value.id= %attribute_value_id))
            .await
            .map_err(Into::into);

    (task_id, result)
}

async fn send_status_update(
    ctx: &DalContext,
    status_update: StatusUpdate,
) -> DependentValueUpdateResult<()> {
    WsEvent::status_update(ctx, status_update)
        .await?
        .publish_immediately(ctx)
        .await?;

    Ok(())
}

impl TryFrom<JobInfo> for DependentValuesUpdate {
    type Error = JobConsumerError;

    fn try_from(job: JobInfo) -> Result<Self, Self::Error> {
        Ok(Self {
            access_builder: job.access_builder,
            visibility: job.visibility,
            job: Some(job),
            set_value_lock: Arc::new(RwLock::new(())),
        })
    }
}
