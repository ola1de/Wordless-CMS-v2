//! Shipping (P8) — `wl_woo_shipping_zones` + `wl_woo_shipping_methods`.
//!
//! A zone is a name plus a country list (an empty list = rest of the
//! world). A method is `flat_rate` (a fixed minor-unit cost) or
//! `free_shipping` (cost 0 once the order subtotal after discount
//! reaches `min_order_minor`). Quote rule: the zone is the first zone
//! whose country list contains the customer's country, else the
//! rest-of-world zone; the method is the one the customer picked
//! (which must belong to that zone) or, when none is picked, the
//! cheapest applicable method — free shipping when its threshold is
//! met, else the cheapest flat rate. No zone or no method means no
//! shipping charge (the store ships free by default rather than
//! blocking the sale — the owner is told in the quote).

use serde_json::Value;
use wordless_commerce::money::require_minor_units;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;
use wordless_kernel::validate::require_string;

use crate::types::{ShippingMethod, ShippingMethodInput, ShippingZone, ShippingZoneInput};

pub(crate) fn row_to_zone(row: &Row) -> ShippingZone {
    let countries: Vec<String> =
        serde_json::from_str(&row_text(row, "countries_json")).unwrap_or_default();
    ShippingZone {
        id: row_int(row, "id"),
        name: row_text(row, "name"),
        countries,
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

pub(crate) fn row_to_method(row: &Row) -> ShippingMethod {
    ShippingMethod {
        id: row_int(row, "id"),
        zone_id: row_int(row, "zone_id"),
        name: row_text(row, "name"),
        r#type: row_text(row, "type"),
        cost_minor: row_int(row, "cost_minor"),
        min_order_minor: row_int(row, "min_order_minor"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// The quote for a country + subtotal: the method that applies and
/// its cost, or `None` when the store has no shipping configured for
/// that country (free by default — see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct ShippingQuote {
    pub method: Option<ShippingMethod>,
    pub cost_minor: i64,
    /// Plain-language note when no method applies.
    pub note: Option<String>,
}

fn require_countries(value: &Value) -> Result<Vec<String>, WordlessError> {
    let Value::Array(items) = value else {
        return Err(bad_request(
            "Field \"countries\" must be an array of 2-letter country codes.",
        ));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(s) = item.as_str() else {
            return Err(bad_request(
                "Field \"countries\" must be an array of 2-letter country codes.",
            ));
        };
        let s = s.trim().to_ascii_uppercase();
        if s.len() != 2 || !s.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(bad_request(&format!(
                "\"{s}\" is not a 2-letter country code."
            )));
        }
        if !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(out)
}

/// All zones, with their methods loaded (used by the admin UI).
pub async fn list_zones_with_methods(
    db: &dyn Db,
) -> Result<Vec<(ShippingZone, Vec<ShippingMethod>)>, WordlessError> {
    let zones = db
        .prepare("SELECT * FROM wl_woo_shipping_zones ORDER BY id ASC")
        .bind(vec![])
        .all::<Row>()
        .await?;
    let mut out = Vec::with_capacity(zones.len());
    for z in &zones {
        let zone = row_to_zone(z);
        let methods = list_methods(db, zone.id).await?;
        out.push((zone, methods));
    }
    Ok(out)
}

/// Methods of one zone.
pub async fn list_methods(db: &dyn Db, zone_id: i64) -> Result<Vec<ShippingMethod>, WordlessError> {
    let rows = db
        .prepare("SELECT * FROM wl_woo_shipping_methods WHERE zone_id = ? ORDER BY id ASC")
        .bind(vec![Param::Integer(zone_id)])
        .all::<Row>()
        .await?;
    Ok(rows.iter().map(row_to_method).collect())
}

/// Create a zone.
pub async fn create_zone(
    db: &dyn Db,
    input: ShippingZoneInput,
) -> Result<ShippingZone, WordlessError> {
    let name = require_string(input.name.as_ref().unwrap_or(&Value::Null), "name", 1, 100)?;
    let countries = match input.countries.as_ref() {
        Some(v) => require_countries(v)?,
        None => Vec::new(),
    };
    let now = now_iso();
    let countries_json = serde_json::to_string(&countries)
        .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    let meta = db
        .prepare(
            "INSERT INTO wl_woo_shipping_zones (name, countries_json, created_at, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Text(name),
            Param::Text(countries_json),
            Param::Text(now.clone()),
            Param::Text(now),
        ])
        .run()
        .await?;
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_shipping_zones WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(meta.last_row_id.unwrap_or(0))])
        .first()
        .await?;
    row.map(|r| row_to_zone(&r))
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Shipping zone was not created."))
}

/// Update a zone (partial: absent fields keep their value).
pub async fn update_zone(
    db: &dyn Db,
    id: i64,
    input: ShippingZoneInput,
) -> Result<ShippingZone, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_shipping_zones WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    let Some(row) = row else {
        return Err(not_found(&format!("Shipping zone {id} not found.")));
    };
    let existing = row_to_zone(&row);
    let name = match input.name.as_ref() {
        Some(v) if !v.is_null() => require_string(v, "name", 1, 100)?,
        _ => existing.name.clone(),
    };
    let countries = match input.countries.as_ref() {
        Some(v) if !v.is_null() => require_countries(v)?,
        _ => existing.countries.clone(),
    };
    let countries_json = serde_json::to_string(&countries)
        .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    db.prepare(
        "UPDATE wl_woo_shipping_zones SET name = ?, countries_json = ?, updated_at = ? WHERE id = ?",
    )
    .bind(vec![
        Param::Text(name),
        Param::Text(countries_json),
        Param::Text(now_iso()),
        Param::Integer(id),
    ])
    .run()
    .await?;
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_shipping_zones WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    row.map(|r| row_to_zone(&r))
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Shipping zone was not updated."))
}

