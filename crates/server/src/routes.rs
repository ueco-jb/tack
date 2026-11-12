use std::sync::Arc;

use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{self, Json, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, patch, post};
use serde::Deserialize;
use tack_core::Error;
use tack_core::api::{
    API_PREFIX, CreateItem, CreateStatus, Deleted, Health, ItemList, ItemView, PatchItem,
    PatchStatus, PatchTag, StatusList, TagList,
};
use tack_core::model::{Status, Tag};
use tack_core::path::Target;

use crate::error::ApiError;
use crate::{Server, blocking, hosts, log, mcp, web};

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
        .route(
            "/mcp",
            post(mcp::endpoint)
                .get(mcp::not_allowed)
                .delete(mcp::not_allowed),
        )
        .nest(API_PREFIX, api)
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(server.clone(), hosts::guard))
        .layer(middleware::from_fn_with_state(server.clone(), log::record))
        .with_state(server)
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
    let items = server.search(params.q.as_deref().unwrap_or(""))?;
    Ok(Json(ItemList { items }))
}

async fn show(
    State(server): State<Arc<Server>>,
    raw: std::result::Result<extract::Path<String>, PathRejection>,
) -> ApiResult<Json<ItemView>> {
    let target = target(raw)?;
    Ok(Json(server.show(&target)?))
}

async fn create(
    State(server): State<Arc<Server>>,
    body: std::result::Result<Json<CreateItem>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<ItemView>)> {
    let Json(req) = body?;
    let view = blocking(&server, move |server| server.add(&req)).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn update(
    State(server): State<Arc<Server>>,
    raw: std::result::Result<extract::Path<String>, PathRejection>,
    body: std::result::Result<Json<PatchItem>, JsonRejection>,
) -> ApiResult<Json<ItemView>> {
    let target = target(raw)?;
    let Json(patch) = body?;
    let view = blocking(&server, move |server| server.update(&target, &patch)).await?;
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
        server.remove(&target, params.recursive)
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
