use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jiff::Timestamp;
use serde_json::{Value, json};
use tack_core::Error;
use tack_core::api::ErrorBody;

use crate::Server;
use crate::error::ApiError;
use crate::hosts::client_ip;

const MAX_BODY_BYTES: usize = 1 << 20;

pub struct RequestLog {
    file: Mutex<File>,
}

impl RequestLog {
    pub fn open(path: &Path) -> tack_core::Result<RequestLog> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::internal(format!("cannot create {}: {e}", dir.display())))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| Error::internal(format!("cannot open {}: {e}", path.display())))?;
        Ok(RequestLog {
            file: Mutex::new(file),
        })
    }

    fn append(&self, entry: &Value) {
        let mut line = entry.to_string();
        line.push('\n');
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = file.write_all(line.as_bytes()) {
            eprintln!("tackd: cannot write request log: {e}");
        }
    }
}

pub async fn record(State(server): State<Arc<Server>>, req: Request, next: Next) -> Response {
    if req.method() == Method::GET {
        return next.run(req).await;
    }
    let started = Instant::now();
    let client = client_ip(&req).ok().map(|ip| ip.to_string());
    let method = req.method().to_string();
    let path = req.uri().to_string();

    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return ApiError(Error::invalid(format!(
                "request body is larger than {MAX_BODY_BYTES} bytes"
            )))
            .into_response();
        }
    };
    let logged_body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };

    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    let status = response.status();

    let (response, error) = if status.is_client_error() || status.is_server_error() {
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, MAX_BODY_BYTES).await.unwrap_or_default();
        let message = serde_json::from_slice::<ErrorBody>(&bytes)
            .map(|b| b.error.message)
            .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned());
        (
            Response::from_parts(parts, Body::from(bytes)),
            Some(message),
        )
    } else {
        (response, None)
    };

    server.log.append(&json!({
        "at": Timestamp::now().to_string(),
        "client": client,
        "method": method,
        "path": path,
        "status": status.as_u16(),
        "ms": started.elapsed().as_millis() as u64,
        "body": logged_body,
        "error": error,
    }));
    response
}
