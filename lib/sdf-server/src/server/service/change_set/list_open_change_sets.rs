use axum::Json;
use chrono::{DateTime, Utc};
//use dal::action::ActionId;
use dal::change_set_pointer::{ChangeSetPointer, ChangeSetPointerId};
use dal::ActionKind;
use dal::{ActionPrototypeId, ChangeSetStatus, ComponentId, UserPk};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use super::ChangeSetResult;
use crate::server::extract::{AccessBuilder, HandlerContext};

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionView {
    // FIXME(nick,zack,jacob): drop ActionId since it does not exist yet for the graph switchover.
    pub id: Ulid,
    pub action_prototype_id: ActionPrototypeId,
    pub kind: ActionKind,
    pub name: String,
    pub component_id: ComponentId,
    pub actor: Option<String>,
    pub parents: Vec<()>,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSetView {
    // TODO: pk and id are now identical and one of them should be removed
    pub id: ChangeSetPointerId,
    pub pk: ChangeSetPointerId,
    pub name: String,
    pub status: ChangeSetStatus,
    pub merge_requested_at: Option<DateTime<Utc>>,
    pub merge_requested_by_user_id: Option<UserPk>,
    pub abandon_requested_at: Option<DateTime<Utc>>,
    pub abandon_requested_by_user_id: Option<UserPk>,
}

pub type ListOpenChangeSetsResponse = Vec<ChangeSetView>;

pub async fn list_open_change_sets(
    HandlerContext(builder): HandlerContext,
    AccessBuilder(access_builder): AccessBuilder,
) -> ChangeSetResult<Json<ListOpenChangeSetsResponse>> {
    let ctx = builder.build_head(access_builder).await?;

    let list = ChangeSetPointer::list_open(&ctx).await?;
    let mut view = Vec::with_capacity(list.len());
    for cs in list {
        // let ctx =
        //     ctx.clone_with_new_visibility(Visibility::new(cs.pk, ctx.visibility().deleted_at));
        // let actions = HashMap::new();
        // for (
        //     _,
        //     ActionBag {
        //         action,
        //         parents,
        //         kind,
        //     },
        // ) in cs.actions(&ctx).await?
        // {
        //     let mut display_name = None;
        //     let prototype = action.prototype(&ctx).await?;
        //     let func_details = Func::get_by_id(&ctx, &prototype.func_id()).await?;
        //     if let Some(func) = func_details {
        //         if func.display_name().is_some() {
        //             display_name = func.display_name().map(|dname| dname.to_string());
        //         }
        //     }

        //     let mut actor_email: Option<String> = None;
        //     {
        //         if let Some(created_at_user) = action.creation_user_id() {
        //             let history_actor = history_event::HistoryActor::User(*created_at_user);
        //             let actor = ActorView::from_history_actor(&ctx, history_actor).await?;
        //             match actor {
        //                 ActorView::System { label } => actor_email = Some(label),
        //                 ActorView::User { label, email, .. } => {
        //                     if let Some(em) = email {
        //                         actor_email = Some(em)
        //                     } else {
        //                         actor_email = Some(label)
        //                     }
        //                 }
        //             };
        //         }
        //     }

        //     actions.insert(
        //         *action.id(),
        //         ActionView {
        //             id: *action.id(),
        //             action_prototype_id: *prototype.id(),
        //             kind,
        //             name: display_name.unwrap_or_else(|| match kind {
        //                 ActionKind::Create => "create".to_owned(),
        //                 ActionKind::Delete => "delete".to_owned(),
        //                 ActionKind::Other => "other".to_owned(),
        //                 ActionKind::Refresh => "refresh".to_owned(),
        //             }),
        //             component_id: *action.component_id(),
        //             actor: actor_email,
        //             parents,
        //         },
        //     );
        // }

        view.push(ChangeSetView {
            // TODO: remove change sets entirely!
            id: cs.id,
            pk: cs.id,
            name: cs.name,
            status: cs.status,
            merge_requested_at: None,           // cs.merge_requested_at,
            merge_requested_by_user_id: None,   // cs.merge_requested_by_user_id,
            abandon_requested_at: None,         // cs.abandon_requested_at,
            abandon_requested_by_user_id: None, // cs.abandon_requested_by_user_id,
        });
    }

    Ok(Json(view))
}
