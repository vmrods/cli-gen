-- x-cli-name / x-cli-resource naming overrides (ADR 0002).
ALTER TABLE operations ADD COLUMN cli_name TEXT;
ALTER TABLE operations ADD COLUMN cli_resource TEXT;  -- JSON: [string]
