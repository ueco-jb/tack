use std::sync::Arc;

use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{self, Json, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, patch};
use jiff::Timestamp;
use serde::Deserialize;
use tack_core::api::{
    API_PREFIX, CreateItem, CreateStatus, Deleted, Health, ItemList, ItemView, PatchItem,
    PatchStatus, PatchTag, StatusList, TagList,
};
use tack_core::model::{Item, ItemId, Status, Tag, whole_seconds};
use tack_core::path::Target;
use tack_core::query::Query;
use tack_core::tree::{Tree, Written};
use tack_core::{Error, Result};
use tack_storage::Storage;

use crate::error::ApiError;
use crate::{Server, hosts, log, web};

type ApiResult<T> = std::result::Result<T, ApiError>;

pub fn router(server: Arc<Server>) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/items", get(list).post(create))
        .route("/items/{*target}", get(show).patch(update).delete(remove))
        .route("/statuses", get(statuses).post(create_status))
        .route(
            "/statuses/{name}",
            patch(update_status).delete(remove_status),
        )
        .route("/tags", get(tags))
        .route("/tags/{name}", patch(update_tag).delete(remove_tag));
    Router::new()
        .route("/", get(web::index))
        .nest(API_PREFIX, api)
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(server.clone(), hosts::guard))
        .layer(middleware::from_fn_with_state(server.clone(), log::record))
        .with_state(server)
}

async fn blocking<T: Send + 'static>(
    server: &Arc<Server>,
    work: impl FnOnce(&Server) -> Result<T> + Send + 'static,
) -> Result<T> {
    let server = server.clone();
    tokio::task::spawn_blocking(move || work(&server))
        .await
        .map_err(|e| Error::internal(format!("worker failed: {e}")))?
}

fn now() -> Timestamp {
    whole_seconds(Timestamp::now())
}

fn target(raw: std::result::Result<extract::Path<String>, PathRejection>) -> ApiResult<Target> {
    let extract::Path(raw) = raw?;
    Ok(raw.parse()?)
}

async fn health() -> Json<Health> {
    Json(Health {
        ok: true,
        version: env!("CARGO_PKG_VERSION").to_owned(),
    })
}

async fn not_found() -> ApiError {
    ApiError(Error::not_found("no such endpoint"))
}

#[derive(Deserialize)]
struct ListParams {
    q: Option<String>,
}

async fn list(
    State(server): State<Arc<Server>>,
    params: std::result::Result<extract::Query<ListParams>, QueryRejection>,
) -> ApiResult<Json<ItemList>> {
    let extract::Query(params) = params?;
    let query = Query::parse(params.q.as_deref().unwrap_or(""))?;
    let tree = server.tree();
    let items = tree
        .list(&query)?
        .into_iter()
        .map(|id| tree.view(id))
        .collect::<Result<_>>()?;
    Ok(Json(ItemList { items }))
}

async fn show(
    State(server): State<Arc<Server>>,
    raw: std::result::Result<extract::Path<String>, PathRejection>,
) -> ApiResult<Json<ItemView>> {
    let target = target(raw)?;
    let tree = server.tree();
    let id = tree.find(&target)?.item.id;
    Ok(Json(tree.view(id)?))
}

async fn create(
    State(server): State<Arc<Server>>,
    body: std::result::Result<Json<CreateItem>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<ItemView>)> {
    let Json(req) = body?;
    let view = blocking(&server, move |server| {
        server.write(
            |tree| tree.add(&req, now()),
            |tree, storage, written| {
                save(tree, storage, &written)?;
                let last = written
                    .created
                    .last()
                    .ok_or_else(|| Error::internal("add created nothing"))?;
                tree.view(*last)
            },
        )
    })
    .await?;
    Ok((StatusCode::CREATED, Json(view)))
}

fn save(tree: &Tree, storage: &mut Storage, written: &Written) -> Result<()> {
    if written.created.is_empty() && written.updated.is_empty() {
        return Ok(());
    }
    let items = |ids: &[ItemId]| -> Vec<&Item> {
        ids.iter()
            .filter_map(|id| tree.get(*id).map(|n| &n.item))
            .collect()
    };
    storage.save(&items(&written.created), &items(&written.updated))
}

