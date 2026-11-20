-- WORDLESS v2 P3 migration 0011 — outbound webhooks (NEW in v2, no v1
-- equivalent). Event subscriptions + delivery log. Additive only;
-- earlier migrations are never edited.
CREATE TABLE IF NOT EXISTS wl_webhook_subscriptions (
  id TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  events TEXT NOT NULL DEFAULT '[]',
  secret TEXT NOT NULL DEFAULT '',
  active INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_webhook_subscriptions_active
  ON wl_webhook_subscriptions (active);
CREATE TABLE IF NOT EXISTS wl_webhook_deliveries (
  id TEXT PRIMARY KEY,
  subscription_id TEXT NOT NULL,
  event TEXT NOT NULL,
  payload TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'pending',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_retry_at TEXT,
  response_status INTEGER,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_webhook_deliveries_retry
  ON wl_webhook_deliveries (status, next_retry_at);
CREATE INDEX IF NOT EXISTS idx_wl_webhook_deliveries_subscription
  ON wl_webhook_deliveries (subscription_id, created_at DESC);
