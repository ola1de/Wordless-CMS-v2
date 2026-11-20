//! WooCommerce CSV import (P8) — products, orders, customers.
//!
//! Format decisions (Q12, §3 P8 outcome block):
//! - **Products:** WooCommerce's native product export. Header row
//!   first; columns matched by name (case-insensitive, trimmed).
//!   `Type` is `simple`, `variable` or `variation` (`grouped` /
//!   `external` are refused with a reason — the subset does not
//!   cover them). `Published` 1 = published, -1 = draft, 0 = draft
//!   with a note (Wordless has no private products). Prices are
//!   decimal major units (`24.99` → 2499 minor, rounded half-up,
//!   integer math only). Variation rows carry `Parent` = parent SKU
//!   (an `id:123` parent is refused — re-export with SKUs) and one
//!   value per `Attribute N` column. Parent rows carry the full
//!   attribute value lists; an attribute becomes a variation
//!   attribute when a variation row uses it.
//! - **Orders:** one row per order, `line_item_1…` columns in the
//!   de-facto WebToffee shape: `Name|SKU|Quantity|Price` (price
//!   decimal major). `order_status` accepts the `wc-` prefix or the
//!   bare word. Imported orders keep their payment state via the
//!   status map (completed→fulfilled, processing→paid,
//!   on-hold→pending, pending→pending,
//!   cancelled/refunded/failed→cancelled) with
//!   `source='woo-import'` and `stock_applied=1` when paid/fulfilled.
//! - **Customers:** WooCommerce's native customer export columns,
//!   upserted by email.
//!
//! Identity: products and variations are matched by SKU (a row
//! whose SKU already exists updates that product/variation —
//! `updated`, not `imported`); orders are matched by
//! (`source='woo-import'`, `source_id` = Woo order id) and a repeat
//! import is `skipped` (already imported). Every row lands in the
//! report as imported / updated / skipped / failed with a reason —
//! nothing is silently dropped.
//!
//! NOT imported, recorded per row: remote product images (URL kept
//! in the row note — no remote fetch is configured), sale-date
//! windows (a sale price is applied as the current price with a
//! note), downloadable files, order notes, refunds as separate
//! records, gateway tokens, tax/shipping settings, customer
//! passwords (same rule as WXR users: empty + forced reset —
//! customers have no login here at all).

use serde::Serialize;
use wordless_commerce::ids::new_uuid_v4;
use wordless_commerce::products::{create_product, get_product_by_slug, update_product};
use wordless_commerce::types::ProductInput;
use wordless_db::row::row_text;
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, WordlessError};
use wordless_kernel::time::now_iso;

use crate::attributes::create_attribute;
use crate::customers::{record_order_for_customer, upsert_customer};
use crate::products::{get_product_by_sku, get_variation_by_sku, set_product_terms};
use crate::types::{AttributeInput, VariationInput};
use crate::variations::{create_variation, sync_parent_price, update_variation};

// ---------------------------------------------------------------- CSV parsing

/// Parse CSV text into (headers, rows). A real tokenizer — never
/// regex: quoted fields, `""` escapes, embedded newlines inside
/// quotes, CRLF/LF/CR endings, a UTF-8 BOM. Empty lines are dropped.
/// An unterminated quoted field is a 400.
pub fn parse_csv(text: &str) -> Result<(Vec<String>, Vec<Vec<String>>), WordlessError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.is_empty() {
            in_quotes = true;
        } else if c == ',' {
            row.push(std::mem::take(&mut field));
        } else if c == '\r' || c == '\n' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
        } else {
            field.push(c);
        }
    }
    if in_quotes {
        return Err(bad_request("CSV has an unterminated quoted field."));
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    if rows.is_empty() {
        return Err(bad_request("The CSV file is empty."));
    }
    let headers = rows.remove(0);
    Ok((headers, rows))
}

/// A cell by header name (case-insensitive, trimmed header). "" when
/// the column is absent or the row is short.
fn col<'a>(headers: &[String], row: &'a [String], name: &str) -> &'a str {
    let want = name.to_ascii_lowercase();
    for (i, h) in headers.iter().enumerate() {
        if h.trim().to_ascii_lowercase() == want {
            return row.get(i).map(|s| s.as_str()).unwrap_or("");
        }
    }
    ""
}

