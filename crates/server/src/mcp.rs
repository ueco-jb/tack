use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tack_core::api::{CreateItem, PatchItem};
use tack_core::path::Target;
use tack_core::{Error, Result};

use crate::{Server, blocking};

const VERSION: &str = "2025-11-25";
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "tack: todo tree addressed by paths like /coding/project-x/milestone. Any item can have children. Search before adding; reuse existing paths and tags. Bodies very short, technical, dry: no filler, no restating the title. Change status, archive or delete only when asked.";

pub async fn not_allowed() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

pub async fn endpoint(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(e) => {
            return error(
                StatusCode::BAD_REQUEST,
                None,
                PARSE_ERROR,
                format!("invalid JSON: {e}"),
            );
        }
    };
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return error(
            StatusCode::BAD_REQUEST,
            message.get("id").cloned(),
            INVALID_REQUEST,
            "jsonrpc must be \"2.0\"".into(),
        );
    }
    let Some(id) = message.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    if !(id.is_string() || id.is_i64() || id.is_u64()) {
        return error(
            StatusCode::BAD_REQUEST,
            None,
            INVALID_REQUEST,
            "id must be a string or an integer".into(),
        );
    }
    let version = headers
        .get("mcp-protocol-version")
        .map(|v| v.to_str().unwrap_or_default());
    if let Some(version) = version.filter(|v| *v != VERSION) {
        return error(
            StatusCode::BAD_REQUEST,
            Some(id),
            INVALID_REQUEST,
            format!("unsupported protocol version {version}, tack supports {VERSION}"),
        );
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return error(
            StatusCode::BAD_REQUEST,
            Some(id),
            INVALID_REQUEST,
            "missing method".into(),
        );
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "initialize" => reply(
            id,
            json!({
                "protocolVersion": VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tack", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }),
        ),
        "ping" => reply(id, json!({})),
        "tools/list" => reply(id, json!({ "tools": tools() })),
        "tools/call" => call(server, id, params).await,
        other => error(
            StatusCode::OK,
            Some(id),
            METHOD_NOT_FOUND,
            format!("method {other} is not supported"),
        ),
    }
}

fn reply(id: Value, result: Value) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn error(status: StatusCode, id: Option<Value>, code: i64, message: String) -> Response {
    let mut body = json!({ "jsonrpc": "2.0", "error": { "code": code, "message": message } });
    if let Some(id) = id {
        body["id"] = id;
    }
    (status, Json(body)).into_response()
}

#[derive(Deserialize)]
struct CallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Deserialize)]
struct SearchArgs {
    #[serde(default)]
    query: String,
}

#[derive(Deserialize)]
struct ItemArgs {
    item: Target,
}

#[derive(Deserialize)]
struct UpdateArgs {
    item: Target,
    #[serde(flatten)]
    patch: PatchItem,
}

#[derive(Deserialize)]
struct DeleteArgs {
    item: Target,
    #[serde(default)]
    recursive: bool,
}

async fn call(server: Arc<Server>, id: Value, params: Value) -> Response {
    let call: CallParams = match serde_json::from_value(params) {
        Ok(call) => call,
        Err(e) => {
            return error(
                StatusCode::OK,
                Some(id),
                INVALID_PARAMS,
                format!("invalid tools/call params: {e}"),
            );
        }
    };
    let arguments = if call.arguments.is_null() {
        json!({})
    } else {
        call.arguments
    };
    if let Some(unknown) = unknown_argument(&call.name, &arguments) {
        return reply(
            id,
            json!({
                "content": [{ "type": "text", "text": format!("unknown argument {unknown} for {}", call.name) }],
                "isError": true,
            }),
        );
    }
    let outcome = match call.name.as_str() {
        "search" => match args::<SearchArgs>(arguments) {
            Ok(a) => server.search(&a.query).map(
                |items| json!({ "items": items.into_iter().map(summary).collect::<Vec<_>>() }),
            ),
            Err(e) => Err(e),
        },
        "get" => match args::<ItemArgs>(arguments) {
            Ok(a) => server.show(&a.item).map(|item| json!(item)),
            Err(e) => Err(e),
        },
        "add" => match args::<CreateItem>(arguments) {
            Ok(req) => blocking(&server, move |s| s.add(&req))
                .await
                .map(|item| json!(item)),
            Err(e) => Err(e),
        },
        "update" => match args::<UpdateArgs>(arguments) {
            Ok(a) => blocking(&server, move |s| s.update(&a.item, &a.patch))
                .await
                .map(|item| json!(item)),
            Err(e) => Err(e),
        },
        "delete" => match args::<DeleteArgs>(arguments) {
            Ok(a) => blocking(&server, move |s| s.remove(&a.item, a.recursive))
                .await
                .map(|deleted| json!({ "deleted": deleted })),
            Err(e) => Err(e),
        },
        "labels" => {
            let tree = server.tree();
            Ok(json!({
                "statuses": tree.statuses().iter().map(|s| json!({
                    "name": s.name, "label": s.label, "category": s.category,
                })).collect::<Vec<_>>(),
                "tags": tree.tags(),
            }))
        }
        other => {
            return error(
                StatusCode::OK,
                Some(id),
                INVALID_PARAMS,
                format!("unknown tool {other}"),
            );
        }
    };
    match outcome {
        Ok(value) => reply(
            id,
            json!({
                "content": [{ "type": "text", "text": value.to_string() }],
                "structuredContent": value,
                "isError": false,
            }),
        ),
        Err(e) => reply(
            id,
            json!({
                "content": [{ "type": "text", "text": e.message }],
                "isError": true,
            }),
        ),
    }
}

