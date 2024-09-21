use std::string::FromUtf8Error;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use dal::{
    qualification::QualificationSummaryError, ComponentError, ComponentId, FuncId, SchemaError,
    SchemaId, StandardModelError, TenancyError, TransactionsError, WsEventError,
};
use telemetry::prelude::*;
use thiserror::Error;

use crate::AppState;

pub mod get_summary;

// code endpoints here are deprecated, removing them from the module tree
// moved to the func service - this probably means we can pair down the
// QualificationError a bit
//pub mod create;
//pub mod get_code;
//pub mod set_code;

#[remain::sorted]
#[derive(Debug, Error)]
pub enum QualificationError {
    #[error("base64 decode error: {0}")]
    Base64Decode(#[from] base64::DecodeError),
    #[error("component error: {0}")]
    Component(#[from] ComponentError),
    #[error("component not found: {0}")]
    ComponentNotFound(ComponentId),
    #[error("func code not found: {0}")]
    FuncCodeNotFound(FuncId),
    #[error("func not found")]
    FuncNotFound,
    #[error(transparent)]
    Nats(#[from] si_data_nats::NatsError),
    #[error(transparent)]
    Pg(#[from] si_data_pg::PgError),
    #[error("qualification summary error: {0}")]
    QualificationSummaryError(#[from] QualificationSummaryError),
    #[error("schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error("schema not found: {0}")]
    SchemaNotFound(SchemaId),
    #[error("schema variant not found")]
    SchemaVariantNotFound,
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("standard model error: {0}")]
    StandardModel(#[from] StandardModelError),
    #[error("tenancy error: {0}")]
    Tenancy(#[from] TenancyError),
    #[error(transparent)]
    Transactions(#[from] TransactionsError),
    #[error("utf8 error: {0}")]
    Utf8(#[from] FromUtf8Error),
    #[error("ws event error: {0}")]
    WsEvent(#[from] WsEventError),
}

pub type QualificationResult<T> = std::result::Result<T, QualificationError>;

impl IntoResponse for QualificationError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            QualificationError::ComponentNotFound(_)
            | QualificationError::FuncCodeNotFound(_)
            | QualificationError::FuncNotFound
            | QualificationError::SchemaNotFound(_)
            | QualificationError::SchemaVariantNotFound
            | QualificationError::Transactions(_) => (StatusCode::NOT_FOUND, self.to_string()),
            QualificationError::Component(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            QualificationError::Tenancy(_) => (StatusCode::FORBIDDEN, self.to_string()),
            QualificationError::Base64Decode(_)
            | QualificationError::Serde(_)
            | QualificationError::Utf8(_) => (StatusCode::UNPROCESSABLE_ENTITY, self.to_string()),

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
    Router::new().route("/get_summary", get(get_summary::get_summary))
}
