/**
 * Wordless v2.0 dashboard themes (P4 admin foundation).
 *
 * Source of truth: the v2 UI preview artifact
 * (~/workspace/ts-spaces/wordless-v2-ui-preview/index.html, THEMES array,
 * lines ~1012-1031). The eight `--th-*` token values below are copied
 * EXACTLY from that array — a theme is a token set over the §6 design
 * system, not a separate stylesheet per screen. `styles.css` maps these
 * `--th-*` vars onto the admin's own tokens (`--bg`, `--surface`, …) via
 * `[data-theme="<id>"]` blocks, so every screen re-skins by switching one
 * attribute on the root element.
 *
 * Persistence contract (API worker — LANDED in the Rust worker during
 * this task, verified in source 2026-11-19, not yet verified live):
 * the theme is stored as wp_usermeta `wl_admin_theme` (see
 * `ADMIN_THEME_META_KEY` in crates/wordless-auth/src/users.rs):
 *   PUT /api/users/:id/profile  { "theme": "<id>" }   → updated UserProfile
 *   GET /api/users/:id                              → UserProfile { theme }
 * The PUT accepts exactly the 8 ids in `ADMIN_THEMES` (400 otherwise).
 * There is NO `GET /api/users/:id/profile` route — the profile is read
 * from `GET /api/users/:id` (self-read allowed; others need edit_users).
 *
 * `theme` is `null` when nothing is stored (fixed by the API worker in
 * P4 — `theme_is_null_when_unset_and_teal_only_when_picked` in
 * crates/wordless-auth/src/users.rs), so a stored "teal" IS an explicit
 * personal choice and wins over a non-teal role default. An earlier
 * draft of this file worked around a "teal means unset" server
 * behaviour; that workaround is removed.
 *
 * Role defaults (preview ROLE_DEFAULTS): Administrator→teal, Editor→ocean,
 * Author→slate, Contributor→slate (task decision — the preview has no
 * Contributor entry), Subscriber→berry. Shop Manager→sunset,
 * Teacher→forest and Student→ocean are the preview's entries for roles
 * that do not exist yet in v2 (P8 WooCommerce / P9 school); they are kept
 * in FUTURE_ROLE_DEFAULTS below, commented as future roles.
 */
import { api } from "./api";
import type { Role } from "./types";

export type ThemeId = "teal" | "midnight" | "ocean" | "forest" | "sunset" | "berry" | "slate" | "contrast";

/** The eight `--th-*` vars per theme — exact copies from the preview THEMES array. */
export interface ThemeVars {
  "--th-bg": string;
  "--th-surface": string;
  "--th-soft": string;
  "--th-accent": string;
  "--th-sidebar": string;
  "--th-ink": string;
  "--th-ink2": string;
  "--th-line": string;
}

export interface Theme {
  id: ThemeId;
  name: string;
  desc: string;
  vars: ThemeVars;
}

