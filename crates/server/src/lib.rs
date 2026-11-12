mod error;
mod hosts;
mod log;
mod mcp;
mod routes;
mod web;

#[cfg(test)]
mod tests;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use jiff::Timestamp;
use tack_core::api::{CreateItem, ItemView, PatchItem};
use tack_core::model::{Item, ItemId, whole_seconds};
use tack_core::path::Target;
use tack_core::query::Query;
use tack_core::tree::{Tree, Written};
use tack_core::{Error, Result};
use tack_storage::Storage;
use tokio::net::TcpListener;

pub use hosts::AllowedHosts;
pub use log::RequestLog;
pub use routes::router;

pub struct Config {
    pub addr: SocketAddr,
    pub db: PathBuf,
    pub request_log: PathBuf,
    pub allowed_hosts: Vec<String>,
}

pub struct Server {
    tree: RwLock<Tree>,
    storage: Mutex<Storage>,
    clients: RwLock<Vec<IpAddr>>,
    hosts: AllowedHosts,
    log: RequestLog,
}

impl Server {
    pub fn new(storage: Storage, hosts: AllowedHosts, log: RequestLog) -> Result<Server> {
        let mut clients = storage.allowed_clients()?;
        clients.sort();
        Ok(Server {
            tree: RwLock::new(storage.load()?),
            clients: RwLock::new(clients),
            storage: Mutex::new(storage),
            hosts,
            log,
        })
    }

    fn tree(&self) -> RwLockReadGuard<'_, Tree> {
        self.tree.read().unwrap_or_else(|_| poisoned())
    }

    fn storage(&self) -> MutexGuard<'_, Storage> {
        self.storage.lock().unwrap_or_else(|_| poisoned())
    }

    fn tree_mut(&self) -> RwLockWriteGuard<'_, Tree> {
        self.tree.write().unwrap_or_else(|_| poisoned())
    }

    fn write<T, R>(
        &self,
        change: impl FnOnce(&mut Tree) -> Result<T>,
        persist: impl FnOnce(&Tree, &mut Storage, T) -> Result<R>,
    ) -> Result<R> {
        let mut storage = self.storage();
        let mut tree = self.tree_mut();
        let changed = change(&mut tree)?;
        persist(&tree, &mut storage, changed).inspect_err(|_| match storage.load() {
            Ok(fresh) => *tree = fresh,
            Err(e) => {
                eprintln!("tackd: cannot reload after a failed write: {}", e.message);
                std::process::exit(1);
            }
        })
    }

    fn search(&self, query: &str) -> Result<Vec<ItemView>> {
        let query = Query::parse(query)?;
        let tree = self.tree();
        tree.list(&query)?
            .into_iter()
            .map(|id| tree.view(id))
            .collect()
    }

    fn show(&self, target: &Target) -> Result<ItemView> {
        let tree = self.tree();
        let id = tree.find(target)?.item.id;
        tree.view(id)
    }

    fn add(&self, req: &CreateItem) -> Result<ItemView> {
        self.write(
            |tree| tree.add(req, now()),
            |tree, storage, written| {
                save(tree, storage, &written)?;
                let last = written
                    .created
                    .last()
                    .ok_or_else(|| Error::internal("add created nothing"))?;
                tree.view(*last)
            },
        )
    }

    fn update(&self, target: &Target, patch: &PatchItem) -> Result<ItemView> {
        self.write(
            |tree| {
                let id = tree.find(target)?.item.id;
                let written = tree.update(id, patch, now())?;
                Ok((id, written))
            },
            |tree, storage, (id, written)| {
                save(tree, storage, &written)?;
                tree.view(id)
            },
        )
    }

    fn remove(&self, target: &Target, recursive: bool) -> Result<Vec<ItemId>> {
        self.write(
            |tree| {
                let id = tree.find(target)?.item.id;
                tree.remove(id, recursive)
            },
            |_, storage, ids| {
                storage.delete_items(&ids)?;
                Ok(ids)
            },
        )
    }

    fn clients(&self) -> Vec<IpAddr> {
        self.clients.read().unwrap_or_else(|_| poisoned()).clone()
    }

    fn allow(&self, ip: IpAddr) -> Result<Vec<IpAddr>> {
        let mut storage = self.storage();
        storage.allow_client(ip)?;
        let mut clients = self.clients.write().unwrap_or_else(|_| poisoned());
        if !clients.contains(&ip) {
            clients.push(ip);
            clients.sort();
        }
        Ok(clients.clone())
    }

    fn deny(&self, ip: IpAddr) -> Result<Vec<IpAddr>> {
        let mut storage = self.storage();
        if !storage.deny_client(ip)? {
            return Err(Error::not_found(format!("{ip} is not on the allowlist")));
        }
        let mut clients = self.clients.write().unwrap_or_else(|_| poisoned());
        clients.retain(|c| *c != ip);
        Ok(clients.clone())
    }
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

fn poisoned() -> ! {
    eprintln!("tackd: a request panicked while holding a lock; exiting so the service restarts");
    std::process::exit(1);
}

fn now() -> Timestamp {
    whole_seconds(Timestamp::now())
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

pub async fn serve(config: Config) -> Result<()> {
    let storage = Storage::open(&config.db)?;
    let log = RequestLog::open(&config.request_log)?;
    let hosts = AllowedHosts::new(
        AllowedHosts::system_hostname().as_deref(),
        &config.allowed_hosts,
    );
    let server = Arc::new(Server::new(storage, hosts, log)?);
    let listener = TcpListener::bind(config.addr)
        .await
        .map_err(|e| Error::internal(format!("cannot listen on {}: {e}", config.addr)))?;

    let clients = server.clients();
    let clients = if clients.is_empty() {
        "everyone, no allowlist".to_owned()
    } else {
        clients
            .iter()
            .map(IpAddr::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    eprintln!(
        "tackd: listening on {}, {} items, database {}, request log {}, allowed clients: {clients}",
        config.addr,
        server.tree().len(),
        config.db.display(),
        config.request_log.display(),
    );

    axum::serve(
        listener,
        router(server).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await
    .map_err(|e| Error::internal(format!("server failed: {e}")))
}

async fn shutdown() {
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate => {}
    }
}
