//! Menus service — port of v1 `src/modules/menus/` (service.ts, repo.ts, types.ts).
//!
//! A menu is a term in the `nav_menu` taxonomy (same tables as
//! categories); a menu item is a `wp_posts` row of type
//! `nav_menu_item`, linked to its menu via `wp_term_relationships`.
//! Item fields live in `wp_postmeta`, the WordPress way
//! (_menu_item_type, _menu_item_url, _menu_item_object_id,
//! _menu_item_object). Sub-items use post_parent; ordering uses
//! menu_order. Menu locations (e.g. `primary`, `footer`) are stored
//! in the option `nav_menu_locations` as a JSON map location → menu
//! term taxonomy id.

use serde::Serialize;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};

use crate::postmeta::{get_post_meta_map, set_post_meta};
use crate::posts::find_post_by_id;
use crate::terms::{
    create_term, delete_term, find_term_by_taxonomy_id, list_terms, CreateTermInput,
    ListTermsOptions,
};

pub const NAV_MENU_TAXONOMY: &str = "nav_menu";
pub const MENU_LOCATIONS_OPTION: &str = "nav_menu_locations";

/// A menu (term taxonomy id of the `nav_menu` term).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Menu {
    pub id: i64,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MenuItemType {
    Custom,
    Post,
    Page,
    Term,
}

impl MenuItemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MenuItemType::Custom => "custom",
            MenuItemType::Post => "post",
            MenuItemType::Page => "page",
            MenuItemType::Term => "term",
        }
    }

    /// See `PostStatus::from_str` for why this is not `std::str::FromStr`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<MenuItemType> {
        match s {
            "custom" => Some(MenuItemType::Custom),
            "post" => Some(MenuItemType::Post),
            "page" => Some(MenuItemType::Page),
            "term" => Some(MenuItemType::Term),
            _ => None,
        }
    }
}

/// A menu item, serialised with the v1 camelCase field names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuItem {
    pub id: i64,
    /// Term taxonomy id of the menu this item belongs to.
    pub menu_id: i64,
    pub title: String,
    pub url: String,
    #[serde(rename = "type")]
    pub item_type: MenuItemType,
    /// Linked object id (post/page id or term taxonomy id) for non-custom items.
    pub object_id: i64,
    pub object_type: String,
    pub parent_id: i64,
    pub order: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<MenuItem>>,
}

