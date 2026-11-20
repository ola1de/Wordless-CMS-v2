-- WORDLESS v2 P8 — WooCommerce compatibility subset.
--
-- Reuse-or-extend decision (doc §3 P8 outcome block): EXTEND the P3
-- commerce tables instead of creating wl_woo_products / wl_woo_orders /
-- wl_woo_order_items. One order pipeline, one cart, one webhook path.
-- New wl_woo_* tables only where P3 has nothing.
--
-- wl_products gains `type` (simple|variable) and `sku` (the WooCommerce
-- stable key; unique across products AND variations, enforced in code
-- because a single-column UNIQUE cannot span the two tables).
-- wl_order_items gains variation_id + variation_label (order-item
-- snapshots, same rule as name/price: later edits never rewrite history).
-- wl_orders gains the Woo totals (coupon_code, discount/tax/shipping)
-- and provenance (source: 'wordless' | 'woo' | 'woo-import', source_id:
-- the WooCommerce order id for imports).
ALTER TABLE wl_products ADD COLUMN type TEXT NOT NULL DEFAULT 'simple' CHECK (type IN ('simple', 'variable'));
ALTER TABLE wl_products ADD COLUMN sku TEXT NOT NULL DEFAULT '';
CREATE UNIQUE INDEX idx_wl_products_sku ON wl_products (sku) WHERE sku <> '';
ALTER TABLE wl_order_items ADD COLUMN variation_id INTEGER;
ALTER TABLE wl_order_items ADD COLUMN variation_label TEXT NOT NULL DEFAULT '';
ALTER TABLE wl_orders ADD COLUMN coupon_code TEXT;
ALTER TABLE wl_orders ADD COLUMN discount_minor INTEGER NOT NULL DEFAULT 0;
ALTER TABLE wl_orders ADD COLUMN tax_minor INTEGER NOT NULL DEFAULT 0;
ALTER TABLE wl_orders ADD COLUMN shipping_minor INTEGER NOT NULL DEFAULT 0;
ALTER TABLE wl_orders ADD COLUMN shipping_method TEXT NOT NULL DEFAULT '';
ALTER TABLE wl_orders ADD COLUMN source TEXT NOT NULL DEFAULT 'wordless';
ALTER TABLE wl_orders ADD COLUMN source_id TEXT NOT NULL DEFAULT '';

-- Product attributes (per product; options are the full value list).
-- options_json: JSON array of strings, e.g. ["S", "M", "L"].
CREATE TABLE wl_woo_attributes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  product_id INTEGER NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  slug TEXT NOT NULL DEFAULT '',
  position INTEGER NOT NULL DEFAULT 0,
  visible INTEGER NOT NULL DEFAULT 1,
  is_variation INTEGER NOT NULL DEFAULT 1,
  options_json TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (product_id, slug)
);
CREATE INDEX idx_wl_woo_attributes_product ON wl_woo_attributes (product_id, position);

-- Variations of a variable product. attributes_json: JSON object of
-- attribute name -> chosen value, e.g. {"Size": "M", "Colour": "Red"}.
-- price_minor is the regular price; sale_price_minor > 0 is the price
-- actually charged. stock -1 = unlimited (same rule as wl_products).
CREATE TABLE wl_woo_variations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  product_id INTEGER NOT NULL,
  sku TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'published' CHECK (status IN ('draft', 'published')),
  price_minor INTEGER NOT NULL DEFAULT 0,
  sale_price_minor INTEGER NOT NULL DEFAULT 0,
  stock INTEGER NOT NULL DEFAULT -1,
  attributes_json TEXT NOT NULL DEFAULT '{}',
  image_id INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_wl_woo_variations_product ON wl_woo_variations (product_id);
CREATE INDEX idx_wl_woo_variations_sku ON wl_woo_variations (sku) WHERE sku <> '';

-- Imported product categories/tags (WooCommerce CSV Categories/Tags
-- columns). taxonomy is 'product_cat' or 'product_tag'; the term is
-- stored as written in the export (a "Clothing > T-Shirts" path stays
-- one term — hierarchy is not reconstructed).
CREATE TABLE wl_woo_product_terms (
  product_id INTEGER NOT NULL,
  taxonomy TEXT NOT NULL,
  term TEXT NOT NULL,
  PRIMARY KEY (product_id, taxonomy, term)
);

-- Coupons. type 'fixed' uses amount_minor; 'percent' uses percent_bp
-- (basis points: 1000 = 10%). max_uses 0 = unlimited. usage_count
-- increments exactly once with the paid transition (see
-- wordless-commerce orders::transition_order).
CREATE TABLE wl_woo_coupons (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  code TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  type TEXT NOT NULL DEFAULT 'fixed' CHECK (type IN ('fixed', 'percent')),
  amount_minor INTEGER NOT NULL DEFAULT 0,
  percent_bp INTEGER NOT NULL DEFAULT 0,
  min_order_minor INTEGER NOT NULL DEFAULT 0,
  max_uses INTEGER NOT NULL DEFAULT 0,
  usage_count INTEGER NOT NULL DEFAULT 0,
  expires_at TEXT,
  active INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- Tax rates. country '' = any country; state '' = any state in that
-- country. rate_bp is basis points (750 = 7.5%). The most specific
-- matching rate wins (country+state > country > global).
CREATE TABLE wl_woo_tax_rates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL DEFAULT '',
  country TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL DEFAULT '',
  rate_bp INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- Shipping zones (countries_json: JSON array of ISO country codes;
-- an empty array = rest of the world) and their methods.
CREATE TABLE wl_woo_shipping_zones (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL DEFAULT '',
  countries_json TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE wl_woo_shipping_methods (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  zone_id INTEGER NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  type TEXT NOT NULL DEFAULT 'flat_rate' CHECK (type IN ('flat_rate', 'free_shipping')),
  cost_minor INTEGER NOT NULL DEFAULT 0,
  min_order_minor INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_wl_woo_shipping_methods_zone ON wl_woo_shipping_methods (zone_id);

-- Customers (created at Woo checkout by email, and by CSV import).
-- total_orders / total_spent_minor are maintained from imported and
-- local orders; source is 'wordless' | 'woo' | 'woo-import'.
CREATE TABLE wl_woo_customers (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  email TEXT NOT NULL UNIQUE,
  first_name TEXT NOT NULL DEFAULT '',
  last_name TEXT NOT NULL DEFAULT '',
  username TEXT NOT NULL DEFAULT '',
  billing_country TEXT NOT NULL DEFAULT '',
  billing_state TEXT NOT NULL DEFAULT '',
  billing_postcode TEXT NOT NULL DEFAULT '',
  total_orders INTEGER NOT NULL DEFAULT 0,
  total_spent_minor INTEGER NOT NULL DEFAULT 0,
  source TEXT NOT NULL DEFAULT 'wordless',
  source_id TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
