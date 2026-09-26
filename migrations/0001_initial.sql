-- peek-server schema v1 (BLUEPRINT §5.2). The same schema is applied to the
-- production database (PEEK_DATABASE_PATH) and to the testing database
-- (PEEK_TEST_DATABASE_PATH). Every table carries `ctx`: `production`, or the
-- hyphenated lowercase UUID of a Honeycomb testing environment. `ctx` has no
-- default and is always resolved server-side from a validated credential.
--
-- There are no IAM token tables: the backend never stores an oat_/ort_ (D5).

-- The Silicon's drawing copy (≤ 256 KiB of JavaScript).
CREATE TABLE drawings (
    ctx        TEXT    NOT NULL CHECK (ctx = 'production' OR ctx GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    org_id     TEXT    NOT NULL,
    actor_id   TEXT    NOT NULL,
    sha256     TEXT    NOT NULL CHECK (length(sha256) = 64),
    bytes      BLOB    NOT NULL CHECK (length(bytes) BETWEEN 1 AND 262144),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (ctx, org_id, actor_id)
) STRICT;

-- Ting recipient grants peek created (never a proof, never a token).
CREATE TABLE ting_enrollments (
    ctx             TEXT    NOT NULL CHECK (ctx = 'production' OR ctx GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    org_id          TEXT    NOT NULL,
    actor_id        TEXT    NOT NULL,
    subscription_id TEXT    NOT NULL,
    registered_at   INTEGER NOT NULL,
    revoked_at      INTEGER,
    PRIMARY KEY (ctx, org_id, actor_id)
) STRICT;

-- One row per delivery event peekd posted (BLUEPRINT §3.5 step 6).
CREATE TABLE deliveries (
    ctx           TEXT    NOT NULL CHECK (ctx = 'production' OR ctx GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    event_id      TEXT    NOT NULL,
    org_id        TEXT    NOT NULL,
    actor_id      TEXT    NOT NULL,
    type          TEXT    NOT NULL,
    ting_key      TEXT    NOT NULL,
    ting_id       TEXT,
    status        TEXT    NOT NULL CHECK (status IN ('pending', 'accepted', 'recipient_not_registered', 'ting_type_missing', 'rejected', 'unavailable')),
    silent        INTEGER CHECK (silent IN (0, 1)),
    last_error    TEXT,
    attempts      INTEGER NOT NULL DEFAULT 0,
    first_seen_at INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    accepted_at   INTEGER,
    PRIMARY KEY (ctx, event_id)
) STRICT;
CREATE INDEX deliveries_actor ON deliveries (ctx, org_id, actor_id);

-- Org-wide bring-your-own Deepgram keys, sealed with AES-256-GCM
-- (PEEK_ENCRYPTION_KEY). The key is never returned after it is stored.
CREATE TABLE byo_keys (
    ctx        TEXT    NOT NULL CHECK (ctx = 'production' OR ctx GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    org_id     TEXT    NOT NULL,
    provider   TEXT    NOT NULL CHECK (provider = 'deepgram'),
    sealed     BLOB    NOT NULL,
    base_url   TEXT,
    updated_at INTEGER NOT NULL,
    updated_by TEXT    NOT NULL,
    PRIMARY KEY (ctx, org_id, provider)
) STRICT;

-- Bug reports (`peek report`). `stored` until filed as a GitHub issue.
CREATE TABLE reports (
    id              TEXT    PRIMARY KEY,
    ctx             TEXT    NOT NULL CHECK (ctx = 'production' OR ctx GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    org_id          TEXT,
    actor_id        TEXT,
    message         TEXT    NOT NULL,
    pr              TEXT,
    context         TEXT,
    attached_status TEXT,
    created_at      INTEGER NOT NULL,
    status          TEXT    NOT NULL CHECK (status IN ('stored', 'filed')),
    issue_url       TEXT,
    filing_error    TEXT
) STRICT;
CREATE INDEX reports_status ON reports (status, created_at);

-- Idempotent POSTs. `status` NULL means the first execution is still running;
-- only successful responses are kept, so a failed attempt never poisons a key.
CREATE TABLE idempotency (
    ctx            TEXT    NOT NULL CHECK (ctx = 'production' OR ctx GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    scope          TEXT    NOT NULL,
    key            TEXT    NOT NULL,
    request_sha256 TEXT    NOT NULL,
    status         INTEGER,
    response       BLOB,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (ctx, scope, key)
) STRICT;
CREATE INDEX idempotency_created ON idempotency (created_at);

-- IAM webhook deliveries already processed (dedupe on event_id).
CREATE TABLE webhook_events (
    event_id    TEXT    PRIMARY KEY,
    ctx         TEXT    NOT NULL CHECK (ctx = 'production' OR ctx GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    event_type  TEXT    NOT NULL,
    received_at INTEGER NOT NULL
) STRICT;
CREATE INDEX webhook_events_received ON webhook_events (received_at);

-- Honeycomb lifecycle bindings: one per testing environment peek was prepared
-- in. Control-plane rows, so they only ever live in the production context.
CREATE TABLE env_bindings (
    ctx                  TEXT    NOT NULL CHECK (ctx = 'production'),
    environment_id       TEXT    PRIMARY KEY,
    org_id               TEXT    NOT NULL,
    environment_revision INTEGER NOT NULL CHECK (environment_revision >= 1),
    generation           INTEGER NOT NULL CHECK (generation >= 1),
    key_version          INTEGER NOT NULL CHECK (key_version >= 1),
    testing_key_sha256   TEXT    NOT NULL,
    sealed_root_key      BLOB    NOT NULL,
    state                TEXT    NOT NULL CHECK (state IN ('pending', 'active', 'disabled', 'retired', 'purged')),
    operation_id         TEXT    NOT NULL,
    last_activity_at     INTEGER,
    activity_reported_at INTEGER,
    updated_at           INTEGER NOT NULL
) STRICT;

-- Durable lifecycle receipts (lost-response recovery and replay).
CREATE TABLE participant_ops (
    ctx            TEXT    NOT NULL CHECK (ctx = 'production'),
    operation_id   TEXT    PRIMARY KEY,
    environment_id TEXT    NOT NULL,
    org_id         TEXT    NOT NULL,
    action         TEXT    NOT NULL,
    request_sha256 TEXT    NOT NULL,
    state          TEXT    NOT NULL CHECK (state IN ('pending', 'completed', 'failed')),
    target_state   TEXT    NOT NULL,
    receipt        BLOB    NOT NULL,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
) STRICT;
CREATE INDEX participant_ops_environment ON participant_ops (environment_id);