/// Decimal major units (`"24.99"`, `"24"`, `"24.9"`) → integer minor
/// units, rounded half-up, with integer math only — no float ever
/// touches a price. Empty → 0. Anything else is an error naming the
/// value.
pub fn parse_major_price(s: &str) -> Result<i64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    if s.starts_with('-') {
        return Err(format!(
            "\"{s}\" is not a price (negative prices are not allowed)."
        ));
    }
    if s.is_empty()
        || !s.chars().all(|c| c.is_ascii_digit() || c == '.')
        || s.matches('.').count() > 1
    {
        return Err(format!("\"{s}\" is not a price."));
    }
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if whole.len() > 12 || frac.len() > 3 {
        return Err(format!("\"{s}\" is not a price."));
    }
    let whole_n: i64 = if whole.is_empty() {
        0
    } else {
        whole
            .parse()
            .map_err(|_| format!("\"{s}\" is not a price."))?
    };
    let mut digits = frac.chars();
    let d1 = digits.next().and_then(|c| c.to_digit(10)).unwrap_or(0) as i64;
    let d2 = digits.next().and_then(|c| c.to_digit(10)).unwrap_or(0) as i64;
    let d3 = digits.next().and_then(|c| c.to_digit(10)).unwrap_or(0) as i64;
    let mut frac_minor = d1 * 10 + d2;
    if d3 >= 5 {
        frac_minor += 1;
    }
    Ok(if frac_minor == 100 {
        (whole_n + 1) * 100
    } else {
        whole_n * 100 + frac_minor
    })
}

fn parse_int_cell(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    s.parse::<i64>().ok()
}

fn is_truthy(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "yes" | "true")
}

/// `"2026-01-15 10:30:00"` (WordPress form) → ISO-8601 UTC; an ISO
/// string passes through; anything else → now.
fn to_iso_or_now(s: &str) -> String {
    let t = s.trim();
    if t.len() == 19
        && t.as_bytes().get(4) == Some(&b'-')
        && t.as_bytes().get(7) == Some(&b'-')
        && t.as_bytes().get(10) == Some(&b' ')
    {
        return format!("{}T{}.000Z", &t[..10], &t[11..]);
    }
    if t.len() >= 19 && t.as_bytes().get(10) == Some(&b'T') {
        return t.to_string();
    }
    now_iso()
}

// ---------------------------------------------------------------- report

/// One imported row: what happened to it, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRowResult {
    /// 1-based data-row number (the header is row 0).
    pub row: i64,
    /// The row's stable key (SKU, order id, or email).
    pub key: String,
    /// `"imported"` | `"updated"` | `"skipped"` | `"failed"`.
    pub status: String,
    /// The reason for a failure/skip, or a note on an import.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The import report: counts plus one entry per row.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub kind: String,
    pub imported: i64,
    pub updated: i64,
    pub skipped: i64,
    pub failed: i64,
    pub rows: Vec<ImportRowResult>,
}

impl ImportReport {
    fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_string(),
            imported: 0,
            updated: 0,
            skipped: 0,
            failed: 0,
            rows: Vec::new(),
        }
    }

    fn add(&mut self, row: i64, key: &str, status: &str, reason: Option<String>) {
        match status {
            "imported" => self.imported += 1,
            "updated" => self.updated += 1,
            "skipped" => self.skipped += 1,
            _ => self.failed += 1,
        }
        self.rows.push(ImportRowResult {
            row,
            key: key.to_string(),
            status: status.to_string(),
            reason,
        });
    }
}

/// Import CSV of `kind` (`"products"`, `"orders"` or `"customers"`).
pub async fn import_csv(
    db: &dyn Db,
    kind: &str,
    csv_text: &str,
) -> Result<ImportReport, WordlessError> {
    match kind {
        "products" => import_products_csv(db, csv_text).await,
        "orders" => import_orders_csv(db, csv_text).await,
        "customers" => import_customers_csv(db, csv_text).await,
        _ => Err(bad_request(
            "Field \"kind\" must be \"products\", \"orders\" or \"customers\".",
        )),
    }
}

// ---------------------------------------------------------------- products

/// Split a comma-separated WooCommerce cell (`"A, B, C"`) into terms.
fn split_list_cell(s: &str) -> Vec<String> {
    s.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Attribute columns for one row: (name, values) for N = 1.. while
/// the `Attribute N name` header exists.
fn row_attributes(headers: &[String], row: &[String]) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for n in 1..=20 {
        let name = col(headers, row, &format!("Attribute {n} name"));
        if name.trim().is_empty() {
            break;
        }
        let values = split_list_cell(col(headers, row, &format!("Attribute {n} value(s)")));
        out.push((name.trim().to_string(), values));
    }
    out
}

fn published_status(cell: &str) -> (&'static str, Option<String>) {
    match cell.trim() {
        "1" => ("published", None),
        "-1" => ("draft", None),
        "0" => (
            "draft",
            Some("private → draft (Wordless has no private products).".to_string()),
        ),
        _ => ("draft", None),
    }
}

