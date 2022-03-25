use super::ChangeSetResult;
use crate::server::extract::{AccessBuilder, HandlerContext};
use crate::server::service::change_set::ChangeSetError;
use axum::Json;
use dal::{ChangeSet, ChangeSetPk};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ApplyChangeSetRequest {
    pub change_set_pk: ChangeSetPk,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ApplyChangeSetResponse {
    pub change_set: ChangeSet,
}

pub async fn apply_change_set(
    HandlerContext(builder, mut txns): HandlerContext,
    AccessBuilder(request_ctx): AccessBuilder,
    Json(request): Json<ApplyChangeSetRequest>,
) -> ChangeSetResult<Json<ApplyChangeSetResponse>> {
    let txns = txns.start().await?;
    let ctx = builder.build(request_ctx.build_head(), &txns);

    let mut change_set =
        ChangeSet::get_by_pk(ctx.pg_txn(), ctx.read_tenancy(), &request.change_set_pk)
            .await?
            .ok_or(ChangeSetError::ChangeSetNotFound)?;
    change_set
        .apply(ctx.pg_txn(), ctx.nats_txn(), ctx.history_actor())
        .await?;

    txns.commit().await?;

    Ok(Json(ApplyChangeSetResponse { change_set }))
}
