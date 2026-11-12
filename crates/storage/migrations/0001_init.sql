CREATE TABLE status (
    id       INTEGER PRIMARY KEY,
    name     TEXT    NOT NULL UNIQUE,
    label    TEXT    NOT NULL,
    category TEXT    NOT NULL CHECK (category IN ('todo', 'active', 'done')),
    position INTEGER NOT NULL
) STRICT;

CREATE TABLE item (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id  INTEGER REFERENCES item (id),
    title      TEXT    NOT NULL,
    name       TEXT    GENERATED ALWAYS AS (replace(title, ' ', '_')) STORED,
    body       TEXT    NOT NULL DEFAULT '',
    status_id  INTEGER NOT NULL REFERENCES status (id),
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

INSERT INTO status (name, label, category, position) VALUES
    ('todo', 'To do', 'todo', 1),
    ('doing', 'Doing', 'active', 2),
    ('done', 'Done', 'done', 3);
