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
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("{API_PREFIX}{uri}"));
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
async fn first_client_is_adopted_and_every_other_is_forbidden() {
    let h = Harness::new();
    let (status, _) = h.send(Method::GET, "/health", None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = h.send_from(PHONE, Method::GET, "/health", None, &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(kind(&body), "forbidden");

    let (status, _) = h
        .send_from("127.0.0.1", Method::GET, "/health", None, &[])
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "loopback is not special");

    let stored = h.server.storage().allowed_clients().unwrap();
    assert_eq!(stored, [LAPTOP.parse::<IpAddr>().unwrap()]);
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
