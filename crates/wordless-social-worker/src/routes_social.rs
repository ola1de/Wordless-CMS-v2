//! Social route table — social networking (BuddyPress-like):
//! profiles, friends, activity feed, groups, direct messaging,
//! notifications, member directory, plus public member/group/activity
//! pages.
//!
//! Moved here verbatim from the main Worker's `routes_p7.rs` in the
//! 2026-11-20 size remedy (only the `Ctx` / helper imports changed).
//! All routes use the v1 `{ok, data, error}` envelope and the same
//! auth/capability guards as the rest of the API. Q11 (decided
//! 2026-11-20): friends (mutual, request/accept) is THE relationship
//! model — there are no follow routes and no `wl_follows` table.
//!
//! Notifications are in-app rows; the email copy is best-effort
//! through the integrations mailer (`send_notification_mail`): with
//! the mock provider it only logs to `wl_mail_log` — never presented
//! as delivered — and a mail failure never fails the request.

use serde_json::{json, Value};
use wordless_integrations::email::{send_mail, SendMailInput};
use wordless_integrations::platform::WorkerHttpClient;
use wordless_kernel::capabilities::Role;
use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::html::escape_html;
use wordless_kernel::router::{read_json_body, RequestContext, RouteOptions, Router};
use wordless_social::activity;
use wordless_social::friends;
use wordless_social::groups;
use wordless_social::messages;
use wordless_social::notifications;
use wordless_social::profiles;
use wordless_social::types::{Notification, UpdateProfileInput};

use crate::ctx::Ctx;
use crate::helpers::{actor, ok, ok_json, opt_str, page_param, parse_id, per_page_param};