async fn update(
    State(server): State<Arc<Server>>,
    raw: std::result::Result<extract::Path<String>, PathRejection>,
    body: std::result::Result<Json<PatchItem>, JsonRejection>,
) -> ApiResult<Json<ItemView>> {
    let target = target(raw)?;
    let Json(patch) = body?;
    let view = blocking(&server, move |server| {
        server.write(
            |tree| {
                let id = tree.find(&target)?.item.id;
                let written = tree.update(id, &patch, now())?;
                Ok((id, written))
            },
            |tree, storage, (id, written)| {
                save(tree, storage, &written)?;
                tree.view(id)
            },
        )
    })
    .await?;
    Ok(Json(view))
}

#[derive(Deserialize)]
struct RemoveParams {
    #[serde(default)]
    recursive: bool,
}

async fn remove(
    State(server): State<Arc<Server>>,
    raw: std::result::Result<extract::Path<String>, PathRejection>,
    params: std::result::Result<extract::Query<RemoveParams>, QueryRejection>,
) -> ApiResult<Json<Deleted>> {
    let target = target(raw)?;
    let extract::Query(params) = params?;
    let deleted = blocking(&server, move |server| {
        server.write(
            |tree| {
                let id = tree.find(&target)?.item.id;
                tree.remove(id, params.recursive)
            },
            |_, storage, ids| {
                storage.delete_items(&ids)?;
                Ok(ids)
            },
        )
    })
    .await?;
    Ok(Json(Deleted { deleted }))
}

async fn statuses(State(server): State<Arc<Server>>) -> Json<StatusList> {
    Json(StatusList {
        statuses: server.tree().statuses().to_vec(),
    })
}

async fn create_status(
    State(server): State<Arc<Server>>,
    body: std::result::Result<Json<CreateStatus>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<Status>)> {
    let Json(req) = body?;
    let status = blocking(&server, move |server| {
        server.write(
            |tree| tree.add_status(&req),
            |tree, storage, id| {
                let status = tree.status(id)?.clone();
                storage.insert_status(&status)?;
                Ok(status)
            },
        )
    })
    .await?;
    Ok((StatusCode::CREATED, Json(status)))
}

async fn update_status(
    State(server): State<Arc<Server>>,
    name: std::result::Result<extract::Path<String>, PathRejection>,
    body: std::result::Result<Json<PatchStatus>, JsonRejection>,
) -> ApiResult<Json<Status>> {
    let extract::Path(name) = name?;
    let Json(patch) = body?;
    let status = blocking(&server, move |server| {
        server.write(
            |tree| tree.update_status(&name, &patch),
            |tree, storage, id| {
                let status = tree.status(id)?.clone();
                storage.update_status(&status)?;
                Ok(status)
            },
        )
    })
    .await?;
    Ok(Json(status))
}

async fn remove_status(
    State(server): State<Arc<Server>>,
    name: std::result::Result<extract::Path<String>, PathRejection>,
) -> ApiResult<StatusCode> {
    let extract::Path(name) = name?;
    blocking(&server, move |server| {
        server.write(
            |tree| tree.remove_status(&name),
            |_, storage, id| storage.delete_status(id),
        )
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn tags(State(server): State<Arc<Server>>) -> Json<TagList> {
    Json(TagList {
        tags: server.tree().tags(),
    })
}

async fn update_tag(
    State(server): State<Arc<Server>>,
    name: std::result::Result<extract::Path<String>, PathRejection>,
    body: std::result::Result<Json<PatchTag>, JsonRejection>,
) -> ApiResult<Json<Tag>> {
    let extract::Path(name) = name?;
    let Json(patch) = body?;
    let tag = blocking(&server, move |server| {
        server.write(
            |tree| tree.set_tag_color(&name, patch.color.as_deref()),
            |_, storage, tag| {
                storage.set_tag_color(&tag)?;
                Ok(tag)
            },
        )
    })
    .await?;
    Ok(Json(tag))
}

async fn remove_tag(
    State(server): State<Arc<Server>>,
    name: std::result::Result<extract::Path<String>, PathRejection>,
) -> ApiResult<StatusCode> {
    let extract::Path(name) = name?;
    blocking(&server, move |server| {
        server.write(
            |tree| tree.remove_tag(&name),
            |_, storage, name| storage.delete_tag(&name),
        )
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
