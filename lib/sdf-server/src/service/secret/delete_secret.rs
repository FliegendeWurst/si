use axum::{response::IntoResponse, Json};
use dal::{ChangeSet, Secret, SecretId, SecretView, Visibility, WsEvent};
use serde::{Deserialize, Serialize};

use super::{SecretError, SecretResult};
use crate::extract::{AccessBuilder, HandlerContext};

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSecretRequest {
    pub id: SecretId,
    #[serde(flatten)]
    pub visibility: Visibility,
}

pub type UpdateSecretResponse = SecretView;

pub async fn delete_secret(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(request_tx): AccessBuilder,
    Json(request): Json<DeleteSecretRequest>,
) -> SecretResult<impl IntoResponse> {
    let mut ctx = builder.build(request_tx.build(request.visibility)).await?;
    let force_change_set_id = ChangeSet::force_new(&mut ctx).await?;

    // Delete Secret
    let secret = Secret::get_by_id_or_error(&ctx, request.id).await?;

    let connected_components = secret.clone().find_connected_components(&ctx).await?;
    if !connected_components.is_empty() {
        return Err(SecretError::CantDeleteSecret(request.id));
    }

    secret.delete(&ctx).await?;

    WsEvent::secret_deleted(&ctx, request.id)
        .await?
        .publish_on_commit(&ctx)
        .await?;

    ctx.commit().await?;

    let mut response = axum::response::Response::builder();
    if let Some(force_change_set_id) = force_change_set_id {
        response = response.header("force_change_set_id", force_change_set_id.to_string());
    }
    Ok(response.body(axum::body::Empty::new())?)
}