/// Register every social route on the router built by the social Worker entry.
pub fn register_social_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------------- profiles
    router.get(
        "/api/social/profile",
        |ctx| {
            Box::pin(async move {
                let me = actor(ctx);
                let profile = profiles::ensure_profile(ctx.db.as_ref(), me).await?;
                ok(profile, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.put(
        "/api/social/profile",
        |ctx| {
            Box::pin(async move {
                let me = actor(ctx);
                let body = read_json_body(ctx)?;
                let input: UpdateProfileInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid profile data."))?;
                let profile = profiles::update_profile(ctx.db.as_ref(), me, &input).await?;
                ok(profile, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/social/profile/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "user")?;
                let profile =
                    profiles::view_profile(ctx.db.as_ref(), actor(ctx), id, is_admin(ctx)).await?;
                ok(profile, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/social/members",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let q = ctx.query("q").unwrap_or("").to_string();
                let (items, total) = profiles::list_members(
                    ctx.db.as_ref(),
                    actor(ctx),
                    &q,
                    page,
                    per_page,
                    is_admin(ctx),
                )
                .await?;
                ok(
                    json!({
                        "items": items, "total": total, "page": page, "perPage": per_page,
                        "totalPages": (total + per_page - 1) / per_page,
                    }),
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    // ------------------------------------------------------- friends
    router.get(
        "/api/social/friends",
        |ctx| {
            Box::pin(async move {
                let pending_only = ctx.query("pending") == Some("1");
                let items =
                    friends::list_friends(ctx.db.as_ref(), actor(ctx), pending_only).await?;
                ok(items, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/friends/request",
        |ctx| {
            Box::pin(async move {
                let me = actor(ctx);
                let body = read_json_body(ctx)?;
                let other = body_user_id(&body)?;
                let notif = friends::request(ctx.db.as_ref(), me, other).await?;
                mail_notification(ctx, &notif).await;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/friends/accept",
        |ctx| {
            Box::pin(async move {
                let me = actor(ctx);
                let body = read_json_body(ctx)?;
                let other = body_user_id(&body)?;
                let notif = friends::accept(ctx.db.as_ref(), me, other).await?;
                mail_notification(ctx, &notif).await;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/friends/decline",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let other = body_user_id(&body)?;
                friends::decline(ctx.db.as_ref(), actor(ctx), other).await?;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/friends/remove",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let other = body_user_id(&body)?;
                friends::remove(ctx.db.as_ref(), actor(ctx), other).await?;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/friends/block",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let other = body_user_id(&body)?;
                friends::block(ctx.db.as_ref(), actor(ctx), other).await?;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/friends/unblock",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let other = body_user_id(&body)?;
                friends::unblock(ctx.db.as_ref(), actor(ctx), other).await?;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/social/friends/blocked",
        |ctx| {
            Box::pin(async move {
                let items = friends::list_blocked(ctx.db.as_ref(), actor(ctx)).await?;
                ok(items, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    // ------------------------------------------------------- activity
    router.get(
        "/api/social/activity",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let actor_id = ctx
                    .query("actorId")
                    .and_then(|v| v.parse::<i64>().ok())
                    .filter(|v| *v >= 1);
                let items =
                    activity::feed(ctx.db.as_ref(), actor(ctx), actor_id, page, per_page).await?;
                ok(items, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/activity",
        |ctx| {
            Box::pin(async move {
                let me = actor(ctx);
                let body = read_json_body(ctx)?;
                let content = opt_str(&body, "content").unwrap_or_default();
                let visibility =
                    opt_str(&body, "visibility").unwrap_or_else(|| "public".to_string());
                let group_id = body
                    .get("groupId")
                    .and_then(|v| v.as_i64())
                    .filter(|v| *v >= 1);
                let (item, notifs) =
                    activity::post(ctx.db.as_ref(), me, &content, &visibility, group_id).await?;
                for n in &notifs {
                    mail_notification(ctx, &Some(n.clone())).await;
                }
                ok(item, 201)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/activity/:id/like",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "activity item")?;
                let notif = activity::like(ctx.db.as_ref(), actor(ctx), id).await?;
                mail_notification(ctx, &notif).await;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.delete(
        "/api/social/activity/:id/like",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "activity item")?;
                activity::unlike(ctx.db.as_ref(), actor(ctx), id).await?;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.delete(
        "/api/social/activity/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "activity item")?;
                activity::delete_own(ctx.db.as_ref(), actor(ctx), id).await?;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/activity/:id/hide",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "activity item")?;
                let item = activity::hide(ctx.db.as_ref(), actor(ctx), id).await?;
                ok(item, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("moderate_comments"),
        },
    );

    router.get(
        "/api/social/admin/activity",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let items = activity::admin_list(ctx.db.as_ref(), page, per_page).await?;
                ok(items, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------- groups
    router.get(
        "/api/social/groups",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let q = ctx.query("q").unwrap_or("").to_string();
                let (items, total) = groups::list(
                    ctx.db.as_ref(),
                    actor(ctx),
                    &q,
                    page,
                    per_page,
                    is_admin(ctx),
                )
                .await?;
                ok(
                    json!({
                        "items": items, "total": total, "page": page, "perPage": per_page,
                        "totalPages": (total + per_page - 1) / per_page,
                    }),
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/groups",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let name = opt_str(&body, "name").unwrap_or_default();
                let description = opt_str(&body, "description").unwrap_or_default();
                let visibility =
                    opt_str(&body, "visibility").unwrap_or_else(|| "public".to_string());
                let group = groups::create(
                    ctx.db.as_ref(),
                    actor(ctx),
                    &name,
                    &description,
                    &visibility,
                )
                .await?;
                ok(group, 201)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/social/groups/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "group")?;
                let group = groups::get(ctx.db.as_ref(), id, actor(ctx), is_admin(ctx))
                    .await?
                    .ok_or_else(|| not_found("Group not found."))?;
                ok(group, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.put(
        "/api/social/groups/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "group")?;
                let body = read_json_body(ctx)?;
                let group = groups::update(
                    ctx.db.as_ref(),
                    actor(ctx),
                    id,
                    &groups::UpdateGroupInput {
                        name: opt_str(&body, "name"),
                        description: opt_str(&body, "description"),
                        visibility: opt_str(&body, "visibility"),
                        avatar_id: body.get("avatarId").and_then(|v| v.as_i64()),
                    },
                    is_admin(ctx),
                )
                .await?;
                ok(group, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/groups/:id/join",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "group")?;
                let (group, notif) = groups::join(ctx.db.as_ref(), actor(ctx), id).await?;
                mail_notification(ctx, &notif).await;
                ok(group, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/groups/:id/leave",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "group")?;
                groups::leave(ctx.db.as_ref(), actor(ctx), id).await?;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/groups/:id/approve",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "group")?;
                let body = read_json_body(ctx)?;
                let user_id = body_user_id(&body)?;
                let notif = groups::approve(ctx.db.as_ref(), actor(ctx), id, user_id).await?;
                mail_notification(ctx, &notif).await;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/groups/:id/invite",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "group")?;
                let body = read_json_body(ctx)?;
                let user_id = body_user_id(&body)?;
                let notif = groups::invite(ctx.db.as_ref(), actor(ctx), id, user_id).await?;
                mail_notification(ctx, &notif).await;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/groups/:id/ban",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "group")?;
                let body = read_json_body(ctx)?;
                let user_id = body_user_id(&body)?;
                groups::ban(ctx.db.as_ref(), actor(ctx), id, user_id).await?;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/social/groups/:id/members",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "group")?;
                let items = groups::members(ctx.db.as_ref(), actor(ctx), id, is_admin(ctx)).await?;
                ok(items, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/social/groups/:id/activity",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "group")?;
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let items = activity::group_feed(
                    ctx.db.as_ref(),
                    actor(ctx),
                    id,
                    page,
                    per_page,
                    is_admin(ctx),
                )
                .await?;
                ok(items, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    // ------------------------------------------------------- messages
    router.get(
        "/api/social/messages/threads",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let items =
                    messages::list_threads(ctx.db.as_ref(), actor(ctx), page, per_page).await?;
                ok(items, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/messages/threads",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let other = body_user_id(&body)?;
                let (thread, _) =
                    messages::get_or_create_thread(ctx.db.as_ref(), actor(ctx), other).await?;
                ok(thread, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/social/messages/threads/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "thread")?;
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let thread = messages::get_thread(ctx.db.as_ref(), actor(ctx), id)
                    .await?
                    .ok_or_else(|| not_found("Thread not found."))?;
                let items =
                    messages::list_messages(ctx.db.as_ref(), actor(ctx), id, page, per_page)
                        .await?;
                ok(json!({ "thread": thread, "messages": items }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/messages/threads/:id/messages",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "thread")?;
                let body = read_json_body(ctx)?;
                let text = opt_str(&body, "body").unwrap_or_default();
                let (msg, notif) = messages::send(ctx.db.as_ref(), actor(ctx), id, &text).await?;
                mail_notification(ctx, &notif).await;
                ok(msg, 201)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/messages/threads/:id/read",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "thread")?;
                messages::mark_read(ctx.db.as_ref(), actor(ctx), id).await?;
                ok_json(json!({ "ok": true }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    // ------------------------------------------------------- notifications
    router.get(
        "/api/social/notifications",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let unread_only = ctx.query("unread") == Some("1");
                let (items, unread, total) =
                    notifications::list(ctx.db.as_ref(), actor(ctx), unread_only, page, per_page).await?;
                ok(
                    json!({
                        "items": items, "unreadCount": unread, "total": total,
                        "page": page, "perPage": per_page, "totalPages": (total + per_page - 1) / per_page,
                    }),
                    200,
                )
            })
        },
        RouteOptions { require_auth: true, capability: None },
    );

    router.get(
        "/api/social/notifications/unread-count",
        |ctx| {
            Box::pin(async move {
                let unread = notifications::unread_count(ctx.db.as_ref(), actor(ctx)).await?;
                ok_json(json!({ "unreadCount": unread }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/social/notifications/read",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let me = actor(ctx);
                match body.get("id").and_then(|v| v.as_i64()).filter(|v| *v >= 1) {
                    Some(id) => {
                        let changed = notifications::mark_read(ctx.db.as_ref(), me, id).await?;
                        ok_json(json!({ "ok": true, "read": changed }), 200)
                    }
                    None => {
                        let changed = notifications::mark_all_read(ctx.db.as_ref(), me).await?;
                        ok_json(json!({ "ok": true, "read": changed }), 200)
                    }
                }
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    // ------------------------------------------------------- public pages
    // Minimal standalone pages (inline CSS, mobile-first): the public
    // member directory, profile pages, group pages, and activity feed.
    // They honour the same visibility rules as the API — anonymous
    // viewers see public profiles, public groups, and public activity
    // only. A CMS page with the same slug never wins: router routes
    // are matched before the public fallback.
    router.get(
        "/members",
        |ctx| {
            Box::pin(async move {
                let q = ctx.query("q").unwrap_or("").to_string();
                let (items, _) =
                    profiles::list_members(ctx.db.as_ref(), 0, &q, 1, 50, false).await?;
                let mut body = String::from("<h1>Members</h1>");
                if items.is_empty() {
                    body.push_str("<p>No members found.</p>");
                } else {
                    body.push_str("<ul class=\"list\">");
                    for m in &items {
                        body.push_str(&format!(
                            "<li><a href=\"/members/{}\">{}</a>{}</li>",
                            m.user_id,
                            escape_html(&m.display_name),
                            if m.bio.is_empty() {
                                String::new()
                            } else {
                                format!(" <span>{}</span>", escape_html(&m.bio))
                            }
                        ));
                    }
                    body.push_str("</ul>");
                }
                Ok(social_html("Members", &body))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/members/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "user")?;
                match profiles::view_profile(ctx.db.as_ref(), 0, id, false).await {
                    Ok(p) => {
                        let mut body = format!("<h1>{}</h1>", escape_html(&p.display_name));
                        if !p.bio.is_empty() {
                            body.push_str(&format!("<p>{}</p>", escape_html(&p.bio)));
                        }
                        let items = activity::feed(ctx.db.as_ref(), 0, Some(id), 1, 20).await?;
                        body.push_str("<h2>Activity</h2>");
                        body.push_str(&activity_list_html(&items));
                        Ok(social_html(&p.display_name, &body))
                    }
                    Err(e) => Ok(social_html(
                        "Member not found",
                        &format!(
                            "<h1>Member not found</h1><p>{}</p>",
                            escape_html(&e.message)
                        ),
                    )),
                }
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/groups",
        |ctx| {
            Box::pin(async move {
                let q = ctx.query("q").unwrap_or("").to_string();
                let (items, _) = groups::list(ctx.db.as_ref(), 0, &q, 1, 50, false).await?;
                let mut body = String::from("<h1>Groups</h1>");
                if items.is_empty() {
                    body.push_str("<p>No groups yet.</p>");
                } else {
                    body.push_str("<ul class=\"list\">");
                    for g in &items {
                        body.push_str(&format!(
                            "<li><a href=\"/groups/{}\">{}</a> <span>({} · {} members)</span></li>",
                            escape_html(&g.slug),
                            escape_html(&g.name),
                            escape_html(&g.visibility),
                            g.member_count
                        ));
                    }
                    body.push_str("</ul>");
                }
                Ok(social_html("Groups", &body))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/groups/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                match groups::get_by_slug(ctx.db.as_ref(), &slug, 0, false).await? {
                    Some(g) => {
                        let mut body = format!("<h1>{}</h1>", escape_html(&g.name));
                        if !g.description.is_empty() {
                            body.push_str(&format!("<p>{}</p>", escape_html(&g.description)));
                        }
                        body.push_str(&format!(
                            "<p>{} · {} members</p>",
                            escape_html(&g.visibility),
                            g.member_count
                        ));
                        match activity::group_feed(ctx.db.as_ref(), 0, g.id, 1, 20, false).await {
                            Ok(items) => {
                                body.push_str("<h2>Activity</h2>");
                                body.push_str(&activity_list_html(&items));
                            }
                            Err(_) => {
                                body.push_str("<p>This group's activity is members-only.</p>");
                            }
                        }
                        Ok(social_html(&g.name, &body))
                    }
                    None => Ok(social_html(
                        "Group not found",
                        "<h1>Group not found</h1><p>This group does not exist or is hidden.</p>",
                    )),
                }
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/activity",
        |ctx| {
            Box::pin(async move {
                let items = activity::feed(ctx.db.as_ref(), 0, None, 1, 20).await?;
                let mut body = String::from("<h1>Activity</h1>");
                body.push_str(&activity_list_html(&items));
                Ok(social_html("Activity", &body))
            })
        },
        RouteOptions::default(),
    );
}

/// True when the session user is an administrator.
fn is_admin(ctx: &Ctx) -> bool {
    ctx.user()
        .map(|u| u.role == Role::Administrator)
        .unwrap_or(false)
}

/// `userId` from a JSON body: an integer >= 1, else `Invalid userId.`
fn body_user_id(body: &Value) -> Result<i64, WordlessError> {
    match body.get("userId").and_then(|v| v.as_i64()) {
        Some(v) if v >= 1 => Ok(v),
        _ => Err(bad_request("Invalid userId.")),
    }
}

/// Best-effort email copy of an in-app notification. A mail failure
/// (or no mail provider configured) never fails the request — the
/// in-app row is the notification; email is a courtesy copy.
async fn mail_notification(ctx: &Ctx, notif: &Option<Notification>) {
    let Some(n) = notif else { return };
    let Ok(Some(email)) = notifications::recipient_email(ctx.db.as_ref(), n.user_id).await else {
        return;
    };
    let (subject, text) = match n.notif_type.as_str() {
        "friend_request" => (
            "New friend request".to_string(),
            format!("{} sent you a friend request.", n.actor_name),
        ),
        "friend_accepted" => (
            "Friend request accepted".to_string(),
            format!("{} accepted your friend request.", n.actor_name),
        ),
        "activity_like" => (
            "Someone liked your activity".to_string(),
            format!("{} liked your activity.", n.actor_name),
        ),
        "activity_mention" => (
            "You were mentioned".to_string(),
            format!("{} mentioned you in an activity post.", n.actor_name),
        ),
        "group_join_request" => (
            "New group join request".to_string(),
            format!("{} asked to join your group.", n.actor_name),
        ),
        "group_approved" => (
            "Group join request approved".to_string(),
            format!("{} approved your request to join the group.", n.actor_name),
        ),
        "group_invite" => (
            "You were invited to a group".to_string(),
            format!("{} invited you to join a group.", n.actor_name),
        ),
        "message" => (
            "New message".to_string(),
            format!("{} sent you a message.", n.actor_name),
        ),
        _ => return,
    };
    let _ = send_mail(
        ctx.db.as_ref(),
        &ctx.int_secrets,
        &WorkerHttpClient,
        &SendMailInput {
            to: email,
            subject,
            text,
            html: None,
        },
    )
    .await;
}

/// One shared page shell for the public social pages: inline CSS,
/// mobile-first (usable at 360 px), no theme dependency.
fn social_html(title: &str, body: &str) -> RouteResponse {
    RouteResponse::html(
        format!(
            "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <title>{}</title><style>\
             body{{font-family:system-ui,sans-serif;margin:0 auto;max-width:720px;padding:16px;color:#0f2a2a}}\
             a{{color:#0b6b5e}}nav{{display:flex;gap:16px;flex-wrap:wrap;margin-bottom:16px}}\
             ul.list{{list-style:none;padding:0}}ul.list li{{padding:10px 0;border-bottom:1px solid #ddd}}\
             ul.list span{{color:#555}}article{{border-bottom:1px solid #ddd;padding:10px 0}}\
             article .meta{{color:#555;font-size:.85em}}\
             </style></head><body><nav><a href=\"/\">Home</a><a href=\"/members\">Members</a>\
             <a href=\"/groups\">Groups</a><a href=\"/activity\">Activity</a></nav>{}</body></html>",
            escape_html(title),
            body
        ),
        200,
    )
}

fn activity_list_html(items: &[wordless_social::types::ActivityItem]) -> String {
    if items.is_empty() {
        return "<p>No activity yet.</p>".to_string();
    }
    let mut out = String::new();
    for item in items {
        out.push_str(&format!(
            "<article><div class=\"meta\">{} · {}</div><p>{}</p></article>",
            escape_html(&item.actor_name),
            escape_html(&item.created_at),
            escape_html(&item.content),
        ));
    }
    out
}
