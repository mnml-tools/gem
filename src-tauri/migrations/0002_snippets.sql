-- Gem schema v2: snippets (saved remote commands), global (site_id NULL) or per site.

CREATE TABLE snippets (
    id         TEXT PRIMARY KEY NOT NULL,
    site_id    TEXT REFERENCES sites (id) ON DELETE CASCADE,
    name       TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    command    TEXT NOT NULL CHECK (length(command) BETWEEN 1 AND 4000),
    confirm    INTEGER NOT NULL DEFAULT 0 CHECK (confirm IN (0, 1)),
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE INDEX snippets_scope ON snippets (site_id, sort_order, name);
