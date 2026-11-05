-- The current version becomes explicit: a refresh back to earlier content
-- reactivates that version instead of duplicating it (spec_versions keeps
-- UNIQUE (api_id, content_hash)).
ALTER TABLE apis ADD COLUMN current_version_id INTEGER REFERENCES spec_versions (id);
UPDATE apis SET current_version_id = (SELECT MAX(id) FROM spec_versions WHERE api_id = apis.id);