/// Delete a zone (and its methods).
pub async fn delete_zone(db: &dyn Db, id: i64) -> Result<bool, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT id FROM wl_woo_shipping_zones WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    if row.is_none() {
        return Err(not_found(&format!("Shipping zone {id} not found.")));
    }
    db.prepare("DELETE FROM wl_woo_shipping_methods WHERE zone_id = ?")
        .bind(vec![Param::Integer(id)])
        .run()
        .await?;
    let meta = db
        .prepare("DELETE FROM wl_woo_shipping_zones WHERE id = ?")
        .bind(vec![Param::Integer(id)])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

/// Create a method in a zone.
pub async fn create_method(
    db: &dyn Db,
    zone_id: i64,
    input: ShippingMethodInput,
) -> Result<ShippingMethod, WordlessError> {
    let zone: Option<Row> = db
        .prepare("SELECT id FROM wl_woo_shipping_zones WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(zone_id)])
        .first()
        .await?;
    if zone.is_none() {
        return Err(not_found(&format!("Shipping zone {zone_id} not found.")));
    }
    let name = require_string(input.name.as_ref().unwrap_or(&Value::Null), "name", 1, 100)?;
    let r#type = match input.r#type.as_ref() {
        Some(Value::String(s)) if s == "flat_rate" || s == "free_shipping" => s.clone(),
        Some(Value::Null) | None => "flat_rate".to_string(),
        _ => {
            return Err(bad_request(
                "Field \"type\" must be \"flat_rate\" or \"free_shipping\".",
            ))
        }
    };
    let cost_minor = match input.cost_minor.as_ref() {
        Some(v) => require_minor_units(v, "costMinor", 0)?,
        None => 0,
    };
    let min_order_minor = match input.min_order_minor.as_ref() {
        Some(v) => require_minor_units(v, "minOrderMinor", 0)?,
        None => 0,
    };
    if r#type == "free_shipping" && cost_minor != 0 {
        return Err(bad_request(
            "A free-shipping method has no cost — set the threshold with \"minOrderMinor\" instead.",
        ));
    }
    let now = now_iso();
    let meta = db
        .prepare(
            "INSERT INTO wl_woo_shipping_methods (zone_id, name, type, cost_minor, min_order_minor, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Integer(zone_id),
            Param::Text(name),
            Param::Text(r#type),
            Param::Integer(cost_minor),
            Param::Integer(min_order_minor),
            Param::Text(now.clone()),
            Param::Text(now),
        ])
        .run()
        .await?;
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_shipping_methods WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(meta.last_row_id.unwrap_or(0))])
        .first()
        .await?;
    row.map(|r| row_to_method(&r)).ok_or_else(|| {
        WordlessError::new("internal_error", 500, "Shipping method was not created.")
    })
}

