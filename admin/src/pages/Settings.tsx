/**
 * Settings (§6.3) — grouped, searchable, with inline explanations.
 *
 * Tabs: General (site, timezone, posts per page), Permalinks,
 * Comments, and Appearance — which holds the Dashboard themes
 * screen and the Login customiser (both P4). The search box filters
 * the setting groups by label and explanation text, so a setting can
 * be found without knowing which group it lives in.
 */
import { useEffect, useMemo, useState } from "react";
import { api } from "../lib/api";
import type { Settings, User } from "../lib/types";
import { Alert, Button, Card, Input, Select, Skeleton, Tabs } from "../components/elements";
import { DashboardThemesScreen } from "./DashboardThemes";
import { LoginCustomiserScreen } from "./LoginCustomiser";
import { applyTheme } from "../lib/theme";
import type { ThemeId } from "../lib/theme";
import type { AppSettings } from "../App";

const PERMALINKS = [
  { value: "plain", label: "Plain — /?p=123" },
  { value: "postname", label: "Post name — /my-post" },
  { value: "day-name", label: "Day and name — /2026/11/19/my-post" },
  { value: "month-name", label: "Month and name — /2026/11/my-post" },
];

export function SettingsPage({ onSaved }: { onSaved: (s: AppSettings) => void }): React.ReactElement {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [me, setMe] = useState<User | null>(null);
  const [theme, setTheme] = useState<ThemeId>("teal");
  const [tab, setTab] = useState("general");
  const [appearanceTab, setAppearanceTab] = useState("themes");
  const [search, setSearch] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api
      .getSettings()
      .then((s) => {
        if (!cancelled) setSettings(s);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "Failed to load settings.");
      });
    api
      .me()
      .then(({ user }) => {
        if (!cancelled) {
          setMe(user);
          setTheme((document.documentElement.getAttribute("data-theme") as ThemeId) ?? "teal");
        }
      })
      .catch(() => {
        // Theme screen needs the user; the rest of Settings still works without it.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const update = <K extends keyof Settings>(key: K, value: Settings[K]): void => {
    setSettings((prev) => (prev ? { ...prev, [key]: value } : prev));
  };

  const save = async (): Promise<void> => {
    if (!settings) return;
    setError(null);
    setNotice(null);
    setSaving(true);
    try {
      const saved = await api.updateSettings(settings);
      setSettings(saved);
      setNotice("Settings saved.");
      onSaved({ timezone: saved.timezone, permalinkStructure: saved.permalinkStructure, siteTitle: saved.siteTitle });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Save failed.");
    } finally {
      setSaving(false);
    }
  };

  const q = search.trim().toLowerCase();
  const matches = (text: string): boolean => q === "" || text.toLowerCase().includes(q);

  const groups = useMemo(
    () => [
      { id: "general", label: "General", text: "site title tagline timezone posts per page" },
      { id: "permalinks", label: "Permalinks", text: "permalink structure url slug" },
      { id: "comments", label: "Comments", text: "comments moderation links default status" },
      { id: "appearance", label: "Appearance", text: "appearance dashboard themes login customiser colours logo background" },
    ],
    [],
  );
  const visibleGroups = groups.filter((g) => matches(g.text) || matches(g.label));

  if (settings === null && error === null) {
    return <Skeleton height={220} label="Loading settings" />;
  }

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Settings" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <div className="toolbar">
        <Input label="Search settings" type="search" placeholder="Search settings, e.g. “timezone” or “login”… " value={search} onChange={(e) => setSearch(e.target.value)} />
      </div>

      <Tabs
        ariaLabel="Settings sections"
        value={tab}
        onChange={setTab}
        tabs={visibleGroups.map((g) => ({ id: g.id, label: g.label }))}
      />
      {visibleGroups.length === 0 ? (
        <Card title="No settings match" sub={`Nothing in Settings matches “${search.trim()}”. Try another word.`}>
          <p className="muted">Settings groups: General, Permalinks, Comments, Appearance (dashboard themes and login page).</p>
        </Card>
      ) : null}

      {tab === "general" && settings !== null ? (
        <Card title="General" sub="The basics of the site.">
          <Input label="Site title" id="set-title" value={settings.siteTitle} onChange={(e) => update("siteTitle", e.target.value)} />
          <Input label="Tagline" id="set-tagline" value={settings.siteTagline} onChange={(e) => update("siteTagline", e.target.value)} hint="A short line about the site, shown under the title in the theme header." />
          <Input label="Timezone" id="set-tz" value={settings.timezone} onChange={(e) => update("timezone", e.target.value)} placeholder="Africa/Lagos" hint="An IANA timezone name, for example Africa/Lagos or UTC. Dates are stored in UTC." />
          <Input label="Posts per page" id="set-perpage" type="number" min={1} max={100} value={String(settings.postsPerPage)} onChange={(e) => update("postsPerPage", Number(e.target.value))} />
          <Button variant="primary" disabled={saving} onClick={() => void save()}>{saving ? "Saving…" : "Save settings"}</Button>
        </Card>
      ) : null}

      {tab === "permalinks" && settings !== null ? (
        <Card title="Permalinks" sub="How post URLs are built.">
          <Select label="Permalink structure" id="set-permalink" value={settings.permalinkStructure} onChange={(e) => update("permalinkStructure", e.target.value)} hint="Changing the structure does not break existing URLs: the old URL keeps working through an automatic redirect.">
            {PERMALINKS.map((p) => (
              <option key={p.value} value={p.value}>{p.label}</option>
            ))}
          </Select>
          <Button variant="primary" disabled={saving} onClick={() => void save()}>{saving ? "Saving…" : "Save settings"}</Button>
        </Card>
      ) : null}

      {tab === "comments" && settings !== null ? (
        <Card title="Comments" sub="What happens to new comments by default.">
          <Select label="Default comment status for new posts" id="set-comment-status" value={settings.defaultCommentStatus} onChange={(e) => update("defaultCommentStatus", e.target.value)}>
            <option value="open">Open</option>
            <option value="closed">Closed</option>
          </Select>
          <Input label="Maximum links before a comment is held for moderation" id="set-max-links" type="number" min={0} max={10} value={String(settings.commentMaxLinks)} onChange={(e) => update("commentMaxLinks", Number(e.target.value))} hint="0 means any comment with a link is held for moderation." />
          <Button variant="primary" disabled={saving} onClick={() => void save()}>{saving ? "Saving…" : "Save settings"}</Button>
        </Card>
      ) : null}

      {tab === "appearance" ? (
        <>
          <Tabs
            ariaLabel="Appearance sections"
            value={appearanceTab}
            onChange={setAppearanceTab}
            tabs={[
              { id: "themes", label: "Dashboard themes", icon: "palette" },
              { id: "login", label: "Login page", icon: "lock" },
            ]}
          />
          {appearanceTab === "themes" ? (
            me === null ? (
              <Skeleton height={200} label="Loading your account for themes" />
            ) : (
              <DashboardThemesScreen
                user={me}
                currentTheme={theme}
                onThemeChange={(id) => {
                  setTheme(id);
                  applyTheme(id);
                }}
              />
            )
          ) : (
            <LoginCustomiserScreen />
          )}
        </>
      ) : null}
    </div>
  );
}
