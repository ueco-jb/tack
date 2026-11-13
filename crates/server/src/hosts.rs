use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tack_core::{Error, Result};

use crate::error::ApiError;
use crate::{Server, poisoned};

#[derive(Clone, Debug)]
pub struct AllowedHosts {
    names: Vec<String>,
}

impl AllowedHosts {
    pub fn new(hostname: Option<&str>, extra: &[String]) -> Self {
        let mut names = vec!["localhost".to_owned()];
        if let Some(h) = hostname.map(str::trim).filter(|h| !h.is_empty()) {
            let h = h.to_lowercase();
            names.push(format!("{h}.local"));
            names.push(h);
        }
        names.extend(extra.iter().map(|e| e.trim().to_lowercase()));
        names.sort();
        names.dedup();
        Self { names }
    }

    pub fn system_hostname() -> Option<String> {
        std::fs::read_to_string("/proc/sys/kernel/hostname")
            .ok()
            .map(|h| h.trim().to_owned())
    }

    pub fn allows_host(&self, host: &str) -> bool {
        let host = strip_port(host.trim()).to_lowercase();
        let host = host.strip_suffix('.').unwrap_or(&host);
        host.parse::<IpAddr>().is_ok() || self.names.iter().any(|n| n == host)
    }
}

fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split_once(']').map_or(rest, |(ip, _)| ip);
    }
    match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    }
}

pub async fn guard(State(server): State<Arc<Server>>, req: Request, next: Next) -> Response {
    let checked = client_ip(&req)
        .and_then(|client| check_client(&server, client))
        .and_then(|()| check_headers(&server, &req));
    match checked {
        Ok(()) => next.run(req).await,
        Err(e) => ApiError(e).into_response(),
    }
}

fn check_client(server: &Server, client: IpAddr) -> Result<()> {
    let clients = server.clients.read().unwrap_or_else(|_| poisoned());
    if client.is_loopback() || clients.is_empty() || clients.contains(&client) {
        Ok(())
    } else {
        Err(Error::forbidden(format!("client {client} is not allowed")))
    }
}

fn check_headers(server: &Server, req: &Request) -> Result<()> {
    let headers = req.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()));
    if !host.is_some_and(|h| server.hosts.allows_host(h)) {
        return Err(Error::forbidden(format!(
            "host {} is not allowed",
            host.unwrap_or("(missing)")
        )));
    }

    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe
        && let Some(origin) = headers.get(header::ORIGIN)
        && !origin
            .to_str()
            .is_ok_and(|o| host.is_some_and(|h| same_origin(o, h)))
    {
        return Err(Error::forbidden("cross origin request rejected"));
    }
    Ok(())
}

fn same_origin(origin: &str, host: &str) -> bool {
    match origin.trim().split_once("://") {
        Some(("http" | "https", authority)) => authority.eq_ignore_ascii_case(host.trim()),
        _ => false,
    }
}

pub fn client_ip(req: &Request) -> Result<IpAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_canonical())
        .ok_or_else(|| Error::internal("connection has no peer address"))
}

pub fn require_loopback(peer: SocketAddr) -> Result<()> {
    if peer.ip().to_canonical().is_loopback() {
        Ok(())
    } else {
        Err(Error::forbidden(
            "the allowlist can only be changed on the server itself",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> AllowedHosts {
        AllowedHosts::new(Some("Pi"), &["tack.lan".into()])
    }

    #[test]
    fn hosts_names_ips_and_ports() {
        let h = hosts();
        for ok in [
            "localhost:15470",
            "pi",
            "PI.local:15470",
            "tack.lan.",
            "192.168.1.5:15470",
            "[::1]:15470",
            "[fe80::1]",
        ] {
            assert!(h.allows_host(ok), "{ok} should be allowed");
        }
        for bad in [
            "evil.example",
            "pi.evil.example:15470",
            "localhost.evil",
            "",
        ] {
            assert!(!h.allows_host(bad), "{bad} should be rejected");
        }
    }

    #[test]
    fn origin_must_match_the_requested_host() {
        assert!(same_origin("http://pi:15470", "pi:15470"));
        assert!(same_origin("http://PI:15470", "pi:15470"));
        assert!(same_origin("http://192.168.1.5:15470", "192.168.1.5:15470"));
        assert!(!same_origin("http://203.0.113.5", "192.168.1.5:15470"));
        assert!(!same_origin("https://evil.example", "pi:15470"));
        assert!(!same_origin("null", "pi:15470"));
        assert!(!same_origin("file://", "pi:15470"));
        assert!(!same_origin("http://pi:15470/path", "pi:15470"));
    }
}
