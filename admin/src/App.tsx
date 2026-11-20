import { useEffect, useMemo, useState } from "react";
import { api, ApiError, getCurrentSiteId, setCurrentSite } from "./lib/api";
import type { Settings, Site, User } from "./lib/types";
import { THEMES, applyTheme, loadTheme, readCachedTheme, saveTheme } from "./lib/theme";
import type { ThemeId, ThemePersistence } from "./lib/theme";
import { Icon, LogoMark } from "./components/icons";
import type { IconName } from "./components/icons";
import { CommandPalette, ToastProvider, useCommandPaletteHotkey } from "./components/elements";
import type { PaletteEntry } from "./components/elements";
import { LoginPage } from "./pages/Login";
import { DashboardPage } from "./pages/Dashboard";
import { PostsPage } from "./pages/Posts";
import { EditorPage } from "./pages/Editor";
import { MediaPage } from "./pages/Media";
import { TermsPage } from "./pages/Terms";
import { CommentsPage } from "./pages/Comments";
import { MenusPage } from "./pages/Menus";
import { UsersPage } from "./pages/Users";
import { PluginsPage } from "./pages/Plugins";
import { SettingsPage } from "./pages/Settings";
import { HealthPage } from "./pages/Health";
import { ActivityPage } from "./pages/Activity";
import { ConversionPage } from "./pages/Conversion";
import { ThemeEditorPage } from "./pages/ThemeEditor";
import { CommandCentrePage } from "./pages/CommandCentre";
import { SeoPage } from "./pages/Seo";
import { StudioPage } from "./pages/Studio";
import { CommercePage } from "./pages/Commerce";
import { IntegrationsPage } from "./pages/Integrations";
import { SecurityPage } from "./pages/Security";
import { BackupPage } from "./pages/Backup";
import { ImportPage } from "./pages/Import";
import { FilesPage } from "./pages/Files";
import { SocialPage } from "./pages/Social";
import { SchoolPage } from "./pages/School";
import { BuilderPage } from "./pages/Builder";
import { SitesPage } from "./pages/Sites";

export interface AppSettings {
  timezone: string;
  permalinkStructure: string;
  siteTitle: string;
}

const NAV: Array<{ hash: string; label: string; icon: IconName; group: string }> = [
  { hash: "#/dashboard", label: "Dashboard", icon: "dashboard", group: "Content" },
  { hash: "#/posts", label: "Posts", icon: "posts", group: "Content" },
  { hash: "#/pages", label: "Pages", icon: "pages", group: "Content" },
  { hash: "#/builder", label: "Builder", icon: "layout", group: "Content" },
  { hash: "#/media", label: "Media", icon: "media", group: "Content" },
  { hash: "#/files", label: "Files", icon: "file", group: "Content" },
  { hash: "#/terms", label: "Categories & Tags", icon: "terms", group: "Content" },
  { hash: "#/comments", label: "Comments", icon: "comments", group: "Content" },
  { hash: "#/menus", label: "Menus", icon: "menus", group: "Content" },
  { hash: "#/users", label: "Users", icon: "users", group: "Site" },
  { hash: "#/settings", label: "Settings", icon: "settings", group: "Site" },
  { hash: "#/sites", label: "Sites", icon: "globe", group: "Site" },
  { hash: "#/health", label: "Site Health", icon: "health", group: "Site" },
  { hash: "#/activity", label: "Activity", icon: "activity", group: "Site" },
  { hash: "#/themes", label: "Themes", icon: "themes", group: "Site" },
  { hash: "#/ai", label: "AI", icon: "ai", group: "Grow" },
  { hash: "#/seo", label: "SEO", icon: "seo", group: "Grow" },
  { hash: "#/studio", label: "Content Studio", icon: "studio", group: "Grow" },
  { hash: "#/commerce", label: "Commerce", icon: "commerce", group: "Grow" },
  { hash: "#/social", label: "Community", icon: "users", group: "Grow" },
  { hash: "#/school", label: "School", icon: "book", group: "Grow" },
  { hash: "#/integrations", label: "Integrations", icon: "integrations", group: "Grow" },
  { hash: "#/plugins", label: "Plugins", icon: "package", group: "System" },
  { hash: "#/conversion", label: "Conversion", icon: "conversion", group: "System" },
  { hash: "#/security", label: "Security", icon: "security", group: "System" },
  { hash: "#/backup", label: "Backup", icon: "backup", group: "System" },
  { hash: "#/import", label: "Import", icon: "import", group: "System" },
];

