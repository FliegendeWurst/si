use axum::{
    extract::{Host, OriginalUri},
    response::IntoResponse,
    Json,
};
use dal::{
    attribute::prototype::argument::AttributePrototypeArgumentId, change_status::ChangeStatus,
    diagram::SummaryDiagramEdge, ChangeSet, Component, ComponentId, InputSocketId, OutputSocketId,
    User, Visibility, WsEvent,
};
use serde::{Deserialize, Serialize};

use super::{DiagramError, DiagramResult};
use crate::{
    extract::{AccessBuilder, HandlerContext, PosthogClient},
    track,
};

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CreateConnectionRequest {
    pub from_component_id: ComponentId,
    pub from_socket_id: OutputSocketId,
    pub to_component_id: ComponentId,
    pub to_socket_id: InputSocketId,
    #[serde(flatten)]
    pub visibility: Visibility,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CreateConnectionResponse {
    pub id: AttributePrototypeArgumentId,
    pub created_by: Option<User>,
    pub deleted_by: Option<User>,
}

pub async fn create_connection(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(request_ctx): AccessBuilder,
    PosthogClient(posthog_client): PosthogClient,
    OriginalUri(original_uri): OriginalUri,
    Host(host_name): Host,
    Json(request): Json<CreateConnectionRequest>,
) -> DiagramResult<impl IntoResponse> {
    let mut ctx = builder.build(request_ctx.build(request.visibility)).await?;

    let force_change_set_id = ChangeSet::force_new(&mut ctx).await?;

    let attribute_prototype_argument_id = Component::connect(
        &ctx,
        request.from_component_id,
        request.from_socket_id,
        request.to_component_id,
        request.to_socket_id,
    )
    .await?
    .ok_or(DiagramError::DuplicatedConnection)?;

    track(
        &posthog_client,
        &ctx,
        &original_uri,
        &host_name,
        "create_connection",
        serde_json::json!({
            "how": "/diagram/create_connection",
            "from_component_id": request.from_component_id,
            "from_socket_id": request.from_socket_id,
            "to_component_id": request.to_component_id,
            "to_socket_id": request.to_socket_id,
            "change_set_id": ctx.change_set_id(),
        }),
    );

    let from_component = Component::get_by_id(&ctx, request.from_component_id).await?;
    let to_component = Component::get_by_id(&ctx, request.to_component_id).await?;
    for incoming_connection in to_component.incoming_connections(&ctx).await? {
        if incoming_connection.to_input_socket_id == request.to_socket_id
            && incoming_connection.from_component_id == from_component.id()
            && incoming_connection.to_component_id == to_component.id()
        {
            let edge = SummaryDiagramEdge::assemble(
                incoming_connection,
                &from_component,
                &to_component,
                ChangeStatus::Added,
            )?;
            WsEvent::connection_upserted(&ctx, edge)
                .await?
                .publish_on_commit(&ctx)
                .await?;
        }
    }

    ctx.commit().await?;

    let mut response = axum::response::Response::builder();
    if let Some(force_change_set_id) = force_change_set_id {
        response = response.header("force_change_set_id", force_change_set_id.to_string());
    }
    Ok(response
        .header("content-type", "application/json")
        .body(serde_json::to_string(&CreateConnectionResponse {
            id: attribute_prototype_argument_id,
            // TODO(nick): figure out what to do with these fields that were left over from the "Connection" struct.
            created_by: None,
            deleted_by: None,
        })?)?)
}
