use serde::{Deserialize, Serialize};
use si_data::{NatsError, NatsTxn, PgError, PgTxn};
use std::default::Default;
use telemetry::prelude::*;
use thiserror::Error;

use crate::{
    func::{binding::FuncBindingId, FuncId},
    impl_standard_model, pk, standard_model, standard_model_accessor, ComponentId, HistoryActor,
    HistoryEventError, ReadTenancy, ResourcePrototypeId, SchemaId, SchemaVariantId, StandardModel,
    StandardModelError, SystemId, Tenancy, Timestamp, Visibility, WriteTenancy,
};

#[derive(Error, Debug)]
pub enum ResourceResolverError {
    #[error("error serializing/deserializing json: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("pg error: {0}")]
    Pg(#[from] PgError),
    #[error("nats txn error: {0}")]
    Nats(#[from] NatsError),
    #[error("history event error: {0}")]
    HistoryEvent(#[from] HistoryEventError),
    #[error("standard model error: {0}")]
    StandardModelError(#[from] StandardModelError),
}

pub type ResourceResolverResult<T> = Result<T, ResourceResolverError>;

pub const UNSET_ID_VALUE: i64 = -1;
const GET_FOR_PROTOTYPE: &str = include_str!("./queries/resource_resolver_get_for_prototype.sql");

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ResourceResolverContext {
    component_id: ComponentId,
    schema_id: SchemaId,
    schema_variant_id: SchemaVariantId,
    system_id: SystemId,
}

// Hrm - is this a universal resolver context? -- Adam
impl Default for ResourceResolverContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceResolverContext {
    pub fn new() -> Self {
        ResourceResolverContext {
            component_id: UNSET_ID_VALUE.into(),
            schema_id: UNSET_ID_VALUE.into(),
            schema_variant_id: UNSET_ID_VALUE.into(),
            system_id: UNSET_ID_VALUE.into(),
        }
    }

    pub fn component_id(&self) -> ComponentId {
        self.component_id
    }

    pub fn set_component_id(&mut self, component_id: ComponentId) {
        self.component_id = component_id;
    }

    pub fn schema_id(&self) -> SchemaId {
        self.schema_id
    }

    pub fn set_schema_id(&mut self, schema_id: SchemaId) {
        self.schema_id = schema_id;
    }

    pub fn schema_variant_id(&self) -> SchemaVariantId {
        self.schema_variant_id
    }

    pub fn set_schema_variant_id(&mut self, schema_variant_id: SchemaVariantId) {
        self.schema_variant_id = schema_variant_id;
    }

    pub fn system_id(&self) -> SystemId {
        self.system_id
    }

    pub fn set_system_id(&mut self, system_id: SystemId) {
        self.system_id = system_id;
    }
}

pk!(ResourceResolverPk);
pk!(ResourceResolverId);

// An ResourceResolver joins a `FuncBinding` to the context in which
// its corresponding `FuncBindingResultValue` is consumed.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ResourceResolver {
    pk: ResourceResolverPk,
    id: ResourceResolverId,
    resource_prototype_id: ResourcePrototypeId,
    func_id: FuncId,
    func_binding_id: FuncBindingId,
    #[serde(flatten)]
    context: ResourceResolverContext,
    #[serde(flatten)]
    tenancy: Tenancy,
    #[serde(flatten)]
    timestamp: Timestamp,
    #[serde(flatten)]
    visibility: Visibility,
}

impl_standard_model! {
    model: ResourceResolver,
    pk: ResourceResolverPk,
    id: ResourceResolverId,
    table_name: "resource_resolvers",
    history_event_label_base: "resource_resolver",
    history_event_message_name: "Resource Resolver"
}

impl ResourceResolver {
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip_all)]
    pub async fn new(
        txn: &PgTxn<'_>,
        nats: &NatsTxn,
        write_tenancy: &WriteTenancy,
        visibility: &Visibility,
        history_actor: &HistoryActor,
        resource_prototype_id: ResourcePrototypeId,
        func_id: FuncId,
        func_binding_id: FuncBindingId,
        context: ResourceResolverContext,
    ) -> ResourceResolverResult<Self> {
        let row = txn
            .query_one(
                "SELECT object FROM resource_resolver_create_v1($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                &[
                    write_tenancy,
                    &visibility,
                    &resource_prototype_id,
                    &func_id,
                    &func_binding_id,
                    &context.component_id(),
                    &context.schema_id(),
                    &context.schema_variant_id(),
                    &context.system_id(),
                ],
            )
            .await?;
        let object = standard_model::finish_create_from_row(
            txn,
            nats,
            &write_tenancy.into(),
            visibility,
            history_actor,
            row,
        )
        .await?;
        Ok(object)
    }

    standard_model_accessor!(
        resource_prototype_id,
        Pk(ResourcePrototypeId),
        ResourceResolverResult
    );
    standard_model_accessor!(func_id, Pk(FuncId), ResourceResolverResult);
    standard_model_accessor!(func_binding_id, Pk(FuncBindingId), ResourceResolverResult);

    pub async fn get_for_prototype_and_component(
        txn: &PgTxn<'_>,
        read_tenancy: &ReadTenancy,
        visibility: &Visibility,
        resource_prototype_id: &ResourcePrototypeId,
        component_id: &ComponentId,
    ) -> ResourceResolverResult<Option<Self>> {
        let row = txn
            .query_opt(
                GET_FOR_PROTOTYPE,
                &[
                    read_tenancy,
                    &visibility,
                    resource_prototype_id,
                    component_id,
                ],
            )
            .await?;
        let object = standard_model::option_object_from_row(row)?;
        Ok(object)
    }
}

#[cfg(test)]
mod test {
    use super::ResourceResolverContext;

    #[test]
    fn context_builder() {
        let mut c = ResourceResolverContext::new();
        c.set_component_id(15.into());
        assert_eq!(c.component_id(), 15.into());
    }
}
