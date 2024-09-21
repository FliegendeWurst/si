use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dal::{
    action::{prototype::ActionPrototypeError, ActionError},
    ActionPrototypeId, ChangeSetApplyError as DalChangeSetApplyError,
    ChangeSetError as DalChangeSetError, ChangeSetId, ComponentError, FuncError, SchemaError,
    SchemaVariantError, StandardModelError, TransactionsError, WorkspaceError,
    WorkspaceSnapshotError, WsEventError,
};

use telemetry::prelude::*;
use thiserror::Error;

use crate::AppState;

pub mod abandon_change_set;
mod abandon_vote;
pub mod add_action;
pub mod apply_change_set;
mod begin_abandon_approval_process;
mod begin_approval_process;
pub mod create_change_set;
pub mod list_open_change_sets;
mod merge_vote;
mod rebase_on_base;
mod status_with_base;

#[remain::sorted]
#[derive(Debug, Error)]
pub enum ChangeSetError {
    #[error("action error: {0}")]
    Action(#[from] ActionError),
    #[error("action already enqueued: {0}")]
    ActionAlreadyEnqueued(ActionPrototypeId),
    #[error("action prototype error: {0}")]
    ActionPrototype(#[from] ActionPrototypeError),
    #[error("cannot abandon head change set")]
    CannotAbandonHead,
    #[error("change set not found")]
    ChangeSetNotFound,
    #[error("component error: {0}")]
    Component(#[from] ComponentError),
    #[error("dal change set error: {0}")]
    DalChangeSet(#[from] DalChangeSetError),
    #[error("dal change set apply error: {0}")]
    DalChangeSetApply(#[from] DalChangeSetApplyError),
    #[error("dvu roots are not empty for change set: {0}")]
    DvuRootsNotEmpty(ChangeSetId),
    #[error("func error: {0}")]
    Func(#[from] FuncError),
    #[error("invalid header name {0}")]
    Hyper(#[from] hyper::http::Error),
    #[error("schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error("schema variant error: {0}")]
    SchemaVariant(#[from] SchemaVariantError),
    #[error("standard model error: {0}")]
    StandardModel(#[from] StandardModelError),
    #[error("transactions error: {0}")]
    Transactions(#[from] TransactionsError),
    #[error("workspace error: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("workspace snapshot error: {0}")]
    WorkspaceSnapshot(#[from] WorkspaceSnapshotError),
    #[error("ws event error: {0}")]
    WsEvent(#[from] WsEventError),
}

pub type ChangeSetResult<T> = std::result::Result<T, ChangeSetError>;

impl IntoResponse for ChangeSetError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            ChangeSetError::ActionAlreadyEnqueued(_) | ChangeSetError::ActionPrototype(_) => {
                (StatusCode::NOT_MODIFIED, self.to_string())
            }
            ChangeSetError::SchemaVariant(_) | ChangeSetError::Schema(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, self.to_string())
            }
            ChangeSetError::Hyper(_) | ChangeSetError::CannotAbandonHead => {
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            ChangeSetError::ChangeSetNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            ChangeSetError::DalChangeSetApply(_) => (StatusCode::CONFLICT, self.to_string()),
            ChangeSetError::DvuRootsNotEmpty(_) => (
                StatusCode::PRECONDITION_REQUIRED,
                "There are dependent values that still need to be calculated. Please retry!"
                    .to_string(),
            ),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        let body = Json(
            serde_json::json!({ "error": { "message": error_message, "code": 42, "statusCode": status.as_u16() } }),
        );

        error!(si.error.message = error_message);
        (status, body).into_response()
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/list_open_change_sets",
            get(list_open_change_sets::list_open_change_sets),
        )
        .route("/add_action", post(add_action::add_action))
        .route(
            "/create_change_set",
            post(create_change_set::create_change_set),
        )
        .route(
            "/apply_change_set",
            post(apply_change_set::apply_change_set),
        )
        .route(
            "/abandon_change_set",
            post(abandon_change_set::abandon_change_set),
        )
        .route(
            "/begin_approval_process",
            post(begin_approval_process::begin_approval_process),
        )
        .route(
            "/cancel_approval_process",
            post(begin_approval_process::cancel_approval_process),
        )
        .route("/merge_vote", post(merge_vote::merge_vote))
        .route(
            "/begin_abandon_approval_process",
            post(begin_abandon_approval_process::begin_abandon_approval_process),
        )
        .route(
            "/cancel_abandon_approval_process",
            post(begin_abandon_approval_process::cancel_abandon_approval_process),
        )
        .route("/abandon_vote", post(abandon_vote::abandon_vote))
        .route("/rebase_on_base", post(rebase_on_base::rebase_on_base))
        .route(
            "/status_with_base",
            post(status_with_base::status_with_base),
        )
}
