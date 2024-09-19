use axum::{
    extract::{Host, OriginalUri},
    Json,
};
use dal::{change_set::ChangeSet, ChangeSetId};
use serde::{Deserialize, Serialize};

use super::ChangeSetResult;
use crate::{
    extract::{AccessBuilder, HandlerContext, PosthogClient},
    service::change_set::ChangeSetError,
    track,
};

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AbandonChangeSetRequest {
    pub change_set_id: ChangeSetId,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AbandonChangeSetResponse {
    pub change_set: ChangeSet,
}

pub async fn abandon_change_set(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(access_builder): AccessBuilder,
    PosthogClient(posthog_client): PosthogClient,
    OriginalUri(original_uri): OriginalUri,
    Host(host_name): Host,
    Json(request): Json<AbandonChangeSetRequest>,
) -> ChangeSetResult<()> {
    let mut ctx = builder.build_head(access_builder).await?;
    let maybe_head_changeset = ctx.get_workspace_default_change_set_id().await?;
    if maybe_head_changeset == request.change_set_id {
        return Err(ChangeSetError::CannotAbandonHead);
    }
    let mut change_set = ChangeSet::find(&ctx, request.change_set_id)
        .await?
        .ok_or(ChangeSetError::ChangeSetNotFound)?;

    ctx.update_visibility_and_snapshot_to_visibility(change_set.id)
        .await?;
    change_set.abandon(&ctx).await?;

    track(
        &posthog_client,
        &ctx,
        &original_uri,
        &host_name,
        "abandon_change_set",
        serde_json::json!({
            "abandoned_change_set": request.change_set_id,
        }),
    );

    ctx.commit_no_rebase().await?;

    Ok(())
}