/// Update a method (partial: absent fields keep their value).
pub async fn update_method(
    db: &dyn Db,
    zone_id: i64,
    method_id: i64,
    input: ShippingMethodInput,
) -> Result<ShippingMethod, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_shipping_methods WHERE id = ? AND zone_id = ? LIMIT 1")
        .bind(vec![Param::Integer(method_id), Param::Integer(zone_id)])
        .first()
        .await?;
    let Some(row) = row else {
        return Err(not_found(&format!(
            "Shipping method {method_id} not found."
        )));
    };
    let existing = row_to_method(&row);
    let name = match input.name.as_ref() {
        Some(v) if !v.is_null() => require_string(v, "name", 1, 100)?,
        _ => existing.name.clone(),
    };
    let r#type = match input.r#type.as_ref() {
        Some(Value::String(s)) if s == "flat_rate" || s == "free_shipping" => s.clone(),
        Some(Value::Null) | None => existing.r#type.clone(),
        _ => {
            return Err(bad_request(
                "Field \"type\" must be \"flat_rate\" or \"free_shipping\".",
            ))
        }
    };
    let cost_minor = match input.cost_minor.as_ref() {
        Some(v) if !v.is_null() => require_minor_units(v, "costMinor", 0)?,
        _ => existing.cost_minor,
    };
    let min_order_minor = match input.min_order_minor.as_ref() {
        Some(v) if !v.is_null() => require_minor_units(v, "minOrderMinor", 0)?,
        _ => existing.min_order_minor,
    };
    if r#type == "free_shipping" && cost_minor != 0 {
        return Err(bad_request(
            "A free-shipping method has no cost — set the threshold with \"minOrderMinor\" instead.",
        ));
    }
    db.prepare(
        "UPDATE wl_woo_shipping_methods SET name = ?, type = ?, cost_minor = ?, min_order_minor = ?, updated_at = ? WHERE id = ? AND zone_id = ?",
    )
    .bind(vec![
        Param::Text(name),
        Param::Text(r#type),
        Param::Integer(cost_minor),
        Param::Integer(min_order_minor),
        Param::Text(now_iso()),
        Param::Integer(method_id),
        Param::Integer(zone_id),
    ])
    .run()
    .await?;
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_shipping_methods WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(method_id)])
        .first()
        .await?;
    row.map(|r| row_to_method(&r)).ok_or_else(|| {
        WordlessError::new("internal_error", 500, "Shipping method was not updated.")
    })
}

/// Delete a method.
pub async fn delete_method(
    db: &dyn Db,
    zone_id: i64,
    method_id: i64,
) -> Result<bool, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT id FROM wl_woo_shipping_methods WHERE id = ? AND zone_id = ? LIMIT 1")
        .bind(vec![Param::Integer(method_id), Param::Integer(zone_id)])
        .first()
        .await?;
    if row.is_none() {
        return Err(not_found(&format!(
            "Shipping method {method_id} not found."
        )));
    }
    let meta = db
        .prepare("DELETE FROM wl_woo_shipping_methods WHERE id = ? AND zone_id = ?")
        .bind(vec![Param::Integer(method_id), Param::Integer(zone_id)])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

