use crate::app_state::AppState;
use crate::service::ApiError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use dal::cached_module::CachedModuleError;
use dal::component::frame::FrameError;
use dal::component::inferred_connection_graph::InferredConnectionGraphError;
use dal::diagram::view::{View, ViewId};
use dal::pkg::PkgError;
use dal::slow_rt::SlowRuntimeError;
use dal::{
    ChangeSetError, ComponentError, DalContext, SchemaError, SchemaId, SchemaVariantError,
    Timestamp, TransactionsError, WorkspaceSnapshotError, WsEventError,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::task::JoinError;

pub mod create_component;
pub mod create_view;
pub mod get_diagram;
pub mod list_views;
mod paste_component;
mod set_component_geometry;
mod set_component_parent;
pub mod update_view;

#[remain::sorted]
#[derive(Debug, Error)]
pub enum ViewError {
    #[error("cached module error: {0}")]
    CachedModule(#[from] CachedModuleError),
    #[error("changeset error: {0}")]
    ChangeSet(#[from] ChangeSetError),
    #[error("component error: {0}")]
    Component(#[from] ComponentError),
    #[error("dal diagram error: {0}")]
    DalDiagram(#[from] dal::diagram::DiagramError),
    #[error("frame error: {0}")]
    Frame(#[from] FrameError),
    #[error("inferred connection graph error: {0}")]
    InferredConnectionGraph(#[from] InferredConnectionGraphError),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("join error: {0}")]
    Join(#[from] JoinError),
    #[error("there is already a view called {0}")]
    NameAlreadyInUse(String),
    #[error("paste error")]
    Paste,
    #[error("pkg error: {0}")]
    Pkg(#[from] PkgError),
    #[error("schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error("No schema installed after successful package import for {0}")]
    SchemaNotInstalledAfterImport(SchemaId),
    #[error("schema variant error: {0}")]
    SchemaVariant(#[from] SchemaVariantError),
    #[error("serrde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("slow runtime error: {0}")]
    SlowRuntime(#[from] SlowRuntimeError),
    #[error("transactions error: {0}")]
    Transactions(#[from] TransactionsError),
    #[error("No installable module found for schema id {0}")]
    UninstalledSchemaNotFound(SchemaId),
    #[error("workspace snapshot error: {0}")]
    WorkspaceSnapshot(#[from] WorkspaceSnapshotError),
    #[error("WsEvent error: {0}")]
    WsEvent(#[from] WsEventError),
}

pub type ViewResult<T> = Result<T, ViewError>;

impl IntoResponse for ViewError {
    fn into_response(self) -> Response {
        let (status_code, error_message) = match self {
            ViewError::NameAlreadyInUse(_) => (StatusCode::UNPROCESSABLE_ENTITY, self.to_string()),

            _ => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        ApiError::new(status_code, error_message).into_response()
    }
}

/// Frontend representation for a [View](View).
/// Yeah, it's a silly name, but all the other frontend representation structs are *View,
/// so we either keep it or change everything.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ViewView {
    id: ViewId,
    name: String,
    is_default: bool,
    #[serde(flatten)]
    timestamp: Timestamp,
}

impl ViewView {
    pub async fn from_view(ctx: &DalContext, view: View) -> ViewResult<Self> {
        Ok(ViewView {
            id: view.id(),
            name: view.name().to_owned(),
            is_default: view.is_default(ctx).await?,
            timestamp: view.timestamp().to_owned(),
        })
    }
}

pub fn v2_routes() -> Router<AppState> {
    Router::new()
        // Func Stuff
        .route("/", get(list_views::list_views))
        .route("/", post(create_view::create_view))
        .route("/:view_id", put(update_view::update_view))
        .route("/:view_id/get_diagram", get(get_diagram::get_diagram))
        .route(
            "/:view_id/component",
            post(create_component::create_component),
        )
        .route(
            "/:view_id/paste_components",
            post(paste_component::paste_component),
        )
        .route(
            "/:view_id/component/set_geometry",
            put(set_component_geometry::set_component_geometry),
        )
        .route(
            "/:view_id/component/set_parent",
            put(set_component_parent::set_component_parent),
        )
}