export const THEMES: Theme[] = [
  { id: "teal", name: "Deep Teal", desc: "The Wordless signature. Calm, confident, readable everywhere.", vars: { "--th-bg": "#f2f6f4", "--th-surface": "#ffffff", "--th-soft": "#e0f2ee", "--th-accent": "#0d9488", "--th-sidebar": "#07332e", "--th-ink": "#0b2422", "--th-ink2": "#43584f", "--th-line": "#dce7e2" } },
  { id: "midnight", name: "Midnight", desc: "A true dark dashboard for late-night work — easy on the eyes, sharp on contrast.", vars: { "--th-bg": "#0b1220", "--th-surface": "#131f36", "--th-soft": "#1b2c49", "--th-accent": "#38bdf8", "--th-sidebar": "#060b16", "--th-ink": "#e6eefc", "--th-ink2": "#9db1cc", "--th-line": "#22334f" } },
  { id: "ocean", name: "Ocean Blue", desc: "Clean corporate blue — familiar for editorial teams moving from other tools.", vars: { "--th-bg": "#eff4fd", "--th-surface": "#ffffff", "--th-soft": "#e3ecfd", "--th-accent": "#2563eb", "--th-sidebar": "#12275c", "--th-ink": "#0d1b33", "--th-ink2": "#44577a", "--th-line": "#d5e0f2" } },
  { id: "forest", name: "Forest", desc: "Deep greens with a warm paper background — the teacher and school favourite.", vars: { "--th-bg": "#f0f7f1", "--th-surface": "#ffffff", "--th-soft": "#dcf0e1", "--th-accent": "#15803d", "--th-sidebar": "#123524", "--th-ink": "#0e2417", "--th-ink2": "#47624e", "--th-line": "#d6e7da" } },
  { id: "sunset", name: "Sunset Amber", desc: "Warm amber energy — built for shop managers who live in orders and stock.", vars: { "--th-bg": "#fdf6ec", "--th-surface": "#ffffff", "--th-soft": "#fdf0dc", "--th-accent": "#d97706", "--th-sidebar": "#3a2408", "--th-ink": "#2b1c07", "--th-ink2": "#6f5a36", "--th-line": "#f0e0c2" } },
  { id: "berry", name: "Berry", desc: "Bold berry accent on a soft blush background — community managers love it.", vars: { "--th-bg": "#fdf1f6", "--th-surface": "#ffffff", "--th-soft": "#fbdfea", "--th-accent": "#db2777", "--th-sidebar": "#3d0f26", "--th-ink": "#2c0f1e", "--th-ink2": "#71455c", "--th-line": "#f3d3e0" } },
  { id: "slate", name: "Slate Mono", desc: "Quiet greys, zero distraction — for writers who want the words to be the colour.", vars: { "--th-bg": "#f1f5f9", "--th-surface": "#ffffff", "--th-soft": "#e2e8f0", "--th-accent": "#475569", "--th-sidebar": "#1e293b", "--th-ink": "#0f172a", "--th-ink2": "#526174", "--th-line": "#dbe3ec" } },
  { id: "contrast", name: "High Contrast", desc: "Maximum contrast, thick focus states — the accessibility-first theme.", vars: { "--th-bg": "#ffffff", "--th-surface": "#ffffff", "--th-soft": "#e8e8e8", "--th-accent": "#005fcc", "--th-sidebar": "#000000", "--th-ink": "#000000", "--th-ink2": "#333333", "--th-line": "#9a9a9a" } },
];

export const DEFAULT_THEME: ThemeId = "teal";

/**
 * Role-based defaults. Preview display names in the comment; keys are the
 * app's Role values (lowercase, see lib/types.ts).
 *   Administrator→teal, Editor→ocean, Author→slate, Contributor→slate,
 *   Subscriber→berry.
 */
export const ROLE_DEFAULTS: Record<Role, ThemeId> = {
  administrator: "teal",
  editor: "ocean",
  author: "slate",
  contributor: "slate",
  subscriber: "berry",
};

/**
 * Future roles from the preview ROLE_DEFAULTS — roles that do not exist
 * yet in v2, so these are NOT active defaults. Uncomment (and add to the
 * Role type) when the role lands:
 *   Shop Manager → sunset   (P8 WooCommerce)
 *   Teacher       → forest   (P9 school portal)
 *   Student       → ocean    (P9 school portal)
 */
export const FUTURE_ROLE_DEFAULTS: Record<string, ThemeId> = {
  // "shop-manager": "sunset",
  // "teacher": "forest",
  // "student": "ocean",
};

export function isThemeId(value: unknown): value is ThemeId {
  return typeof value === "string" && THEMES.some((t) => t.id === value);
}

export function themeById(id: ThemeId): Theme {
  return THEMES.find((t) => t.id === id) ?? THEMES[0]!;
}

