-- Migration runner wraps this in a transaction. Existing v1 databases are retained.
ALTER TABLE annotations ADD COLUMN remote_snapshot_json TEXT;
PRAGMA user_version = 2;
