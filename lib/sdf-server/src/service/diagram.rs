use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use dal::attribute::prototype::argument::AttributePrototypeArgumentError;
use dal::attribute::prototype::AttributePrototypeError;
use dal::attribute::value::AttributeValueError;
use dal::component::ComponentError;
use dal::slow_rt::SlowRuntimeError;
use dal::socket::input::InputSocketError;
use dal::socket::output::OutputSocketError;
use dal::workspace_snapshot::WorkspaceSnapshotError;
use dal::WsEventError;
use dal::{ChangeSetError, SchemaVariantId, StandardModelError, TransactionsError};
use std::num::ParseFloatError;
use telemetry::prelude::*;
use thiserror::Error;
use tokio::task::JoinError;

use crate::AppState;

pub mod create_component;
pub mod create_connection;
pub mod get_diagram;
pub mod list_schemas;
pub mod set_component_position;

pub mod delete_component;
pub mod delete_connection;
pub mod paste_component;
pub mod remove_delete_intent;

pub mod dvu_roots;

#[remain::sorted]
#[derive(Debug, Error)]
pub enum DiagramError {
    #[error("attribute prototype error: {0}")]
    AttributePrototype(#[from] AttributePrototypeError),
    #[error("attribute prototype argument error: {0}")]
    AttributePrototypeArgument(#[from] AttributePrototypeArgumentError),
    #[error("attribute value error: {0}")]
    AttributeValue(#[from] AttributeValueError),
    #[error("changeset error: {0}")]
    ChangeSet(#[from] ChangeSetError),
    #[error("change set not found")]
    ChangeSetNotFound,
    #[error("component error: {0}")]
    Component(#[from] ComponentError),
    #[error("component not found")]
    ComponentNotFound,
    #[error(transparent)]
    ContextTransaction(#[from] TransactionsError),
    #[error("dal diagram error: {0}")]
    DalDiagram(#[from] dal::diagram::DiagramError),
    #[error("dal frame error: {0}")]
    DalFrame(#[from] dal::component::frame::FrameError),
    #[error("dal schema variant error: {0}")]
    DalSchemaVariant(#[from] dal::schema::variant::SchemaVariantError),
    #[error("dal schema view error: {0}")]
    DalSchemaView(#[from] dal::schema::view::SchemaViewError),
    #[error("duplicated connection")]
    DuplicatedConnection,
    #[error("edge not found")]
    EdgeNotFound,
    #[error("frame socket not found for schema variant id: {0}")]
    FrameSocketNotFound(SchemaVariantId),
    #[error("invalid header name {0}")]
    Hyper(#[from] hyper::http::Error),
    #[error("input socket error: {0}")]
    InputSocket(#[from] InputSocketError),
    #[error("invalid request")]
    InvalidRequest,
    #[error("invalid system")]
    InvalidSystem,
    #[error("tokio join error: {0}")]
    Join(#[from] JoinError),
    #[error(transparent)]
    Nats(#[from] si_data_nats::NatsError),
    #[error("not authorized")]
    NotAuthorized,
    #[error("output socket error: {0}")]
    OutputSocket(#[from] OutputSocketError),
    #[error("parse float error: {0}")]
    ParseFloat(#[from] ParseFloatError),
    #[error("paste failed")]
    Paste,
    #[error(transparent)]
    Pg(#[from] si_data_pg::PgError),
    #[error(transparent)]
    PgPool(#[from] si_data_pg::PgPoolError),
    #[error("schema not found")]
    SchemaNotFound,
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("slow runtime error: {0}")]
    SlowRuntime(#[from] SlowRuntimeError),
    #[error("socket not found")]
    SocketNotFound,
    #[error(transparent)]
    StandardModel(#[from] StandardModelError),
    #[error(transparent)]
    WorkspaceSnaphot(#[from] WorkspaceSnapshotError),
    #[error("ws event error: {0}")]
    WsEvent(#[from] WsEventError),
}

pub type DiagramResult<T> = Result<T, DiagramError>;

impl IntoResponse for DiagramError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            DiagramError::SchemaNotFound
            | DiagramError::ChangeSetNotFound
            | DiagramError::ComponentNotFound
            | DiagramError::FrameSocketNotFound(_)
            | DiagramError::EdgeNotFound
            | DiagramError::SocketNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            DiagramError::DuplicatedConnection => (StatusCode::NOT_MODIFIED, self.to_string()),
            DiagramError::AttributePrototypeArgument(_)
            | DiagramError::AttributeValue(_)
            | DiagramError::ChangeSet(_)
            | DiagramError::Component(_)
            | DiagramError::AttributePrototype(_)
            | DiagramError::Hyper(_)
            | DiagramError::InputSocket(_)
            | DiagramError::OutputSocket(_)
            | DiagramError::Paste
            | DiagramError::InvalidRequest
            | DiagramError::InvalidSystem => (StatusCode::BAD_REQUEST, self.to_string()),
            DiagramError::NotAuthorized => (StatusCode::FORBIDDEN, self.to_string()),
            DiagramError::ParseFloat(_) | DiagramError::Serde(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, self.to_string())
            }
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
        .route("/paste_components", post(paste_component::paste_components))
        .route(
            "/delete_connection",
            post(delete_connection::delete_connection),
        )
        .route(
            "/delete_components",
            post(delete_component::delete_components),
        )
        .route(
            "/remove_delete_intent",
            post(remove_delete_intent::remove_delete_intent),
        )
        .route(
            "/create_connection",
            post(create_connection::create_connection),
        )
        .route(
            "/create_component",
            post(create_component::create_component),
        )
        .route(
            "/set_component_position",
            post(set_component_position::set_component_position),
        )
        .route("/get_diagram", get(get_diagram::get_diagram))
        .route("/list_schemas", get(list_schemas::list_schemas))
        .route("/dvu_roots", get(dvu_roots::dvu_roots))
}