/// Quote shipping for a country + subtotal-after-discount. `method_id`
/// (when given) must belong to the matching zone, else a 400.
pub async fn quote_shipping(
    db: &dyn Db,
    country: &str,
    subtotal_after_discount_minor: i64,
    method_id: Option<i64>,
    currency: &str,
) -> Result<ShippingQuote, WordlessError> {
    let country = country.trim().to_ascii_uppercase();
    let zones = db
        .prepare("SELECT * FROM wl_woo_shipping_zones ORDER BY id ASC")
        .bind(vec![])
        .all::<Row>()
        .await?;
    // First zone whose list contains the country; else the
    // rest-of-world zone (empty list); else no shipping configured.
    let mut matched: Option<ShippingZone> = None;
    let mut rest_of_world: Option<ShippingZone> = None;
    for z in &zones {
        let zone = row_to_zone(z);
        if zone.countries.is_empty() {
            if rest_of_world.is_none() {
                rest_of_world = Some(zone);
            }
        } else if zone.countries.iter().any(|c| *c == country) && matched.is_none() {
            matched = Some(zone);
        }
    }
    let Some(zone) = matched.or(rest_of_world) else {
        return Ok(ShippingQuote {
            method: None,
            cost_minor: 0,
            note: Some("No shipping zones are configured — shipping is free.".to_string()),
        });
    };
    let methods = list_methods(db, zone.id).await?;
    if methods.is_empty() {
        return Ok(ShippingQuote {
            method: None,
            cost_minor: 0,
            note: Some(format!(
                "Zone \"{}\" has no shipping methods — shipping is free.",
                zone.name
            )),
        });
    }
    let applicable: Vec<&ShippingMethod> = methods
        .iter()
        .filter(|m| {
            m.r#type != "free_shipping" || subtotal_after_discount_minor >= m.min_order_minor
        })
        .collect();
    if let Some(wanted) = method_id {
        let Some(method) = methods.iter().find(|m| m.id == wanted) else {
            return Err(bad_request(&format!(
                "Shipping method {wanted} does not exist in zone \"{}\".",
                zone.name
            )));
        };
        if method.r#type == "free_shipping"
            && subtotal_after_discount_minor < method.min_order_minor
        {
            return Err(bad_request(&format!(
                "Free shipping \"{}\" needs an order of at least {}.",
                method.name,
                wordless_commerce::money::format_money(method.min_order_minor, currency)
            )));
        }
        return Ok(ShippingQuote {
            method: Some(method.clone()),
            cost_minor: method_cost(method, subtotal_after_discount_minor),
            note: None,
        });
    }
    // Cheapest applicable method wins (a met free-shipping threshold
    // costs 0, so it always wins when it applies).
    let Some(cheapest) = applicable
        .iter()
        .min_by_key(|m| method_cost(m, subtotal_after_discount_minor))
    else {
        return Ok(ShippingQuote {
            method: None,
            cost_minor: 0,
            note: Some(
                "No shipping method applies to this order yet (free-shipping threshold not met)."
                    .to_string(),
            ),
        });
    };
    Ok(ShippingQuote {
        method: Some((*cheapest).clone()),
        cost_minor: method_cost(cheapest, subtotal_after_discount_minor),
        note: None,
    })
}

fn method_cost(method: &ShippingMethod, subtotal_after_discount_minor: i64) -> i64 {
    if method.r#type == "free_shipping" {
        0
    } else {
        let _ = subtotal_after_discount_minor;
        method.cost_minor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDb;
    use futures_executor::block_on;
    use serde_json::json;

    fn zone_input(name: &str, countries: Vec<&str>) -> ShippingZoneInput {
        ShippingZoneInput {
            name: Some(json!(name)),
            countries: Some(json!(countries)),
        }
    }

    fn method_input(name: &str, r#type: &str, cost: i64, min_order: i64) -> ShippingMethodInput {
        ShippingMethodInput {
            name: Some(json!(name)),
            r#type: Some(json!(r#type)),
            cost_minor: Some(json!(cost)),
            min_order_minor: Some(json!(min_order)),
        }
    }

    #[test]
    fn cheapest_applicable_method_wins_and_free_threshold_applies() {
        let db = TestDb::new();
        let zone = block_on(create_zone(&db, zone_input("Nigeria", vec!["NG"]))).unwrap();
        block_on(create_method(
            &db,
            zone.id,
            method_input("Flat rate", "flat_rate", 250000, 0),
        ))
        .unwrap();
        block_on(create_method(
            &db,
            zone.id,
            method_input("Free over ₦50k", "free_shipping", 0, 5000000),
        ))
        .unwrap();
        // Below the threshold: flat rate.
        let q = block_on(quote_shipping(&db, "NG", 1000000, None, "NGN")).unwrap();
        assert_eq!(q.cost_minor, 250000);
        assert_eq!(q.method.as_ref().unwrap().name, "Flat rate");
        // At the threshold: free.
        let q = block_on(quote_shipping(&db, "NG", 5000000, None, "NGN")).unwrap();
        assert_eq!(q.cost_minor, 0);
        assert_eq!(q.method.as_ref().unwrap().name, "Free over ₦50k");
        // Explicit method from another zone is refused (no such method here).
        let err = block_on(quote_shipping(&db, "NG", 1000000, Some(9999), "NGN")).unwrap_err();
        assert!(err.message.contains("does not exist"), "{}", err.message);
    }

    #[test]
    fn no_zones_means_free_shipping_with_a_note() {
        let db = TestDb::new();
        let q = block_on(quote_shipping(&db, "NG", 1000000, None, "NGN")).unwrap();
        assert_eq!(q.cost_minor, 0);
        assert!(q.note.is_some());
    }
}
