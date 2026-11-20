//! GraphQL execution over the same content model as REST.
//!
//! Root fields (queries only):
//! - `posts(status, type, authorId, page, perPage)` → PostList
//! - `post(id, slug)` → Post
//! - `terms(taxonomy, parentId, page, perPage)` → TermList
//! - `term(id, slug, taxonomy)` → Term
//! - `media(page, perPage, mimeType)` → MediaList (needs `upload_files`)
//! - `user(id)` / `users(page, perPage)` → User (needs `edit_users`)
//! - `option(key)` → JSON value (needs any logged-in user, like REST)
//! - `siteInfo` → `{ title, url }` (public)
//!
//! Guards mirror the REST routes exactly: the `posts` list passes its
//! `status` argument through unchanged (REST `GET /api/posts` does the
//! same — a v1 behaviour, preserved, not tightened here); a single
//! non-published `post` is visible only to a logged-in user (REST
//! `GET /api/posts/:slug` rule); `media`/`user`/`users`/`option` carry
//! the same capability requirements as their REST counterparts.
//! A GraphQL query therefore returns the same post a REST call returns,
//! under the same guard — the P3 done-when.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use wordless_auth::{get_user_by_id, list_users, User};
use wordless_cms::posts::{get_post_by_slug_or_id, list_posts, ListPostsOptions, Post, PostStatus};
use wordless_cms::terms::{get_post_terms, get_term_by_slug, list_terms, ListTermsOptions, Term};
use wordless_db::Db;
use wordless_kernel::capabilities::{user_can, AuthUser};
use wordless_media::service::{list_media, ListMediaOpts, MediaRecord};

use crate::parser::{parse_query, Document, Field, GqlValue, Selection};

/// The outcome of one GraphQL request: `data` plus (possibly empty)
/// `errors`, in the usual GraphQL shape. The worker wraps this in the
/// v1 `{ok, data, error}` envelope.
#[derive(Debug, Clone)]
pub struct GraphqlOutcome {
    pub data: Value,
    pub errors: Vec<String>,
}

/// Execute one GraphQL query string. `variables` is the optional JSON
/// object from the request body (`{"query": "...", "variables": {...}}`);
/// `operation_name` selects one operation when the document has
/// several (a document with several operations and no name is an
/// error). Never panics on bad input — parse/validation failures come
/// back as `data: null` plus an error entry.
pub async fn execute(
    db: &dyn Db,
    site_id: i64,
    user: Option<&AuthUser>,
    query: &str,
    variables: Option<Value>,
    operation_name: Option<&str>,
) -> GraphqlOutcome {
    let vars = variables.unwrap_or(Value::Null);
    let doc = match parse_query(query) {
        Ok(d) => d,
        Err(e) => {
            return GraphqlOutcome {
                data: Value::Null,
                errors: vec![e],
            };
        }
    };
    // One operation: it runs. Several: `operationName` selects one;
    // without it the request is an error (the client must name one).
    let op = if doc.operations.len() == 1 {
        &doc.operations[0]
    } else if let Some(name) = operation_name {
        match doc
            .operations
            .iter()
            .find(|o| o.name.as_deref() == Some(name))
        {
            Some(op) => op,
            None => {
                return GraphqlOutcome {
                    data: Value::Null,
                    errors: vec![format!("Unknown operation \"{name}\" in this query.")],
                }
            }
        }
    } else {
        return GraphqlOutcome {
            data: Value::Null,
            errors: vec![
                "Multiple operations in one request require an operationName; send one query per request"
                    .to_string(),
            ],
        };
    };
    // Resolve variable defaults, then caller-supplied values.
    let mut resolved_vars = serde_json::Map::new();
    for def in &op.variables {
        if let Some(d) = &def.default {
            resolved_vars.insert(def.name.clone(), d.to_json());
        }
    }
    if let Value::Object(map) = &vars {
        for (k, v) in map {
            resolved_vars.insert(k.clone(), v.clone());
        }
    }
    let vars_json = Value::Object(resolved_vars);

    let mut ctx = ExecCtx {
        db,
        site_id,
        user,
        doc: &doc,
        vars: &vars_json,
        errors: Vec::new(),
    };
    let mut data = serde_json::Map::new();
    for sel in &op.selection {
        expand_selection(&mut ctx, sel, &mut data, 0).await;
    }
    GraphqlOutcome {
        data: Value::Object(data),
        errors: ctx.errors,
    }
}