#[derive(Debug, Clone)]
pub struct MenuItemRow {
    pub id: i64,
    pub title: String,
    pub parent_id: i64,
    pub order: i64,
    pub meta: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct CreateMenuItemInput {
    pub title: String,
    pub item_type: String,
    /// Required for `custom` items; ignored for post/page/term items (URL is resolved).
    pub url: Option<String>,
    pub object_id: Option<i64>,
    pub parent_id: Option<i64>,
    /// 0-based position; omitted = append at the end.
    pub position: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateMenuItemInput {
    pub title: Option<String>,
    pub url: Option<String>,
    pub parent_id: Option<i64>,
    pub position: Option<i64>,
}

fn row_to_menu_item(row: &MenuItemRow, menu_id: i64, resolved_url: String) -> MenuItem {
    let item_type = row
        .meta
        .get("_menu_item_type")
        .and_then(|t| MenuItemType::from_str(t))
        .unwrap_or(MenuItemType::Custom);
    MenuItem {
        id: row.id,
        menu_id,
        title: row.title.clone(),
        url: resolved_url,
        item_type,
        object_id: row
            .meta
            .get("_menu_item_object_id")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0),
        object_type: row
            .meta
            .get("_menu_item_object")
            .cloned()
            .unwrap_or_default(),
        parent_id: row.parent_id,
        order: row.order,
        children: None,
    }
}

async fn resolve_item_url(db: &dyn Db, site_id: i64, row: &MenuItemRow) -> Result<String, WordlessError> {
    let item_type = row
        .meta
        .get("_menu_item_type")
        .map(|s| s.as_str())
        .unwrap_or("custom");
    if item_type == "custom" {
        return Ok(row
            .meta
            .get("_menu_item_url")
            .cloned()
            .unwrap_or_else(|| "#".to_string()));
    }
    let object_id: i64 = row
        .meta
        .get("_menu_item_object_id")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if object_id == 0 {
        return Ok("#".to_string());
    }
    if item_type == "post" || item_type == "page" {
        let post = find_post_by_id(db, site_id, object_id).await?;
        return Ok(match post {
            Some(post) => crate::permalinks::post_url(db, site_id, &post).await?,
            None => "#".to_string(),
        });
    }
    if item_type == "term" {
        let term = find_term_by_taxonomy_id(db, site_id, object_id).await?;
        return Ok(match term {
            Some(term) => crate::permalinks::term_url(&term.taxonomy, &term.slug),
            None => "#".to_string(),
        });
    }
    Ok(row
        .meta
        .get("_menu_item_url")
        .cloned()
        .unwrap_or_else(|| "#".to_string()))
}

fn nest_items(items: Vec<MenuItem>) -> Vec<MenuItem> {
    let mut by_id: std::collections::HashMap<i64, MenuItem> = std::collections::HashMap::new();
    for item in &items {
        by_id.insert(
            item.id,
            MenuItem {
                children: Some(Vec::new()),
                ..item.clone()
            },
        );
    }
    for item in &items {
        if item.parent_id != 0 && by_id.contains_key(&item.parent_id) {
            let child = by_id.get(&item.id).cloned();
            if let (Some(child), Some(parent)) = (child, by_id.get_mut(&item.parent_id)) {
                parent.children.get_or_insert_with(Vec::new).push(child);
            }
        }
    }
    fn rebuild(map: &std::collections::HashMap<i64, MenuItem>, id: i64) -> Option<MenuItem> {
        let node = map.get(&id)?;
        let mut out = node.clone();
        if let Some(children) = &node.children {
            out.children = Some(children.iter().filter_map(|c| rebuild(map, c.id)).collect());
        }
        Some(out)
    }
    let mut roots: Vec<MenuItem> = Vec::new();
    for item in &items {
        if item.parent_id == 0 || !by_id.contains_key(&item.parent_id) {
            if let Some(root) = rebuild(&by_id, item.id) {
                roots.push(root);
            }
        }
    }
    roots
}

// ---------------------------------------------------------------- repo

async fn insert_menu_item_row(
    db: &dyn Db,
    site_id: i64,
    title: &str,
    parent_id: i64,
    order: i64,
) -> Result<i64, WordlessError> {
    let meta = db
        .prepare(
            "INSERT INTO wp_posts
               (site_id, post_author, post_date, post_date_gmt, post_content, post_title, post_excerpt,
                post_status, post_name, post_modified, post_modified_gmt, post_parent,
                post_type, post_mime_type, menu_order)
             VALUES (?, 0, datetime('now'), datetime('now'), '', ?, '', 'publish', '', datetime('now'), datetime('now'), ?, 'nav_menu_item', '', ?)",
        )
        .bind(vec![
            Param::Integer(site_id),
            Param::Text(title.to_string()),
            Param::Integer(parent_id),
            Param::Integer(order),
        ])
        .run()
        .await?;
    Ok(meta.last_row_id.unwrap_or(0))
}

async fn link_item_to_menu(
    db: &dyn Db,
    site_id: i64,
    item_id: i64,
    menu_term_taxonomy_id: i64,
) -> Result<(), WordlessError> {
    db.prepare("INSERT OR IGNORE INTO wp_term_relationships (site_id, object_id, term_taxonomy_id, term_order) VALUES (?, ?, ?, 0)")
        .bind(vec![Param::Integer(site_id), Param::Integer(item_id), Param::Integer(menu_term_taxonomy_id)])
        .run()
        .await?;
    Ok(())
}

pub async fn find_menu_item_rows(
    db: &dyn Db,
    site_id: i64,
    menu_term_taxonomy_id: i64,
) -> Result<Vec<MenuItemRow>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare(
            "SELECT p.ID, p.post_title, p.post_parent, p.menu_order
               FROM wp_posts p
               JOIN wp_term_relationships tr ON tr.object_id = p.ID
              WHERE tr.term_taxonomy_id = ? AND p.post_type = 'nav_menu_item'
                AND p.site_id = ? AND tr.site_id = ?
              ORDER BY p.menu_order ASC, p.ID ASC",
        )
        .bind(vec![Param::Integer(menu_term_taxonomy_id), Param::Integer(site_id), Param::Integer(site_id)])
        .all()
        .await?;
    let mut items = Vec::new();
    for row in &rows {
        let id = row_int(row, "ID");
        items.push(MenuItemRow {
            id,
            title: row_text(row, "post_title"),
            parent_id: row_int(row, "post_parent"),
            order: row_int(row, "menu_order"),
            meta: get_post_meta_map(db, site_id, id).await?,
        });
    }
    Ok(items)
}

