-- Milestone 8: integrations + commerce.
-- Products, orders, order items, carts, mail log, webhook log.
-- All money is stored in INTEGER minor units (kobo/cents) — never float.
-- Secrets are NEVER stored here: provider keys live only in env/secrets
-- bindings (see docs/integrations/integrations.md). Additive only.
CREATE TABLE IF NOT EXISTS wl_products (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  slug TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  price_minor INTEGER NOT NULL DEFAULT 0,
  currency TEXT NOT NULL DEFAULT 'NGN',
  stock INTEGER NOT NULL DEFAULT -1,
  status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published')),
  image_id INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_products_status ON wl_products (status, slug);
CREATE INDEX IF NOT EXISTS idx_wl_products_slug ON wl_products (slug);

CREATE TABLE IF NOT EXISTS wl_orders (
  id TEXT PRIMARY KEY,
  customer_name TEXT NOT NULL DEFAULT '',
  customer_email TEXT NOT NULL DEFAULT '',
  total_minor INTEGER NOT NULL DEFAULT 0,
  currency TEXT NOT NULL DEFAULT 'NGN',
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'paid', 'fulfilled', 'cancelled')),
  payment_provider TEXT NOT NULL DEFAULT '',
  payment_reference TEXT NOT NULL DEFAULT '',
  stock_applied INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_orders_status ON wl_orders (status, created_at);
CREATE INDEX IF NOT EXISTS idx_wl_orders_reference ON wl_orders (payment_reference);

CREATE TABLE IF NOT EXISTS wl_order_items (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  order_id TEXT NOT NULL,
  product_id INTEGER NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  price_minor INTEGER NOT NULL DEFAULT 0,
  quantity INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_order_items_order ON wl_order_items (order_id);

-- Server-side carts, keyed by the `wordless_cart` cookie token.
-- Items are a JSON array of { productId, quantity }. Expire after 7 days.
CREATE TABLE IF NOT EXISTS wl_carts (
  id TEXT PRIMARY KEY,
  items TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- Mail log: the mock email provider records here (never claims delivery);
-- the HTTP provider records sent/failed outcomes here too.
CREATE TABLE IF NOT EXISTS wl_mail_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider TEXT NOT NULL DEFAULT 'mock',
  to_email TEXT NOT NULL DEFAULT '',
  subject TEXT NOT NULL DEFAULT '',
  body TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'logged' CHECK (status IN ('logged', 'sent', 'failed')),
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_mail_log_created ON wl_mail_log (created_at DESC);

-- Webhook attempts: provider + event + reference + result. Never a secret.
CREATE TABLE IF NOT EXISTS wl_webhook_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider TEXT NOT NULL DEFAULT '',
  event TEXT NOT NULL DEFAULT '',
  reference TEXT NOT NULL DEFAULT '',
  result TEXT NOT NULL DEFAULT '' CHECK (result IN ('accepted', 'rejected', 'duplicate', 'ignored', 'error')),
  detail TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wl_webhook_log_created ON wl_webhook_log (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_wl_webhook_log_reference ON wl_webhook_log (provider, reference);
