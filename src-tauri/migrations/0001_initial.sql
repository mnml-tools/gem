-- Gem schema v1: saved sites, trusted server identities and settings.
-- Secrets (passwords, passphrases) are never stored here: they live in the OS keyring.

CREATE TABLE sites (
    id           TEXT PRIMARY KEY NOT NULL,
    name         TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    protocol     TEXT NOT NULL CHECK (protocol IN ('sftp', 'ftp', 'ftps')),
    host         TEXT NOT NULL CHECK (length(host) BETWEEN 1 AND 253),
    port         INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    user         TEXT NOT NULL DEFAULT '',
    auth         TEXT NOT NULL CHECK (auth IN ('password', 'key', 'agent')),
    key_path     TEXT,
    ftps_mode    TEXT NOT NULL DEFAULT 'explicit' CHECK (ftps_mode IN ('explicit', 'implicit')),
    passive      INTEGER NOT NULL DEFAULT 1 CHECK (passive IN (0, 1)),
    initial_path TEXT,
    color        TEXT,
    sort_order   INTEGER NOT NULL DEFAULT 0,
    has_secret   INTEGER NOT NULL DEFAULT 0 CHECK (has_secret IN (0, 1)),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
) STRICT;

CREATE INDEX sites_order ON sites (sort_order, name);

-- One trusted identity per host and port (SSH host key or FTPS certificate). A different key
-- for the same host and port is reported as changed, whatever its type.
CREATE TABLE known_hosts (
    host        TEXT NOT NULL,
    port        INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    key_type    TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    added_at    TEXT NOT NULL,
    PRIMARY KEY (host, port)
) STRICT;

CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) STRICT;
