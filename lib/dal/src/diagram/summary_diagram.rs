use std::num::{ParseFloatError, ParseIntError};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use strum::{AsRefStr, Display, EnumIter, EnumString};
use telemetry::prelude::*;
use thiserror::Error;

use si_data_pg::PgError;

use crate::change_status::ChangeStatus;
use crate::diagram::DiagramResult;
use crate::edge::{EdgeId, EdgeKind};
use crate::history_event::HistoryEventMetadata;
use crate::schema::SchemaUiMenu;
use crate::socket::SocketEdgeKind;
use crate::standard_model::{self, objects_from_rows};
use crate::{
    history_event, impl_standard_model, pk, standard_model_accessor, ActorView, Component,
    ComponentError, ComponentId, ComponentStatus, DalContext, DiagramError, Edge, EdgeError,
    HistoryActor, HistoryEventError, Node, NodeId, Schema, SchemaError, SchemaId, SchemaVariant,
    SchemaVariantId, SocketArity, SocketId, StandardModel, StandardModelError, Tenancy, Timestamp,
    TransactionsError, Visibility,
};

const LIST_SUMMARY_DIAGRAM_COMPONENTS: &str =
    include_str!("../queries/summary_diagram/list_summary_diagram_components.sql");
const LIST_SUMMARY_DIAGRAM_EDGES: &str =
    include_str!("../queries/summary_diagram/list_summary_diagram_edges.sql");

