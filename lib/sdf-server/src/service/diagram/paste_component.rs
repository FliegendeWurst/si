use std::collections::HashMap;

use super::{DiagramError, DiagramResult};
use crate::{
    extract::{AccessBuilder, HandlerContext, PosthogClient},
    service::force_change_set_response::ForceChangeSetResponse,
    track,
};
use axum::{
    extract::{Host, OriginalUri},
    http::uri::Uri,
    Json,
};
use dal::diagram::geometry::RawGeometry;
use dal::{
    change_status::ChangeStatus, component::frame::Frame, diagram::SummaryDiagramEdge, ChangeSet,
    Component, ComponentId, DalContext, Visibility, WsEvent,
};
use serde::{Deserialize, Serialize};

#[allow(clippy::too_many_arguments)]
async fn paste_single_component(
    ctx: &DalContext,
    component_id: ComponentId,
    component_geometry: RawGeometry,
    original_uri: &Uri,
    host_name: &String,
    PosthogClient(posthog_client): &PosthogClient,
) -> DiagramResult<Component> {
    let original_comp = Component::get_by_id(ctx, component_id).await?;
    let pasted_comp = original_comp.copy_paste(ctx, component_geometry).await?;

    let schema = pasted_comp.schema(ctx).await?;
    track(
        posthog_client,
        ctx,
        original_uri,
        host_name,
        "paste_component",
        serde_json::json!({
            "how": "/diagram/paste_component",
            "component_id": pasted_comp.id(),
            "component_schema_name": schema.name(),
        }),
    );

    Ok(pasted_comp)
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PasteSingleComponentPayload {
    id: ComponentId,
    component_geometry: RawGeometry,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PasteComponentsRequest {
    pub components: Vec<PasteSingleComponentPayload>,
    pub new_parent_node_id: Option<ComponentId>,
    #[serde(flatten)]
    pub visibility: Visibility,
}

/// Paste a set of [`Component`](dal::Component)s via their componentId. Creates change-set if on head
pub async fn paste_components(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(request_ctx): AccessBuilder,
    PosthogClient(posthog_client): PosthogClient,
    OriginalUri(original_uri): OriginalUri,
    Host(host_name): Host,
    Json(request): Json<PasteComponentsRequest>,
) -> DiagramResult<ForceChangeSetResponse<()>> {
    let mut ctx = builder.build(request_ctx.build(request.visibility)).await?;

    let force_change_set_id = ChangeSet::force_new(&mut ctx).await?;

    let mut pasted_components_by_original = HashMap::new();
    for component_payload in &request.components {
        let component_id = component_payload.id;

        let posthog_client = PosthogClient(posthog_client.clone());
        let pasted_comp = paste_single_component(
            &ctx,
            component_id,
            component_payload.component_geometry.clone(),
            &original_uri,
            &host_name,
            &posthog_client,
        )
        .await?;

        pasted_components_by_original.insert(component_id, pasted_comp);
    }

    for component_payload in &request.components {
        let component_id = component_payload.id;

        let pasted_component =
            if let Some(component) = pasted_components_by_original.get(&component_id) {
                component
            } else {
                return Err(DiagramError::Paste);
            };
        let component = Component::get_by_id(&ctx, component_id).await?;

        // If component parent was also pasted on this batch, keep relationship between new components
        if let Some(parent_id) = component.parent(&ctx).await? {
            if let Some(pasted_parent) = pasted_components_by_original.get(&parent_id) {
                Frame::upsert_parent(&ctx, pasted_component.id(), pasted_parent.id()).await?;
            };
        }

        // If the pasted component didn't get a parent already, set the new parent
        if pasted_component.parent(&ctx).await?.is_none() {
            if let Some(parent_id) = request.new_parent_node_id {
                Frame::upsert_parent(&ctx, pasted_component.id(), parent_id).await?;
            }
        }

        // re-fetch component with possible parentage
        let pasted_component = Component::get_by_id(&ctx, pasted_component.id()).await?;
        let mut diagram_sockets = HashMap::new();
        let payload = pasted_component
            .into_frontend_type(
                &ctx,
                dal::change_status::ChangeStatus::Added,
                &mut diagram_sockets,
            )
            .await?;
        WsEvent::component_created(&ctx, payload)
            .await?
            .publish_on_commit(&ctx)
            .await?;

        // Create on pasted components copies of edges that existed between original components
        for connection in component.incoming_connections(&ctx).await? {
            if let Some(from_component) =
                pasted_components_by_original.get(&connection.from_component_id)
            {
                Component::connect(
                    &ctx,
                    from_component.id(),
                    connection.from_output_socket_id,
                    pasted_component.id(),
                    connection.to_input_socket_id,
                )
                .await?;

                let edge = SummaryDiagramEdge {
                    from_component_id: from_component.id(),
                    from_socket_id: connection.from_output_socket_id,
                    to_component_id: pasted_component.id(),
                    to_socket_id: connection.to_input_socket_id,
                    change_status: ChangeStatus::Added,
                    created_info: serde_json::to_value(connection.created_info)?,
                    deleted_info: serde_json::to_value(connection.deleted_info)?,
                    to_delete: false,
                    from_base_change_set: false,
                };
                WsEvent::connection_upserted(&ctx, edge)
                    .await?
                    .publish_on_commit(&ctx)
                    .await?;
            }
        }
    }

    ctx.commit().await?;

    Ok(ForceChangeSetResponse::empty(force_change_set_id))
}