export function getRoleDefaultTheme(role: Role): ThemeId {
  return ROLE_DEFAULTS[role] ?? DEFAULT_THEME;
}

/**
 * Personal choice overrides the role default. `personal` is the theme the
 * user picked (from their profile or local fallback); `role` is their
 * WordPress role. Returns the theme id that must be applied.
 */
export function getEffectiveTheme(user: { role: Role; theme?: ThemeId | null }): ThemeId {
  if (user.theme !== undefined && user.theme !== null && isThemeId(user.theme)) return user.theme;
  return getRoleDefaultTheme(user.role);
}

/** localStorage key for the session fallback (see module header). */
export const THEME_STORAGE_KEY = "wl_admin_theme";

/** Apply a theme: set `data-theme` on the root element (+ a local cache write). */
export function applyTheme(id: ThemeId): void {
  if (typeof document !== "undefined") {
    document.documentElement.setAttribute("data-theme", id);
  }
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, id);
  } catch {
    // Storage unavailable (private mode, etc.) — the attribute still applies.
  }
}

/** Read the locally cached theme choice, if any (no network). */
export function readCachedTheme(): ThemeId | null {
  try {
    const raw = window.localStorage.getItem(THEME_STORAGE_KEY);
    return isThemeId(raw) ? raw : null;
  } catch {
    return null;
  }
}

export type ThemePersistence = "api" | "local";

/**
 * Load the user's stored theme. Precedence: the server profile first
 * (GET /api/users/:id → UserProfile.theme), then the localStorage
 * fallback, then the role default. Never throws: on any failure it
 * returns the role default and reports where the value came from.
 * A server theme (including an explicit "teal") is always a personal
 * choice and wins over the role default; `theme: null` means unset.
 */
export async function loadTheme(user: { id: number; role: Role }): Promise<{ theme: ThemeId; source: ThemePersistence | "role-default" }> {
  try {
    const profile = await api.get<{ theme?: unknown }>(`/api/users/${user.id}`);
    if (isThemeId(profile.theme)) {
      return { theme: profile.theme, source: "api" };
    }
  } catch {
    // Profile read failed (API unreachable, or the P4 profile work not
    // deployed yet) — fall through to the local cache, then role default.
  }
  const cached = readCachedTheme();
  if (cached !== null) return { theme: cached, source: "local" };
  return { theme: getEffectiveTheme({ role: user.role }), source: "role-default" };
}

/**
 * Save the user's theme choice. Calls exactly the contract endpoint
 * (PUT /api/users/:id/profile { theme }) and applies the theme locally
 * either way. If the endpoint is unreachable or not deployed yet, the
 * choice is kept in localStorage for the session and the result
 * reports `persisted: "local"` — the caller can surface that the
 * server-side persistence is still pending. Never invents another
 * endpoint or meta key.
 */
export async function saveTheme(userId: number, id: ThemeId): Promise<{ persisted: ThemePersistence }> {
  applyTheme(id);
  try {
    await api.put(`/api/users/${userId}/profile`, { theme: id });
    return { persisted: "api" };
  } catch {
    // Contract endpoint not live yet — local fallback (applyTheme already cached it).
    return { persisted: "local" };
  }
}

/** Density setting hook (§6 principle 8): comfortable (default) / compact. */
export type Density = "comfortable" | "compact";
export const DENSITY_STORAGE_KEY = "wl_admin_density";

export function applyDensity(density: Density): void {
  if (typeof document === "undefined") return;
  document.documentElement.classList.toggle("density-compact", density === "compact");
  try {
    window.localStorage.setItem(DENSITY_STORAGE_KEY, density);
  } catch {
    // Ignore storage failures; the class still applies.
  }
}

export function readCachedDensity(): Density {
  try {
    return window.localStorage.getItem(DENSITY_STORAGE_KEY) === "compact" ? "compact" : "comfortable";
  } catch {
    return "comfortable";
  }
}
