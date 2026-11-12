use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tack_storage::Storage;
use tempfile::TempDir;
use tower::ServiceExt;

use tack_core::api::API_PREFIX;

use crate::{AllowedHosts, RequestLog, Server, router};

const LAPTOP: &str = "192.168.1.20";
const PHONE: &str = "192.168.1.30";

struct Harness {
    server: Arc<Server>,
    app: Router,
    log_dir: TempDir,
}

impl Harness {
    fn new() -> Self {
        let log_dir = tempfile::tempdir().unwrap();
        let server = Arc::new(
            Server::new(
                Storage::open_in_memory().unwrap(),
                AllowedHosts::new(Some("pi"), &[]),
                RequestLog::open(&log_dir.path().join("requests.log")).unwrap(),
            )
            .unwrap(),
        );
        Self {
            app: router(server.clone()),
            server,
            log_dir,
        }
    }

    async fn send_from(
        &self,
        client: &str,
        method: Method,
        uri: &str,
        body: Option<Value>,
        headers: &[(header::HeaderName, &str)],
    ) -> (StatusCode, Value) {
        self.request(client, method, &format!("{API_PREFIX}{uri}"), body, headers)
            .await
    }

    async fn request(
        &self,
        client: &str,
        method: Method,
        uri: &str,
        body: Option<Value>,
        headers: &[(header::HeaderName, &str)],
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if !headers.iter().any(|(name, _)| name == header::HOST) {
            builder = builder.header(header::HOST, "pi:15470");
        }
        for (name, value) in headers {
            builder = builder.header(name, *value);
        }
        let mut req = match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let ip: IpAddr = client.parse().unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip, 50_000)));
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    async fn send(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.send_from(LAPTOP, method, uri, body, &[]).await
    }

    fn log_lines(&self) -> Vec<Value> {
        std::fs::read_to_string(self.log_dir.path().join("requests.log"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

fn kind(body: &Value) -> &str {
    body["error"]["kind"].as_str().unwrap()
}

#[tokio::test]
async fn allowlist_is_opt_in_and_only_changed_from_the_server_itself() {
    let h = Harness::new();
    const PI: &str = "127.0.0.1";
    for client in [LAPTOP, PHONE] {
        let (status, _) = h.send_from(client, Method::GET, "/health", None, &[]).await;
        assert_eq!(status, StatusCode::OK, "{client} without an allowlist");
    }

    let allow_laptop = Some(json!({ "ip": LAPTOP }));
    let (status, body) = h.send(Method::POST, "/clients", allow_laptop.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "only the server itself");
    assert_eq!(kind(&body), "forbidden");

    let (status, body) = h
        .send_from(PI, Method::POST, "/clients", allow_laptop, &[])
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "clients": [LAPTOP] }));

    let (status, _) = h.send(Method::GET, "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = h.send_from(PHONE, Method::GET, "/health", None, &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(kind(&body), "forbidden");
    let (status, _) = h.send_from(PI, Method::GET, "/health", None, &[]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the server itself is always allowed"
    );

    let stored = h.server.storage().allowed_clients().unwrap();
    assert_eq!(stored, [LAPTOP.parse::<IpAddr>().unwrap()]);

    let (status, _) = h
        .send_from(PI, Method::DELETE, &format!("/clients/{PHONE}"), None, &[])
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = h
        .send_from(PI, Method::DELETE, &format!("/clients/{LAPTOP}"), None, &[])
        .await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "clients": [] })));
    let (status, _) = h.send_from(PHONE, Method::GET, "/health", None, &[]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an emptied allowlist allows everyone"
    );
}

