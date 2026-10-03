-- Ordinary sessions remain client-owned. Feature authority is separately
-- encrypted and bound to the initiating account, organization and clean generation.
CREATE TABLE obo_requests (
  ctx TEXT NOT NULL, org_id TEXT NOT NULL, actor_id TEXT NOT NULL,
  id TEXT NOT NULL, request_key TEXT NOT NULL, generation INTEGER NOT NULL,
  payload BLOB NOT NULL, authorization_id TEXT, consent TEXT,
  code_hash TEXT, completed INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL,
  PRIMARY KEY (ctx, id), UNIQUE (ctx, org_id, actor_id, request_key)
);
CREATE TABLE obo_roots (
  ctx TEXT NOT NULL, org_id TEXT NOT NULL, actor_id TEXT NOT NULL,
  endpoint TEXT NOT NULL, generation INTEGER NOT NULL, request_id TEXT NOT NULL,
  credentials BLOB NOT NULL,
  PRIMARY KEY (ctx, org_id, actor_id, endpoint)
);
CREATE TABLE obo_operations (
  ctx TEXT NOT NULL, org_id TEXT NOT NULL, actor_id TEXT NOT NULL,
  endpoint TEXT NOT NULL, operation_key TEXT NOT NULL,
  request_hash TEXT NOT NULL, provider_org TEXT NOT NULL, provider_actor TEXT NOT NULL,
  PRIMARY KEY (ctx, org_id, actor_id, endpoint, operation_key)
);
CREATE TABLE obo_locks (
  ctx TEXT NOT NULL, org_id TEXT NOT NULL, actor_id TEXT NOT NULL,
  owner TEXT NOT NULL, expires_at INTEGER NOT NULL,
  PRIMARY KEY (ctx, org_id, actor_id)
);
