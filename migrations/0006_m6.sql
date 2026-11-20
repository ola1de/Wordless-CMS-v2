-- Milestone 6: AI operating system.
-- Approvals queue (agents propose changes; a human approves or rejects)
-- and website memory (facts + admin notes agents use as context).
-- Neither table stores secrets. Both are additive; no existing table changes.
CREATE TABLE IF NOT EXISTS wl_ai_approvals (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  agent TEXT NOT NULL,
  action TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'rejected')),
  created_by INTEGER NOT NULL,
  decided_by INTEGER,
  decided_at TEXT,
  reason TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_ai_approvals_status ON wl_ai_approvals (status, created_at);
CREATE INDEX IF NOT EXISTS idx_wl_ai_approvals_task ON wl_ai_approvals (task_id);

CREATE TABLE IF NOT EXISTS wl_ai_memory (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL CHECK (kind IN ('fact', 'note')),
  key TEXT NOT NULL UNIQUE,
  value TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'system',
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_ai_memory_kind ON wl_ai_memory (kind, key);