fn unknown_argument(tool: &str, arguments: &Value) -> Option<String> {
    let tools = tools();
    let schema = tools.as_array()?.iter().find(|t| t["name"] == tool)?;
    let known = schema["inputSchema"]["properties"].as_object();
    arguments
        .as_object()?
        .keys()
        .find(|key| known.is_none_or(|k| !k.contains_key(*key)))
        .cloned()
}

fn args<T: DeserializeOwned>(arguments: Value) -> Result<T> {
    serde_json::from_value(arguments).map_err(|e| Error::invalid(format!("invalid arguments: {e}")))
}

fn summary(item: tack_core::api::ItemView) -> Value {
    json!({
        "id": item.id,
        "path": item.path,
        "status": item.status,
        "tags": item.tags,
        "due": item.due,
        "done": item.done_at.is_some(),
        "archived": item.archived,
    })
}

fn tools() -> Value {
    let item = json!({
        "type": ["string", "integer"],
        "description": "Path like /coding/project-x/milestone, or numeric id.",
    });
    json!([
        {
            "name": "search",
            "description": "List items matching a query. Words match the path; /a/b/ matches that item and its subtree, /a/b any path starting with it; -word excludes; +tag; todo, doing, done filter status; is:archived, is:all. Empty query: everything not archived.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Example: /coding/project-x +bug todo" }
                },
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true },
        },
        {
            "name": "get",
            "description": "One item with body, status, tags, due and timestamps.",
            "inputSchema": {
                "type": "object",
                "properties": { "item": item },
                "required": ["item"],
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true },
        },
        {
            "name": "add",
            "description": "Add an item. Missing parents are created. A taken name gets a suffix: milestone-2.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Full path." },
                    "body": { "type": "string", "description": "Very short, technical, dry." },
                    "status": { "type": "string", "description": "Default todo." },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "due": { "type": "string", "format": "date", "description": "YYYY-MM-DD." },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        },
        {
            "name": "update",
            "description": "Change only the given fields. path moves; tags replaces the list; due null clears; archived hides the subtree. done with open children needs force.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "item": item,
                    "path": { "type": "string" },
                    "body": { "type": "string", "description": "Very short, technical, dry." },
                    "status": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "due": { "type": ["string", "null"], "format": "date", "description": "YYYY-MM-DD or null." },
                    "archived": { "type": "boolean" },
                    "force": { "type": "boolean" },
                },
                "required": ["item"],
                "additionalProperties": false,
            },
        },
        {
            "name": "delete",
            "description": "Delete permanently. Children require recursive. Prefer archiving.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "item": item,
                    "recursive": { "type": "boolean" },
                },
                "required": ["item"],
                "additionalProperties": false,
            },
            "annotations": { "destructiveHint": true },
        },
        {
            "name": "labels",
            "description": "Existing statuses and tags.",
            "inputSchema": { "type": "object", "additionalProperties": false },
            "annotations": { "readOnlyHint": true },
        },
    ])
}
