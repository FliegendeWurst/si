use super::ViewResult;
use crate::{
    extract::{AccessBuilder, HandlerContext},
    service::force_change_set_response::ForceChangeSetResponse,
};
use axum::extract::Path;
use axum::Json;
use dal::diagram::view::ViewId;
use dal::{ChangeSet, ChangeSetId, Component, ComponentId, WorkspacePk, WsEvent};
use serde::{Deserialize, Serialize};
use si_frontend_types::{RawGeometry, StringGeometry};
use std::collections::HashMap;
use ulid::Ulid;

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetComponentPositionRequest {
    pub data_by_component_id: HashMap<ComponentId, StringGeometry>,
    pub client_ulid: Ulid,
    pub request_ulid: Ulid,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetComponentPositionResponse {
    pub request_ulid: Ulid,
}

pub async fn set_component_geometry(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(access_builder): AccessBuilder,
    Path((_workspace_pk, change_set_id, view_id)): Path<(WorkspacePk, ChangeSetId, ViewId)>,
    Json(request): Json<SetComponentPositionRequest>,
) -> ViewResult<ForceChangeSetResponse<SetComponentPositionResponse>> {
    let mut ctx = builder
        .build(access_builder.build(change_set_id.into()))
        .await?;

    let force_change_set_id = ChangeSet::force_new(&mut ctx).await?;

    let mut geometry_list = vec![];
    for (id, string_geometry) in request.data_by_component_id {
        let new_geometry: RawGeometry = string_geometry.try_into()?;

        let mut component = Component::get_by_id(&ctx, id).await?;

        let current_geometry = component.geometry(&ctx, view_id).await?;

        let new_geometry_cache = new_geometry.clone();

        let (width, height) = (
            new_geometry.width.or_else(|| current_geometry.width()),
            new_geometry.height.or_else(|| current_geometry.height()),
        );

        component
            .set_geometry(
                &ctx,
                view_id,
                new_geometry_cache.x,
                new_geometry_cache.y,
                width,
                height,
            )
            .await?;

        geometry_list.push((
            id,
            RawGeometry {
                x: new_geometry.x,
                y: new_geometry.y,
                width,
                height,
            },
        ))
    }

    WsEvent::set_component_position(
        &ctx,
        ctx.change_set_id(),
        view_id,
        geometry_list,
        Some(request.client_ulid),
    )
    .await?
    .publish_on_commit(&ctx)
    .await?;

    ctx.commit().await?;

    Ok(ForceChangeSetResponse::new(
        force_change_set_id,
        SetComponentPositionResponse {
            request_ulid: request.request_ulid,
        },
    ))
}
