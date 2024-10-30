use std::collections::HashMap;

use axum::extract::Path;
use axum::{
    extract::{Host, OriginalUri},
    Json,
};
use serde::{Deserialize, Serialize};

use dal::diagram::view::ViewId;
use dal::{
    cached_module::CachedModule,
    change_status::ChangeStatus,
    component::frame::Frame,
    generate_name,
    pkg::{import_pkg_from_pkg, ImportOptions},
    ChangeSet, ChangeSetId, Component, ComponentId, Schema, SchemaId, SchemaVariant,
    SchemaVariantId, WorkspacePk, WsEvent,
};
use si_frontend_types::SchemaVariant as FrontendVariant;

use crate::{
    extract::{AccessBuilder, HandlerContext, PosthogClient},
    service::force_change_set_response::ForceChangeSetResponse,
    track,
};

use super::{ViewError, ViewResult};

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum CreateComponentSchemaType {
    Installed,
    Uninstalled,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CreateComponentRequest {
    pub schema_type: CreateComponentSchemaType,
    pub schema_variant_id: Option<SchemaVariantId>,
    pub schema_id: Option<SchemaId>,
    pub parent_id: Option<ComponentId>,
    pub x: String,
    pub y: String,
    pub height: Option<String>,
    pub width: Option<String>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CreateComponentResponse {
    pub component_id: ComponentId,
    pub installed_variant: Option<FrontendVariant>,
}

pub async fn create_component(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(access_builder): AccessBuilder,
    PosthogClient(posthog_client): PosthogClient,
    OriginalUri(original_uri): OriginalUri,
    Host(host_name): Host,
    Path((_workspace_pk, change_set_id, view_id)): Path<(WorkspacePk, ChangeSetId, ViewId)>,
    Json(request): Json<CreateComponentRequest>,
) -> ViewResult<ForceChangeSetResponse<CreateComponentResponse>> {
    let mut ctx = builder
        .build(access_builder.build(change_set_id.into()))
        .await?;

    let force_change_set_id = ChangeSet::force_new(&mut ctx).await?;

    let name = generate_name();

    let (schema_variant_id, installed_variant) = match request.schema_type {
        CreateComponentSchemaType::Installed => (
            request.schema_variant_id.ok_or(ViewError::InvalidRequest(
                "schemaVariantId missing on installed schema create component request".into(),
            ))?,
            None,
        ),
        // Install assets on demand when creating a component
        CreateComponentSchemaType::Uninstalled => {
            let schema_id = request.schema_id.ok_or(ViewError::InvalidRequest(
                "schemaId missing on uninstalled schema create component request".into(),
            ))?;

            let variant_id = match Schema::get_by_id(&ctx, schema_id).await? {
                // We want to be sure that we don't have stale frontend data,
                // since this module might have just been installed, or
                // installed by another user
                Some(schema) => schema.get_default_schema_variant_id_or_error(&ctx).await?,
                None => {
                    let mut uninstalled_module = CachedModule::latest_by_schema_id(&ctx, schema_id)
                        .await?
                        .ok_or(ViewError::UninstalledSchemaNotFound(schema_id))?;

                    let si_pkg = uninstalled_module.si_pkg(&ctx).await?;
                    import_pkg_from_pkg(
                        &ctx,
                        &si_pkg,
                        Some(ImportOptions {
                            schema_id: Some(schema_id.into()),
                            ..Default::default()
                        }),
                    )
                    .await?;

                    Schema::get_default_schema_variant_by_id(&ctx, schema_id)
                        .await?
                        .ok_or(ViewError::SchemaNotInstalledAfterImport(schema_id))?
                }
            };

            let variant = SchemaVariant::get_by_id_or_error(&ctx, variant_id).await?;

            (
                variant_id,
                Some(variant.into_frontend_type(&ctx, schema_id).await?),
            )
        }
    };

    let variant = SchemaVariant::get_by_id_or_error(&ctx, schema_variant_id).await?;
    let mut component = Component::new(&ctx, &name, variant.id(), view_id).await?;
    let initial_geometry = component.geometry(&ctx, view_id).await?;

    let geometry = component
        .set_geometry(
            &ctx,
            view_id,
            request.x.clone(),
            request.y.clone(),
            request
                .width
                .or_else(|| initial_geometry.width().map(ToString::to_string)),
            request
                .height
                .or_else(|| initial_geometry.height().map(ToString::to_string)),
        )
        .await?;

    if let Some(frame_id) = request.parent_id {
        Frame::upsert_parent(&ctx, component.id(), frame_id).await?;

        track(
            &posthog_client,
            &ctx,
            &original_uri,
            &host_name,
            "component_attached_to_frame",
            serde_json::json!({
                "how": "/diagram/create_component",
                "component_id": component.id(),
                "parent_id": frame_id.clone(),
                "change_set_id": ctx.change_set_id(),
                "installed_on_demand": matches!(request.schema_type, CreateComponentSchemaType::Uninstalled),
            }),
        );
    } else {
        track(
            &posthog_client,
            &ctx,
            &original_uri,
            &host_name,
            "component_created",
            serde_json::json!({
                "how": "/diagram/create_component",
                "component_id": component.id(),
                "component_name": name.clone(),
                "change_set_id": ctx.change_set_id(),
                "installed_on_demand": matches!(request.schema_type, CreateComponentSchemaType::Uninstalled),
            }),
        );
    }

    let mut diagram_sockets = HashMap::new();
    let payload = component
        .into_frontend_type(
            &ctx,
            Some(&geometry),
            ChangeStatus::Added,
            &mut diagram_sockets,
        )
        .await?;
    WsEvent::component_created(&ctx, payload)
        .await?
        .publish_on_commit(&ctx)
        .await?;

    ctx.commit().await?;

    Ok(ForceChangeSetResponse::new(
        force_change_set_id,
        CreateComponentResponse {
            component_id: component.id(),
            installed_variant,
        },
    ))
}