struct ExecCtx<'a> {
    db: &'a dyn Db,
    site_id: i64,
    user: Option<&'a AuthUser>,
    doc: &'a Document,
    vars: &'a Value,
    errors: Vec<String>,
}

const MAX_DEPTH: usize = 8;

/// A resolved value that still knows its type, so nested fields
/// (`Post.terms`, `Post.author`) can be resolved lazily.
enum Resolved {
    Json(Value),
    Post(Post),
    Term(Term),
    Media(MediaRecord),
    User(User),
    List(Vec<Resolved>),
}

impl Resolved {
    fn json(&self) -> Value {
        match self {
            Resolved::Json(v) => v.clone(),
            Resolved::Post(p) => serde_json::to_value(p).unwrap_or(Value::Null),
            Resolved::Term(t) => serde_json::to_value(t).unwrap_or(Value::Null),
            Resolved::Media(m) => m.to_json(),
            Resolved::User(u) => serde_json::to_value(u).unwrap_or(Value::Null),
            Resolved::List(items) => Value::Array(items.iter().map(|i| i.json()).collect()),
        }
    }
}

fn can(user: Option<&AuthUser>, capability: &str) -> bool {
    user.map(|u| user_can(u, capability)).unwrap_or(false)
}

fn arg_str(field: &Field, vars: &Value, name: &str) -> Result<Option<String>, String> {
    match field.arg(name) {
        None => Ok(None),
        Some(v) => {
            let r = v.resolve(vars)?;
            match r {
                GqlValue::Null => Ok(None),
                GqlValue::String(s) | GqlValue::Enum(s) => Ok(Some(s)),
                other => Err(format!("Argument '{name}' must be a string, got {other:?}")),
            }
        }
    }
}

fn arg_i64(field: &Field, vars: &Value, name: &str) -> Result<Option<i64>, String> {
    match field.arg(name) {
        None => Ok(None),
        Some(v) => {
            let r = v.resolve(vars)?;
            match r {
                GqlValue::Null => Ok(None),
                GqlValue::Int(n) => Ok(Some(n)),
                other => Err(format!(
                    "Argument '{name}' must be an integer, got {other:?}"
                )),
            }
        }
    }
}

fn page_of(v: Option<i64>) -> i64 {
    v.unwrap_or(1).clamp(1, 10000)
}

fn per_page_of(v: Option<i64>) -> i64 {
    v.unwrap_or(20).clamp(1, 100)
}

/// Expand one selection into `out` (a JSON object under construction).
async fn expand_selection(
    ctx: &mut ExecCtx<'_>,
    sel: &Selection,
    out: &mut serde_json::Map<String, Value>,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        ctx.errors
            .push("Selection nesting is too deep (max 8)".to_string());
        return;
    }
    match sel {
        Selection::Field(f) => {
            let value = resolve_root_field(ctx, f).await;
            match value {
                Ok(resolved) => {
                    let projected = project(ctx, resolved, &f.selection, depth + 1).await;
                    out.insert(f.response_key.clone(), projected);
                }
                Err(e) => {
                    ctx.errors.push(e);
                    out.insert(f.response_key.clone(), Value::Null);
                }
            }
        }
        Selection::FragmentSpread(name) => {
            let frag = ctx
                .doc
                .fragments
                .iter()
                .find(|(n, _, _)| n == name)
                .cloned();
            match frag {
                Some((_, _, sels)) => {
                    for s in &sels {
                        Box::pin(expand_selection(ctx, s, out, depth)).await;
                    }
                }
                None => ctx.errors.push(format!("Unknown fragment '{name}'")),
            }
        }
        Selection::InlineFragment {
            type_condition,
            selection,
        } => {
            // Root inline fragments only apply to Query (no type
            // condition, or `on Query`).
            match type_condition.as_deref() {
                None | Some("Query") => {
                    for s in selection {
                        Box::pin(expand_selection(ctx, s, out, depth)).await;
                    }
                }
                Some(t) => ctx.errors.push(format!(
                    "Inline fragment on '{t}' is not valid at the query root"
                )),
            }
        }
    }
}

