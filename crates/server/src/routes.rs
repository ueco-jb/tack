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
    PatchStatus, StatusList,
};
use tack_core::model::{Item, Status, whole_seconds};
use tack_core::path::Target;
use tack_core::query::Query;
use tack_core::{Error, Result};

use crate::error::ApiError;
use crate::{Server, hosts, log};

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
        );
    Router::new()
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
            |tree, storage, ids| {
                let items: Vec<&Item> = ids
                    .iter()
                    .filter_map(|id| tree.get(*id).map(|n| &n.item))
                    .collect();
                storage.insert_items(&items)?;
                let last = ids
                    .last()
                    .ok_or_else(|| Error::internal("add created nothing"))?;
                tree.view(*last)
            },
        )
    })
    .await?;
    Ok((StatusCode::CREATED, Json(view)))
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
                let updated = tree.update(id, &patch, now())?;
                Ok((id, updated))
            },
            |tree, storage, (id, updated)| {
                let created: Vec<&Item> = updated
                    .created
                    .iter()
                    .filter_map(|c| tree.get(*c).map(|n| &n.item))
                    .collect();
                if !created.is_empty() {
                    storage.insert_items(&created)?;
                }
                if updated.changed {
                    storage.update_item(&tree.node(id)?.item)?;
                }
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
