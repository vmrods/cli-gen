-- Another API whose stored credential this one uses (ADR 0011), set with
-- `api set-credentials --from`. Removing that API clears the link.
ALTER TABLE apis ADD COLUMN credentials_from INTEGER REFERENCES apis (id) ON DELETE SET NULL;
