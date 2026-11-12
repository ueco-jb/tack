use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tack_core::model::{Category, Item, ItemId, Status, StatusId};
use tack_core::tree::Tree;
use tack_core::{Error, Result};

const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Storage> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::internal(format!("cannot create {}: {e}", dir.display())))?;
        }
        let conn = Connection::open(path).map_err(db)?;
        let journal: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(db)?;
        if journal != "wal" {
            return Err(Error::internal(format!(
                "could not enable WAL on {}, journal mode is {journal}",
                path.display()
            )));
        }
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Storage> {
        Self::init(Connection::open_in_memory().map_err(db)?)
    }

    fn init(conn: Connection) -> Result<Storage> {
        conn.execute_batch(
            "PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;",
        )
        .map_err(db)?;
        let mut storage = Storage { conn };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&mut self) -> Result<()> {
        let version = self.schema_version()?;
        let known = MIGRATIONS.len() as i64;
        if version > known {
            return Err(Error::internal(format!(
                "database schema version {version} is newer than this tack knows ({known}); upgrade tack"
            )));
        }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = self.conn.transaction().map_err(db)?;
            tx.execute_batch(sql).map_err(db)?;
            tx.pragma_update(None, "user_version", index as i64 + 1)
                .map_err(db)?;
            tx.commit().map_err(db)?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db)
    }

    pub fn load(&self) -> Result<Tree> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, label, category, position FROM status")
            .map_err(db)?;
        let statuses = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(db)?
            .map(|row| {
                let (id, name, label, category, position) = row.map_err(db)?;
                Ok(Status {
                    id: StatusId(id),
                    name,
                    label,
                    category: category.parse::<Category>()?,
                    position,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT item_tag.item_id, tag.name FROM item_tag \
                 JOIN tag ON tag.id = item_tag.tag_id ORDER BY tag.name",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db)?;
        for row in rows {
            let (item_id, name) = row.map_err(db)?;
            tags.entry(item_id).or_default().push(name);
        }

        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, parent_id, title, body, status_id, \
                 created_at, updated_at, started_at, done_at FROM item",
            )
            .map_err(db)?;
        let items = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                ))
            })
            .map_err(db)?
            .map(|row| {
                let (id, parent, title, body, status, created, updated, started, done) =
                    row.map_err(db)?;
                Ok(Item {
                    id: ItemId(id),
                    parent: parent.map(ItemId),
                    title,
                    body,
                    status: StatusId(status),
                    tags: tags.remove(&id).unwrap_or_default(),
                    created_at: from_secs(created)?,
                    updated_at: from_secs(updated)?,
                    started_at: started.map(from_secs).transpose()?,
                    done_at: done.map(from_secs).transpose()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let last_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'item'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(db)?;

        Tree::new(statuses, items, last_id.unwrap_or(0) + 1)
    }

    pub fn insert_items(&mut self, items: &[&Item]) -> Result<()> {
        let tx = self.conn.transaction().map_err(db)?;
        for item in items {
            tx.execute(
                "INSERT INTO item (id, parent_id, title, body, status_id, \
                 created_at, updated_at, started_at, done_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    item.id.0,
                    item.parent.map(|p| p.0),
                    item.title,
                    item.body,
                    item.status.0,
                    item.created_at.as_second(),
                    item.updated_at.as_second(),
                    item.started_at.map(|t| t.as_second()),
                    item.done_at.map(|t| t.as_second()),
                ],
            )
            .map_err(db)?;
            write_tags(&tx, item)?;
        }
        tx.commit().map_err(db)
    }

    pub fn update_item(&mut self, item: &Item) -> Result<()> {
        let tx = self.conn.transaction().map_err(db)?;
        let updated = tx
            .execute(
                "UPDATE item SET parent_id = ?2, title = ?3, body = ?4, status_id = ?5, \
                 updated_at = ?6, started_at = ?7, done_at = ?8 WHERE id = ?1",
                params![
                    item.id.0,
                    item.parent.map(|p| p.0),
                    item.title,
                    item.body,
                    item.status.0,
                    item.updated_at.as_second(),
                    item.started_at.map(|t| t.as_second()),
                    item.done_at.map(|t| t.as_second()),
                ],
            )
            .map_err(db)?;
        if updated != 1 {
            return Err(Error::internal(format!(
                "item {} is not in the database",
                item.id
            )));
        }
        tx.execute("DELETE FROM item_tag WHERE item_id = ?1", [item.id.0])
            .map_err(db)?;
        write_tags(&tx, item)?;
        delete_unused_tags(&tx)?;
        tx.commit().map_err(db)
    }

    pub fn delete_items(&mut self, ids: &[ItemId]) -> Result<()> {
        let tx = self.conn.transaction().map_err(db)?;
        {
            let mut stmt = tx.prepare("DELETE FROM item WHERE id = ?1").map_err(db)?;
            for id in ids {
                stmt.execute([id.0]).map_err(db)?;
            }
        }
        delete_unused_tags(&tx)?;
        tx.commit().map_err(db)
    }

    pub fn insert_status(&mut self, status: &Status) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO status (id, name, label, category, position) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    status.id.0,
                    status.name,
                    status.label,
                    status.category.as_str(),
                    status.position
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn update_status(&mut self, status: &Status) -> Result<()> {
        self.conn
            .execute(
                "UPDATE status SET name = ?2, label = ?3, category = ?4, position = ?5 WHERE id = ?1",
                params![
                    status.id.0,
                    status.name,
                    status.label,
                    status.category.as_str(),
                    status.position
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn delete_status(&mut self, id: StatusId) -> Result<()> {
        self.conn
            .execute("DELETE FROM status WHERE id = ?1", [id.0])
            .map_err(db)?;
        Ok(())
    }
    pub fn allowed_clients(&self) -> Result<Vec<IpAddr>> {
        let mut stmt = self
            .conn
            .prepare("SELECT ip FROM allowed_client ORDER BY ip")
            .map_err(db)?;
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map_err(db)?
            .map(|row| {
                let ip = row.map_err(db)?;
                ip.parse().map_err(|_| {
                    Error::internal(format!("stored client address {ip:?} is not an IP address"))
                })
            })
            .collect()
    }

    pub fn allow_client(&mut self, ip: IpAddr) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO allowed_client (ip) VALUES (?1)",
                [ip.to_string()],
            )
            .map_err(db)?;
        Ok(())
    }
}

fn write_tags(tx: &Transaction<'_>, item: &Item) -> Result<()> {
    for tag in &item.tags {
        tx.execute("INSERT OR IGNORE INTO tag (name) VALUES (?1)", [tag])
            .map_err(db)?;
        tx.execute(
            "INSERT INTO item_tag (item_id, tag_id) SELECT ?1, id FROM tag WHERE name = ?2",
            params![item.id.0, tag],
        )
        .map_err(db)?;
    }
    Ok(())
}

fn delete_unused_tags(tx: &Transaction<'_>) -> Result<()> {
    tx.execute(
        "DELETE FROM tag WHERE id NOT IN (SELECT tag_id FROM item_tag)",
        [],
    )
    .map_err(db)?;
    Ok(())
}

fn from_secs(secs: i64) -> Result<Timestamp> {
    Timestamp::from_second(secs)
        .map_err(|e| Error::internal(format!("stored timestamp {secs} is out of range: {e}")))
}

fn db(e: rusqlite::Error) -> Error {
    if let rusqlite::Error::SqliteFailure(failure, _) = &e
        && matches!(
            failure.extended_code,
            rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE | rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
        )
    {
        return Error::conflict(format!("rejected by the database: {e}"));
    }
    Error::internal(format!("database error: {e}"))
}