pub async fn find_menu_item_row(
    db: &dyn Db,
    site_id: i64,
    item_id: i64,
) -> Result<Option<MenuItemRow>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT ID, post_title, post_parent, menu_order FROM wp_posts WHERE ID = ? AND post_type = 'nav_menu_item' AND site_id = ? LIMIT 1")
        .bind(vec![Param::Integer(item_id), Param::Integer(site_id)])
        .first()
        .await?;
    match row {
        None => Ok(None),
        Some(row) => {
            let id = row_int(&row, "ID");
            Ok(Some(MenuItemRow {
                id,
                title: row_text(&row, "post_title"),
                parent_id: row_int(&row, "post_parent"),
                order: row_int(&row, "menu_order"),
                meta: get_post_meta_map(db, site_id, id).await?,
            }))
        }
    }
}

async fn max_menu_order(db: &dyn Db, site_id: i64, menu_term_taxonomy_id: i64) -> Result<i64, WordlessError> {
    let row: Option<Row> = db
        .prepare(
            "SELECT MAX(p.menu_order) AS m FROM wp_posts p
               JOIN wp_term_relationships tr ON tr.object_id = p.ID
              WHERE tr.term_taxonomy_id = ? AND p.post_type = 'nav_menu_item'
                AND p.site_id = ? AND tr.site_id = ?",
        )
        .bind(vec![Param::Integer(menu_term_taxonomy_id), Param::Integer(site_id), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(row.map(|r| row_int(&r, "m")).unwrap_or(0))
}

async fn update_menu_item_row(
    db: &dyn Db,
    site_id: i64,
    item_id: i64,
    title: Option<&str>,
    parent_id: Option<i64>,
    order: Option<i64>,
) -> Result<u64, WordlessError> {
    let mut sets: Vec<String> = Vec::new();
    let mut params: Vec<Param> = Vec::new();
    if let Some(t) = title {
        sets.push("post_title = ?".to_string());
        params.push(Param::Text(t.to_string()));
    }
    if let Some(p) = parent_id {
        sets.push("post_parent = ?".to_string());
        params.push(Param::Integer(p));
    }
    if let Some(o) = order {
        sets.push("menu_order = ?".to_string());
        params.push(Param::Integer(o));
    }
    if sets.is_empty() {
        return Ok(0);
    }
    params.push(Param::Integer(item_id));
    params.push(Param::Integer(site_id));
    let meta = db
        .prepare(&format!(
            "UPDATE wp_posts SET {} WHERE ID = ? AND post_type = 'nav_menu_item' AND site_id = ?",
            sets.join(", ")
        ))
        .bind(params)
        .run()
        .await?;
    Ok(meta.changes)
}

async fn delete_menu_item_row(db: &dyn Db, site_id: i64, item_id: i64) -> Result<u64, WordlessError> {
    db.prepare("DELETE FROM wp_postmeta WHERE post_id = ? AND site_id = ?")
        .bind(vec![Param::Integer(item_id), Param::Integer(site_id)])
        .run()
        .await?;
    db.prepare("DELETE FROM wp_term_relationships WHERE object_id = ? AND site_id = ?")
        .bind(vec![Param::Integer(item_id), Param::Integer(site_id)])
        .run()
        .await?;
    // Children of a deleted item become top-level items.
    db.prepare(
        "UPDATE wp_posts SET post_parent = 0 WHERE post_parent = ? AND post_type = 'nav_menu_item' AND site_id = ?",
    )
    .bind(vec![Param::Integer(item_id), Param::Integer(site_id)])
    .run()
    .await?;
    let meta = db
        .prepare("DELETE FROM wp_posts WHERE ID = ? AND post_type = 'nav_menu_item' AND site_id = ?")
        .bind(vec![Param::Integer(item_id), Param::Integer(site_id)])
        .run()
        .await?;
    Ok(meta.changes)
}

async fn delete_menu_items(db: &dyn Db, site_id: i64, menu_term_taxonomy_id: i64) -> Result<(), WordlessError> {
    let rows = find_menu_item_rows(db, site_id, menu_term_taxonomy_id).await?;
    for row in rows {
        delete_menu_item_row(db, site_id, row.id).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------- service

pub async fn create_menu(db: &dyn Db, site_id: i64, name: &str) -> Result<Menu, WordlessError> {
    let term = create_term(
        db,
        site_id,
        CreateTermInput {
            name: name.to_string(),
            taxonomy: NAV_MENU_TAXONOMY.to_string(),
            ..Default::default()
        },
    )
    .await?;
    Ok(Menu {
        id: term.term_taxonomy_id,
        name: term.name,
        slug: term.slug,
    })
}

pub async fn list_menus(db: &dyn Db, site_id: i64) -> Result<Vec<Menu>, WordlessError> {
    let result = list_terms(
        db,
        site_id,
        NAV_MENU_TAXONOMY,
        ListTermsOptions {
            per_page: Some(100),
            ..Default::default()
        },
    )
    .await?;
    Ok(result
        .terms
        .into_iter()
        .map(|t| Menu {
            id: t.term_taxonomy_id,
            name: t.name,
            slug: t.slug,
        })
        .collect())
}

pub async fn get_menu(db: &dyn Db, site_id: i64, menu_id: i64) -> Result<Menu, WordlessError> {
    let term = find_term_by_taxonomy_id(db, site_id, menu_id).await?;
    match term {
        Some(term) if term.taxonomy == NAV_MENU_TAXONOMY => Ok(Menu {
            id: term.term_taxonomy_id,
            name: term.name,
            slug: term.slug,
        }),
        _ => Err(not_found(&format!("Menu {menu_id} not found."))),
    }
}

pub async fn delete_menu(db: &dyn Db, site_id: i64, menu_id: i64) -> Result<bool, WordlessError> {
    get_menu(db, site_id, menu_id).await?;
    delete_menu_items(db, site_id, menu_id).await?;
    // Remove the menu from any location pointing at it.
    let mut locations = get_menu_locations(db, site_id).await?;
    let mut changed = false;
    locations.retain(|_, id| {
        if *id == menu_id {
            changed = true;
            false
        } else {
            true
        }
    });
    if changed {
        // P12: menu locations are a per-site option.
        wordless_multisite::set_site_option(db, site_id, MENU_LOCATIONS_OPTION, &locations).await?;
    }
    delete_term(db, site_id, menu_id).await?;
    Ok(true)
}

pub async fn add_menu_item(
    db: &dyn Db,
    site_id: i64,
    menu_id: i64,
    input: CreateMenuItemInput,
) -> Result<MenuItem, WordlessError> {
    get_menu(db, site_id, menu_id).await?;
    let title = input.title.trim().to_string();
    if title.is_empty() || title.len() > 200 {
        return Err(bad_request(
            "Field \"title\" is required and must be at most 200 characters.",
        ));
    }
    let item_type = MenuItemType::from_str(&input.item_type)
        .ok_or_else(|| bad_request("Field \"type\" must be one of custom, post, page, term."))?;
    let mut url = String::new();
    let mut object_id = 0i64;
    let mut object_type = String::new();
    match item_type {
        MenuItemType::Custom => {
            url = input.url.clone().unwrap_or_default().trim().to_string();
            if url.is_empty() {
                return Err(bad_request(
                    "Field \"url\" is required for custom menu items.",
                ));
            }
        }
        MenuItemType::Post | MenuItemType::Page | MenuItemType::Term => {
            object_id = input.object_id.unwrap_or(0);
            if object_id == 0 {
                return Err(bad_request(&format!(
                    "Field \"objectId\" is required for {} menu items.",
                    item_type.as_str()
                )));
            }
            match item_type {
                MenuItemType::Post | MenuItemType::Page => {
                    let post = find_post_by_id(db, site_id, object_id).await?;
                    let post =
                        post.ok_or_else(|| not_found(&format!("Post {object_id} not found.")))?;
                    if item_type == MenuItemType::Page && post.post_type != "page" {
                        return Err(bad_request(&format!("Post {object_id} is not a page.")));
                    }
                    if item_type == MenuItemType::Post && post.post_type != "post" {
                        return Err(bad_request(&format!("Post {object_id} is not a post.")));
                    }
                    object_type = post.post_type;
                }
                MenuItemType::Term => {
                    let term = find_term_by_taxonomy_id(db, site_id, object_id).await?;
                    let term =
                        term.ok_or_else(|| not_found(&format!("Term {object_id} not found.")))?;
                    object_type = term.taxonomy;
                }
                MenuItemType::Custom => unreachable!(),
            }
        }
    }
    let parent_id = input.parent_id.unwrap_or(0);
    if parent_id != 0 {
        let parent = find_menu_item_row(db, site_id, parent_id).await?;
        if parent.is_none() {
            return Err(not_found(&format!(
                "Parent menu item {parent_id} not found."
            )));
        }
    }
    let order = match input.position {
        Some(p) => p,
        None => max_menu_order(db, site_id, menu_id).await? + 1,
    };
    let id = insert_menu_item_row(db, site_id, &title, parent_id, order).await?;
    set_post_meta(db, site_id, id, "_menu_item_type", item_type.as_str()).await?;
    set_post_meta(db, site_id, id, "_menu_item_object", &object_type).await?;
    set_post_meta(db, site_id, id, "_menu_item_object_id", &object_id.to_string()).await?;
    set_post_meta(db, site_id, id, "_menu_item_url", &url).await?;
    link_item_to_menu(db, site_id, id, menu_id).await?;
    let row = find_menu_item_row(db, site_id, id).await?;
    let row = row.ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the created menu item.",
        )
    })?;
    let resolved = resolve_item_url(db, site_id, &row).await?;
    Ok(row_to_menu_item(&row, menu_id, resolved))
}

pub async fn list_menu_items(db: &dyn Db, site_id: i64, menu_id: i64) -> Result<Vec<MenuItem>, WordlessError> {
    get_menu(db, site_id, menu_id).await?;
    let rows = find_menu_item_rows(db, site_id, menu_id).await?;
    let mut items = Vec::new();
    for row in &rows {
        let resolved = resolve_item_url(db, site_id, row).await?;
        items.push(row_to_menu_item(row, menu_id, resolved));
    }
    Ok(nest_items(items))
}

pub async fn update_menu_item(
    db: &dyn Db,
    site_id: i64,
    menu_id: i64,
    item_id: i64,
    patch: UpdateMenuItemInput,
) -> Result<MenuItem, WordlessError> {
    get_menu(db, site_id, menu_id).await?;
    find_menu_item_row(db, site_id, item_id)
        .await?
        .ok_or_else(|| not_found(&format!("Menu item {item_id} not found.")))?;
    if let Some(parent_id) = patch.parent_id {
        if parent_id != 0 {
            if parent_id == item_id {
                return Err(bad_request("A menu item cannot be its own parent."));
            }
            let parent = find_menu_item_row(db, site_id, parent_id).await?;
            if parent.is_none() {
                return Err(not_found(&format!(
                    "Parent menu item {parent_id} not found."
                )));
            }
        }
    }
    let mut next_title: Option<String> = None;
    if let Some(title) = &patch.title {
        let title = title.trim().to_string();
        if title.is_empty() || title.len() > 200 {
            return Err(bad_request("Field \"title\" must be 1–200 characters."));
        }
        next_title = Some(title);
    }
    update_menu_item_row(
        db,
        site_id,
        item_id,
        next_title.as_deref(),
        patch.parent_id,
        patch.position,
    )
    .await?;
    if let Some(url) = &patch.url {
        set_post_meta(db, site_id, item_id, "_menu_item_url", url.trim()).await?;
    }
    let updated = find_menu_item_row(db, site_id, item_id).await?;
    let updated = updated.ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the updated menu item.",
        )
    })?;
    let resolved = resolve_item_url(db, site_id, &updated).await?;
    Ok(row_to_menu_item(&updated, menu_id, resolved))
}

