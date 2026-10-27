-- Design v1. Run once for an empty database, outside any existing transaction.
-- The application must also configure these PRAGMAs on every new connection.
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA journal_mode = DELETE;
PRAGMA synchronous = FULL;

BEGIN IMMEDIATE;

CREATE TABLE accounts (
    account_id TEXT PRIMARY KEY NOT NULL CHECK (length(account_id) > 0), -- WeRead vid
    display_name TEXT NOT NULL DEFAULT '',
    auth_epoch INTEGER NOT NULL DEFAULT 0 CHECK (auth_epoch >= 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

-- Only one active account. Logout deletes this row, not the account's books.
CREATE TABLE sessions (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    access_token TEXT NOT NULL CHECK (length(access_token) > 0),
    refresh_token TEXT NOT NULL CHECK (length(refresh_token) > 0),
    device_id TEXT NOT NULL CHECK (length(device_id) > 0),
    updated_at_ms INTEGER NOT NULL
);

-- Account-scoped metadata: no assumption that access rights are shared.
CREATE TABLE books (
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    book_id TEXT NOT NULL CHECK (length(book_id) > 0),
    title TEXT NOT NULL,
    author TEXT NOT NULL DEFAULT '',
    cover_url TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    content_version TEXT, -- NULL means unknown; never invent a remote version
    format TEXT NOT NULL DEFAULT 'unknown'
        CHECK (format IN ('unknown', 'txt', 'html', 'epub', 'pdf')),
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (account_id, book_id)
);

CREATE TABLE shelf_entries (
    account_id TEXT NOT NULL,
    book_id TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    finished INTEGER NOT NULL DEFAULT 0 CHECK (finished IN (0, 1)),
    added_at_ms INTEGER, -- NULL if not supplied by the server
    last_read_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (account_id, book_id),
    FOREIGN KEY (account_id, book_id)
        REFERENCES books(account_id, book_id) ON DELETE CASCADE
);
CREATE INDEX shelf_order ON shelf_entries(account_id, sort_order, book_id);

CREATE TABLE chapters (
    account_id TEXT NOT NULL,
    book_id TEXT NOT NULL,
    chapter_uid TEXT NOT NULL CHECK (length(chapter_uid) > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0), -- local TOC order, not UID
    title TEXT NOT NULL,
    parent_uid TEXT, -- optional logical parent; headings may not be readable
    readable INTEGER NOT NULL DEFAULT 1 CHECK (readable IN (0, 1)),
    content_version TEXT,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (account_id, book_id, chapter_uid),
    FOREIGN KEY (account_id, book_id)
        REFERENCES books(account_id, book_id) ON DELETE CASCADE
);
CREATE INDEX chapter_order ON chapters(account_id, book_id, ordinal, chapter_uid);

-- All large bytes live outside SQLite. Rows describe fully committed files only.
CREATE TABLE cached_files (
    account_id TEXT NOT NULL,
    file_id TEXT NOT NULL, -- local random ID / digest, never a raw remote path
    book_id TEXT NOT NULL,
    chapter_uid TEXT, -- logical association; no FK to replaceable TOC rows
    kind TEXT NOT NULL CHECK (kind IN ('book', 'chapter', 'cover', 'illustration')),
    resource_key TEXT NOT NULL, -- logical resource key / URL digest
    source_url TEXT, -- optional; do not persist signed URLs or tokens
    content_version TEXT,
    variant TEXT NOT NULL DEFAULT 'original', -- e.g. gray-200x300-v1
    relative_path TEXT NOT NULL CHECK (length(relative_path) > 0),
    media_type TEXT NOT NULL,
    byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
    pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    last_accessed_at_ms INTEGER NOT NULL,
    PRIMARY KEY (account_id, file_id),
    UNIQUE (relative_path),
    FOREIGN KEY (account_id, book_id)
        REFERENCES books(account_id, book_id) ON DELETE CASCADE,
    CHECK ((kind = 'chapter' AND chapter_uid IS NOT NULL) OR kind != 'chapter')
);
CREATE INDEX file_lookup ON cached_files
    (account_id, book_id, kind, resource_key, variant);
CREATE INDEX file_eviction ON cached_files(account_id, pinned, last_accessed_at_ms);

-- Local position and last observed remote position are separate snapshots.
CREATE TABLE reading_positions (
    account_id TEXT NOT NULL,
    book_id TEXT NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('local', 'remote')),
    chapter_uid TEXT,
    content_version TEXT,
    locator_json TEXT NOT NULL, -- versioned locator; validated in Rust
    progress_ratio REAL CHECK (progress_ratio BETWEEN 0.0 AND 1.0),
    progress_source TEXT NOT NULL DEFAULT 'unknown'
        CHECK (progress_source IN ('unknown', 'estimated', 'remote')),
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0), -- local monotonic revision
    remote_revision TEXT, -- opaque; NULL if unavailable
    remote_updated_at_ms INTEGER,
    observed_at_ms INTEGER NOT NULL,
    PRIMARY KEY (account_id, book_id, origin),
    FOREIGN KEY (account_id, book_id)
        REFERENCES books(account_id, book_id) ON DELETE CASCADE
);

