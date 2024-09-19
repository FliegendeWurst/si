use axum::{response::IntoResponse, Json};
use dal::{
    key_pair::KeyPairPk, ChangeSet, Secret, SecretAlgorithm, SecretId, SecretVersion, SecretView,
    Visibility, WsEvent,
};
use serde::{Deserialize, Serialize};

use super::SecretResult;
use crate::extract::{AccessBuilder, HandlerContext};

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NewSecretData {
    pub crypted: Vec<u8>,
    pub key_pair_pk: KeyPairPk,
    pub version: SecretVersion,
    pub algorithm: SecretAlgorithm,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSecretRequest {
    pub id: SecretId,
    pub name: String,
    pub description: Option<String>,
    pub new_secret_data: Option<NewSecretData>,
    #[serde(flatten)]
    pub visibility: Visibility,
}

pub type UpdateSecretResponse = SecretView;

pub async fn update_secret(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(request_tx): AccessBuilder,
    Json(request): Json<UpdateSecretRequest>,
) -> SecretResult<impl IntoResponse> {
    let mut ctx = builder.build(request_tx.build(request.visibility)).await?;

    let force_change_set_id = ChangeSet::force_new(&mut ctx).await?;

    // Update secret metadata.
    let mut secret = Secret::get_by_id_or_error(&ctx, request.id).await?;
    secret = secret
        .update_metadata(&ctx, request.name, request.description)
        .await?;

    // Update encrypted contents.
    if let Some(new_data) = request.new_secret_data {
        secret = secret
            .update_encrypted_contents(
                &ctx,
                new_data.crypted.as_slice(),
                new_data.key_pair_pk,
                new_data.version,
                new_data.algorithm,
            )
            .await?;
    }

    WsEvent::secret_updated(&ctx, secret.id())
        .await?
        .publish_on_commit(&ctx)
        .await?;

    ctx.commit().await?;

    let mut response = axum::response::Response::builder();
    if let Some(force_change_set_id) = force_change_set_id {
        response = response.header("force_change_set_id", force_change_set_id.to_string());
    }

    Ok(response.body(serde_json::to_string(
        &SecretView::from_secret(&ctx, secret).await?,
    )?)?)
}
