/**
 * Tweakable login page — branding model (P4, doc §6.3 Login + §3 P4).
 *
 * The login page is customised from Settings → Appearance → Login and
 * stored as wp_options keys `wl_login_*` (see LOGIN_OPTION_KEYS).
 *
 * READ PATH — LANDED (API worker, P4): public `GET /api/login-branding`
 * (no auth) returns exactly these eleven values, defaulting every
 * missing/invalid one — see crates/wordless-cms/src/login_branding.rs.
 * `fetchLoginBranding()` calls it and falls back to the local copy,
 * then the defaults, so the login page never breaks if the endpoint
 * is unreachable.
 *
 * WRITE PATH — LANDED (API worker, P4): the same eleven fields are
 * written flat (camelCase: loginLogo, loginAccent, …) through the
 * existing `PUT /api/settings` (manage_options) — see
 * `validate_login_branding_patch` in login_branding.rs. Bad hex
 * colours are rejected 400; the server validates, the client also
 * validates before sending. `saveLoginBranding()` uses that route
 * and throws on failure — the customiser surfaces the server message
 * instead of silently saving locally. A local copy is still kept as
 * a read cache for the login page when the endpoint is unreachable.
 *
 * Server defaults reproduce the v1 login look: accent #0d9488, deep
 * #07332e, bgMode "soft", layout "card", heading "Welcome back",
 * tagline "The autonomous website operating system", showSocial false,
 * showBackLink true. `bgMode` also accepts "image" on the wire (the
 * API worker's round-trip test uses it); it is normalised to "photo"
 * here. `layout` "card" is the centred card; "split" is the split
 * brand panel.
 */
import { api } from "./api";

export type LoginBgMode = "soft" | "gradient" | "dark" | "photo";
export type LoginLayout = "card" | "split";

export interface LoginBranding {
  /** Logo image URL (media library) — empty = the Wordless mark. */
  logo: string;
  /** Brand accent colour (buttons, links, focus). */
  accent: string;
  /** Deep colour (page background gradient base / split panel). */
  deep: string;
  bgMode: LoginBgMode;
  /** Photo URL used only when bgMode is "photo". */
  bgImage: string;
  layout: LoginLayout;
  heading: string;
  tagline: string;
  showSocial: boolean;
  showBackLink: boolean;
  /** Extra CSS applied to the login page only. */
  customCss: string;
}

export const DEFAULT_LOGIN_BRANDING: LoginBranding = {
  logo: "",
  accent: "#0d9488",
  deep: "#07332e",
  bgMode: "soft",
  bgImage: "",
  layout: "card",
  heading: "Welcome back",
  tagline: "The autonomous website operating system",
  showSocial: false,
  showBackLink: true,
  customCss: "",
};

/** The wp_options keys, exactly as specified for this feature. */
export const LOGIN_OPTION_KEYS = [
  "wl_login_logo",
  "wl_login_accent",
  "wl_login_deep",
  "wl_login_bg_mode",
  "wl_login_bg_image",
  "wl_login_layout",
  "wl_login_heading",
  "wl_login_tagline",
  "wl_login_show_social",
  "wl_login_show_back_link",
  "wl_login_custom_css",
] as const;

export const LOGIN_BRANDING_STORAGE_KEY = "wl_login_branding";

/** The public branding endpoint. */
export const LOGIN_BRANDING_ENDPOINT = "/api/login-branding";

function isBgMode(v: unknown): v is LoginBgMode {
  return v === "soft" || v === "gradient" || v === "dark" || v === "photo" || v === "image";
}

function isLayout(v: unknown): v is LoginLayout {
  return v === "card" || v === "split" || v === "centred";
}