#[tokio::test]
async fn foreign_hosts_and_origins_are_forbidden() {
    let h = Harness::new();
    let (status, _) = h
        .send_from(
            LAPTOP,
            Method::GET,
            "/health",
            None,
            &[(header::HOST, "evil.example")],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let add = json!({"path": "home"});
    let (status, _) = h
        .send_from(
            LAPTOP,
            Method::POST,
            "/items",
            Some(add.clone()),
            &[(header::ORIGIN, "https://evil.example")],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .send_from(
            LAPTOP,
            Method::POST,
            "/items",
            Some(add),
            &[(header::ORIGIN, "http://pi:15470")],
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn items_by_id_or_path_with_error_codes() {
    let h = Harness::new();
    let (status, item) = h
        .send(
            Method::POST,
            "/items",
            Some(json!({"path": "Home/Weekly/Buy groceries", "tags": ["food"]})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(item["path"], "/home/weekly/buy_groceries");
    let id = item["id"].as_i64().unwrap();

    let (status, again) = h
        .send(
            Method::POST,
            "/items",
            Some(json!({"path": "home/weekly/buy groceries"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(again["path"], "/home/weekly/buy_groceries-2");

    let (_, list) = h.send(Method::GET, "/items?q=groc", None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);

    let (status, by_id) = h.send(Method::GET, &format!("/items/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, by_path) = h
        .send(Method::GET, "/items/home/weekly/buy_groceries", None)
        .await;
    assert_eq!(by_id, by_path);

    let (status, moved) = h
        .send(
            Method::PATCH,
            &format!("/items/{id}"),
            Some(json!({"path": "home/shopping"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(moved["path"], "/home/shopping");

    h.send(
        Method::POST,
        "/items",
        Some(json!({"path": "home/weekly/clean"})),
    )
    .await;
    let (status, body) = h
        .send(
            Method::PATCH,
            "/items/home/weekly",
            Some(json!({"status": "done"})),
        )
        .await;
    assert_eq!((status, kind(&body)), (StatusCode::CONFLICT, "conflict"));

    let (status, body) = h.send(Method::DELETE, "/items/home", None).await;
    assert_eq!((status, kind(&body)), (StatusCode::CONFLICT, "conflict"));
    let (status, deleted) = h
        .send(Method::DELETE, "/items/home?recursive=true", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["deleted"].as_array().unwrap().len(), 5);

    let (status, body) = h.send(Method::GET, &format!("/items/{id}"), None).await;
    assert_eq!((status, kind(&body)), (StatusCode::NOT_FOUND, "not_found"));

    let (status, body) = h
        .send(Method::POST, "/items", Some(json!({"nope": 1})))
        .await;
    assert_eq!(
        (status, kind(&body)),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid")
    );

    let (status, body) = h.send(Method::GET, "/nothing", None).await;
    assert_eq!((status, kind(&body)), (StatusCode::NOT_FOUND, "not_found"));
}

#[tokio::test]
async fn archive_hides_and_tags_keep_colors() {
    let h = Harness::new();
    h.send(
        Method::POST,
        "/items",
        Some(json!({"path": "home/weekly/clean", "tags": ["chores"]})),
    )
    .await;
    let (status, archived) = h
        .send(
            Method::PATCH,
            "/items/home/weekly",
            Some(json!({"archived": true})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(archived["archived_at"].is_string());
    let paths = |list: &Value| -> Vec<String> {
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["path"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, listed) = h.send(Method::GET, "/items", None).await;
    assert_eq!(paths(&listed), ["/home"]);
    let (_, archive) = h.send(Method::GET, "/items?q=is:archived", None).await;
    assert_eq!(paths(&archive), ["/home/weekly", "/home/weekly/clean"]);

    let (status, tag) = h
        .send(
            Method::PATCH,
            "/tags/chores",
            Some(json!({"color": "green"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tag, json!({"name": "chores", "color": "green"}));
    let (_, tags) = h.send(Method::GET, "/tags", None).await;
    assert_eq!(
        tags,
        json!({"tags": [{"name": "chores", "color": "green"}]})
    );
    let (status, body) = h
        .send(Method::PATCH, "/tags/nope", Some(json!({"color": "red"})))
        .await;
    assert_eq!((status, kind(&body)), (StatusCode::NOT_FOUND, "not_found"));

    let (status, _) = h.send(Method::DELETE, "/tags/chores", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, clean) = h.send(Method::GET, "/items/home/weekly/clean", None).await;
    assert_eq!(clean["tags"], json!([]));

    let loaded = h.server.storage().load().unwrap();
    assert_eq!(*h.server.tree(), loaded);
}

#[tokio::test]
async fn statuses_add_update_remove() {
    let h = Harness::new();
    let (status, created) = h
        .send(
            Method::POST,
            "/statuses",
            Some(json!({"name": "blocked", "category": "active"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["name"], "blocked");

    let (status, updated) = h
        .send(
            Method::PATCH,
            "/statuses/blocked",
            Some(json!({"label": "Blocked"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["label"], "Blocked");

    h.send(
        Method::POST,
        "/items",
        Some(json!({"path": "home", "status": "blocked"})),
    )
    .await;
    let (status, body) = h.send(Method::DELETE, "/statuses/blocked", None).await;
    assert_eq!((status, kind(&body)), (StatusCode::CONFLICT, "conflict"));

    h.send(Method::DELETE, "/items/home", None).await;
    let (status, _) = h.send(Method::DELETE, "/statuses/blocked", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn tree_matches_storage_after_api_writes() {
    let h = Harness::new();
    for path in [
        "/home/weekly/buy groceries",
        "/home/weekly/clean",
        "/work/report",
    ] {
        h.send(Method::POST, "/items", Some(json!({"path": path})))
            .await;
    }
    h.send(
        Method::PATCH,
        "/items/home/weekly/clean",
        Some(json!({"status": "doing", "tags": ["chores"], "path": "archive/2026/clean"})),
    )
    .await;
    h.send(Method::DELETE, "/items/work/report", None).await;

    let loaded = h.server.storage().load().unwrap();
    assert_eq!(*h.server.tree(), loaded);
}

#[tokio::test]
async fn every_request_except_get_is_logged_with_body_and_error() {
    let h = Harness::new();
    h.send(Method::POST, "/items", Some(json!({"path": "home"})))
        .await;
    h.send(Method::GET, "/items", None).await;
    h.send(
        Method::PATCH,
        "/items/nowhere",
        Some(json!({"status": "done"})),
    )
    .await;

    let lines = h.log_lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["method"], "POST");
    assert_eq!(lines[0]["client"], LAPTOP);
    assert_eq!(lines[0]["status"], 201);
    assert_eq!(lines[0]["body"], json!({"path": "home"}));
    assert_eq!(lines[0]["error"], Value::Null);
    assert_eq!(lines[1]["path"], format!("{API_PREFIX}/items/nowhere"));
    assert_eq!(lines[1]["status"], 404);
    assert_eq!(lines[1]["error"], "no item at /nowhere");
}

const MCP_VERSION: &str = "2025-11-25";

fn rpc(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params })
}

impl Harness {
    async fn mcp_with(&self, body: Value, headers: &[(&'static str, &str)]) -> (StatusCode, Value) {
        let headers: Vec<_> = headers
            .iter()
            .map(|(name, value)| (header::HeaderName::from_static(name), *value))
            .collect();
        self.request(LAPTOP, Method::POST, "/mcp", Some(body), &headers)
            .await
    }

    async fn mcp(&self, method: &str, params: Value) -> (StatusCode, Value) {
        self.mcp_with(
            rpc(method, params),
            &[("mcp-protocol-version", MCP_VERSION)],
        )
        .await
    }

    async fn tool(&self, name: &str, arguments: Value) -> Value {
        let (status, body) = self
            .mcp(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["result"].clone()
    }
}

#[tokio::test]
async fn mcp_handshake_and_tool_list() {
    let h = Harness::new();
    let initialize = rpc(
        "initialize",
        json!({
            "protocolVersion": "2099-01-01",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "1" },
        }),
    );
    let (status, body) = h.mcp_with(initialize, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], 7);
    assert_eq!(body["result"]["protocolVersion"], MCP_VERSION);
    assert_eq!(body["result"]["capabilities"], json!({ "tools": {} }));
    assert_eq!(body["result"]["serverInfo"]["name"], "tack");

    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let (status, _) = h
        .mcp_with(initialized, &[("mcp-protocol-version", MCP_VERSION)])
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let (status, body) = h.mcp("ping", json!({})).await;
    assert_eq!(
        (status, body["result"].clone()),
        (StatusCode::OK, json!({}))
    );

    let (status, body) = h.mcp("tools/list", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<_> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["search", "get", "add", "update", "delete", "labels"]
    );
}

#[tokio::test]
async fn mcp_tools_share_the_rules_of_the_api() {
    let h = Harness::new();
    let added = h
        .tool(
            "add",
            json!({ "path": "/home/weekly/laundry", "tags": ["chores"], "due": "2026-12-01" }),
        )
        .await;
    assert_eq!(added["isError"], false);
    assert_eq!(added["structuredContent"]["path"], "/home/weekly/laundry");
    let text: Value = serde_json::from_str(added["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, added["structuredContent"]);

    let again = h
        .tool("add", json!({ "path": "/home/weekly/laundry" }))
        .await;
    assert_eq!(again["structuredContent"]["path"], "/home/weekly/laundry-2");

    let found = h
        .tool("search", json!({ "query": "/home/weekly +chores" }))
        .await;
    let items = found["structuredContent"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["due"], "2026-12-01");

    let refused = h
        .tool(
            "update",
            json!({ "item": "/home/weekly", "status": "done" }),
        )
        .await;
    assert_eq!(refused["isError"], true);
    assert!(
        refused["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("open items")
    );

    let forced = h
        .tool(
            "update",
            json!({ "item": "/home/weekly", "status": "done", "force": true, "due": null }),
        )
        .await;
    assert_eq!(forced["structuredContent"]["status"], "done");

    let item = h
        .tool("get", json!({ "item": "/home/weekly/laundry" }))
        .await;
    let id = item["structuredContent"]["id"].as_i64().unwrap();
    let by_id = h.tool("get", json!({ "item": id })).await;
    assert_eq!(by_id["structuredContent"], item["structuredContent"]);

    let labels = h.tool("labels", json!({})).await;
    assert_eq!(labels["structuredContent"]["tags"][0]["name"], "chores");
    assert_eq!(labels["structuredContent"]["statuses"][1]["name"], "doing");

    let blocked = h.tool("delete", json!({ "item": "/home" })).await;
    assert_eq!(blocked["isError"], true);
    let deleted = h
        .tool("delete", json!({ "item": "/home", "recursive": true }))
        .await;
    assert_eq!(
        deleted["structuredContent"]["deleted"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    let bad = h
        .tool("add", json!({ "path": "/x", "colour": "red" }))
        .await;
    assert_eq!(bad["isError"], true);

    let (status, body) = h
        .mcp("tools/call", json!({ "name": "nope", "arguments": {} }))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32602);

    let loaded = h.server.storage().load().unwrap();
    assert_eq!(*h.server.tree(), loaded);
}

#[tokio::test]
async fn mcp_rejects_other_versions_unknown_methods_and_foreign_origins() {
    let h = Harness::new();
    let (status, body) = h
        .mcp_with(
            rpc("server/discover", json!({})),
            &[("mcp-protocol-version", "2026-07-28")],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "clients fall back on 400");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains(MCP_VERSION)
    );

    let (status, body) = h.mcp("resources/list", json!({})).await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (StatusCode::OK, json!(-32601))
    );

    let (status, _) = h.request(LAPTOP, Method::GET, "/mcp", None, &[]).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    let (status, _) = h
        .request(
            LAPTOP,
            Method::POST,
            "/mcp",
            Some(rpc("ping", json!({}))),
            &[(header::ORIGIN, "https://evil.example")],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
