/**
 * Dashboard themes screen (§6.3 + preview screen 11).
 *
 * The eight themes with live previews, the role default for the
 * current user's role, personal override saved to the profile via
 * lib/theme.ts saveTheme (PUT /api/users/:id/profile { theme }),
 * and reset-to-default. Role defaults themselves are constants in
 * lib/theme.ts (ROLE_DEFAULTS) — there is no API to change them, so
 * this screen displays them and says so instead of offering a
 * control that writes nowhere.
 *
 * Used from Settings → Appearance → Dashboard themes (and from the
 * Dashboard's own theme switcher, which is the same contract).
 */
import { useState } from "react";
import type { Role, User } from "../lib/types";
import { Alert, Badge, Button, Card } from "../components/elements";
import { THEMES, getRoleDefaultTheme, saveTheme, themeById } from "../lib/theme";
import type { ThemeId, ThemePersistence } from "../lib/theme";

export function DashboardThemesScreen({ user, currentTheme, onThemeChange }: {
  user: User;
  currentTheme: ThemeId;
  onThemeChange: (id: ThemeId) => void;
}): React.ReactElement {
  const [busy, setBusy] = useState(false);
  const [persisted, setPersisted] = useState<ThemePersistence | "role-default" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const roleDefault = getRoleDefaultTheme(user.role as Role);

  const pick = (id: ThemeId): void => {
    setBusy(true);
    setError(null);
    void saveTheme(user.id, id)
      .then(({ persisted: p }) => {
        setPersisted(p);
        onThemeChange(id);
      })
      .catch(() => setError("The theme was applied locally but could not be saved to your profile."))
      .finally(() => setBusy(false));
  };

  return (
    <div>
      {persisted === "local" ? (
        <Alert tone="warn" title="Saved in this browser only">The profile endpoint (PUT /api/users/:id/profile) is not reachable from here yet, so this choice is kept in this browser for now. It will not follow your account to another device until the endpoint is live.</Alert>
      ) : null}
      {error !== null ? <Alert tone="err" title="Theme" onDismiss={() => setError(null)}>{error}</Alert> : null}

      <Card title="Dashboard themes" sub="Your choice is personal: it follows your account, not the site. Role defaults apply only until you pick your own.">
        <p>
          You are signed in as <strong>{user.displayName}</strong> ({user.role}). Your role default is{" "}
          <strong>{themeById(roleDefault).name}</strong>. Every shipped theme re-skins the whole admin — sidebar,
          cards, buttons, badges — from one token set, so a theme cannot break a screen.
        </p>
        <div className="theme-grid" role="group" aria-label="Dashboard themes">
          {THEMES.map((t) => {
            const active = t.id === currentTheme;
            return (
              <button
                key={t.id}
                type="button"
                className={`theme-card${active ? " active" : ""}`}
                aria-pressed={active}
                disabled={busy}
                onClick={() => pick(t.id)}
              >
                <span className="theme-swatch" style={{ background: t.vars["--th-bg"], borderColor: t.vars["--th-line"] }} aria-hidden="true">
                  <span className="theme-swatch-bar" style={{ background: t.vars["--th-sidebar"] }} />
                  <span className="theme-swatch-card" style={{ background: t.vars["--th-surface"] }}>
                    <span style={{ background: t.vars["--th-accent"] }} />
                    <span style={{ background: t.vars["--th-soft"] }} />
                  </span>
                </span>
                <span className="theme-name">
                  {t.name}
                  {t.id === roleDefault ? " · role default" : ""}
                  {active ? " · current" : ""}
                </span>
                <span className="muted">{t.desc}</span>
              </button>
            );
          })}
        </div>
        <div className="actions" style={{ marginTop: 14 }}>
          <Button variant="secondary" disabled={busy || currentTheme === roleDefault} onClick={() => pick(roleDefault)}>
            Reset to role default ({themeById(roleDefault).name})
          </Button>
        </div>
      </Card>

      <Card title="Role defaults" sub="Fixed in the admin code (lib/theme.ts ROLE_DEFAULTS). There is no API to change them yet, so they are shown here for reference rather than as editable controls.">
        <ul>
          <li>Administrator → Deep Teal</li>
          <li>Editor → Ocean Blue</li>
          <li>Author → Slate Mono</li>
          <li>Contributor → Slate Mono</li>
          <li>Subscriber → Berry</li>
        </ul>
        <p className="hint">Future roles (not in v2 yet): Shop Manager → Sunset Amber, Teacher → Forest, Student → Ocean Blue.</p>
        {persisted === "api" ? <Badge tone="teal">Your choice is saved to your profile</Badge> : null}
      </Card>
    </div>
  );
}