const NAV_GROUPS = ["Content", "Site", "Grow", "System"];

function routeFromHash(): string {
  const hash = window.location.hash.replace(/^#\/?/, "");
  return hash === "" ? "dashboard" : hash;
}

function pageTitle(route: string): string {
  const item = NAV.find((n) => n.hash === `#/${route.split("/")[0] ?? ""}`);
  if (route.startsWith("posts/new")) return "New Post";
  if (route.startsWith("posts/")) return "Edit Post";
  if (route.startsWith("pages/new")) return "New Page";
  if (route.startsWith("pages/")) return "Edit Page";
  if (route.startsWith("builder/")) return "Visual Builder";
  if (route === "builder") return "Visual Builder";
  return item?.label ?? "Dashboard";
}

export function App(): React.ReactElement {
  const [user, setUser] = useState<User | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  // P12 multisite: the sites list for the header switcher, and the
  // id the admin is acting on (null = main site). The header select
  // writes it via lib/api and reloads so every page re-fetches
  // under the new site.
  const [sites, setSites] = useState<Site[]>([]);
  const [siteId, setSiteIdState] = useState<number | null>(() => getCurrentSiteId());
  const refreshSites = (): void => {
    api
      .listSites()
      .then((list) => setSites(list))
      .catch(() => {
        // Non-admins get 403 here; the switcher simply stays hidden.
      });
  };
  const [checking, setChecking] = useState(true);
  const [route, setRoute] = useState(routeFromHash());
  const [navOpen, setNavOpen] = useState(false);
  // P4 theme: cached choice first (no flash — main.tsx already applied it),
  // refined from the user profile once /api/auth/me resolves.
  const [theme, setTheme] = useState<ThemeId>(() => readCachedTheme() ?? "teal");
  const [themePersisted, setThemePersisted] = useState<ThemePersistence | "role-default" | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  // Set when session restore finds a forced password change pending:
  // the user is sent to sign-in with an explanation (see below).
  const [loginNotice, setLoginNotice] = useState<string | null>(null);

  useCommandPaletteHotkey(() => {
    if (user !== null) setPaletteOpen(true);
  });

  useEffect(() => {
    const onHash = (): void => {
      setRoute(routeFromHash());
      setNavOpen(false);
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  useEffect(() => {
    let cancelled = false;
    api
      .me()
      .then(async ({ user: me, mustChangePassword }) => {
        if (cancelled) return;
        if (mustChangePassword === true) {
          // Forced password change gate (P4, coordinator fix): a session
          // restored from /me must NOT bypass the forced-change step.
          // A temp-password session is not an admin session — revoke it
          // (best effort; the flag stays set server-side) and send the
          // user to sign-in with an explanation. The Login page's
          // forced-change step takes over after they sign in with the
          // temporary password; until then nothing in the admin loads.
          void api.logout().catch(() => {
            // The session may already be gone; the flag is what matters.
          });
          setLoginNotice(
            "Your password was reset — sign in with your temporary password, then set a new one before you can use the admin.",
          );
          setUser(null);
          return;
        }
        setUser(me);
        // /api/auth/me carries no theme field (User = id/login/email/
        // displayName/role/registered), so the effective theme comes from
        // the user profile — lib/theme.ts loadTheme() reads it from
        // GET /api/users/:id (UserProfile.theme), then the local fallback,
        // then the role default. Personal choice overrides role default.
        void loadTheme(me).then(({ theme: t, source }) => {
          if (cancelled) return;
          setTheme(t);
          setThemePersisted(source);
          applyTheme(t);
        });
        try {
          const s: Settings = await api.getSettings();
          if (!cancelled) {
            setSettings({ timezone: s.timezone, permalinkStructure: s.permalinkStructure, siteTitle: s.siteTitle });
          }
        } catch {
          // Settings need manage_options; editors can still use the admin.
          if (!cancelled) setSettings({ timezone: "UTC", permalinkStructure: "postname", siteTitle: "Wordless" });
        }
        if (!cancelled) refreshSites();
      })
      .catch((err: unknown) => {
        if (!cancelled && !(err instanceof ApiError && err.status === 401)) {
          // Any failure to check the session → treat as logged out.
        }
        if (!cancelled) setUser(null);
      })
      .finally(() => {
        if (!cancelled) setChecking(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const go = (hash: string): void => {
    window.location.hash = hash;
  };

  const changeTheme = (id: ThemeId): void => {
    setTheme(id);
    applyTheme(id);
    if (user !== null) {
      void saveTheme(user.id, id).then(({ persisted }) => setThemePersisted(persisted));
    }
  };

  if (checking) {
    return (
      <div className="login-wrap">
        <div className="spinner-row">
          <span className="spinner" aria-hidden="true" />
          <span style={{ color: "rgba(255,255,255,0.75)" }}>Loading Wordless admin…</span>
        </div>
      </div>
    );
  }

  if (!user) {
    return (
      <LoginPage
        notice={loginNotice ?? undefined}
        onLoggedIn={(me) => {
          setLoginNotice(null);
          setUser(me);
          go("#/dashboard");
        }}
      />
    );
  }

  const logout = async (): Promise<void> => {
    try {
      await api.logout();
    } catch {
      // Ignore logout errors; the session cookie is cleared server-side on success.
    }
    setLoginNotice(null);
    setUser(null);
    go("#/login");
  };

  const [section, ...rest] = route.split("/");
  const tz = settings?.timezone ?? "UTC";
  const permalinkStructure = settings?.permalinkStructure ?? "postname";

  let page: React.ReactElement;
  switch (section) {
    case "posts":
      if (rest[0] === "new") {
        page = <EditorPage type="post" settings={{ timezone: tz, permalinkStructure }} onDone={() => go("#/posts")} />;
      } else if (rest[0]) {
        page = (
          <EditorPage
            type="post"
            postId={Number(rest[0])}
            settings={{ timezone: tz, permalinkStructure }}
            onDone={() => go("#/posts")}
          />
        );
      } else {
        page = <PostsPage type="post" timezone={tz} />;
      }
      break;
    case "pages":
      if (rest[0] === "new") {
        page = <EditorPage type="page" settings={{ timezone: tz, permalinkStructure }} onDone={() => go("#/pages")} />;
      } else if (rest[0]) {
        page = (
          <EditorPage
            type="page"
            postId={Number(rest[0])}
            settings={{ timezone: tz, permalinkStructure }}
            onDone={() => go("#/pages")}
          />
        );
      } else {
        page = <PostsPage type="page" timezone={tz} />;
      }
      break;
    case "builder":
      page = rest[0] ? <BuilderPage postId={Number(rest[0])} /> : <BuilderPage />;
      break;
    case "media":
      page = <MediaPage />;
      break;
    case "files":
      page = <FilesPage />;
      break;
    case "terms":
      page = <TermsPage />;
      break;
    case "comments":
      page = <CommentsPage timezone={tz} />;
      break;
    case "menus":
      page = <MenusPage />;
      break;
    case "users":
      page = <UsersPage currentUser={user} />;
      break;
    case "plugins":
      page = <PluginsPage />;
      break;
    case "settings":
      page = <SettingsPage onSaved={setSettings} />;
      break;
    case "sites":
      page = <SitesPage onSitesChanged={refreshSites} />;
      break;
    case "health":
      page = <HealthPage />;
      break;
    case "activity":
      page = <ActivityPage timezone={tz} />;
      break;
    case "conversion":
      page = <ConversionPage />;
      break;
    case "themes":
      page = <ThemeEditorPage />;
      break;
    case "ai":
      page = <CommandCentrePage />;
      break;
    case "seo":
      page = <SeoPage />;
      break;
    case "studio":
      page = <StudioPage />;
      break;
    case "commerce":
      page = <CommercePage />;
      break;
    case "integrations":
      page = <IntegrationsPage />;
      break;
    case "security":
      page = <SecurityPage />;
      break;
    case "social":
      page = <SocialPage timezone={tz} />;
      break;
    case "school":
      page = <SchoolPage timezone={tz} user={user} />;
      break;
    case "backup":
      page = <BackupPage />;
      break;
    case "import":
      page = <ImportPage />;
      break;
    case "dashboard":
    default:
      page = <DashboardPage timezone={tz} />;
      break;
  }

  const paletteEntries: PaletteEntry[] = [
    ...NAV.map((item) => ({
      id: `page-${item.label}`,
      label: item.label,
      group: item.group,
      icon: item.icon,
      onSelect: () => go(item.hash),
    })),
    { id: "action-new-post", label: "New post", group: "Actions", icon: "plus" as IconName, onSelect: () => go("#/posts/new") },
    { id: "action-logout", label: "Log out", group: "Actions", icon: "logout" as IconName, onSelect: () => void logout() },
  ];

  return (
    <ToastProvider>
    <div className={`app-shell${navOpen ? " nav-open" : ""}`}>
      {navOpen ? (
        <button
          type="button"
          className="sidebar-scrim"
          aria-label="Close navigation"
          onClick={() => setNavOpen(false)}
        />
      ) : null}
      <aside className="sidebar">
        <div className="brand">
          <LogoMark size={40} />
          <div className="wordmark">
            <h1>Wordless</h1>
            <p>{settings?.siteTitle ?? "Admin"}</p>
          </div>
        </div>
        <nav>
          {NAV_GROUPS.map((group) => (
            <div key={group}>
              <div className="nav-section">{group}</div>
              {NAV.filter((item) => item.group === group).map((item) => (
                <a
                  key={item.hash}
                  href={item.hash}
                  className={section === item.hash.replace("#/", "") ? "active" : ""}
                >
                  <Icon name={item.icon} size={18} />
                  {item.label}
                </a>
              ))}
            </div>
          ))}
        </nav>
        <div className="side-foot">
          <div className="side-user">
            <span className="avatar" aria-hidden="true">
              {initials(user.displayName)}
            </span>
            <div style={{ minWidth: 0 }}>
              <div className="who-name">{user.displayName}</div>
              <div className="who-role">{user.role}</div>
            </div>
          </div>
          <button type="button" onClick={() => void logout()}>
            <Icon name="logout" size={16} />
            Log out
          </button>
          <a className="side-link" href="/" target="_blank" rel="noreferrer">
            View site <Icon name="external" size={14} />
          </a>
        </div>
      </aside>
      <div className="main">
        <header className="topbar">
          <button
            type="button"
            className="menu-btn icon-btn"
            aria-label="Open navigation"
            onClick={() => setNavOpen(true)}
          >
            <Icon name="menu" size={20} />
          </button>
          <h2>{pageTitle(route)}</h2>
          {sites.length > 0 ? (
            <span className="theme-switcher">
              <label htmlFor="site-select">Site</label>
              <select
                id="site-select"
                aria-label="Current site"
                value={String(siteId ?? 1)}
                onChange={(e) => {
                  const id = Number(e.target.value);
                  setSiteIdState(id === 1 ? null : id);
                  setCurrentSite(id === 1 ? null : id);
                  // Reload so settings, posts, media and every other
                  // page re-fetch under the new site.
                  window.location.reload();
                }}
              >
                {sites.map((site) => (
                  <option key={site.id} value={site.id}>
                    {site.title}
                    {site.slug === "" ? "" : ` (/${site.slug})`}
                  </option>
                ))}
              </select>
            </span>
          ) : null}
          <span className="theme-switcher">
            <label htmlFor="theme-select">Theme</label>
            <select
              id="theme-select"
              aria-label="Dashboard theme"
              value={theme}
              onChange={(e) => changeTheme(e.target.value as ThemeId)}
            >
              {THEMES.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </select>
            {themePersisted === "local" ? (
              <span
                className="theme-note"
                title="The profile endpoint (PUT /api/users/:id/profile) is not reachable from here yet — your theme is saved in this browser for now."
              >
                Saved locally
              </span>
            ) : null}
          </span>
          <button
            type="button"
            className="icon-btn"
            aria-label="Open command palette (Ctrl+K)"
            title="Search / commands (Ctrl+K)"
            onClick={() => setPaletteOpen(true)}
          >
            <Icon name="search" size={20} />
          </button>
          <span className="who">
            <span className="avatar light" aria-hidden="true">
              {initials(user.displayName)}
            </span>
            <span className="who-name">{user.login}</span>
          </span>
        </header>
        <main className="content">{page}</main>
      </div>
    </div>
    <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} entries={paletteEntries} />
    </ToastProvider>
  );
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "W";
  const first = parts[0]?.[0] ?? "W";
  const last = parts.length > 1 ? (parts[parts.length - 1]?.[0] ?? "") : "";
  return (first + last).toUpperCase();
}
