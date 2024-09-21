use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dal::prop::PropError;
use dal::property_editor::PropertyEditorError;
use dal::validation::ValidationError;
use dal::{
    action::prototype::ActionPrototypeError, action::ActionError,
    ComponentError as DalComponentError, FuncError, StandardModelError, WorkspaceError,
    WorkspaceSnapshotError,
};
use dal::{
    attribute::value::debug::AttributeDebugViewError, component::ComponentId, PropId,
    SchemaVariantError, SecretError as DalSecretError, WsEventError,
};
use dal::{attribute::value::AttributeValueError, component::debug::ComponentDebugViewError};
use dal::{ChangeSetError, TransactionsError};
use telemetry::prelude::*;
use thiserror::Error;

use crate::{service::component::conflicts_for_component::conflicts_for_component, AppState};

pub mod delete_property_editor_value;
pub mod get_actions;
pub mod get_diff;
pub mod get_property_editor_schema;
pub mod get_property_editor_values;
pub mod get_resource;
pub mod insert_property_editor_value;
pub mod json;
pub mod list_qualifications;
pub mod update_property_editor_value;
// pub mod list_resources;
pub mod conflicts_for_component;
pub mod debug;
pub mod get_code;
pub mod refresh;
pub mod restore_default_function;
pub mod set_name;
pub mod set_type;
mod upgrade;

#[remain::sorted]
#[derive(Debug, Error)]
pub enum ComponentError {
    #[error("action error: {0}")]
    Action(#[from] ActionError),
    #[error("action prototype error: {0}")]
    ActionPrototype(#[from] ActionPrototypeError),
    #[error("attribute debug view error: {0}")]
    AttributeDebugViewError(#[from] AttributeDebugViewError),
    #[error("attribute value error: {0}")]
    AttributeValue(#[from] AttributeValueError),
    #[error("change set error: {0}")]
    ChangeSet(#[from] ChangeSetError),
    #[error("component debug view error: {0}")]
    ComponentDebugView(#[from] ComponentDebugViewError),
    #[error("dal component error: {0}")]
    DalComponent(#[from] DalComponentError),
    #[error("diagram error: {0}")]
    DiagramError(#[from] dal::diagram::DiagramError),
    #[error("func error: {0}")]
    Func(#[from] FuncError),
    #[error("hyper error: {0}")]
    Http(#[from] axum::http::Error),
    #[error("invalid visibility")]
    InvalidVisibility,
    #[error("key {0} already exists for that map")]
    KeyAlreadyExists(String),
    #[error("component not found for id: {0}")]
    NotFound(ComponentId),
    #[error(transparent)]
    Prop(#[from] PropError),
    #[error("property editor error: {0}")]
    PropertyEditor(#[from] PropertyEditorError),
    #[error("prop not found for id: {0}")]
    PropNotFound(PropId),
    #[error("schema not found")]
    SchemaNotFound,
    #[error("schema variant error: {0}")]
    SchemaVariant(#[from] SchemaVariantError),
    #[error("schema variant not found")]
    SchemaVariantNotFound,
    #[error("schema variant upgrade not required")]
    SchemaVariantUpgradeSkipped,
    #[error("dal secret error: {0}")]
    Secret(#[from] DalSecretError),
    #[error("serde json error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error(transparent)]
    StandardModel(#[from] StandardModelError),
    #[error(transparent)]
    Transactions(#[from] TransactionsError),
    #[error("component upgrade skipped due to running or dispatched actions")]
    UpgradeSkippedDueToActions,
    #[error("validation resolver error: {0}")]
    ValidationResolver(#[from] ValidationError),
    #[error("workspace error: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("workspace snapshot error: {0}")]
    WorkspaceSnapshot(#[from] WorkspaceSnapshotError),
    #[error("ws event error: {0}")]
    WsEvent(#[from] WsEventError),
}

pub type ComponentResult<T> = Result<T, ComponentError>;

impl IntoResponse for ComponentError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            ComponentError::SchemaNotFound
            | ComponentError::InvalidVisibility
            | ComponentError::PropNotFound(_)
            | ComponentError::SchemaVariantNotFound
            | ComponentError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            ComponentError::PropertyEditor(err) => match err {
                PropertyEditorError::ComponentNotFound
                | PropertyEditorError::PropertyEditorValueNotFoundByPropId(_)
                | PropertyEditorError::SchemaVariantNotFound(_) => {
                    (StatusCode::NOT_FOUND, err.to_string())
                }
                PropertyEditorError::AttributePrototype(_)
                | PropertyEditorError::AttributeValue(_)
                | PropertyEditorError::BadAttributeReadContext(_)
                | PropertyEditorError::SchemaVariant(_)
                | PropertyEditorError::Secret(_)
                | PropertyEditorError::SerdeJson(_)
                | PropertyEditorError::SecretPropLeadsToStaticValue(_, _)
                | PropertyEditorError::Validation(_)
                | PropertyEditorError::ValueSource(_) => (StatusCode::BAD_REQUEST, err.to_string()),
                PropertyEditorError::CycleDetected(_) => {
                    (StatusCode::LOOP_DETECTED, err.to_string())
                }
                _ => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
            },
            ComponentError::SchemaVariantUpgradeSkipped => {
                (StatusCode::NOT_MODIFIED, self.to_string())
            }
            ComponentError::UpgradeSkippedDueToActions => {
                (StatusCode::PRECONDITION_FAILED, self.to_string())
            }
            ComponentError::KeyAlreadyExists(_) | ComponentError::SerdeJson(_) => {
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
        .route("/get_actions", get(get_actions::get_actions))
        .route(
            "/get_property_editor_schema",
            get(get_property_editor_schema::get_property_editor_schema),
        )
        .route(
            "/get_property_editor_values",
            get(get_property_editor_values::get_property_editor_values),
        )
        .route(
            "/list_qualifications",
            get(list_qualifications::list_qualifications),
        )
        .route("/get_code", get(get_code::get_code))
        .route("/get_diff", get(get_diff::get_diff))
        .route("/get_resource", get(get_resource::get_resource))
        .route(
            "/update_property_editor_value",
            post(update_property_editor_value::update_property_editor_value),
        )
        .route(
            "/insert_property_editor_value",
            post(insert_property_editor_value::insert_property_editor_value),
        )
        .route(
            "/delete_property_editor_value",
            post(delete_property_editor_value::delete_property_editor_value),
        )
        .route(
            "/restore_default_function",
            post(restore_default_function::restore_default_function),
        )
        .route("/set_type", post(set_type::set_type))
        .route("/set_name", post(set_name::set_name))
        .route("/refresh", post(refresh::refresh))
        .route("/debug", get(debug::debug_component))
        .route("/json", get(json::json))
        .route("/upgrade_component", post(upgrade::upgrade))
        .route("/conflicts", get(conflicts_for_component))
}