/// Resolve one root `Query` field to a typed value.
async fn resolve_root_field(ctx: &mut ExecCtx<'_>, field: &Field) -> Result<Resolved, String> {
    match field.name.as_str() {
        "posts" => {
            let status = arg_str(field, ctx.vars, "status")?;
            let post_type = arg_str(field, ctx.vars, "type")?;
            let author_id = arg_i64(field, ctx.vars, "authorId")?;
            let page = page_of(arg_i64(field, ctx.vars, "page")?);
            let per_page = per_page_of(arg_i64(field, ctx.vars, "perPage")?);
            let result = list_posts(
                ctx.db,
                ctx.site_id,
                ListPostsOptions {
                    status,
                    post_type,
                    author_id,
                    page: Some(page),
                    per_page: Some(per_page),
                    include_trash: false,
                },
            )
            .await
            .map_err(|e| e.message.clone())?;
            let posts: Vec<Resolved> = result.posts.into_iter().map(Resolved::Post).collect();
            let mut obj = BTreeMap::new();
            obj.insert("posts".to_string(), Resolved::List(posts));
            obj.insert("total".to_string(), Resolved::Json(json!(result.total)));
            obj.insert("page".to_string(), Resolved::Json(json!(result.page)));
            obj.insert(
                "perPage".to_string(),
                Resolved::Json(json!(result.per_page)),
            );
            obj.insert(
                "totalPages".to_string(),
                Resolved::Json(json!(result.total_pages)),
            );
            Ok(Resolved::Json(
                project_object(ctx, obj, &field.selection, 1).await,
            ))
        }
        "post" => {
            let id = arg_i64(field, ctx.vars, "id")?;
            let slug = arg_str(field, ctx.vars, "slug")?;
            let key = match (id, slug) {
                (Some(i), _) => i.to_string(),
                (None, Some(s)) => s,
                (None, None) => {
                    return Err("post requires an id or slug argument".to_string());
                }
            };
            let found = get_post_by_slug_or_id(ctx.db, ctx.site_id, &key)
                .await
                .map_err(|e| e.message.clone())?;
            match found {
                None => Ok(Resolved::Json(Value::Null)),
                Some(p) => {
                    // REST GET /api/posts/:slug rule: drafts and private
                    // posts are visible only to logged-in users.
                    if p.status != PostStatus::Publish && ctx.user.is_none() {
                        return Err(format!("No post found with slug \"{key}\"."));
                    }
                    Ok(Resolved::Post(p))
                }
            }
        }
        "terms" => {
            let taxonomy =
                arg_str(field, ctx.vars, "taxonomy")?.unwrap_or_else(|| "category".to_string());
            let parent_id = arg_i64(field, ctx.vars, "parentId")?;
            let page = page_of(arg_i64(field, ctx.vars, "page")?);
            let per_page = per_page_of(arg_i64(field, ctx.vars, "perPage")?);
            let result = list_terms(
                ctx.db,
                ctx.site_id,
                &taxonomy,
                ListTermsOptions {
                    parent_id,
                    page: Some(page),
                    per_page: Some(per_page),
                },
            )
            .await
            .map_err(|e| e.message.clone())?;
            let terms: Vec<Resolved> = result.terms.into_iter().map(Resolved::Term).collect();
            let mut obj = BTreeMap::new();
            obj.insert("terms".to_string(), Resolved::List(terms));
            obj.insert("total".to_string(), Resolved::Json(json!(result.total)));
            obj.insert("page".to_string(), Resolved::Json(json!(result.page)));
            obj.insert(
                "perPage".to_string(),
                Resolved::Json(json!(result.per_page)),
            );
            obj.insert(
                "totalPages".to_string(),
                Resolved::Json(json!(result.total_pages)),
            );
            Ok(Resolved::Json(
                project_object(ctx, obj, &field.selection, 1).await,
            ))
        }
        "term" => {
            let id = arg_i64(field, ctx.vars, "id")?;
            let slug = arg_str(field, ctx.vars, "slug")?;
            let taxonomy =
                arg_str(field, ctx.vars, "taxonomy")?.unwrap_or_else(|| "category".to_string());
            let found = match (id, slug) {
                (Some(i), _) => {
                    use wordless_cms::terms::get_term;
                    get_term(ctx.db, ctx.site_id, i).await.ok()
                }
                (None, Some(s)) => match get_term_by_slug(ctx.db, ctx.site_id, &taxonomy, &s).await {
                    Ok(t) => Some(t),
                    Err(_) => None,
                },
                (None, None) => {
                    return Err("term requires an id or slug argument".to_string());
                }
            };
            Ok(match found {
                Some(t) => Resolved::Term(t),
                None => Resolved::Json(Value::Null),
            })
        }
        "media" => {
            if !can(ctx.user, "upload_files") {
                return Err("You do not have permission to do that.".to_string());
            }
            let page = page_of(arg_i64(field, ctx.vars, "page")?);
            let per_page = per_page_of(arg_i64(field, ctx.vars, "perPage")?);
            let mime_type = arg_str(field, ctx.vars, "mimeType")?;
            let result = list_media(
                ctx.db,
                ctx.site_id,
                ListMediaOpts {
                    page: Some(page),
                    per_page: Some(per_page),
                    mime_type,
                },
            )
            .await
            .map_err(|e| e.message.clone())?;
            let items: Vec<Resolved> = result.media.into_iter().map(Resolved::Media).collect();
            let mut obj = BTreeMap::new();
            obj.insert("media".to_string(), Resolved::List(items));
            obj.insert("total".to_string(), Resolved::Json(json!(result.total)));
            obj.insert("page".to_string(), Resolved::Json(json!(result.page)));
            obj.insert(
                "perPage".to_string(),
                Resolved::Json(json!(result.per_page)),
            );
            obj.insert(
                "totalPages".to_string(),
                Resolved::Json(json!(result.total_pages)),
            );
            Ok(Resolved::Json(
                project_object(ctx, obj, &field.selection, 1).await,
            ))
        }
        "user" => {
            if !can(ctx.user, "edit_users") {
                return Err("You do not have permission to do that.".to_string());
            }
            let id = arg_i64(field, ctx.vars, "id")?
                .ok_or_else(|| "user requires an id argument".to_string())?;
            let found = get_user_by_id(ctx.db, id)
                .await
                .map_err(|e| e.message.clone())?;
            Ok(match found {
                Some(u) => Resolved::User(u),
                None => Resolved::Json(Value::Null),
            })
        }
        "users" => {
            if !can(ctx.user, "edit_users") {
                return Err("You do not have permission to do that.".to_string());
            }
            let page = page_of(arg_i64(field, ctx.vars, "page")?);
            let per_page = per_page_of(arg_i64(field, ctx.vars, "perPage")?);
            let result = list_users(ctx.db, page, per_page)
                .await
                .map_err(|e| e.message.clone())?;
            let items: Vec<Resolved> = result.users.into_iter().map(Resolved::User).collect();
            let mut obj = BTreeMap::new();
            obj.insert("users".to_string(), Resolved::List(items));
            obj.insert("total".to_string(), Resolved::Json(json!(result.total)));
            obj.insert("page".to_string(), Resolved::Json(json!(result.page)));
            obj.insert(
                "perPage".to_string(),
                Resolved::Json(json!(result.per_page)),
            );
            obj.insert(
                "totalPages".to_string(),
                Resolved::Json(json!(result.total_pages)),
            );
            Ok(Resolved::Json(
                project_object(ctx, obj, &field.selection, 1).await,
            ))
        }
        "option" => {
            if ctx.user.is_none() {
                return Err("You do not have permission to do that.".to_string());
            }
            let key = arg_str(field, ctx.vars, "key")?
                .ok_or_else(|| "option requires a key argument".to_string())?;
            let value = wordless_multisite::get_site_option::<Value>(ctx.db, ctx.site_id, &key, Value::Null)
                .await
                .map_err(|e| e.message.clone())?;
            Ok(Resolved::Json(value))
        }
        "siteInfo" => {
            let title = wordless_multisite::get_site_option::<Value>(ctx.db, ctx.site_id, "blogname", Value::Null)
                .await
                .map_err(|e| e.message.clone())?;
            let url = wordless_multisite::get_site_option::<Value>(ctx.db, ctx.site_id, "siteurl", Value::Null)
                .await
                .map_err(|e| e.message.clone())?;
            let mut obj = BTreeMap::new();
            obj.insert("title".to_string(), Resolved::Json(title));
            obj.insert("url".to_string(), Resolved::Json(url));
            Ok(Resolved::Json(
                project_object(ctx, obj, &field.selection, 1).await,
            ))
        }
        other => Err(format!("Unknown field '{other}' on Query")),
    }
}

