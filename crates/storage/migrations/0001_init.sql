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
    priority   INTEGER NOT NULL DEFAULT 0 CHECK (priority BETWEEN 0 AND 3),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    started_at INTEGER,
    done_at    INTEGER
) STRICT;

CREATE INDEX item_parent ON item (parent_id);

-- sqlite treats NULL parents as distinct
CREATE UNIQUE INDEX item_open_sibling_name ON item (coalesce(parent_id, 0), name) WHERE done_at IS NULL;

CREATE TABLE tag (
    id   INTEGER PRIMARY KEY,
    name TEXT    NOT NULL UNIQUE
) STRICT;

CREATE TABLE item_tag (
    item_id INTEGER NOT NULL REFERENCES item (id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tag (id),
    PRIMARY KEY (item_id, tag_id)
) STRICT, WITHOUT ROWID;

INSERT INTO status (name, label, category, position) VALUES
    ('todo', 'To do', 'todo', 1),
    ('progress', 'In progress', 'active', 2),
    ('done', 'Done', 'done', 3);
