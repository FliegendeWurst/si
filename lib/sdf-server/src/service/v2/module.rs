use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use dal::UserError;
use si_frontend_types as frontend_types;
use telemetry::prelude::*;
use thiserror::Error;

use crate::{service::ApiError, AppState};

mod builtins;
mod contribute;
mod list;
mod sync;

pub type ModuleAPIResult<T> = Result<T, ModulesAPIError>;

#[remain::sorted]
#[derive(Debug, Error)]
pub enum ModulesAPIError {
    #[error("axum http error: {0}")]
    AxumHttp(#[from] axum::http::Error),
    #[error("module not contributed: {0:?}")]
    ContributionFailure(frontend_types::ModuleContributeRequest),
    #[error("module error: {0}")]
    Module(#[from] dal::module::ModuleError),
    #[error("module index client error: {0}")]
    ModuleIndexClient(#[from] module_index_client::ModuleIndexClientError),
    #[error("module index not configured")]
    ModuleIndexNotConfigured,
    #[error("schema error: {0}")]
    SchemaVariant(#[from] dal::SchemaVariantError),
    #[error("transactions error: {0}")]
    Transactions(#[from] dal::TransactionsError),
    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),
    #[error("user error: {0}")]
    User(#[from] UserError),
}

impl IntoResponse for ModulesAPIError {
    fn into_response(self) -> Response {
        let status_code = match &self {
            Self::Transactions(dal::TransactionsError::BadWorkspaceAndChangeSet) => {
                StatusCode::FORBIDDEN
            }
            Self::SchemaVariant(dal::SchemaVariantError::NotFound(schema_variant_id)) => {
                error!(%schema_variant_id, "schema variant not found");
                StatusCode::NOT_FOUND
            }
            Self::Module(dal::module::ModuleError::EmptyMetadata(_, _)) => StatusCode::BAD_REQUEST,
            Self::ContributionFailure(_) => StatusCode::BAD_REQUEST,
            _ => ApiError::DEFAULT_ERROR_STATUS_CODE,
        };

        ApiError::new(status_code, self).into_response()
    }
}

pub fn v2_routes() -> Router<AppState> {
    Router::new()
        .route("/contribute", post(contribute::contribute))
        .route("/sync", get(sync::sync))
        .route("/", get(list::list))
        .route("/:module_id/builtins/reject", post(builtins::reject))
        .route("/:module_id/builtins/promote", post(builtins::promote))
}
