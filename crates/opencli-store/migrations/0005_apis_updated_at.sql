-- Audit column (database standards): when the API row last changed (source or
-- current version). SQLite can't add a NOT NULL column without a default, so
-- it is added nullable and backfilled; queries read COALESCE(updated_at, created_at).
ALTER TABLE apis ADD COLUMN updated_at TEXT;
UPDATE apis SET updated_at = created_at WHERE updated_at IS NULL;