async fn import_products_csv(db: &dyn Db, csv_text: &str) -> Result<ImportReport, WordlessError> {
    let (headers, rows) = parse_csv(csv_text)?;
    let mut report = ImportReport::new("products");
    // Pass 1: parents (simple + variable) in file order.
    // Pass 2: variations (their parents must exist already).
    let mut variation_rows: Vec<(i64, usize)> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let row_no = (i + 1) as i64;
        let r#type = col(&headers, row, "Type").trim().to_ascii_lowercase();
        let sku = col(&headers, row, "SKU").trim().to_string();
        let name = col(&headers, row, "Name").trim().to_string();
        if r#type == "variation" {
            variation_rows.push((row_no, i));
            continue;
        }
        if r#type != "simple" && r#type != "variable" {
            report.add(
                row_no,
                if sku.is_empty() { &name } else { &sku },
                "failed",
                Some(format!(
                    "Type \"{}\" is not supported by the WooCommerce subset (simple, variable and variation only).",
                    col(&headers, row, "Type").trim()
                )),
            );
            continue;
        }
        if name.is_empty() {
            report.add(
                row_no,
                &sku,
                "failed",
                Some("A product row needs a Name.".to_string()),
            );
            continue;
        }
        let regular = match parse_major_price(col(&headers, row, "Regular price")) {
            Ok(v) => v,
            Err(e) => {
                report.add(row_no, &sku, "failed", Some(format!("Regular price: {e}")));
                continue;
            }
        };
        let sale = match parse_major_price(col(&headers, row, "Sale price")) {
            Ok(v) => v,
            Err(e) => {
                report.add(row_no, &sku, "failed", Some(format!("Sale price: {e}")));
                continue;
            }
        };
        // A simple product's charged price is its sale price when one
        // is set (sale-date windows are not imported — see the note).
        let (price_minor, mut notes) = if r#type == "simple" && sale > 0 {
            (
                sale,
                vec![
                    "sale price applied as the current price (sale dates are not imported)."
                        .to_string(),
                ],
            )
        } else {
            (regular, Vec::new())
        };
        let stock = match col(&headers, row, "Stock").trim() {
            "" => {
                if is_truthy(col(&headers, row, "In stock?"))
                    || col(&headers, row, "In stock?").trim().is_empty()
                {
                    -1
                } else {
                    0
                }
            }
            s => match parse_int_cell(s) {
                Some(v) => v,
                None => {
                    report.add(
                        row_no,
                        &sku,
                        "failed",
                        Some(format!("Stock \"{s}\" is not a number.")),
                    );
                    continue;
                }
            },
        };
        let (status, status_note) = published_status(col(&headers, row, "Published"));
        if let Some(n) = status_note {
            notes.push(n);
        }
        let images = col(&headers, row, "Images");
        if !images.trim().is_empty() {
            notes.push("image URL(s) not fetched — upload the image in Media and attach it to the product.".to_string());
        }
        let description = col(&headers, row, "Description").trim().to_string();
        let short = col(&headers, row, "Short description").trim().to_string();
        let full_description = if short.is_empty() {
            description
        } else if description.is_empty() {
            short
        } else {
            format!("{short}\n\n{description}")
        };
        // Identity: SKU when present, else the slug derived from the name.
        let existing = if !sku.is_empty() {
            get_product_by_sku(db, &sku).await?
        } else {
            let slug = wordless_commerce::validate::slugify(&name);
            get_product_by_slug(db, &slug).await?
        };
        let product_id = match existing {
            Some(p) => {
                let updated = update_product(
                    db,
                    p.id,
                    ProductInput {
                        name: Some(serde_json::Value::String(name.clone())),
                        description: Some(serde_json::Value::String(full_description.clone())),
                        price_minor: Some(serde_json::json!(price_minor)),
                        stock: Some(serde_json::json!(stock)),
                        status: Some(serde_json::Value::String(status.to_string())),
                        product_type: Some(serde_json::Value::String(r#type.clone())),
                        sku: Some(serde_json::Value::String(sku.clone())),
                        ..Default::default()
                    },
                )
                .await;
                match updated {
                    Ok(p) => {
                        report.add(
                            row_no,
                            if sku.is_empty() { &name } else { &sku },
                            "updated",
                            if notes.is_empty() {
                                None
                            } else {
                                Some(notes.join(" "))
                            },
                        );
                        p.id
                    }
                    Err(e) => {
                        report.add(row_no, &sku, "failed", Some(e.message.clone()));
                        continue;
                    }
                }
            }
            None => {
                let created = create_product(
                    db,
                    ProductInput {
                        name: Some(serde_json::Value::String(name.clone())),
                        description: Some(serde_json::Value::String(full_description)),
                        price_minor: Some(serde_json::json!(price_minor)),
                        currency: Some(serde_json::Value::String("NGN".to_string())),
                        stock: Some(serde_json::json!(stock)),
                        status: Some(serde_json::Value::String(status.to_string())),
                        product_type: Some(serde_json::Value::String(r#type.clone())),
                        sku: Some(serde_json::Value::String(sku.clone())),
                        ..Default::default()
                    },
                )
                .await;
                match created {
                    Ok(p) => {
                        report.add(
                            row_no,
                            if sku.is_empty() { &name } else { &sku },
                            "imported",
                            if notes.is_empty() {
                                None
                            } else {
                                Some(notes.join(" "))
                            },
                        );
                        p.id
                    }
                    Err(e) => {
                        report.add(row_no, &sku, "failed", Some(e.message.clone()));
                        continue;
                    }
                }
            }
        };
        // Categories / tags replace the product's terms on update.
        let cats = split_list_cell(col(&headers, row, "Categories"));
        let tags = split_list_cell(col(&headers, row, "Tags"));
        let _ = set_product_terms(db, product_id, "product_cat", &cats).await;
        let _ = set_product_terms(db, product_id, "product_tag", &tags).await;
        // Attributes (parent rows carry the full value lists).
        if r#type == "variable" {
            let attrs = row_attributes(&headers, row);
            // Replace attributes wholesale on re-import.
            let _ = db
                .prepare("DELETE FROM wl_woo_attributes WHERE product_id = ?")
                .bind(vec![Param::Integer(product_id)])
                .run()
                .await;
            for (attr_name, values) in attrs {
                if values.is_empty() {
                    continue;
                }
                let _ = create_attribute(
                    db,
                    product_id,
                    AttributeInput {
                        name: Some(serde_json::Value::String(attr_name)),
                        options: Some(serde_json::Value::Array(
                            values.into_iter().map(serde_json::Value::String).collect(),
                        )),
                        ..Default::default()
                    },
                )
                .await;
            }
        }
    }
    // Pass 2: variations.
    for (row_no, i) in variation_rows {
        let row = &rows[i];
        let sku = col(&headers, row, "SKU").trim().to_string();
        let parent_ref = col(&headers, row, "Parent").trim().to_string();
        if parent_ref.is_empty() {
            report.add(
                row_no,
                &sku,
                "failed",
                Some("A variation row needs a Parent SKU.".to_string()),
            );
            continue;
        }
        if let Some(id_part) = parent_ref.strip_prefix("id:") {
            report.add(
                row_no,
                &sku,
                "failed",
                Some(format!(
                    "Parent \"id:{id_part}\" uses a WooCommerce id — re-export with SKUs (the parent SKU is the stable key)."
                )),
            );
            continue;
        }
        let Some(parent) = get_product_by_sku(db, &parent_ref).await? else {
            report.add(
                row_no,
                &sku,
                "failed",
                Some(format!(
                    "Parent SKU \"{parent_ref}\" was not imported — parent rows must come before their variations."
                )),
            );
            continue;
        };
        if parent.product_type != "variable" {
            report.add(
                row_no,
                &sku,
                "failed",
                Some(format!(
                    "Parent \"{parent_ref}\" is not a variable product."
                )),
            );
            continue;
        }
        let regular = match parse_major_price(col(&headers, row, "Regular price")) {
            Ok(v) => v,
            Err(e) => {
                report.add(row_no, &sku, "failed", Some(format!("Regular price: {e}")));
                continue;
            }
        };
        let sale = match parse_major_price(col(&headers, row, "Sale price")) {
            Ok(v) => v,
            Err(e) => {
                report.add(row_no, &sku, "failed", Some(format!("Sale price: {e}")));
                continue;
            }
        };
        let stock = match col(&headers, row, "Stock").trim() {
            "" => {
                if is_truthy(col(&headers, row, "In stock?"))
                    || col(&headers, row, "In stock?").trim().is_empty()
                {
                    -1
                } else {
                    0
                }
            }
            s => match parse_int_cell(s) {
                Some(v) => v,
                None => {
                    report.add(
                        row_no,
                        &sku,
                        "failed",
                        Some(format!("Stock \"{s}\" is not a number.")),
                    );
                    continue;
                }
            },
        };
        let (status, _) = published_status(col(&headers, row, "Published"));
        // One value per attribute column on a variation row.
        let mut attributes = serde_json::Map::new();
        for (attr_name, values) in row_attributes(&headers, row) {
            if let Some(first) = values.into_iter().next() {
                attributes.insert(attr_name, serde_json::Value::String(first));
            }
        }
        let input = VariationInput {
            sku: Some(serde_json::Value::String(sku.clone())),
            status: Some(serde_json::Value::String(status.to_string())),
            price_minor: Some(serde_json::json!(regular)),
            sale_price_minor: Some(serde_json::json!(sale)),
            stock: Some(serde_json::json!(stock)),
            attributes: Some(serde_json::Value::Object(attributes)),
            ..Default::default()
        };
        // Match an existing variation by SKU for updates.
        let existing = get_variation_by_sku(db, &sku).await?;
        let key = if sku.is_empty() {
            format!("variation row {row_no}")
        } else {
            sku.clone()
        };
        match existing {
            Some((_, vid)) => match update_variation(db, parent.id, vid, input).await {
                Ok(_) => report.add(row_no, &key, "updated", None),
                Err(e) => report.add(row_no, &key, "failed", Some(e.message.clone())),
            },
            None => match create_variation(db, parent.id, input).await {
                Ok(_) => report.add(row_no, &key, "imported", None),
                Err(e) => report.add(row_no, &key, "failed", Some(e.message.clone())),
            },
        }
        // A variation row may introduce a value the parent did not
        // list; the attribute is a variation attribute from now on.
        let _ = sync_parent_price(db, parent.id).await;
    }
    Ok(report)
}

// ---------------------------------------------------------------- orders

/// WooCommerce order status → Wordless order status. The `wc-`
/// prefix is stripped first.
fn map_woo_order_status(raw: &str) -> Option<&'static str> {
    let s = raw.trim().to_ascii_lowercase();
    let s = s.strip_prefix("wc-").unwrap_or(&s);
    match s {
        "completed" => Some("fulfilled"),
        "processing" => Some("paid"),
        "on-hold" | "pending" => Some("pending"),
        "cancelled" | "refunded" | "failed" => Some("cancelled"),
        _ => None,
    }
}