/** Normalise an unknown payload onto the defaults (unknown fields ignored). */
export function normaliseBranding(raw: unknown): LoginBranding {
  const r = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  const str = (k: string, fallback: string): string =>
    typeof r[k] === "string" && (r[k] as string).length > 0 ? (r[k] as string) : fallback;
  // Tagline: blank HIDES it (the server keeps "" as ""), so an empty
  // string is a real value here, not a missing one.
  const tagline = typeof r.tagline === "string" ? r.tagline : DEFAULT_LOGIN_BRANDING.tagline;
  const bgMode: LoginBgMode =
    r.bgMode === "image" ? "photo" : isBgMode(r.bgMode) ? (r.bgMode as LoginBgMode) : DEFAULT_LOGIN_BRANDING.bgMode;
  const layout: LoginLayout =
    r.layout === "centred" ? "card" : isLayout(r.layout) ? (r.layout as LoginLayout) : DEFAULT_LOGIN_BRANDING.layout;
  return {
    logo: str("logo", DEFAULT_LOGIN_BRANDING.logo),
    accent: str("accent", DEFAULT_LOGIN_BRANDING.accent),
    deep: str("deep", DEFAULT_LOGIN_BRANDING.deep),
    bgMode,
    bgImage: str("bgImage", DEFAULT_LOGIN_BRANDING.bgImage),
    layout,
    heading: str("heading", DEFAULT_LOGIN_BRANDING.heading),
    tagline,
    showSocial: typeof r.showSocial === "boolean" ? r.showSocial : DEFAULT_LOGIN_BRANDING.showSocial,
    showBackLink: typeof r.showBackLink === "boolean" ? r.showBackLink : DEFAULT_LOGIN_BRANDING.showBackLink,
    customCss: typeof r.customCss === "string" ? r.customCss : DEFAULT_LOGIN_BRANDING.customCss,
  };
}

/** Local (browser) copy — read cache only; the server is the source of truth. */
export function readLocalBranding(): LoginBranding | null {
  try {
    const raw = window.localStorage.getItem(LOGIN_BRANDING_STORAGE_KEY);
    if (raw === null) return null;
    return normaliseBranding(JSON.parse(raw) as unknown);
  } catch {
    return null;
  }
}

export function saveLocalBranding(b: LoginBranding): void {
  try {
    window.localStorage.setItem(LOGIN_BRANDING_STORAGE_KEY, JSON.stringify(b));
  } catch {
    // Storage unavailable — the in-memory state still applies for this session.
  }
}

export function clearLocalBranding(): void {
  try {
    window.localStorage.removeItem(LOGIN_BRANDING_STORAGE_KEY);
  } catch {
    // Ignore.
  }
}

/**
 * Fetch the public branding for the login page. Tries, in order:
 * the public endpoint, then the local copy, then the defaults.
 * Never throws. Returns where the value came from so the UI can be
 * honest about it.
 */
export async function fetchLoginBranding(): Promise<{ branding: LoginBranding; source: "api" | "local" | "default" }> {
  try {
    const remote = await api.get<Record<string, unknown>>(LOGIN_BRANDING_ENDPOINT);
    return { branding: normaliseBranding(remote), source: "api" };
  } catch {
    const local = readLocalBranding();
    if (local !== null) return { branding: local, source: "local" };
    return { branding: DEFAULT_LOGIN_BRANDING, source: "default" };
  }
}

/**
 * Save branding to the server via PUT /api/settings (flat camelCase
 * branding fields). Throws on failure — callers surface the message.
 * On success the local read cache is refreshed too.
 */
export async function saveLoginBranding(b: LoginBranding): Promise<void> {
  await api.put("/api/settings", {
    loginLogo: b.logo,
    loginAccent: b.accent,
    loginDeep: b.deep,
    loginBgMode: b.bgMode,
    loginBgImage: b.bgImage,
    loginLayout: b.layout,
    loginHeading: b.heading,
    loginTagline: b.tagline,
    loginShowSocial: b.showSocial,
    loginShowBackLink: b.showBackLink,
    loginCustomCss: b.customCss,
  });
  saveLocalBranding(b);
}

/** Serialise branding to the wl_login_* option-key map (shown in the customiser for reference). */
export function brandingToOptionMap(b: LoginBranding): Record<string, string> {
  return {
    wl_login_logo: b.logo,
    wl_login_accent: b.accent,
    wl_login_deep: b.deep,
    wl_login_bg_mode: b.bgMode,
    wl_login_bg_image: b.bgImage,
    wl_login_layout: b.layout,
    wl_login_heading: b.heading,
    wl_login_tagline: b.tagline,
    wl_login_show_social: b.showSocial ? "1" : "0",
    wl_login_show_back_link: b.showBackLink ? "1" : "0",
    wl_login_custom_css: b.customCss,
  };
}
