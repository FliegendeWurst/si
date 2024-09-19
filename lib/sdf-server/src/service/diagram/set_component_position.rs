use std::collections::HashMap;

use axum::{response::IntoResponse, Json};
use dal::{
    change_status::ChangeStatus,
    component::{frame::Frame, ComponentGeometry},
    diagram::SummaryDiagramInferredEdge,
    ChangeSet, Component, ComponentId, ComponentType, Visibility, WsEvent,
};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use super::DiagramResult;
use crate::extract::{AccessBuilder, HandlerContext};

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SingleComponentGeometryUpdate {
    pub geometry: ComponentGeometry,
    pub detach: bool,
    pub new_parent: Option<ComponentId>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetComponentPositionRequest {
    #[serde(flatten)]
    pub visibility: Visibility,
    pub data_by_component_id: HashMap<ComponentId, SingleComponentGeometryUpdate>,
    pub client_ulid: Ulid,
    pub request_ulid: Ulid,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetComponentPositionResponse {
    pub request_ulid: Ulid,
}

pub async fn set_component_position(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(request_ctx): AccessBuilder,
    Json(request): Json<SetComponentPositionRequest>,
) -> DiagramResult<impl IntoResponse> {
    let mut ctx = builder.build(request_ctx.build(request.visibility)).await?;

    let force_change_set_id = ChangeSet::force_new(&mut ctx).await?;

    let mut components: Vec<Component> = vec![];
    let mut diagram_inferred_edges: Vec<SummaryDiagramInferredEdge> = vec![];

    let mut socket_map = HashMap::new();
    for (id, update) in request.data_by_component_id {
        let mut component = Component::get_by_id(&ctx, id).await?;

        if update.detach {
            Frame::orphan_child(&ctx, component.id()).await?;
            let payload = component
                .into_frontend_type(&ctx, ChangeStatus::Unmodified, &mut socket_map)
                .await?;
            WsEvent::component_updated(&ctx, payload)
                .await?
                .publish_on_commit(&ctx)
                .await?;
        } else if let Some(new_parent) = update.new_parent {
            Frame::upsert_parent(&ctx, component.id(), new_parent).await?;
            let payload = component
                .into_frontend_type(&ctx, ChangeStatus::Unmodified, &mut socket_map)
                .await?;
            WsEvent::component_updated(&ctx, payload)
                .await?
                .publish_on_commit(&ctx)
                .await?;

            // Queue new implicit edges to send to frontend
            {
                let component = Component::get_by_id(&ctx, new_parent).await?;
                for inferred_incoming_connection in
                    component.inferred_incoming_connections(&ctx).await?
                {
                    diagram_inferred_edges.push(SummaryDiagramInferredEdge::assemble(
                        inferred_incoming_connection,
                    )?)
                }
                for inferred_outgoing_connection in
                    component.inferred_outgoing_connections(&ctx).await?
                {
                    diagram_inferred_edges.push(SummaryDiagramInferredEdge::assemble(
                        inferred_outgoing_connection,
                    )?)
                }
            }
        }

        let (width, height) = {
            let mut size = (None, None);

            let component_type = component.get_type(&ctx).await?;

            if component_type != ComponentType::Component {
                size = (
                    update
                        .geometry
                        .width
                        .or_else(|| component.width().map(|v| v.to_string())),
                    update
                        .geometry
                        .height
                        .or_else(|| component.height().map(|v| v.to_string())),
                );
            }

            size
        };

        component
            .set_geometry(&ctx, update.geometry.x, update.geometry.y, width, height)
            .await?;
        components.push(component);
    }

    WsEvent::set_component_position(
        &ctx,
        ctx.change_set_id(),
        components,
        Some(request.client_ulid),
    )
    .await?
    .publish_on_commit(&ctx)
    .await?;

    if !diagram_inferred_edges.is_empty() {
        WsEvent::upsert_inferred_edges(&ctx, diagram_inferred_edges)
            .await?
            .publish_on_commit(&ctx)
            .await?;
    }

    ctx.commit().await?;

    let mut response = axum::response::Response::builder();
    if let Some(force_change_set_id) = force_change_set_id {
        response = response.header("force_change_set_id", force_change_set_id.to_string());
    }

    Ok(
        response.body(serde_json::to_string(&SetComponentPositionResponse {
            request_ulid: request.request_ulid,
        })?)?,
    )
}
