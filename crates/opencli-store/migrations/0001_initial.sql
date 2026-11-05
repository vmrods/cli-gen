-- Metadata store for imported specifications (ADR 0003).
-- Each import of a changed document adds a spec_versions row; the newest is current.

CREATE TABLE apis (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL UNIQUE,
    source      TEXT    NOT NULL,
    created_at  TEXT    NOT NULL
) STRICT;

CREATE TABLE spec_versions (
    id            INTEGER PRIMARY KEY,
    api_id        INTEGER NOT NULL REFERENCES apis (id) ON DELETE CASCADE,
    format        TEXT    NOT NULL,
    title         TEXT    NOT NULL,
    info_version  TEXT    NOT NULL,
    description   TEXT,
    content_hash  TEXT    NOT NULL,
    document      TEXT    NOT NULL,
    imported_at   TEXT    NOT NULL,
    UNIQUE (api_id, content_hash)
) STRICT;

CREATE TABLE servers (
    version_id  INTEGER NOT NULL REFERENCES spec_versions (id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    url         TEXT    NOT NULL,
    PRIMARY KEY (version_id, position)
) STRICT;

CREATE TABLE operations (
    id            INTEGER PRIMARY KEY,
    version_id    INTEGER NOT NULL REFERENCES spec_versions (id) ON DELETE CASCADE,
    position      INTEGER NOT NULL,
    operation_id  TEXT,
    method        TEXT    NOT NULL,
    path          TEXT    NOT NULL,
    summary       TEXT,
    description   TEXT,
    deprecated    INTEGER NOT NULL,
    request_body  TEXT,              -- JSON: RequestBody
    security      TEXT    NOT NULL,  -- JSON: [SecurityRequirement]; an empty requirement means anonymous is allowed
    UNIQUE (version_id, method, path)
) STRICT;
CREATE INDEX operations_version ON operations (version_id, position);

CREATE TABLE parameters (
    operation_id  INTEGER NOT NULL REFERENCES operations (id) ON DELETE CASCADE,
    position      INTEGER NOT NULL,
    name          TEXT    NOT NULL,
    location      TEXT    NOT NULL,
    required      INTEGER NOT NULL,
    description   TEXT,
    schema        TEXT,              -- JSON Schema
    PRIMARY KEY (operation_id, position),
    UNIQUE (operation_id, location, name)
) STRICT;

CREATE TABLE security_schemes (
    version_id  INTEGER NOT NULL REFERENCES spec_versions (id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,
    kind        TEXT    NOT NULL,
    definition  TEXT    NOT NULL,    -- JSON: SecurityScheme
    PRIMARY KEY (version_id, name)
) STRICT;

CREATE TABLE schemas (
    version_id  INTEGER NOT NULL REFERENCES spec_versions (id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,
    definition  TEXT    NOT NULL,    -- JSON Schema
    PRIMARY KEY (version_id, name)
) STRICT;