/// Parse one `line_item_N` cell: `Name|SKU|Quantity|Price` (price
/// decimal major). Returns (name, sku, quantity, price_minor).
fn parse_line_item(cell: &str) -> Result<(String, String, i64, i64), String> {
    let parts: Vec<&str> = cell.split('|').collect();
    if parts.len() != 4 {
        return Err(format!(
            "line item \"{cell}\" must be Name|SKU|Quantity|Price."
        ));
    }
    let name = parts[0].trim().to_string();
    let sku = parts[1].trim().to_string();
    let qty: i64 = parts[2].trim().parse().map_err(|_| {
        format!(
            "line item quantity \"{}\" is not a number.",
            parts[2].trim()
        )
    })?;
    if qty < 1 {
        return Err("line item quantity must be at least 1.".to_string());
    }
    let price = parse_major_price(parts[3])?;
    if name.is_empty() {
        return Err("line item needs a name.".to_string());
    }
    Ok((name, sku, qty, price))
}

async fn import_orders_csv(db: &dyn Db, csv_text: &str) -> Result<ImportReport, WordlessError> {
    let (headers, rows) = parse_csv(csv_text)?;
    let mut report = ImportReport::new("orders");
    for (i, row) in rows.iter().enumerate() {
        let row_no = (i + 1) as i64;
        let order_id = col(&headers, row, "order_id").trim().to_string();
        if order_id.is_empty() {
            report.add(
                row_no,
                "",
                "failed",
                Some("An order row needs an order_id.".to_string()),
            );
            continue;
        }
        // Idempotency: a repeat import of the same Woo order is a no-op.
        let existing: Option<Row> = db
            .prepare(
                "SELECT id FROM wl_orders WHERE source = 'woo-import' AND source_id = ? LIMIT 1",
            )
            .bind(vec![Param::Text(order_id.clone())])
            .first()
            .await?;
        if existing.is_some() {
            report.add(
                row_no,
                &order_id,
                "skipped",
                Some("This order was already imported.".to_string()),
            );
            continue;
        }
        let Some(status) = map_woo_order_status(col(&headers, row, "order_status")) else {
            report.add(
                row_no,
                &order_id,
                "failed",
                Some(format!(
                    "Order status \"{}\" has no Wordless equivalent.",
                    col(&headers, row, "order_status").trim()
                )),
            );
            continue;
        };
        let email = col(&headers, row, "customer_email").trim().to_string();
        if email.is_empty() {
            report.add(
                row_no,
                &order_id,
                "failed",
                Some("An order row needs a customer_email.".to_string()),
            );
            continue;
        }
        // Line items (line_item_1, line_item_2, …).
        let mut items: Vec<(String, String, i64, i64)> = Vec::new();
        let mut item_error: Option<String> = None;
        for n in 1..=100 {
            let cell = col(&headers, row, &format!("line_item_{n}"));
            if cell.trim().is_empty() {
                if n == 1 {
                    // No line_item_1 at all — keep looking only when a
                    // later column exists (sparse exports).
                    continue;
                }
                break;
            }
            match parse_line_item(cell) {
                Ok(item) => items.push(item),
                Err(e) => {
                    item_error = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = item_error {
            report.add(row_no, &order_id, "failed", Some(e));
            continue;
        }
        if items.is_empty() {
            report.add(
                row_no,
                &order_id,
                "failed",
                Some("The order has no line items.".to_string()),
            );
            continue;
        }
        let discount = parse_major_price(col(&headers, row, "discount_total")).unwrap_or(0);
        let tax = parse_major_price(col(&headers, row, "tax_total")).unwrap_or(0);
        let shipping = parse_major_price(col(&headers, row, "shipping_total")).unwrap_or(0);
        let items_total: i64 = items.iter().map(|(_, _, q, p)| q * p).sum();
        let total = match parse_major_price(col(&headers, row, "order_total")) {
            Ok(v) if v > 0 => v,
            _ => (items_total - discount + tax + shipping).max(0),
        };
        let currency = col(&headers, row, "order_currency")
            .trim()
            .to_ascii_uppercase();
        let currency = if currency.is_empty() {
            "NGN".to_string()
        } else {
            currency
        };
        let customer_name = format!(
            "{} {}",
            col(&headers, row, "customer_first_name").trim(),
            col(&headers, row, "customer_last_name").trim()
        )
        .trim()
        .to_string();
        let customer_name = if customer_name.is_empty() {
            email.clone()
        } else {
            customer_name
        };
        let created_at = to_iso_or_now(col(&headers, row, "order_date"));
        let stock_applied = if status == "paid" || status == "fulfilled" {
            1
        } else {
            0
        };
        let id = new_uuid_v4()?;
        let now = now_iso();
        let payment_method = col(&headers, row, "payment_method").trim().to_string();
        let transaction_id = col(&headers, row, "transaction_id").trim().to_string();
        let shipping_method = col(&headers, row, "shipping_method").trim().to_string();
        let inserted = db
            .prepare(
                "INSERT INTO wl_orders (id, customer_name, customer_email, total_minor, currency, status, payment_provider, payment_reference, stock_applied, coupon_code, discount_minor, tax_minor, shipping_minor, shipping_method, source, source_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, 'woo-import', ?, ?, ?)",
            )
            .bind(vec![
                Param::Text(id.clone()),
                Param::Text(customer_name.clone()),
                Param::Text(email.clone()),
                Param::Integer(total),
                Param::Text(currency),
                Param::Text(status.to_string()),
                Param::Text(payment_method),
                Param::Text(transaction_id),
                Param::Integer(stock_applied),
                Param::Integer(discount),
                Param::Integer(tax),
                Param::Integer(shipping),
                Param::Text(shipping_method),
                Param::Text(order_id.clone()),
                Param::Text(created_at),
                Param::Text(now.clone()),
            ])
            .run()
            .await;
        if let Err(e) = inserted {
            report.add(row_no, &order_id, "failed", Some(e.to_string()));
            continue;
        }
        for (name, sku, qty, price) in &items {
            // Resolve the SKU: variation first, then product, else 0
            // (the item is still recorded with its name/price — the
            // audit trail survives even when the catalogue does not
            // have that product).
            let (product_id, variation_id) = if sku.is_empty() {
                (0, None)
            } else if let Some((pid, vid)) = get_variation_by_sku(db, sku).await? {
                (pid, Some(vid))
            } else if let Some(p) = get_product_by_sku(db, sku).await? {
                (p.id, None)
            } else {
                (0, None)
            };
            let label = match variation_id {
                Some(vid) => {
                    let vrow: Option<Row> = db
                        .prepare(
                            "SELECT attributes_json FROM wl_woo_variations WHERE id = ? LIMIT 1",
                        )
                        .bind(vec![Param::Integer(vid)])
                        .first()
                        .await?;
                    vrow.map(|r| {
                        let attrs: serde_json::Value =
                            serde_json::from_str(&row_text(&r, "attributes_json"))
                                .unwrap_or(serde_json::Value::Object(Default::default()));
                        crate::variations::variation_label(&attrs)
                    })
                    .unwrap_or_default()
                }
                None => String::new(),
            };
            let _ = db
                .prepare(
                    "INSERT INTO wl_order_items (order_id, product_id, name, price_minor, quantity, variation_id, variation_label, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(vec![
                    Param::Text(id.clone()),
                    Param::Integer(product_id),
                    Param::Text(name.clone()),
                    Param::Integer(*price),
                    Param::Integer(*qty),
                    match variation_id {
                        Some(v) => Param::Integer(v),
                        None => Param::Null,
                    },
                    Param::Text(label),
                    Param::Text(now.clone()),
                ])
                .run()
                .await;
        }
        // Customer record (paid/fulfilled orders count towards totals).
        let _ = upsert_customer(
            db,
            &email,
            col(&headers, row, "customer_first_name").trim(),
            col(&headers, row, "customer_last_name").trim(),
            col(&headers, row, "billing_country").trim(),
            col(&headers, row, "billing_state").trim(),
            col(&headers, row, "billing_postcode").trim(),
            "woo-import",
            &order_id,
        )
        .await;
        if status == "paid" || status == "fulfilled" {
            let _ = record_order_for_customer(db, &email, total).await;
        }
        report.add(row_no, &order_id, "imported", None);
    }
    Ok(report)
}

// ---------------------------------------------------------------- customers

async fn import_customers_csv(db: &dyn Db, csv_text: &str) -> Result<ImportReport, WordlessError> {
    let (headers, rows) = parse_csv(csv_text)?;
    let mut report = ImportReport::new("customers");
    for (i, row) in rows.iter().enumerate() {
        let row_no = (i + 1) as i64;
        let email = col(&headers, row, "email").trim().to_string();
        if email.is_empty() {
            report.add(
                row_no,
                "",
                "failed",
                Some("A customer row needs an email.".to_string()),
            );
            continue;
        }
        let existing: Option<Row> = db
            .prepare("SELECT id FROM wl_woo_customers WHERE email = ? LIMIT 1")
            .bind(vec![Param::Text(email.clone())])
            .first()
            .await?;
        let was_existing = existing.is_some();
        let result = upsert_customer(
            db,
            &email,
            col(&headers, row, "first_name").trim(),
            col(&headers, row, "last_name").trim(),
            col(&headers, row, "billing_country").trim(),
            col(&headers, row, "billing_state").trim(),
            col(&headers, row, "billing_postcode").trim(),
            "woo-import",
            "",
        )
        .await;
        match result {
            Ok(_) => report.add(
                row_no,
                &email,
                if was_existing { "updated" } else { "imported" },
                None,
            ),
            Err(e) => report.add(row_no, &email, "failed", Some(e.message.clone())),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDb;
    use futures_executor::block_on;

    const PRODUCTS_CSV: &str = "ID,Type,SKU,Name,Published,Is featured?,Visibility in catalog,Short description,Description,Tax status,Tax class,In stock?,Stock,Backorders allowed?,Sold individually?,Weight (kg),Length (cm),Width (cm),Height (cm),Allow customer reviews?,Sale price,Regular price,Categories,Tags,Shipping class,Images,Parent,Position,Attribute 1 name,Attribute 1 value(s),Attribute 1 visible,Attribute 1 global,Attribute 1 default,Attribute 2 name,Attribute 2 value(s),Attribute 2 visible,Attribute 2 global,Attribute 2 default\n\
,simple,TEE-001,Plain Tee,1,0,visible,\"A plain tee.\",\"\",taxable,,1,10,0,0,,,,,1,9.99,12.99,Clothing,tee,,,,,,,,,,,,,,\n\
,variable,HOODIE,Pack Hoodie,1,0,visible,\"A hoodie.\",\"\",taxable,,1,,0,0,,,,,1,,,Clothing,hoodie,,,,,Size,\"S, M, L\",1,0,,Colour,\"Red, Blue\",1,0,\n\
,variation,HOODIE-S,,1,0,visible,\"\",\"\",taxable,,1,5,0,0,,,,,1,,12.99,,,,,HOODIE,,Size,S,1,0,,Colour,Red,1,0,\n\
,variation,HOODIE-M,,1,0,visible,\"\",\"\",taxable,,1,3,0,0,,,,,1,,14.99,,,,,HOODIE,,Size,M,1,0,,Colour,Blue,1,0,\n";

    #[test]
    fn csv_parser_handles_quotes_commas_and_crlf() {
        let (headers, rows) =
            parse_csv("a,b,c\r\n\"x, y\",\"say \"\"hi\"\"\",3\nplain,2,1\n").unwrap();
        assert_eq!(headers, vec!["a", "b", "c"]);
        assert_eq!(rows[0], vec!["x, y", "say \"hi\"", "3"]);
        assert_eq!(rows[1], vec!["plain", "2", "1"]);
        assert_eq!(parse_major_price("24.99").unwrap(), 2499);
        assert_eq!(parse_major_price("24.9").unwrap(), 2490);
        assert_eq!(parse_major_price("24").unwrap(), 2400);
        assert_eq!(parse_major_price("24.999").unwrap(), 2500);
        assert_eq!(parse_major_price("").unwrap(), 0);
        assert!(parse_major_price("abc").is_err());
    }

    #[test]
    fn imports_products_and_variations_from_woo_csv() {
        let db = TestDb::new();
        let report = block_on(import_products_csv(&db, PRODUCTS_CSV)).unwrap();
        assert_eq!(report.imported, 4, "{:?}", report.rows);
        assert_eq!(report.failed, 0, "{:?}", report.rows);
        let hoodie = block_on(get_product_by_sku(&db, "HOODIE"))
            .unwrap()
            .unwrap();
        assert_eq!(hoodie.product_type, "variable");
        let detail = block_on(crate::products::get_woo_product(&db, hoodie.id))
            .unwrap()
            .unwrap();
        assert_eq!(detail.attributes.len(), 2);
        assert_eq!(detail.variations.len(), 2);
        // Parent price is the minimum effective variation price.
        assert_eq!(detail.product.price_minor, 1299);
        let small = detail
            .variations
            .iter()
            .find(|v| v.sku == "HOODIE-S")
            .unwrap();
        assert_eq!(small.stock, 5);
        assert_eq!(small.label, "Colour: Red, Size: S");
        assert_eq!(detail.categories, vec!["Clothing"]);
        assert_eq!(detail.tags, vec!["hoodie"]);
        let tee = block_on(get_product_by_sku(&db, "TEE-001"))
            .unwrap()
            .unwrap();
        assert_eq!(tee.price_minor, 999); // sale price 9.99 applied
        assert_eq!(tee.stock, 10);
    }

    #[test]
    fn reimport_updates_and_reports_it() {
        let db = TestDb::new();
        block_on(import_products_csv(&db, PRODUCTS_CSV)).unwrap();
        let report = block_on(import_products_csv(&db, PRODUCTS_CSV)).unwrap();
        assert_eq!(report.updated, 4, "{:?}", report.rows);
        assert_eq!(report.imported, 0);
    }

    const ORDERS_CSV: &str = "order_id,order_number,order_date,order_status,customer_email,customer_first_name,customer_last_name,billing_country,billing_state,billing_postcode,payment_method,transaction_id,order_currency,order_total,discount_total,tax_total,shipping_total,shipping_method,line_item_1,line_item_2\n\
1001,1001,2026-01-15 10:30:00,wc-completed,ada@example.test,Ada,Lovelace,NG,LA,100001,paystack,txn_1001,NGN,70.96,0,12.99,25.00,Flat rate,\"Pack Hoodie|HOODIE-S|1|12.99\",\"Plain Tee|TEE-001|2|9.99\"\n\
1002,1002,2026-01-16 09:00:00,processing,carol@example.test,Carol,,NG,OY,,mock,mock_1002,NGN,14.99,0,0,0,,\"Pack Hoodie|HOODIE-M|1|14.99\",\n";

    #[test]
    fn imports_orders_with_status_map_and_line_items() {
        let db = TestDb::new();
        block_on(import_products_csv(&db, PRODUCTS_CSV)).unwrap();
        let report = block_on(import_orders_csv(&db, ORDERS_CSV)).unwrap();
        assert_eq!(report.imported, 2, "{:?}", report.rows);
        assert_eq!(report.failed, 0, "{:?}", report.rows);
        let orders = block_on(wordless_commerce::orders::list_orders(&db, None, 1, 20)).unwrap();
        assert_eq!(orders.total, 2);
        let first = orders
            .orders
            .iter()
            .find(|o| o.source_id.as_deref() == Some("1001"))
            .unwrap();
        assert_eq!(first.status, "fulfilled");
        assert_eq!(first.total_minor, 7096); // 32.97 items + 12.99 tax + 25.00 shipping
        assert_eq!(first.items.len(), 2);
        assert_eq!(first.source.as_deref(), Some("woo-import"));
        assert!(first.stock_applied); // paid/fulfilled: stock already taken
        let second = orders
            .orders
            .iter()
            .find(|o| o.source_id.as_deref() == Some("1002"))
            .unwrap();
        assert_eq!(second.status, "paid");
        // Re-import is a no-op.
        let report = block_on(import_orders_csv(&db, ORDERS_CSV)).unwrap();
        assert_eq!(report.skipped, 2);
    }

    const CUSTOMERS_CSV: &str = "email,first_name,last_name,username,date_created,billing_country,billing_state,billing_postcode\n\
ada@example.test,Ada,Lovelace,ada,2026-01-10,NG,LA,100001\n\
carol@example.test,Carol,,carol,2026-01-11,NG,OY,\n";

    #[test]
    fn imports_customers_by_email() {
        let db = TestDb::new();
        let report = block_on(import_customers_csv(&db, CUSTOMERS_CSV)).unwrap();
        assert_eq!(report.imported, 2, "{:?}", report.rows);
        let report = block_on(import_customers_csv(&db, CUSTOMERS_CSV)).unwrap();
        assert_eq!(report.updated, 2);
    }
}
