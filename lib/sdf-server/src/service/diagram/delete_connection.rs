use axum::{
    extract::{Host, OriginalUri},
    response::IntoResponse,
    Json,
};
use dal::{
    change_status::ChangeStatus, diagram::SummaryDiagramEdge, ChangeSet, Component, ComponentId,
    InputSocket, InputSocketId, OutputSocket, OutputSocketId, Visibility, WsEvent,
};
use serde::{Deserialize, Serialize};

use super::DiagramResult;
use crate::{
    extract::{AccessBuilder, HandlerContext, PosthogClient},
    track,
};

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]

pub struct DeleteConnectionRequest {
    pub from_socket_id: OutputSocketId,
    pub from_component_id: ComponentId,
    pub to_component_id: ComponentId,
    pub to_socket_id: InputSocketId,
    #[serde(flatten)]
    pub visibility: Visibility,
}

/// Delete a [`Connection`](dal::Connection) via its EdgeId. Creating change-set if on head.
pub async fn delete_connection(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(request_ctx): AccessBuilder,
    PosthogClient(posthog_client): PosthogClient,
    OriginalUri(original_uri): OriginalUri,
    Host(host_name): Host,
    Json(request): Json<DeleteConnectionRequest>,
) -> DiagramResult<impl IntoResponse> {
    let mut ctx = builder.build(request_ctx.build(request.visibility)).await?;

    let force_change_set_id = ChangeSet::force_new(&mut ctx).await?;
    Component::remove_connection(
        &ctx,
        request.from_component_id,
        request.from_socket_id,
        request.to_component_id,
        request.to_socket_id,
    )
    .await?;

    let from_component = Component::get_by_id(&ctx, request.from_component_id).await?;

    let to_component = Component::get_by_id(&ctx, request.to_component_id).await?;

    let output_socket = OutputSocket::get_by_id(&ctx, request.from_socket_id).await?;
    let input_socket = InputSocket::get_by_id(&ctx, request.to_socket_id).await?;

    let base_change_set_ctx = ctx.clone_with_base().await?;

    let base_from_component =
        Component::try_get_by_id(&base_change_set_ctx, request.from_component_id).await?;
    let base_to_component =
        Component::try_get_by_id(&base_change_set_ctx, request.to_component_id).await?;

    let mut payload: Option<SummaryDiagramEdge> = None;
    if let Some((base_from, base_to)) = base_from_component.zip(base_to_component) {
        let incoming_edges = base_to
            .incoming_connections(&base_change_set_ctx)
            .await
            .ok();
        if let Some(edges) = incoming_edges {
            for incoming in edges {
                if incoming.from_output_socket_id == request.from_socket_id
                    && incoming.from_component_id == base_from.id()
                    && incoming.to_input_socket_id == request.to_socket_id
                {
                    payload = Some(SummaryDiagramEdge::assemble(
                        incoming,
                        &from_component,
                        &to_component,
                        ChangeStatus::Deleted,
                    )?);
                }
            }
        }
    }

    if let Some(edge) = payload {
        WsEvent::connection_upserted(&ctx, edge)
            .await?
            .publish_on_commit(&ctx)
            .await?;
    } else {
        WsEvent::connection_deleted(
            &ctx,
            request.from_component_id,
            request.to_component_id,
            request.from_socket_id,
            request.to_socket_id,
        )
        .await?
        .publish_on_commit(&ctx)
        .await?;
    }

    let from_component_schema =
        Component::schema_for_component_id(&ctx, request.from_component_id).await?;

    let to_component_schema =
        Component::schema_for_component_id(&ctx, request.to_component_id).await?;
    track(
        &posthog_client,
        &ctx,
        &original_uri,
        &host_name,
        "delete_connection",
        serde_json::json!({
            "how": "/diagram/delete_connection",
            "from_component_id": request.from_component_id,
            "from_component_schema_name": from_component_schema.name(),
            "from_socket_id": request.from_socket_id,
            "from_socket_name": &output_socket.name(),
            "to_component_id": request.to_component_id,
            "to_component_schema_name": to_component_schema.name(),
            "to_socket_id": request.to_socket_id,
            "to_socket_name":  &input_socket.name(),
            "change_set_id": ctx.change_set_id(),
        }),
    );

    ctx.commit().await?;

    let mut response = axum::response::Response::builder();
    if let Some(force_change_set_id) = force_change_set_id {
        response = response.header("force_change_set_id", force_change_set_id.to_string());
    }
    Ok(response.body(axum::body::Empty::new())?)
}