/// Project a map of already-resolved child values through a selection
/// set (used for the `*List` wrapper objects).
async fn project_object(
    ctx: &mut ExecCtx<'_>,
    obj: BTreeMap<String, Resolved>,
    selection: &[Selection],
    depth: usize,
) -> Value {
    if selection.is_empty() {
        // No sub-selection: return every child as plain JSON.
        let mut out = serde_json::Map::new();
        for (k, v) in obj {
            out.insert(k, project(ctx, v, &[], depth).await);
        }
        return Value::Object(out);
    }
    let mut out = serde_json::Map::new();
    for sel in selection {
        match sel {
            Selection::Field(f) => {
                if let Some(child) = obj.get(&f.name) {
                    let projected = project_owned(ctx, child, &f.selection, depth).await;
                    out.insert(f.response_key.clone(), projected);
                } else {
                    ctx.errors.push(format!("Unknown field '{}'", f.name));
                    out.insert(f.response_key.clone(), Value::Null);
                }
            }
            Selection::FragmentSpread(name) => {
                if let Some((_, _, sels)) = ctx.doc.fragments.iter().find(|(n, _, _)| n == name) {
                    let sels = sels.clone();
                    for s in &sels {
                        let mut sub = serde_json::Map::new();
                        match s {
                            Selection::Field(f) => {
                                if let Some(child) = obj.get(&f.name) {
                                    let projected =
                                        project_owned(ctx, child, &f.selection, depth).await;
                                    sub.insert(f.response_key.clone(), projected);
                                }
                            }
                            _ => {}
                        }
                        for (k, v) in sub {
                            out.insert(k, v);
                        }
                    }
                } else {
                    ctx.errors.push(format!("Unknown fragment '{name}'"));
                }
            }
            Selection::InlineFragment { selection, .. } => {
                for s in selection {
                    if let Selection::Field(f) = s {
                        if let Some(child) = obj.get(&f.name) {
                            let projected = project_owned(ctx, child, &f.selection, depth).await;
                            out.insert(f.response_key.clone(), projected);
                        }
                    }
                }
            }
        }
    }
    Value::Object(out)
}

