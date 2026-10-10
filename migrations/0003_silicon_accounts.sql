-- Preserve historical rows, but new data partitions always use account UUIDs.
ALTER TABLE drawings RENAME COLUMN org_id TO account_id;
ALTER TABLE ting_enrollments RENAME COLUMN org_id TO account_id;
ALTER TABLE deliveries RENAME COLUMN org_id TO account_id;
ALTER TABLE byo_keys RENAME COLUMN org_id TO account_id;
ALTER TABLE reports RENAME COLUMN org_id TO account_id;
-- Former environment and consent tables are retained only as historical storage.
-- Token response replay is encrypted; one account refresh reaches Accounts once.
CREATE TABLE account_token_exchanges (
    fingerprint TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('pending','complete','uncertain')),
    response BLOB,
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    expires_at INTEGER
) STRICT;

CREATE UNIQUE INDEX drawings_personal_account ON drawings(ctx,account_id) WHERE length(account_id)=36;
CREATE UNIQUE INDEX enrollments_personal_account ON ting_enrollments(ctx,account_id) WHERE length(account_id)=36;