-- Notes, bookmarks and highlights share identity, locator and sync lifecycle.
CREATE TABLE annotations (
    account_id TEXT NOT NULL,
    annotation_id TEXT NOT NULL, -- stable local UUID, including before first upload
    book_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('note', 'bookmark', 'highlight')),
    remote_id TEXT,
    chapter_uid TEXT,
    content_version TEXT,
    locator_json TEXT NOT NULL,
    quote_text TEXT NOT NULL DEFAULT '',
    note_text TEXT NOT NULL DEFAULT '',
    style_json TEXT NOT NULL DEFAULT '{}',
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    sync_status TEXT NOT NULL DEFAULT 'local'
        CHECK (sync_status IN ('local', 'pending', 'synced', 'conflict')),
    remote_revision TEXT,
    deleted_at_ms INTEGER, -- tombstone retained until delete is acknowledged
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (account_id, annotation_id),
    UNIQUE (account_id, book_id, kind, remote_id),
    FOREIGN KEY (account_id, book_id)
        REFERENCES books(account_id, book_id) ON DELETE CASCADE
);
CREATE INDEX annotation_book ON annotations
    (account_id, book_id, deleted_at_ms, chapter_uid);

-- Durable outbox: payload is a frozen business snapshot, not a signed HTTP request.
CREATE TABLE sync_jobs (
    account_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    book_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('progress', 'annotation_put', 'annotation_delete')),
    entity_id TEXT NOT NULL, -- book_id or local annotation_id
    entity_revision INTEGER NOT NULL CHECK (entity_revision >= 0),
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'in_flight', 'retry', 'uncertain', 'blocked', 'done')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at_ms INTEGER NOT NULL,
    attempt_auth_epoch INTEGER CHECK (attempt_auth_epoch >= 0),
    last_error_code TEXT, -- sanitized category, no token / raw HTTP response
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (account_id, job_id),
    UNIQUE (account_id, kind, entity_id, entity_revision),
    FOREIGN KEY (account_id, book_id)
        REFERENCES books(account_id, book_id) ON DELETE CASCADE
);
CREATE INDEX sync_due ON sync_jobs(account_id, status, next_attempt_at_ms);

-- Monotonic-clock elapsed duration, persisted as intervals, never inferred from offsets.
CREATE TABLE reading_intervals (
    account_id TEXT NOT NULL,
    interval_id TEXT NOT NULL,
    book_id TEXT NOT NULL,
    started_at_ms INTEGER NOT NULL,
    ended_at_ms INTEGER NOT NULL CHECK (ended_at_ms >= started_at_ms),
    active_ms INTEGER NOT NULL CHECK (active_ms >= 0),
    job_id TEXT,
    acknowledged_at_ms INTEGER,
    PRIMARY KEY (account_id, interval_id),
    FOREIGN KEY (account_id, book_id)
        REFERENCES books(account_id, book_id) ON DELETE CASCADE,
    FOREIGN KEY (account_id, job_id)
        REFERENCES sync_jobs(account_id, job_id)
);
CREATE INDEX interval_pending ON reading_intervals
    (account_id, book_id, acknowledged_at_ms, job_id);
CREATE INDEX interval_job ON reading_intervals(account_id, job_id);

CREATE TABLE sync_cursors (
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    scope TEXT NOT NULL CHECK (scope IN ('shelf', 'chapters', 'annotations', 'progress')),
    resource_id TEXT NOT NULL, -- '' for shelf, book_id for per-book resources
    cursor TEXT, -- opaque synckey, never parsed as a local counter
    last_success_at_ms INTEGER,
    last_attempt_at_ms INTEGER,
    last_error_code TEXT,
    PRIMARY KEY (account_id, scope, resource_id),
    CHECK ((scope = 'shelf' AND resource_id = '')
        OR (scope != 'shelf' AND length(resource_id) > 0))
);

CREATE TABLE reader_settings (
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    book_id TEXT NOT NULL DEFAULT '', -- '' = account defaults; otherwise per-book override
    settings_json TEXT NOT NULL, -- versioned, validated settings object
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (account_id, book_id)
);

PRAGMA user_version = 1;
COMMIT;