async fn project_owned(
    ctx: &mut ExecCtx<'_>,
    value: &Resolved,
    selection: &[Selection],
    depth: usize,
) -> Value {
    // Clone into an owned Resolved so `project` can consume it.
    let owned = match value {
        Resolved::Json(v) => Resolved::Json(v.clone()),
        Resolved::Post(p) => Resolved::Post(p.clone()),
        Resolved::Term(t) => Resolved::Term(t.clone()),
        Resolved::Media(m) => Resolved::Media(m.clone()),
        Resolved::User(u) => Resolved::User(u.clone()),
        Resolved::List(items) => Resolved::List(
            items
                .iter()
                .map(|i| match i {
                    Resolved::Json(v) => Resolved::Json(v.clone()),
                    Resolved::Post(p) => Resolved::Post(p.clone()),
                    Resolved::Term(t) => Resolved::Term(t.clone()),
                    Resolved::Media(m) => Resolved::Media(m.clone()),
                    Resolved::User(u) => Resolved::User(u.clone()),
                    Resolved::List(_) => Resolved::Json(i.json()),
                })
                .collect(),
        ),
    };
    project(ctx, owned, selection, depth).await
}

/// Project a resolved value through a selection set, resolving lazy
/// nested fields (`Post.terms`, `Post.author`) from the database.
async fn project(
    ctx: &mut ExecCtx<'_>,
    value: Resolved,
    selection: &[Selection],
    depth: usize,
) -> Value {
    if depth > MAX_DEPTH {
        ctx.errors
            .push("Selection nesting is too deep (max 8)".to_string());
        return Value::Null;
    }
    match value {
        Resolved::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                // Boxed: `project` is recursive (via project_post), so
                // the recursive call must not inline the future.
                out.push(Box::pin(project(ctx, item, selection, depth)).await);
            }
            Value::Array(out)
        }
        Resolved::Json(v) => {
            if selection.is_empty() {
                return v;
            }
            // An array reaches here when a list was already projected
            // once (root resolvers pre-project their wrapper objects);
            // project each element instead of dropping the array.
            if let Value::Array(items) = v {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(Box::pin(project(ctx, Resolved::Json(item), selection, depth)).await);
                }
                return Value::Array(out);
            }
            let Value::Object(map) = v else {
                return Value::Null;
            };
            let mut out = serde_json::Map::new();
            for sel in selection {
                match sel {
                    Selection::Field(f) => {
                        let child = map.get(&f.name).cloned().unwrap_or(Value::Null);
                        let child =
                            Box::pin(project(ctx, Resolved::Json(child), &f.selection, depth + 1))
                                .await;
                        out.insert(f.response_key.clone(), child);
                    }
                    Selection::FragmentSpread(name) => {
                        if let Some((_, _, sels)) =
                            ctx.doc.fragments.iter().find(|(n, _, _)| n == name)
                        {
                            let sels = sels.clone();
                            for s in &sels {
                                if let Selection::Field(f) = s {
                                    let child = map.get(&f.name).cloned().unwrap_or(Value::Null);
                                    let child = Box::pin(project(
                                        ctx,
                                        Resolved::Json(child),
                                        &f.selection,
                                        depth + 1,
                                    ))
                                    .await;
                                    out.insert(f.response_key.clone(), child);
                                }
                            }
                        } else {
                            ctx.errors.push(format!("Unknown fragment '{name}'"));
                        }
                    }
                    Selection::InlineFragment { selection, .. } => {
                        for s in selection {
                            if let Selection::Field(f) = s {
                                let child = map.get(&f.name).cloned().unwrap_or(Value::Null);
                                let child = Box::pin(project(
                                    ctx,
                                    Resolved::Json(child),
                                    &f.selection,
                                    depth + 1,
                                ))
                                .await;
                                out.insert(f.response_key.clone(), child);
                            }
                        }
                    }
                }
            }
            Value::Object(out)
        }
        Resolved::Post(p) => project_post(ctx, p, selection, depth).await,
        Resolved::Term(t) => project_typed(ctx, Resolved::Term(t), selection, depth).await,
        Resolved::Media(m) => project_typed(ctx, Resolved::Media(m), selection, depth).await,
        Resolved::User(u) => project_typed(ctx, Resolved::User(u), selection, depth).await,
    }
}