pub async fn remove_menu_item(
    db: &dyn Db,
    site_id: i64,
    menu_id: i64,
    item_id: i64,
) -> Result<bool, WordlessError> {
    get_menu(db, site_id, menu_id).await?;
    let row = find_menu_item_row(db, site_id, item_id)
        .await?
        .ok_or_else(|| not_found(&format!("Menu item {item_id} not found.")))?;
    let _ = row;
    Ok(delete_menu_item_row(db, site_id, item_id).await? > 0)
}

/// Menu locations are a per-site option (P12): site 1 reads/writes
/// `wp_options` exactly as before; other sites use `wl_site_options`
/// with fallback to the network defaults.
pub async fn get_menu_locations(
    db: &dyn Db,
    site_id: i64,
) -> Result<std::collections::HashMap<String, i64>, WordlessError> {
    wordless_multisite::get_site_option::<std::collections::HashMap<String, i64>>(
        db,
        site_id,
        MENU_LOCATIONS_OPTION,
        Default::default(),
    )
    .await
}

pub async fn set_menu_location(
    db: &dyn Db,
    site_id: i64,
    location: &str,
    menu_id: Option<i64>,
) -> Result<std::collections::HashMap<String, i64>, WordlessError> {
    let loc = location.trim().to_lowercase();
    let valid = !loc.is_empty()
        && loc
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid {
        return Err(bad_request(
            "Field \"location\" must be a lowercase key (letters, digits, hyphen, underscore).",
        ));
    }
    if let Some(id) = menu_id {
        get_menu(db, site_id, id).await?;
    }
    let mut locations = get_menu_locations(db, site_id).await?;
    match menu_id {
        None => {
            locations.remove(&loc);
        }
        Some(id) => {
            locations.insert(loc, id);
        }
    }
    wordless_multisite::set_site_option(db, site_id, MENU_LOCATIONS_OPTION, &locations).await?;
    Ok(locations)
}

/// The `GET /api/menus/location/:location` payload.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuByLocation {
    pub location: String,
    pub menu: Option<Menu>,
    pub items: Vec<MenuItem>,
}

/// Resolve the menu assigned to a location, with its items (empty when unassigned).
pub async fn resolve_menu_by_location(
    db: &dyn Db,
    site_id: i64,
    location: &str,
) -> Result<MenuByLocation, WordlessError> {
    let locations = get_menu_locations(db, site_id).await?;
    let menu_id = locations.get(&location.trim().to_lowercase()).copied();
    match menu_id {
        None => Ok(MenuByLocation {
            location: location.to_string(),
            menu: None,
            items: Vec::new(),
        }),
        Some(id) => {
            let menu = get_menu(db, site_id, id).await?;
            let items = list_menu_items(db, site_id, id).await?;
            Ok(MenuByLocation {
                location: location.to_string(),
                menu: Some(menu),
                items,
            })
        }
    }
}
