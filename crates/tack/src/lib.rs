use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tack_core::api::{API_PREFIX, ErrorBody};
use tack_core::{Error, Result};
use ureq::http::Response;
use ureq::{Agent, Body};

pub struct Client {
    agent: Agent,
    base: String,
}

impl Client {
    pub fn new(url: &str) -> Client {
        let agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into();
        Client {
            agent,
            base: format!("{}{API_PREFIX}", url.trim_end_matches('/')),
        }
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let mut req = self.agent.get(self.url(path));
        for (key, value) in query {
            req = req.query(*key, *value);
        }
        self.read(req.call())
    }

    pub fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.read(self.agent.post(self.url(path)).send_json(body))
    }

    pub fn patch<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.read(self.agent.patch(self.url(path)).send_json(body))
    }

    pub fn delete<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let mut req = self.agent.delete(self.url(path));
        for (key, value) in query {
            req = req.query(*key, *value);
        }
        self.read(req.call())
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn read<T: DeserializeOwned>(
        &self,
        sent: std::result::Result<Response<Body>, ureq::Error>,
    ) -> Result<T> {
        let mut response =
            sent.map_err(|e| Error::internal(format!("cannot reach {}: {e}", self.base)))?;
        let status = response.status();
        if status.is_success() {
            return response
                .body_mut()
                .with_config()
                .limit(u64::MAX)
                .read_json()
                .map_err(|e| Error::internal(format!("unreadable response: {e}")));
        }
        match response.body_mut().read_json::<ErrorBody>() {
            Ok(ErrorBody { error }) => Err(Error::new(error.kind, error.message)),
            Err(_) => Err(Error::internal(format!("server answered {status}"))),
        }
    }
}

pub fn item_path(item: &str) -> String {
    let mut out = String::from("/items/");
    for (i, byte) in item.trim().bytes().enumerate() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || (byte == b'/' && i > 0) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::item_path;

    #[test]
    fn a_leading_slash_keeps_numeric_paths_apart_from_ids() {
        assert_eq!(item_path("/2026"), "/items/%2F2026");
        assert_eq!(item_path("2026"), "/items/2026");
        assert_eq!(item_path("work/fix login"), "/items/work/fix%20login");
    }
}