/// Project a typed (non-Post) value: fields come from its JSON shape.
async fn project_typed(
    ctx: &mut ExecCtx<'_>,
    value: Resolved,
    selection: &[Selection],
    depth: usize,
) -> Value {
    let json = value.json();
    Box::pin(project(ctx, Resolved::Json(json), selection, depth)).await
}

/// Project a Post, resolving the lazy `terms` and `author` fields.
async fn project_post(
    ctx: &mut ExecCtx<'_>,
    post: Post,
    selection: &[Selection],
    depth: usize,
) -> Value {
    if selection.is_empty() {
        return serde_json::to_value(&post).unwrap_or(Value::Null);
    }
    let json = serde_json::to_value(&post).unwrap_or(Value::Null);
    let map = match json {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let mut out = serde_json::Map::new();
    for sel in selection {
        match sel {
            Selection::Field(f) => match f.name.as_str() {
                "terms" => {
                    let taxonomy = arg_str(f, ctx.vars, "taxonomy")
                        .unwrap_or(None)
                        .unwrap_or_else(|| "category".to_string());
                    match get_post_terms(ctx.db, ctx.site_id, post.id, Some(&taxonomy)).await {
                        Ok(terms) => {
                            let items: Vec<Resolved> =
                                terms.into_iter().map(Resolved::Term).collect();
                            let v = Box::pin(project(
                                ctx,
                                Resolved::List(items),
                                &f.selection,
                                depth + 1,
                            ))
                            .await;
                            out.insert(f.response_key.clone(), v);
                        }
                        Err(e) => {
                            ctx.errors.push(e.message.clone());
                            out.insert(f.response_key.clone(), Value::Null);
                        }
                    }
                }
                "author" => {
                    match get_user_by_id(ctx.db, post.author_id).await {
                        Ok(Some(u)) => {
                            // Public author shape: id + display name only
                            // (email/login stay behind the edit_users
                            // guard on the root `user` field).
                            let mut author = serde_json::Map::new();
                            author.insert("id".to_string(), json!(u.id));
                            author.insert("displayName".to_string(), json!(u.display_name));
                            let v = Box::pin(project(
                                ctx,
                                Resolved::Json(Value::Object(author)),
                                &f.selection,
                                depth + 1,
                            ))
                            .await;
                            out.insert(f.response_key.clone(), v);
                        }
                        Ok(None) => {
                            out.insert(f.response_key.clone(), Value::Null);
                        }
                        Err(e) => {
                            ctx.errors.push(e.message.clone());
                            out.insert(f.response_key.clone(), Value::Null);
                        }
                    }
                }
                other => {
                    if let Some(child) = map.get(other) {
                        let child = Box::pin(project(
                            ctx,
                            Resolved::Json(child.clone()),
                            &f.selection,
                            depth + 1,
                        ))
                        .await;
                        out.insert(f.response_key.clone(), child);
                    } else {
                        ctx.errors.push(format!("Unknown field '{other}' on Post"));
                        out.insert(f.response_key.clone(), Value::Null);
                    }
                }
            },
            Selection::FragmentSpread(name) => {
                if let Some((_, _, sels)) = ctx.doc.fragments.iter().find(|(n, _, _)| n == name) {
                    let sels = sels.clone();
                    let sub = Box::pin(project_post(ctx, post.clone(), &sels, depth)).await;
                    if let Value::Object(m) = sub {
                        for (k, v) in m {
                            out.insert(k, v);
                        }
                    }
                } else {
                    ctx.errors.push(format!("Unknown fragment '{name}'"));
                }
            }
            Selection::InlineFragment { selection, .. } => {
                let sub = Box::pin(project_post(ctx, post.clone(), selection, depth)).await;
                if let Value::Object(m) = sub {
                    for (k, v) in m {
                        out.insert(k, v);
                    }
                }
            }
        }
    }
    Value::Object(out)
}

/// The GraphQL schema, as documentation (the endpoint is hand-rolled;
/// there is no introspection query — the schema is this string).
pub const SCHEMA: &str = r#"type Query {
  posts(status: String, type: String, authorId: Int, page: Int, perPage: Int): PostList
  post(id: Int, slug: String): Post
  terms(taxonomy: String, parentId: Int, page: Int, perPage: Int): TermList
  term(id: Int, slug: String, taxonomy: String): Term
  media(page: Int, perPage: Int, mimeType: String): MediaList
  user(id: Int): User
  users(page: Int, perPage: Int): UserList
  option(key: String): JSON
  siteInfo: SiteInfo
}
type Post { id: Int authorId: Int date: String dateGmt: String content: String title: String excerpt: String status: String slug: String modified: String type: String commentCount: Int author: User terms(taxonomy: String): [Term] }
type PostList { posts: [Post] total: Int page: Int perPage: Int totalPages: Int }
type Term { id: Int termTaxonomyId: Int name: String slug: String taxonomy: String description: String parentId: Int count: Int }
type TermList { terms: [Term] total: Int page: Int perPage: Int totalPages: Int }
type Media { id: Int title: String slug: String mimeType: String url: String filename: String size: Int alt: String authorId: Int date: String }
type MediaList { media: [Media] total: Int page: Int perPage: Int totalPages: Int }
type User { id: Int login: String nicename: String email: String url: String displayName: String role: String }
type UserList { users: [User] total: Int page: Int perPage: Int totalPages: Int }
type SiteInfo { title: String url: String }
"#;
