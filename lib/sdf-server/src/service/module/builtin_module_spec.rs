use axum::{
    extract::{Host, OriginalUri},
    Json,
};
use dal::{HistoryActor, User, Visibility};
use module_index_client::ModuleIndexClient;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::{
    extract::{AccessBuilder, HandlerContext, PosthogClient, RawAccessToken},
    service::module::{ModuleError, ModuleResult},
    track,
};

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PromoteToBuiltinModuleRequest {
    pub id: Ulid,
    #[serde(flatten)]
    pub visibility: Visibility,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PromoteToBuiltinModuleResponse {
    pub success: bool,
}

pub async fn promote_to_builtin(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(request_ctx): AccessBuilder,
    RawAccessToken(raw_access_token): RawAccessToken,
    PosthogClient(posthog_client): PosthogClient,
    OriginalUri(original_uri): OriginalUri,
    Host(host_name): Host,
    Json(request): Json<PromoteToBuiltinModuleRequest>,
) -> ModuleResult<Json<PromoteToBuiltinModuleResponse>> {
    let ctx = builder.build(request_ctx.build(request.visibility)).await?;

    let module_index_url = match ctx.module_index_url() {
        Some(url) => url,
        None => return Err(ModuleError::ModuleIndexNotConfigured),
    };

    let user = match ctx.history_actor() {
        HistoryActor::User(user_pk) => User::get_by_pk(&ctx, *user_pk).await?,
        _ => None,
    };

    let (_, created_by_email) = user
        .map(|user| (user.name().to_owned(), user.email().to_owned()))
        .unwrap_or((
            "unauthenticated user name".into(),
            "unauthenticated user email".into(),
        ));

    let module_id = request.id;

    let module_index_client =
        ModuleIndexClient::new(module_index_url.try_into()?, &raw_access_token);

    module_index_client
        .promote_to_builtin(module_id, created_by_email.clone())
        .await?;

    track(
        &posthog_client,
        &ctx,
        &original_uri,
        &host_name,
        "promote_to_builtin",
        serde_json::json!({
                    "pkg_id": module_id,
                    "pkg_promoted_to_builtin_by": created_by_email,
        }),
    );

    ctx.commit().await?;

    Ok(Json(PromoteToBuiltinModuleResponse { success: true }))
}
