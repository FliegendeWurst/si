use itertools::Itertools;
use std::collections::{HashMap, HashSet, VecDeque};
use telemetry::prelude::*;

use crate::{
    action::{Action, ActionId},
    component::inferred_connection_graph::InferredConnectionGraph,
    dependency_graph::DependencyGraph,
    Component, ComponentId, DalContext,
};

use super::{
    prototype::{ActionKind, ActionPrototype},
    ActionError, ActionResult,
};

#[derive(Debug, Clone)]
pub struct ActionDependencyGraph {
    inner: DependencyGraph<ActionId>,
}

impl Default for ActionDependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionDependencyGraph {
    pub fn new() -> Self {
        Self {
            inner: DependencyGraph::new(),
        }
    }

    pub fn is_acyclic(&self) -> bool {
        petgraph::algo::toposort(self.inner.graph(), None).is_ok()
    }

    /// Construct an [`ActionDependencyGraph`] of all of the queued [`Action`s][crate::action::Action]
    /// for the current [`WorkspaceSnapshot`][crate::WorkspaceSnapshot].
    #[instrument(
        level = "info",
        name = "action.dependency_graph.for_workspace",
        skip(ctx)
    )]
    pub async fn for_workspace(ctx: &DalContext) -> ActionResult<Self> {
        // * Get all ActionId -> ComponentId mappings.
        // * For each of these ComponentIds (A):
        //     * For each Input Socket:
        //         * For each source ComponentId (B):
        //           * All Actions for Component A depend on All actions for Component B
        let mut component_dependencies: HashMap<ComponentId, HashSet<ComponentId>> = HashMap::new();
        let mut component_reverse_dependencies: HashMap<ComponentId, HashSet<ComponentId>> =
            HashMap::new();
        let mut actions_by_component_id: HashMap<ComponentId, HashSet<ActionId>> = HashMap::new();
        let mut action_dependency_graph = Self::new();
        let mut action_kinds: HashMap<ActionId, ActionKind> = HashMap::new();

        // Need to get all actions that are still in the "queue", including those that have failed,
        // or are currently running.
        for action_id in Action::all_ids(ctx).await? {
            action_dependency_graph.inner.add_id(action_id);
            // Theoretically, we may have Actions at some point that aren't Component specific.
            if let Some(component_id) = Action::component_id(ctx, action_id).await? {
                actions_by_component_id
                    .entry(component_id)
                    .or_default()
                    .insert(action_id);
            }
            let action_prototype_id = Action::prototype_id(ctx, action_id).await?;
            let action_prototype = ActionPrototype::get_by_id(ctx, action_prototype_id).await?;
            action_kinds.insert(action_id, action_prototype.kind);
        }

        // TODO: Account for explicitly defiend dependencies between actions. These should be edges
        //       directly between two Actions, but are not implemented yet.

        // Get all inferred connections up front so we don't build this tree each time
        let components_to_find = actions_by_component_id.keys().copied().collect_vec();
        let component_tree =
            InferredConnectionGraph::assemble_for_components(ctx, components_to_find, None).await?;
        // Action dependencies are primarily based on the data flow between Components. Things that
        // feed data into other things must have their actions run before the actions for the
        // things they are feeding data into.
        for component_id in actions_by_component_id.keys().copied() {
            let component = Component::get_by_id(ctx, component_id).await?;
            for incoming_connection in component.incoming_connections(ctx).await? {
                component_dependencies
                    .entry(component_id)
                    .or_default()
                    .insert(incoming_connection.from_component_id);
            }
            for inferred_connection in
                component_tree.get_inferred_incoming_connections_to_component(component_id)
            {
                if inferred_connection.input_socket.component_id != component_id {
                    continue;
                }

                component_dependencies
                    .entry(component_id)
                    .or_default()
                    .insert(inferred_connection.output_socket.component_id);
            }

            // Destroy Actions follow the flow of data backwards, so we need the reverse dependency
            // graph between the components.
            for outgoing_connection in component.outgoing_connections(ctx).await? {
                component_reverse_dependencies
                    .entry(component_id)
                    .or_default()
                    .insert(outgoing_connection.to_component_id);
            }
            for inferred_outgoing_connection in
                component_tree.get_inferred_outgoing_connections_for_component(component_id)
            {
                if inferred_outgoing_connection.output_socket.component_id != component_id {
                    continue;
                }

                component_reverse_dependencies
                    .entry(component_id)
                    .or_default()
                    .insert(inferred_outgoing_connection.input_socket.component_id);
            }
        }

        // Each Component's Actions need to be marked as depending on the Actions that the
        // Component itself has been determined to be depending on.
        for (component_id, dependencies) in component_dependencies {
            if let Some(component_action_ids) = actions_by_component_id.get(&component_id) {
                for component_action_id in component_action_ids {
                    let action_kind = action_kinds
                        .get(component_action_id)
                        .copied()
                        .ok_or(ActionError::UnableToGetKind(*component_action_id))?;
                    if action_kind == ActionKind::Destroy {
                        continue;
                    }

                    for dependency_component_id in &dependencies {
                        for dependency_action_id in actions_by_component_id
                            .get(dependency_component_id)
                            .cloned()
                            .unwrap_or_default()
                        {
                            action_dependency_graph
                                .action_depends_on(*component_action_id, dependency_action_id);
                        }
                    }
                }
            }
        }

        // We get to do it all over again, but this time using the reverse dependency graph for the
        // Destroy Actions.
        for (component_id, reverse_dependencies) in component_reverse_dependencies {
            if let Some(component_action_ids) = actions_by_component_id.get(&component_id) {
                for component_action_id in component_action_ids {
                    let action_kind = action_kinds
                        .get(component_action_id)
                        .copied()
                        .ok_or(ActionError::UnableToGetKind(*component_action_id))?;
                    if action_kind != ActionKind::Destroy {
                        continue;
                    }

                    for dependency_compoonent_id in &reverse_dependencies {
                        for dependency_action_id in actions_by_component_id
                            .get(dependency_compoonent_id)
                            .cloned()
                            .unwrap_or_default()
                        {
                            action_dependency_graph
                                .action_depends_on(*component_action_id, dependency_action_id);
                        }
                    }
                }
            }
        }

        Ok(action_dependency_graph)
    }

    pub fn action_depends_on(&mut self, action_id: ActionId, depends_on_id: ActionId) {
        self.inner.id_depends_on(action_id, depends_on_id);
    }

    pub fn contains_value(&self, action_id: ActionId) -> bool {
        self.inner.contains_id(action_id)
    }
    /// gets what actions are directly dependent on a given action id
    /// ex: Create -> Update -> Delete
    /// graph.direct_dependencies_of(update.actionid) -> Create
    pub fn direct_dependencies_of(&self, action_id: ActionId) -> Vec<ActionId> {
        self.inner.direct_dependencies_of(action_id)
    }

    pub fn remove_action(&mut self, action_id: ActionId) {
        self.inner.remove_id(action_id);
    }

    pub fn cycle_on_self(&mut self, action_id: ActionId) {
        self.inner.cycle_on_self(action_id);
    }

    pub fn independent_actions(&self) -> Vec<ActionId> {
        self.inner.independent_ids()
    }

    pub fn remaining_actions(&self) -> Vec<ActionId> {
        self.inner.remaining_ids()
    }

    /// Gets all downstream dependencies for the provided ActionId. This includes the entire subgraph
    /// starting at ActionId.
    #[instrument(level = "info", skip(self))]
    pub fn get_all_dependencies(&self, action_id: ActionId) -> Vec<ActionId> {
        let current_dependencies = self.inner.direct_reverse_dependencies_of(action_id);
        let mut all_dependencies = HashSet::new();
        let mut work_queue = VecDeque::from(current_dependencies.clone());
        while let Some(action) = work_queue.pop_front() {
            match all_dependencies.insert(action) {
                true => {
                    let next = self.inner.direct_reverse_dependencies_of(action);
                    work_queue.extend(next);
                }
                false => continue,
            }
        }
        all_dependencies.into_iter().collect_vec()
    }
}
