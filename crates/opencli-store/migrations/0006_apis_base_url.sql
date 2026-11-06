-- The base URL chosen at onboarding (or later with `api set-server`). NULL
-- means "follow the spec's servers". Kept across refreshes: it belongs to the
-- user's setup, not to the document.
ALTER TABLE apis ADD COLUMN base_url TEXT;
