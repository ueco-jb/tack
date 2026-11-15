CREATE TABLE item (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id  INTEGER REFERENCES item (id),
    title      TEXT    NOT NULL,
    name       TEXT    GENERATED ALWAYS AS (replace(title, ' ', '_')) STORED,
    body       TEXT    NOT NULL DEFAULT '',
    status     TEXT    NOT NULL CHECK (status IN ('todo', 'doing', 'blocked', 'done')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    started_at INTEGER,
    done_at    INTEGER,
    archived_at INTEGER,
    due        TEXT
) STRICT;

CREATE INDEX item_parent ON item (parent_id);

-- sqlite treats NULL parents as distinct
CREATE UNIQUE INDEX item_sibling_name ON item (coalesce(parent_id, 0), name);

CREATE TABLE item_blocker (
    item_id    INTEGER NOT NULL REFERENCES item (id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    blocker_id INTEGER REFERENCES item (id) ON DELETE CASCADE,
    note       TEXT,
    CHECK ((blocker_id IS NULL) <> (note IS NULL)),
    PRIMARY KEY (item_id, position)
) STRICT, WITHOUT ROWID;

CREATE TABLE tag (
    id    INTEGER PRIMARY KEY,
    name  TEXT    NOT NULL UNIQUE,
    color TEXT
) STRICT;

CREATE TABLE item_tag (
    item_id INTEGER NOT NULL REFERENCES item (id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tag (id) ON DELETE CASCADE,
    PRIMARY KEY (item_id, tag_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE allowed_client (
    ip TEXT PRIMARY KEY
) STRICT, WITHOUT ROWID;