#[remain::sorted]
#[derive(Error, Debug)]
pub enum SummaryDiagramError {
    #[error(transparent)]
    ChronoParse(#[from] chrono::ParseError),
    #[error(transparent)]
    Component(#[from] ComponentError),
    #[error(transparent)]
    Diagram(#[from] DiagramError),
    #[error(transparent)]
    Edge(#[from] EdgeError),
    #[error("history event error: {0}")]
    HistoryEvent(#[from] HistoryEventError),
    #[error("no timestamp for deleting an edge when one was expected; bug!")]
    NoTimestamp,
    #[error(transparent)]
    ParseFloat(#[from] ParseFloatError),
    #[error(transparent)]
    ParseInt(#[from] ParseIntError),
    #[error(transparent)]
    PgError(#[from] PgError),
    #[error(transparent)]
    Schema(#[from] SchemaError),
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
    #[error(transparent)]
    StandardModel(#[from] StandardModelError),
    #[error(transparent)]
    Transactions(#[from] TransactionsError),
}

pub type SummaryDiagramResult<T> = Result<T, SummaryDiagramError>;

pk!(SummaryDiagramComponentPk);
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct SummaryDiagramComponent {
    pk: SummaryDiagramComponentPk,
    id: ComponentId,
    #[serde(flatten)]
    tenancy: Tenancy,
    #[serde(flatten)]
    timestamp: Timestamp,
    #[serde(flatten)]
    visibility: Visibility,
    component_id: ComponentId,
    schema_name: String,
    schema_id: SchemaId,
    schema_variant_id: SchemaVariantId,
    schema_variant_name: String,
    schema_category: String,
    sockets: serde_json::Value,
    node_id: NodeId,
    display_name: String,
    position: GridPoint,
    size: Size2D,
    color: String,
    node_type: String,
    change_status: String,
    has_resource: bool,
    parent_node_id: Option<NodeId>,
    child_node_ids: serde_json::Value,
    created_info: serde_json::Value,
    updated_info: serde_json::Value,
    deleted_info: serde_json::Value,
    using_default_variant: bool,
}

impl_standard_model! {
    model: SummaryDiagramComponent,
    pk: SummaryDiagramComponentPk,
    id: ComponentId,
    table_name: "summary_diagram_components",
    history_event_label_base: "summary_diagram_components",
    history_event_message_name: "Summary Diagram Components"
}

pub async fn create_component_entry(
    ctx: &DalContext,
    component: &Component,
    node: &Node,
    schema: &Schema,
    schema_variant: &SchemaVariant,
) -> SummaryDiagramResult<()> {
    let schema_category_name = SchemaUiMenu::find_for_schema(ctx, *schema.id())
        .await?
        .map_or("None".to_string(), |um| um.category().to_string());
    let sockets = DiagramSocket::list(ctx, schema_variant).await?;
    let display_name = component.name(ctx).await?;
    let position = GridPoint {
        x: node.x().parse::<f64>()?.round() as isize,
        y: node.y().parse::<f64>()?.round() as isize,
    };
    let size = if let (Some(w), Some(h)) = (node.width(), node.height()) {
        Size2D {
            height: h.parse()?,
            width: w.parse()?,
        }
    } else {
        Size2D {
            height: 500,
            width: 500,
        }
    };
    let color = component.color(ctx).await?.unwrap_or("#111111".to_string());
    let node_type = component.get_type(ctx).await?;

    let change_status = ChangeStatus::Added;

    // This could also be refactored away from history actors
    let component_status = ComponentStatus::get_by_id(ctx, component.id())
        .await?
        .ok_or_else(|| DiagramError::ComponentStatusNotFound(*component.id()))?;
    let created_info =
        HistoryEventMetadata::from_history_actor_timestamp(ctx, component_status.creation())
            .await?;
    let updated_info =
        HistoryEventMetadata::from_history_actor_timestamp(ctx, component_status.update()).await?;
    let mut deleted_info: Option<HistoryEventMetadata> = None;
    {
        if let Some(deleted_at) = ctx.visibility().deleted_at {
            if let Some(deletion_user_pk) = component.deletion_user_pk() {
                let history_actor = history_event::HistoryActor::User(*deletion_user_pk);
                let actor = ActorView::from_history_actor(ctx, history_actor).await?;

                deleted_info = Some(HistoryEventMetadata {
                    actor,
                    timestamp: deleted_at,
                });
            }
        }
    }

    let using_default_variant = schema.default_schema_variant_id() == Some(schema_variant.id());

    let _row = ctx
        .txns()
        .await?
        .pg()
        .query_one(
            "SELECT object FROM summary_diagram_component_create_v2($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21)",
            &[
                ctx.tenancy(),
                ctx.visibility(),
                component.id(),
                &schema.name(),
                schema.id(),
                schema_variant.id(),
                &schema_variant.name(),
                &schema_category_name,
                &serde_json::to_value(sockets)?,
                node.id(),
                &display_name,
                &serde_json::to_value(position)?,
                &serde_json::to_value(size)?,
                &color,
                &node_type.to_string(),
                &change_status.to_string(),
                &false,
                &serde_json::to_value(created_info)?,
                &serde_json::to_value(updated_info)?,
                &serde_json::to_value(deleted_info)?,
                &using_default_variant,
            ],
        )
        .await?;
    Ok(())
}

pub async fn falsify_using_default_variant_for_components_of_schema(
    ctx: &DalContext,
    schema_id: SchemaId,
) -> SummaryDiagramResult<()> {
    ctx.txns()
        .await?
        .pg()
        .execute(
            "SELECT falsify_using_default_variant_for_components_of_schema_v1($1, $2, $3)",
            &[ctx.tenancy(), ctx.visibility(), &schema_id],
        )
        .await?;

    Ok(())
}

pub async fn component_update_geometry(
    ctx: &DalContext,
    node_id: &NodeId,
    x: impl AsRef<str>,
    y: impl AsRef<str>,
    width: Option<impl AsRef<str>>,
    height: Option<impl AsRef<str>>,
) -> SummaryDiagramResult<()> {
    let position = GridPoint {
        x: x.as_ref().parse::<f64>()?.round() as isize,
        y: y.as_ref().parse::<f64>()?.round() as isize,
    };
    let size = if let (Some(w), Some(h)) = (width, height) {
        Size2D {
            height: h.as_ref().parse()?,
            width: w.as_ref().parse()?,
        }
    } else {
        Size2D {
            height: 500,
            width: 500,
        }
    };

    let _row = ctx
        .txns()
        .await?
        .pg()
        .query_one(
            "SELECT object FROM summary_diagram_component_update_geometry_v2($1, $2, $3, $4, $5)",
            &[
                ctx.tenancy(),
                ctx.visibility(),
                &node_id,
                &serde_json::to_value(position)?,
                &serde_json::to_value(size)?,
            ],
        )
        .await?;
    Ok(())
}

pub async fn component_update(
    ctx: &DalContext,
    component_id: &ComponentId,
    name: impl AsRef<str>,
    color: impl AsRef<str>,
    component_type: impl AsRef<str>,
    has_resource: bool,
    deleted_at: Option<String>,
) -> SummaryDiagramResult<()> {
    let component_status = ComponentStatus::get_by_id(ctx, component_id)
        .await?
        .ok_or_else(|| DiagramError::ComponentStatusNotFound(*component_id))?;

    let updated_info =
        HistoryEventMetadata::from_history_actor_timestamp(ctx, component_status.update()).await?;

    // We really have to clean up how we keep summaries of history events, which will make it so
    // we no longer need to fetch the full component in order to pull this off. It's a bit insane.
    // But it is what it is, for now. -- Adam
    let mut deleted_info = None;
    let mut deleted_at_datetime: Option<DateTime<Utc>> = None;
    if let Some(ref deleted_at) = deleted_at {
        let deleted_at_datetime_inner: DateTime<Utc> = deleted_at.parse()?;
        deleted_at_datetime = Some(deleted_at_datetime_inner);
        let new_ctx = ctx.clone_with_delete_visibility();
        if let Some(component) = Component::get_by_id(&new_ctx, component_id).await? {
            if let Some(deletion_user_pk) = component.deletion_user_pk() {
                let history_actor = HistoryActor::User(*deletion_user_pk);
                let actor = ActorView::from_history_actor(ctx, history_actor).await?;
                deleted_info = Some(HistoryEventMetadata {
                    actor,
                    timestamp: deleted_at_datetime_inner,
                });
            }
        }
    }

    // Set the change_status to deleted if we are adding the delete data
    let _row = ctx
        .txns()
        .await?
        .pg()
        .query_one(
            "SELECT object FROM summary_diagram_component_update_v3($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            &[
                ctx.tenancy(),
                ctx.visibility(),
                &component_id,
                &name.as_ref(),
                &color.as_ref(),
                &component_type.as_ref(),
                &has_resource,
                &serde_json::to_value(updated_info)?,
                &deleted_at_datetime,
                &serde_json::to_value(deleted_info)?,
            ],
        )
        .await?;
    Ok(())
}

pub async fn component_list(
    ctx: &DalContext,
) -> SummaryDiagramResult<Vec<SummaryDiagramComponent>> {
    let rows = ctx
        .txns()
        .await?
        .pg()
        .query(
            LIST_SUMMARY_DIAGRAM_COMPONENTS,
            &[ctx.tenancy(), &ctx.visibility().change_set_pk],
        )
        .await?;
    let objects: Vec<SummaryDiagramComponent> = objects_from_rows(rows)?;
    Ok(objects)
}

impl_standard_model! {
    model: SummaryDiagramEdge,
    pk: SummaryDiagramEdgePk,
    id: EdgeId,
    table_name: "summary_diagram_edges",
    history_event_label_base: "summary_diagram_edges",
    history_event_message_name: "Summary Diagram Edges"
}

impl SummaryDiagramEdge {
    pub fn edge_id(&self) -> EdgeId {
        self.edge_id
    }
}

pub async fn create_edge_entry(ctx: &DalContext, edge: &Edge) -> SummaryDiagramResult<()> {
    let mut created_info: Option<HistoryEventMetadata> = None;
    if let Some(user_pk) = edge.creation_user_pk() {
        let history_actor = HistoryActor::User(*user_pk);
        let actor = ActorView::from_history_actor(ctx, history_actor).await?;
        created_info = Some(HistoryEventMetadata {
            actor,
            timestamp: edge.timestamp().created_at,
        })
    }

    let _row = ctx
        .txns()
        .await?
        .pg()
        .query_one(
            "SELECT object FROM summary_diagram_edge_create_v1($1, $2, $3, $4, $5, $6, $7, $8)",
            &[
                ctx.tenancy(),
                ctx.visibility(),
                &edge.id(),
                &edge.tail_node_id(),
                &edge.tail_socket_id(),
                &edge.head_node_id(),
                &edge.head_socket_id(),
                &serde_json::to_value(created_info)?,
            ],
        )
        .await?;

    // If this is a symbolic edge, we need to set the relevant summary diagram component row's parent node id.
    if edge.kind() == &EdgeKind::Symbolic {
        ctx
            .txns()
            .await?
            .pg()
            .query_one(
                "SELECT object FROM summary_diagram_component_set_parent_node_id_v3($1, $2, $3, $4)",
                &[
                    ctx.tenancy(),
                    ctx.visibility(),
                    &edge.tail_component_id(),
                    &edge.head_node_id(),
                ],
            )
            .await?;
    }

    Ok(())
}

pub async fn delete_edge_entry(ctx: &DalContext, edge: &Edge) -> SummaryDiagramResult<()> {
    let mut deleted_info = None;
    let new_ctx = ctx.clone_with_delete_visibility();
    let mut deleted_timestamp = None;
    // I hate how this makes me feel inside my heart. -- Adam
    if let Some(deleted_edge) = Edge::get_by_id(&new_ctx, edge.id()).await? {
        if let Some(deletion_user_pk) = deleted_edge.deletion_user_pk() {
            let history_actor = HistoryActor::User(*deletion_user_pk);
            let actor = ActorView::from_history_actor(ctx, history_actor).await?;
            deleted_timestamp = Some(
                deleted_edge
                    .visibility()
                    .deleted_at
                    .ok_or_else(|| SummaryDiagramError::NoTimestamp)?,
            );

            deleted_info = Some(HistoryEventMetadata {
                actor,
                timestamp: deleted_timestamp
                    .expect("we know we have a deleted timestamp, but... we don't. Bug!"),
            });
        } else {
            let history_actor = HistoryActor::SystemInit;
            let actor = ActorView::from_history_actor(ctx, history_actor).await?;
            deleted_timestamp = Some(
                deleted_edge
                    .visibility()
                    .deleted_at
                    .ok_or_else(|| SummaryDiagramError::NoTimestamp)?,
            );

            deleted_info = Some(HistoryEventMetadata {
                actor,
                timestamp: deleted_timestamp
                    .expect("we know we have a deleted timestamp, but... we don't. Bug!"),
            });
        }
    }

    let _row = ctx
        .txns()
        .await?
        .pg()
        .query_one(
            "SELECT object FROM summary_diagram_edge_delete_v1($1, $2, $3, $4, $5)",
            &[
                ctx.tenancy(),
                ctx.visibility(),
                &edge.id(),
                &deleted_timestamp,
                &serde_json::to_value(deleted_info)?,
            ],
        )
        .await?;

    // If this is a symbolic edge, we need to unset the relevant summary diagram component row's parent node id.
    if edge.kind() == &EdgeKind::Symbolic {
        ctx.txns()
            .await?
            .pg()
            .query_one(
                "SELECT object FROM summary_diagram_component_set_parent_node_id_v3($1, $2, $3, NULL)",
                &[ctx.tenancy(), ctx.visibility(), &edge.tail_component_id()],
            )
            .await?;
    }
    Ok(())
}

pub async fn restore_edge_entry(ctx: &DalContext, edge: &Edge) -> SummaryDiagramResult<()> {
    let ctx_with_deleted = ctx.clone_with_delete_visibility();

    let deleted_edge = Edge::get_by_id(&ctx_with_deleted, edge.id())
        .await?
        .ok_or(EdgeError::EdgeNotFound(*edge.id()))?;

    if deleted_edge.visibility().deleted_at.is_none()
        || deleted_edge.visibility().change_set_pk != ctx.visibility().change_set_pk
    {
        return Err(SummaryDiagramError::Edge(
            EdgeError::RestoringNonDeletedEdge(*edge.id()),
        ));
    }

    ctx.txns()
        .await?
        .pg()
        .query_one(
            "SELECT object FROM restore_summary_edge_by_id_v1($1, $2, $3)",
            &[ctx.tenancy(), ctx.visibility(), &deleted_edge.id()],
        )
        .await?;

    // If this is a symbolic edge, we need to update the relevant summary diagram component row's parent node id.
    if edge.kind() == &EdgeKind::Symbolic {
        ctx.txns()
            .await?
            .pg()
            .query_one(
                "SELECT object FROM summary_diagram_component_set_parent_node_id_v3($1, $2, $3, $4)",
                &[
                    ctx.tenancy(),
                    ctx.visibility(),
                    &edge.tail_component_id(),
                    &edge.head_node_id(),
                ],
            )
            .await?;
    }
    Ok(())
}

pub async fn edge_list(ctx: &DalContext) -> SummaryDiagramResult<Vec<SummaryDiagramEdge>> {
    let rows = ctx
        .txns()
        .await?
        .pg()
        .query(
            LIST_SUMMARY_DIAGRAM_EDGES,
            &[ctx.tenancy(), &ctx.visibility().change_set_pk],
        )
        .await?;
    let objects: Vec<SummaryDiagramEdge> = objects_from_rows(rows)?;
    Ok(objects)
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DiagramSocket {
    pub id: String,
    pub label: String,
    pub connection_annotations: Vec<String>,
    pub direction: DiagramSocketDirection,
    pub max_connections: Option<usize>,
    pub is_required: Option<bool>,
    pub node_side: DiagramSocketNodeSide,
}

impl DiagramSocket {
    pub async fn list(
        ctx: &DalContext,
        schema_variant: &SchemaVariant,
    ) -> DiagramResult<Vec<Self>> {
        Ok(schema_variant
            .sockets(ctx)
            .await?
            .into_iter()
            .filter_map(|socket| {
                (!socket.ui_hidden()).then(|| {
                    let connection_annotations =
                        serde_json::from_str(socket.connection_annotations())
                            .unwrap_or(vec![socket.name().to_owned()]);

                    Self {
                        id: socket.id().to_string(),
                        label: socket.human_name().unwrap_or(socket.name()).to_owned(),
                        connection_annotations,
                        // Note: it's not clear if this mapping is correct, and there is no backend support for bidirectional sockets for now
                        direction: match socket.edge_kind() {
                            SocketEdgeKind::ConfigurationOutput => DiagramSocketDirection::Output,
                            _ => DiagramSocketDirection::Input,
                        },
                        max_connections: match socket.arity() {
                            SocketArity::Many => None,
                            SocketArity::One => Some(1),
                        },
                        is_required: Some(socket.required()),
                        node_side: match socket.edge_kind() {
                            SocketEdgeKind::ConfigurationOutput => DiagramSocketNodeSide::Right,
                            _ => DiagramSocketNodeSide::Left,
                        },
                    }
                })
            })
            .collect())
    }
}

#[remain::sorted]
#[derive(
    AsRefStr,
    Clone,
    Copy,
    Debug,
    Deserialize,
    Display,
    EnumIter,
    EnumString,
    Eq,
    PartialEq,
    Serialize,
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
enum DiagramSocketDirection {
    Bidirectional,
    Input,
    Output,
}

#[remain::sorted]
#[derive(
    AsRefStr,
    Clone,
    Copy,
    Debug,
    Deserialize,
    Display,
    EnumIter,
    EnumString,
    Eq,
    PartialEq,
    Serialize,
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
enum DiagramSocketNodeSide {
    Left,
    Right,
}
