/**
 * Dashboard — a real command deck (§6.3).
 *
 * Today's numbers come only from live API totals (posts, drafts,
 * pending comments, pending AI approvals, health checks) — no invented
 * traffic, revenue, or visitor figures. The next three actions are the
 * three most urgent real queues, each one click away. The theme
 * switcher lives on the dashboard itself (preview + §6.3): eight
 * themes, the role default shown, personal override saved via
 * lib/theme.ts saveTheme (PUT /api/users/:id/profile).
 */
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { formatDateTime } from "../lib/format";
import type { ActivityItem, ActivityList, AiApprovalList, CommentList, HealthCheck, PostList, User } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Select, Skeleton, StatCard } from "../components/elements";
import { Icon } from "../components/icons";
import { THEMES, applyTheme, getRoleDefaultTheme, saveTheme, themeById } from "../lib/theme";
import type { ThemeId, ThemePersistence } from "../lib/theme";

interface Action {
  id: string;
  label: string;
  detail: string;
  href: string;
  icon: "pen" | "comments" | "ai" | "health" | "checkCircle";
  tone: "warn" | "danger" | "teal" | "blue";
}

export function DashboardPage({ timezone }: { timezone: string }): React.ReactElement {
  const [health, setHealth] = useState<HealthCheck[] | null>(null);
  const [posts, setPosts] = useState<PostList | null>(null);
  const [drafts, setDrafts] = useState<PostList | null>(null);
  const [comments, setComments] = useState<CommentList | null>(null);
  const [approvals, setApprovals] = useState<AiApprovalList | null>(null);
  const [activity, setActivity] = useState<ActivityItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [me, setMe] = useState<User | null>(null);
  const [theme, setTheme] = useState<ThemeId>("teal");
  const [themePersisted, setThemePersisted] = useState<ThemePersistence | "role-default" | null>(null);
  const [themeBusy, setThemeBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [h, p, d, c, ap, a, { user }] = await Promise.all([
          api.get<{ checks: HealthCheck[] }>("/api/site-health"),
          api.get<PostList>("/api/posts?per_page=1"),
          api.get<PostList>("/api/posts?status=draft&per_page=1"),
          api.get<CommentList>("/api/comments?status=pending&per_page=1"),
          api.listAiApprovals("pending"),
          api.get<ActivityList>("/api/activity?per_page=8"),
          api.me(),
        ]);
        if (cancelled) return;
        setHealth(h.checks);
        setPosts(p);
        setDrafts(d);
        setComments(c);
        setApprovals(ap);
        setActivity(a.entries);
        setMe(user);
        setTheme((document.documentElement.getAttribute("data-theme") as ThemeId) ?? "teal");
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : "Failed to load the dashboard.");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const critical = health?.filter((c) => c.status === "critical").length ?? 0;
  const recommended = health?.filter((c) => c.status === "recommended").length ?? 0;
  const draftCount = drafts?.total ?? 0;
  const pendingComments = comments?.total ?? 0;
  const pendingApprovals = approvals?.total ?? 0;

  const actions: Action[] = [];
  if (pendingApprovals > 0) {
    actions.push({
      id: "approvals",
      label: `${pendingApprovals} AI approval${pendingApprovals === 1 ? "" : "s"} waiting`,
      detail: "Review what the AI wants to do before it applies anything.",
      href: "#/ai",
      icon: "ai",
      tone: "warn",
    });
  }
  if (pendingComments > 0) {
    actions.push({
      id: "comments",
      label: `${pendingComments} comment${pendingComments === 1 ? "" : "s"} to moderate`,
      detail: "Approve, mark as spam, or trash — nothing publishes itself.",
      href: "#/comments",
      icon: "comments",
      tone: "warn",
    });
  }
  if (draftCount > 0) {
    actions.push({
      id: "drafts",
      label: `${draftCount} draft${draftCount === 1 ? "" : "s"} to review`,
      detail: "Finish and publish the work already started — open Posts and filter by Draft.",
      href: "#/posts",
      icon: "pen",
      tone: "blue",
    });
  }
  if (critical > 0) {
    actions.push({
      id: "health",
      label: `${critical} critical health issue${critical === 1 ? "" : "s"}`,
      detail: "Fix these first — they can break the site.",
      href: "#/health",
      icon: "health",
      tone: "danger",
    });
  }
  const nextActions = actions.slice(0, 3);
  const allClear = actions.length === 0 && health !== null;

  const changeTheme = (id: ThemeId): void => {
    setTheme(id);
    applyTheme(id);
    if (me === null) return;
    setThemeBusy(true);
    void saveTheme(me.id, id)
      .then(({ persisted }) => setThemePersisted(persisted))
      .finally(() => setThemeBusy(false));
  };

  const roleDefault = me !== null ? getRoleDefaultTheme(me.role) : "teal";
  const loading = posts === null && error === null;

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Dashboard failed to load" onDismiss={() => setError(null)}>{error}</Alert> : null}

      <div className="page-head">
        <h2>{me !== null ? `Good to see you, ${me.displayName}` : "Dashboard"}</h2>
        <p>
          {critical > 0
            ? `${critical} critical health issue${critical === 1 ? "" : "s"} need${critical === 1 ? "s" : ""} attention.`
            : recommended > 0
              ? `Everything is up. ${recommended} recommended health fix${recommended === 1 ? "" : "es"} outstanding.`
              : "Everything is healthy."}{" "}
          {nextActions.length > 0 ? `${nextActions.length} thing${nextActions.length === 1 ? "" : "s"} need${nextActions.length === 1 ? "s" : ""} your eye.` : ""}
        </p>
      </div>

      {loading ? (
        <div className="grid" aria-label="Loading dashboard">
          <Skeleton height={92} label="Loading posts count" />
          <Skeleton height={92} label="Loading comments count" />
          <Skeleton height={92} label="Loading approvals count" />
          <Skeleton height={92} label="Loading health count" />
        </div>
      ) : (
        <div className="grid">
          <StatCard label="Published + draft posts" value={String(posts?.total ?? 0)} icon="posts" />
          <StatCard label="Comments awaiting moderation" value={String(pendingComments)} icon="comments" tone={pendingComments > 0 ? "warn" : undefined} />
          <StatCard label="AI approvals waiting" value={String(pendingApprovals)} icon="ai" tone={pendingApprovals > 0 ? "warn" : undefined} />
          <StatCard label="Critical health issues" value={String(critical)} icon="alert" tone={critical > 0 ? "danger" : undefined} />
        </div>
      )}

      <Card title="Next actions" sub="The three most urgent real queues on this site — nothing here is a dead widget.">
        {health === null && error === null ? (
          <Skeleton height={120} label="Loading next actions" />
        ) : allClear ? (
          <EmptyState icon="checkCircle" title="Nothing needs you right now" description="No drafts waiting, no comments to moderate, no AI approvals pending, no critical health issues." action={<Button variant="primary" icon="plus" onClick={() => { window.location.hash = "#/posts/new"; }}>Write a new post</Button>} />
        ) : (
          <ul className="action-list">
            {nextActions.map((a) => (
              <li key={a.id} className="action-row">
                <span className={`action-icon tone-${a.tone}`} aria-hidden="true"><Icon name={a.icon} size={18} /></span>
                <span className="action-body">
                  <strong>{a.label}</strong>
                  <span className="muted">{a.detail}</span>
                </span>
                <Button variant="secondary" size="small" icon="arrowRight" onClick={() => { window.location.hash = a.href; }}>
                  Open
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Card>

      <div className="grid-2">
        <Card title="Recent posts" sub="Latest content, newest first." actions={<Button variant="ghost" size="small" onClick={() => { window.location.hash = "#/posts"; }}>All posts</Button>}>
          {!posts ? (
            <Skeleton height={120} label="Loading recent posts" />
          ) : posts.posts.length === 0 ? (
            <EmptyState icon="posts" title="No posts yet" description="Write your first post to get the site started." action={<Button variant="primary" icon="plus" onClick={() => { window.location.hash = "#/posts/new"; }}>Write a post</Button>} />
          ) : (
            <ul className="row-list">
              {posts.posts.slice(0, 5).map((post) => (
                <li key={post.id}>
                  <a href={`#/posts/${post.id}/edit`}>{post.title || "(untitled)"}</a>
                  <span className="muted">{post.status} · {formatDateTime(post.modified, timezone)}</span>
                </li>
              ))}
            </ul>
          )}
        </Card>

        <Card title="Recent activity" sub="What changed on the site." actions={<Button variant="ghost" size="small" onClick={() => { window.location.hash = "#/activity"; }}>Full log</Button>}>
          {!activity ? (
            <Skeleton height={120} label="Loading activity" />
          ) : activity.length === 0 ? (
            <EmptyState icon="activity" title="No activity recorded yet" description="Changes to posts, settings and orders will appear here." />
          ) : (
            <ul className="row-list">
              {activity.map((item) => (
                <li key={item.id}>
                  <span>{item.action}{item.objectType ? ` (${item.objectType}${item.objectId ? ` #${item.objectId}` : ""})` : ""}</span>
                  <span className="muted">{formatDateTime(item.createdAt, timezone)}</span>
                </li>
              ))}
            </ul>
          )}
        </Card>
      </div>

      <Card title="Dashboard theme" sub={me !== null ? `Your role default is ${themeById(roleDefault).name}. Your personal choice overrides it and follows your account.` : "Pick the look of your admin."} actions={themePersisted === "local" ? <Badge tone="amber">Saved in this browser only</Badge> : null}>
        <div className="theme-grid" role="group" aria-label="Dashboard themes">
          {THEMES.map((t) => {
            const active = t.id === theme;
            return (
              <button
                key={t.id}
                type="button"
                className={`theme-card${active ? " active" : ""}`}
                aria-pressed={active}
                disabled={themeBusy}
                onClick={() => changeTheme(t.id)}
              >
                <span className="theme-swatch" style={{ background: t.vars["--th-bg"], borderColor: t.vars["--th-line"] }} aria-hidden="true">
                  <span className="theme-swatch-bar" style={{ background: t.vars["--th-sidebar"] }} />
                  <span className="theme-swatch-card" style={{ background: t.vars["--th-surface"] }}>
                    <span style={{ background: t.vars["--th-accent"] }} />
                    <span style={{ background: t.vars["--th-soft"] }} />
                  </span>
                </span>
                <span className="theme-name">{t.name}{t.id === roleDefault ? " · role default" : ""}</span>
                <span className="muted">{t.desc}</span>
              </button>
            );
          })}
        </div>
        <div className="actions" style={{ marginTop: 14 }}>
          <Select label="Theme" value={theme} onChange={(e) => changeTheme(e.target.value as ThemeId)} aria-label="Dashboard theme">
            {THEMES.map((t) => (
              <option key={t.id} value={t.id}>{t.name}</option>
            ))}
          </Select>
          <Button variant="secondary" size="small" disabled={themeBusy || theme === roleDefault} onClick={() => changeTheme(roleDefault)}>
            Reset to role default ({themeById(roleDefault).name})
          </Button>
        </div>
        <p className="hint">Theme choices apply instantly to every admin screen — sidebar, cards, buttons and badges all re-skin from one token set. Manage all eight themes and role defaults in Settings → Appearance → Dashboard themes.</p>
      </Card>
    </div>
  );
}
