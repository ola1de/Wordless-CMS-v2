/**
 * Minimal fetch wrapper for the Wordless JSON envelope
 * `{ ok, data, error }`. Same-origin credentials so the session cookie
 * (`wordless_session`) is sent. Throws ApiError on non-OK responses.
 */
import type {
  AgentInfo,
  AiApproval,
  AiApprovalList,
  AiMode,
  AiModeResponse,
  AiProvidersResponse,
  AiTask,
  AiTaskList,
  AnalyticsStatus,
  BindingStatus,
  ContentCalendar,
  ContentTemplate,
  ConversionJob,
  ConversionJobList,
  ExtensionDetail,
  ExtensionLoadResult,
  ExtensionLogList,
  ExtensionPermissions,
  ExtensionPermissionsResult,
  ExtensionRecord,
  ExtensionRunList,
  ExtensionRuntimeList,
  ExtensionScript,
  ExtensionStatusResult,
  ExtensionUnloadResult,
  FileEntry,
  FileListResult,
  IntegrationInfo,
  MailLogList,
  MarketplaceExtension,
  MediaItem,
  MemoryEntry,
  MemoryList,
  NotFoundList,
  Order,
  OrderList,
  PaymentProviderInfo,
  PluginCompatibilityReport,
  PluginSummary,
  PostSeo,
  PreviewToken,
  Product,
  ProductList,
  ProviderInfo,
  RedirectList,
  RedirectRecord,
  RevisionDiff,
  SelectedPayment,
  CwvBudgets,
  SeoAnalysis,
  SeoAuditRun,
  SeoImportResult,
  SeoJsonLdPreview,
  SeoSitemapFile,
  Settings,
  Site,
  ShippingMethod,
  ShippingZone,
  TaxRate,
  TaxSettings,
  Term,
  TermList,
  ThemeDetail,
  ThemeRecord,
  ThemeVariation,
  User,
  UserProfile,
  WebhookLogList,
  WooAttribute,
  WooCustomer,
  WooImportReport,
  WooProduct,
  WooVariation,
  ZoneWithMethods,
  Coupon,
} from "./types";
import type { BuilderPalette, ExtensionWidget, PreviewResult } from "./builder";

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

interface Envelope<T> {
  ok: boolean;
  data?: T;
  error?: { code?: string; message?: string };
}

// P12 multisite: the admin acts on one site at a time. The selected
// site id persists in localStorage (`wl_site`) and is sent as the
// `X-Wordless-Site` header on every API call, so the admin context
// follows the selected site everywhere. Null = main site (id 1);
// the header is omitted then, and the server defaults to site 1.
const SITE_STORAGE_KEY = "wl_site";

let currentSiteId: number | null = (() => {
  try {
    const raw = window.localStorage.getItem(SITE_STORAGE_KEY);
    const n = raw === null ? NaN : Number(raw);
    return Number.isInteger(n) && n >= 1 ? n : null;
  } catch {
    return null;
  }
})();

export function getCurrentSiteId(): number | null {
  return currentSiteId;
}

export function setCurrentSite(id: number | null): void {
  currentSiteId = id;
  try {
    if (id === null) window.localStorage.removeItem(SITE_STORAGE_KEY);
    else window.localStorage.setItem(SITE_STORAGE_KEY, String(id));
  } catch {
    // Private mode etc. — the in-memory value still applies this session.
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: "same-origin",
    headers: {
      "Content-Type": "application/json",
      ...(currentSiteId !== null ? { "X-Wordless-Site": String(currentSiteId) } : {}),
      ...(init?.headers ?? {}),
    },
    ...init,
  });
  let body: Envelope<T>;
  try {
    body = (await res.json()) as Envelope<T>;
  } catch {
    throw new ApiError(res.status, `Request failed (${res.status}).`);
  }
  if (!res.ok || !body.ok) {
    throw new ApiError(res.status, body.error?.message ?? `Request failed (${res.status}).`);
  }
  return body.data as T;
}

export const api = {
  get<T>(path: string): Promise<T> {
    return request<T>(path);
  },
  post<T>(path: string, data?: unknown): Promise<T> {
    return request<T>(path, { method: "POST", body: data === undefined ? undefined : JSON.stringify(data) });
  },
  put<T>(path: string, data: unknown): Promise<T> {
    return request<T>(path, { method: "PUT", body: JSON.stringify(data) });
  },
  del<T>(path: string): Promise<T> {
    return request<T>(path, { method: "DELETE" });
  },

  me(): Promise<{ user: User; mustChangePassword?: boolean }> {
    return request<{ user: User; mustChangePassword?: boolean }>("/api/auth/me");
  },
  login(login: string, password: string, totp?: string): Promise<{ user: User; mustChangePassword?: boolean }> {
    return request<{ user: User; mustChangePassword?: boolean }>("/api/auth/login", {
      method: "POST",
      body: JSON.stringify({ login, password, ...(totp !== undefined ? { totp } : {}) }),
    });
  },
  logout(): Promise<{ loggedOut: boolean }> {
    return request<{ loggedOut: boolean }>("/api/auth/logout", { method: "POST" });  },

  // ------------------------------------------------------- P12 sites
  listSites(): Promise<Site[]> {
    return request<Site[]>("/api/sites");
  },
  currentSite(): Promise<Site> {
    return request<Site>("/api/sites/current");
  },
  createSite(slug: string, title: string): Promise<Site> {
    return request<Site>("/api/sites", { method: "POST", body: JSON.stringify({ slug, title }) });
  },
  updateSite(id: number, patch: { title?: string; status?: string }): Promise<Site> {
    return request<Site>(`/api/sites/${id}`, { method: "PUT", body: JSON.stringify(patch) });
  },
  deleteSite(id: number): Promise<{ deleted: boolean; deletedPosts: number }> {
    return request<{ deleted: boolean; deletedPosts: number }>(`/api/sites/${id}`, { method: "DELETE" });
  },
  getSiteOption<T = unknown>(id: number, key: string): Promise<{ key: string; value: T }> {
    return request<{ key: string; value: T }>(`/api/sites/${id}/options/${encodeURIComponent(key)}`);
  },
  setSiteOption(id: number, key: string, value: unknown): Promise<{ key: string; value: unknown }> {
    return request<{ key: string; value: unknown }>(`/api/sites/${id}/options/${encodeURIComponent(key)}`, {
      method: "PUT",
      body: JSON.stringify({ value }),
    });
  },
  deleteSiteOption(id: number, key: string): Promise<{ key: string; deleted: boolean }> {
    return request<{ key: string; deleted: boolean }>(`/api/sites/${id}/options/${encodeURIComponent(key)}`, {
      method: "DELETE",
    });
  },

  /** P4: the signed-in user changes their own password. The API clears
   *  the forced-change flag, revokes every session and clears the
   *  session cookie — the client must sign in again afterwards. */
  changePassword(currentPassword: string, newPassword: string): Promise<{ changed: boolean }> {
    return request<{ changed: boolean }>("/api/auth/change-password", {
      method: "POST",
      body: JSON.stringify({ currentPassword, newPassword }),
    });
  },

  /** P4: full profile for one user (own profile, or any user with
   *  edit_users). Carries bio, avatarId, theme, mustChangePassword. */
  getUserProfile(id: number): Promise<UserProfile> {
    return request<UserProfile>(`/api/users/${id}`);
  },
  /** P4: edit a profile. Only the fields present are changed; avatarId
   *  null clears the avatar. A user may edit their own profile;
   *  editing another user's needs edit_users. */
  updateUserProfile(
    id: number,
    patch: { displayName?: string; email?: string; bio?: string; avatarId?: number | null; theme?: string },
  ): Promise<UserProfile> {
    return request<UserProfile>(`/api/users/${id}/profile`, { method: "PUT", body: JSON.stringify(patch) });
  },

  /** P4: terms. listTerms returns the paginated TermList; setPostTerms
   *  replaces a post's terms in one taxonomy (termIds are
   *  termTaxonomyIds, as returned by listTerms). */
  listTerms(taxonomy: string, page = 1, perPage = 100): Promise<TermList> {
    return request<TermList>(`/api/terms?taxonomy=${encodeURIComponent(taxonomy)}&page=${page}&per_page=${perPage}`);
  },
  createTerm(name: string, taxonomy: string, slug?: string): Promise<Term> {
    return request<Term>("/api/terms", {
      method: "POST",
      body: JSON.stringify({ name, taxonomy, ...(slug !== undefined ? { slug } : {}) }),
    });
  },
  getPostTerms(postId: number, taxonomy?: string): Promise<Term[]> {
    const q = taxonomy !== undefined ? `?taxonomy=${encodeURIComponent(taxonomy)}` : "";
    return request<Term[]>(`/api/posts/${postId}/terms${q}`);
  },
  setPostTerms(postId: number, taxonomy: string, termIds: number[]): Promise<Term[]> {
    return request<Term[]>(`/api/posts/${postId}/terms`, {
      method: "POST",
      body: JSON.stringify({ taxonomy, termIds }),
    });
  },

  /** P4: one media item by id (avatar / featured-image display). */
  getMedia(id: number): Promise<MediaItem> {
    return request<MediaItem>(`/api/media/${id}`);
  },

  /** P4: line diff between two revisions. */
  getRevisionDiff(fromId: number, toId: number): Promise<RevisionDiff> {
    return request<RevisionDiff>(`/api/revisions/${fromId}/diff/${toId}`);
  },

  getSettings(): Promise<Settings> {
    return request<Settings>("/api/settings");
  },
  updateSettings(patch: Partial<Settings>): Promise<Settings> {
    return request<Settings>("/api/settings", { method: "PUT", body: JSON.stringify(patch) });
  },

  /** Multipart media upload (no JSON content-type; the browser sets it). */
  async uploadMedia(file: File, opts?: { title?: string; alt?: string }): Promise<MediaItem> {
    const form = new FormData();
    form.append("file", file);
    if (opts?.title) form.append("title", opts.title);
    if (opts?.alt) form.append("alt", opts.alt);
    const res = await fetch("/api/media", { method: "POST", credentials: "same-origin", body: form });
    const body = (await res.json()) as Envelope<MediaItem>;
    if (!res.ok || !body.ok) {
      throw new ApiError(res.status, body.error?.message ?? `Upload failed (${res.status}).`);
    }
    return body.data as MediaItem;
  },

  /** Conversion jobs (Milestone 4): list, one job, start a fixture job, upload a PHP file. */
  listConversionJobs(): Promise<ConversionJobList> {
    return request<ConversionJobList>("/api/conversion/jobs");
  },
  getConversionJob(id: string): Promise<ConversionJob> {
    return request<ConversionJob>(`/api/conversion/jobs/${encodeURIComponent(id)}`);
  },
  startFixtureJob(fixture: string): Promise<ConversionJob> {
    return request<ConversionJob>("/api/conversion/jobs", {
      method: "POST",
      body: JSON.stringify({ fixture }),
    });
  },
  async uploadPhpFile(file: File): Promise<ConversionJob> {
    const form = new FormData();
    form.append("file", file);
    const res = await fetch("/api/conversion/jobs", { method: "POST", credentials: "same-origin", body: form });
    const body = (await res.json()) as Envelope<ConversionJob>;
    if (!res.ok || !body.ok) {
      throw new ApiError(res.status, body.error?.message ?? `Conversion failed (${res.status}).`);
    }
    return body.data as ConversionJob;
  },
  listExtensions(): Promise<ExtensionRecord[]> {
    return request<ExtensionRecord[]>("/api/extensions");
  },

  /** P6: the sandboxed extension runtime (GET /api/extensions/runtime).
   *  NOTE: GET /api/extensions itself stays the P3 route (an array of
   *  ExtensionRecord) — the router returns the first matching
   *  registration, so the runtime list lives at /runtime.
   *  route returns `{ loaded, installed }` — loaded instances (with
   *  runtime status + granted permissions) plus installed extensions
   *  that are not loaded yet. `listExtensions()` above is untouched:
   *  it still returns the P3 array from GET /api/extensions. */
  listExtensionRuntime(): Promise<ExtensionRuntimeList> {
    return request<ExtensionRuntimeList>("/api/extensions/runtime");
  },
  /** P6: one loaded instance (GET /api/extensions/:slug). 404 when the
   *  extension is not loaded. */
  getExtension(slug: string): Promise<ExtensionDetail> {
    return request<ExtensionDetail>(`/api/extensions/${encodeURIComponent(slug)}`);
  },
  /** P6: load an extension into the sandbox (POST
   *  /api/extensions/:slug/load) with NO body — the admin never knows
   *  a devUrl; in production the resolver uses the published service
   *  binding or dispatch namespace. A refusal is HTTP 200 with
   *  `loaded: false` plus `reasons` and `blockedFiles`. */
  loadExtension(slug: string): Promise<ExtensionLoadResult> {
    return request<ExtensionLoadResult>(`/api/extensions/${encodeURIComponent(slug)}/load`, {
      method: "POST",
    });
  },
  /** P6: unload an extension (POST /api/extensions/:slug/unload).
   *  Its code stays in R2 and its marketplace record stays installed;
   *  loading again re-runs the gate. */
  unloadExtension(slug: string): Promise<ExtensionUnloadResult> {
    return request<ExtensionUnloadResult>(`/api/extensions/${encodeURIComponent(slug)}/unload`, {
      method: "POST",
    });
  },
  /** P6: the kill switch (POST /api/extensions/:slug/disable and
   *  .../enable). Disabling stops the extension's hooks within one
   *  request; it is distinct from Deactivate (marketplace status). */
  setExtensionStatus(slug: string, status: "disable" | "enable"): Promise<ExtensionStatusResult> {
    return request<ExtensionStatusResult>(`/api/extensions/${encodeURIComponent(slug)}/${status}`, {
      method: "POST",
    });
  },

  /** P11: the builder palette (GET /api/builder/palette) — the full
   *  element and icon libraries plus extension widgets that are
   *  installed AND loaded; the server is the single source of truth. */
  getBuilderPalette(): Promise<BuilderPalette> {
    return request<BuilderPalette>("/api/builder/palette");
  },
  /** P11: render a layout through the P3 renderer (POST
   *  /api/builder/preview). `unsupported` lists widget types that
   *  will not render on the public site. */
  previewBuilderLayout(data: unknown, device: string): Promise<PreviewResult> {
    return request<PreviewResult>("/api/builder/preview", {
      method: "POST",
      body: JSON.stringify({ data, device }),
    });
  },
  /** P11: list registered extension widgets (GET /api/builder/widgets). */
  listBuilderWidgets(): Promise<ExtensionWidget[]> {
    return request<ExtensionWidget[]>("/api/builder/widgets");
  },
  /** P11: read a post's meta map (GET /api/posts/:id/meta) — the
   *  builder reads `_elementor_data` from it. */
  getPostMeta(id: number): Promise<Record<string, string>> {
    return request<Record<string, string>>(`/api/posts/${id}/meta`);
  },
  /** P11: set postmeta keys (PUT /api/posts/:id/meta) — the builder
   *  saves the layout as `_elementor_data`. */
  putPostMeta(id: number, meta: Record<string, string | number | boolean | null>): Promise<Record<string, string>> {
    return request<Record<string, string>>(`/api/posts/${id}/meta`, {
      method: "PUT",
      body: JSON.stringify(meta),
    });
  },
  /** P6: the capability list for one loaded extension (GET
   *  /api/extensions/:slug/permissions). `available` comes from the
   *  server — never hardcode the capability list in the UI. */
  getExtensionPermissions(slug: string): Promise<ExtensionPermissions> {
    return request<ExtensionPermissions>(`/api/extensions/${encodeURIComponent(slug)}/permissions`);
  },
  /** P6: save the granted capabilities (POST
   *  /api/extensions/:slug/permissions, body `{ permissions: string[] }`).
   *  Only send ids from the `available` list — the server rejects
   *  unknown capability ids. */
  setExtensionPermissions(slug: string, permissions: string[]): Promise<ExtensionPermissionsResult> {
    return request<ExtensionPermissionsResult>(`/api/extensions/${encodeURIComponent(slug)}/permissions`, {
      method: "POST",
      body: JSON.stringify({ permissions }),
    });
  },
  /** P6: recent log lines, most recent first (GET
   *  /api/extensions/:slug/logs?limit=50). */
  listExtensionLogs(slug: string, limit = 50): Promise<ExtensionLogList> {
    return request<ExtensionLogList>(`/api/extensions/${encodeURIComponent(slug)}/logs?limit=${limit}`);
  },
  /** P6: recent hook runs, most recent first (GET
   *  /api/extensions/:slug/runs?limit=50). */
  listExtensionRuns(slug: string, limit = 50): Promise<ExtensionRunList> {
    return request<ExtensionRunList>(`/api/extensions/${encodeURIComponent(slug)}/runs?limit=${limit}`);
  },
  /** P6: the generated extension Worker project for one loaded
   *  instance (GET /api/extensions/:slug/script). Not rendered by the
   *  Plugins page — here so a future view can fetch it. */
  getExtensionScript(slug: string): Promise<ExtensionScript> {
    return request<ExtensionScript>(`/api/extensions/${encodeURIComponent(slug)}/script`);
  },

  /** P4: the dedicated Plugins page list (GET /api/plugins) — installed
   *  extensions with active state + compatibility verdict summary.
   *  NOTE: per-plugin settings (GET/PUT /api/plugins/:slug/settings in
   *  doc §0.1 A) do NOT exist server-side — the worker deliberately did
   *  not add them (no settings store exists); the compat report's
   *  carried-over setting defaults are shown instead. Do not invent
   *  a settings endpoint here. */
  listPlugins(): Promise<PluginSummary[]> {
    return request<PluginSummary[]>("/api/plugins");
  },
  setPluginStatus(slug: string, status: "activate" | "deactivate"): Promise<ExtensionRecord> {
    return request<ExtensionRecord>(`/api/plugins/${encodeURIComponent(slug)}/${status}`, { method: "POST" });
  },

  /** Themes (Milestone 5). */
  listThemes(): Promise<ThemeRecord[]> {
    return request<ThemeRecord[]>("/api/themes");
  },
  getTheme(slug: string): Promise<ThemeDetail> {
    return request<ThemeDetail>(`/api/themes/${encodeURIComponent(slug)}`);
  },
  getActiveTheme(): Promise<ThemeRecord> {
    return request<ThemeRecord>("/api/themes/active");
  },
  async importTheme(file: File, kind: "block" | "classic"): Promise<ThemeRecord> {
    const form = new FormData();
    form.append("file", file);
    const path = kind === "classic" ? "/api/themes/import-classic" : "/api/themes/import";
    const res = await fetch(path, { method: "POST", credentials: "same-origin", body: form });
    const body = (await res.json()) as Envelope<{ theme: ThemeRecord }>;
    if (!res.ok || !body.ok) {
      throw new ApiError(res.status, body.error?.message ?? `Theme import failed (${res.status}).`);
    }
    return (body.data as { theme: ThemeRecord }).theme;
  },
  activateTheme(slug: string): Promise<ThemeRecord> {
    return request<ThemeRecord>(`/api/themes/${encodeURIComponent(slug)}/activate`, { method: "POST" });
  },
  deleteTheme(slug: string): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/themes/${encodeURIComponent(slug)}/delete`, { method: "POST" });
  },
  getVariation(slug: string): Promise<ThemeVariation | null> {
    return request<ThemeVariation | null>(`/api/themes/${encodeURIComponent(slug)}/variation`);
  },
  saveVariation(slug: string, name: string, settings: Record<string, unknown>): Promise<ThemeVariation> {
    return request<ThemeVariation>(`/api/themes/${encodeURIComponent(slug)}/variation`, {
      method: "PUT",
      body: JSON.stringify({ name, settings }),
    });
  },
  deleteVariation(slug: string): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/themes/${encodeURIComponent(slug)}/variation`, { method: "DELETE" });
  },
  createPreview(slug: string, settings: Record<string, unknown>): Promise<PreviewToken> {
    return request<PreviewToken>("/api/themes/preview", {
      method: "POST",
      body: JSON.stringify({ slug, settings }),
    });
  },

  /** AI command centre (Milestone 6). */
  getAiProviders(): Promise<AiProvidersResponse> {
    return request<AiProvidersResponse>("/api/ai/providers");
  },
  selectAiProvider(provider: string): Promise<AiProvidersResponse> {
    return request<AiProvidersResponse>("/api/ai/providers/select", {
      method: "POST",
      body: JSON.stringify({ provider }),
    });
  },
  testAiProvider(id: string): Promise<{ ok: boolean; message: string }> {
    return request<{ ok: boolean; message: string }>(`/api/ai/providers/${encodeURIComponent(id)}/test`, {
      method: "POST",
    });
  },
  getAiMode(): Promise<AiModeResponse> {
    return request<AiModeResponse>("/api/ai/mode");
  },
  setAiMode(mode: AiMode): Promise<AiModeResponse> {
    return request<AiModeResponse>("/api/ai/mode", { method: "PUT", body: JSON.stringify({ mode }) });
  },
  listAiAgents(): Promise<AgentInfo[]> {
    return request<AgentInfo[]>("/api/ai/agents");
  },
  createAiTask(input: { agent: string; input: string; mode?: AiMode }): Promise<AiTask> {
    return request<AiTask>("/api/ai/tasks", { method: "POST", body: JSON.stringify(input) });
  },
  listAiTasks(): Promise<AiTaskList> {
    return request<AiTaskList>("/api/ai/tasks");
  },
  listAiApprovals(status?: string): Promise<AiApprovalList> {
    const q = status !== undefined ? `?status=${encodeURIComponent(status)}` : "";
    return request<AiApprovalList>(`/api/ai/approvals${q}`);
  },
  approveAiApproval(id: string): Promise<AiApproval> {
    return request<AiApproval>(`/api/ai/approvals/${encodeURIComponent(id)}/approve`, { method: "POST" });
  },
  rejectAiApproval(id: string, reason?: string): Promise<AiApproval> {
    return request<AiApproval>(`/api/ai/approvals/${encodeURIComponent(id)}/reject`, {
      method: "POST",
      body: JSON.stringify({ reason: reason ?? "" }),
    });
  },
  getAiMemory(): Promise<MemoryList> {
    return request<MemoryList>("/api/ai/memory");
  },
  addAiMemoryNote(text: string): Promise<MemoryEntry> {
    return request<MemoryEntry>("/api/ai/memory/notes", { method: "POST", body: JSON.stringify({ text }) });
  },
  deleteAiMemoryNote(id: number): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/ai/memory/notes/${id}`, { method: "DELETE" });
  },
  rebuildAiMemory(): Promise<{ facts: number; notes: number }> {
    return request<{ facts: number; notes: number }>("/api/ai/memory/rebuild", { method: "POST" });
  },

  /** SEO (Milestone 7). */
  getPostSeo(id: number): Promise<PostSeo> {
    return request<PostSeo>(`/api/posts/${id}/seo`);
  },
  updatePostSeo(id: number, patch: Partial<PostSeo>): Promise<PostSeo> {
    return request<PostSeo>(`/api/posts/${id}/seo`, { method: "PUT", body: JSON.stringify(patch) });
  },
  getPostSeoAnalysis(id: number): Promise<SeoAnalysis> {
    return request<SeoAnalysis>(`/api/posts/${id}/seo/analysis`);
  },

  getPostJsonLd(id: number): Promise<SeoJsonLdPreview> {
    return request<SeoJsonLdPreview>(`/api/posts/${id}/seo/json-ld`);
  },

  getCwvBudgets(): Promise<CwvBudgets> {
    return request<CwvBudgets>("/api/seo/cwv-budgets");
  },

  updateCwvBudgets(budgets: CwvBudgets): Promise<CwvBudgets> {
    return request<CwvBudgets>("/api/seo/cwv-budgets", { method: "PUT", body: JSON.stringify(budgets) });
  },

  listAuditRuns(limit = 20): Promise<SeoAuditRun[]> {
    return request<SeoAuditRun[]>(`/api/seo/audit-runs?limit=${limit}`);
  },

  recordAuditRun(run: {
    url: string;
    tool?: string;
    seoScore?: number | null;
    accessibilityScore?: number | null;
    performanceScore?: number | null;
    bestPracticesScore?: number | null;
    lcpMs?: number | null;
    inpMs?: number | null;
    clsMilli?: number | null;
    notes?: string;
  }): Promise<SeoAuditRun> {
    return request<SeoAuditRun>("/api/seo/audit-runs", { method: "POST", body: JSON.stringify(run) });
  },

  getSitemapOverview(): Promise<{ files: SeoSitemapFile[] }> {
    return request<{ files: SeoSitemapFile[] }>("/api/seo/sitemap");
  },
  getRobots(): Promise<{ robots: string }> {
    return request<{ robots: string }>("/api/seo/robots");
  },
  updateRobots(robots: string): Promise<{ robots: string }> {
    return request<{ robots: string }>("/api/seo/robots", { method: "PUT", body: JSON.stringify({ robots }) });
  },
  listTemplates(): Promise<ContentTemplate[]> {
    return request<ContentTemplate[]>("/api/seo/templates");
  },
  createFromTemplate(slug: string, title?: string): Promise<{ id: number }> {
    return request<{ id: number }>(`/api/seo/templates/${encodeURIComponent(slug)}/create`, {
      method: "POST",
      body: JSON.stringify({ title: title ?? "" }),
    });
  },
  getCalendar(days = 30): Promise<ContentCalendar> {
    return request<ContentCalendar>(`/api/seo/calendar?days=${days}`);
  },
  importSeo(posts: unknown[]): Promise<SeoImportResult> {
    return request<SeoImportResult>("/api/seo/import", { method: "POST", body: JSON.stringify({ posts }) });
  },
  listRedirects(page = 1): Promise<RedirectList> {
    return request<RedirectList>(`/api/seo/redirects?page=${page}`);
  },
  createRedirect(oldPath: string, newPath: string): Promise<RedirectRecord> {
    return request<RedirectRecord>("/api/seo/redirects", { method: "POST", body: JSON.stringify({ oldPath, newPath }) });
  },
  updateRedirect(id: number, newPath: string): Promise<RedirectRecord> {
    return request<RedirectRecord>(`/api/seo/redirects/${id}`, { method: "PUT", body: JSON.stringify({ newPath }) });
  },
  deleteRedirect(id: number): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/seo/redirects/${id}`, { method: "DELETE" });
  },
  listNotFound(page = 1): Promise<NotFoundList> {
    return request<NotFoundList>(`/api/seo/404s?page=${page}`);
  },
  deleteNotFound(path: string): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/seo/404s?path=${encodeURIComponent(path)}`, { method: "DELETE" });
  },

  /** Commerce + integrations (Milestone 8). */
  listProducts(page = 1): Promise<ProductList> {
    return request<ProductList>(`/api/commerce/products?status=all&page=${page}`);
  },
  createProduct(input: {
    name: string;
    slug?: string;
    description?: string;
    priceMinor: number;
    currency?: string;
    stock?: number;
    status?: "draft" | "published";
    featuredImageId?: number;
  }): Promise<Product> {
    return request<Product>("/api/commerce/products", { method: "POST", body: JSON.stringify(input) });
  },
  updateProduct(id: number, patch: Partial<Product>): Promise<Product> {
    return request<Product>(`/api/commerce/products/${id}`, { method: "PUT", body: JSON.stringify(patch) });
  },
  deleteProduct(id: number): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/commerce/products/${id}`, { method: "DELETE" });
  },
  /** P8 WooCommerce subset: the Woo delete also removes the product's attributes, variations and imported terms. */
  getWooProduct(id: number): Promise<WooProduct> {
    return request<WooProduct>(`/api/woo/products/${id}`);
  },
  createWooProduct(input: Record<string, unknown>): Promise<WooProduct> {
    return request<WooProduct>("/api/woo/products", { method: "POST", body: JSON.stringify(input) });
  },
  updateWooProduct(id: number, patch: Record<string, unknown>): Promise<WooProduct> {
    return request<WooProduct>(`/api/woo/products/${id}`, { method: "PUT", body: JSON.stringify(patch) });
  },
  deleteWooProduct(id: number): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/woo/products/${id}`, { method: "DELETE" });
  },
  listAttributes(productId: number): Promise<WooAttribute[]> {
    return request<WooAttribute[]>(`/api/woo/products/${productId}/attributes`);
  },
  createAttribute(productId: number, input: Record<string, unknown>): Promise<WooAttribute> {
    return request<WooAttribute>(`/api/woo/products/${productId}/attributes`, { method: "POST", body: JSON.stringify(input) });
  },
  updateAttribute(productId: number, attrId: number, input: Record<string, unknown>): Promise<WooAttribute> {
    return request<WooAttribute>(`/api/woo/products/${productId}/attributes/${attrId}`, { method: "PUT", body: JSON.stringify(input) });
  },
  deleteAttribute(productId: number, attrId: number): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/woo/products/${productId}/attributes/${attrId}`, { method: "DELETE" });
  },
  listVariations(productId: number): Promise<WooVariation[]> {
    return request<WooVariation[]>(`/api/woo/products/${productId}/variations`);
  },
  createVariation(productId: number, input: Record<string, unknown>): Promise<WooVariation> {
    return request<WooVariation>(`/api/woo/products/${productId}/variations`, { method: "POST", body: JSON.stringify(input) });
  },
  updateVariation(productId: number, varId: number, input: Record<string, unknown>): Promise<WooVariation> {
    return request<WooVariation>(`/api/woo/products/${productId}/variations/${varId}`, { method: "PUT", body: JSON.stringify(input) });
  },
  deleteProductVariation(productId: number, varId: number): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/woo/products/${productId}/variations/${varId}`, { method: "DELETE" });
  },
  listCoupons(): Promise<Coupon[]> {
    return request<Coupon[]>("/api/woo/coupons");
  },
  createCoupon(input: Record<string, unknown>): Promise<Coupon> {
    return request<Coupon>("/api/woo/coupons", { method: "POST", body: JSON.stringify(input) });
  },
  updateCoupon(id: number, input: Record<string, unknown>): Promise<Coupon> {
    return request<Coupon>(`/api/woo/coupons/${id}`, { method: "PUT", body: JSON.stringify(input) });
  },
  deleteCoupon(id: number): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/woo/coupons/${id}`, { method: "DELETE" });
  },
  listTaxRates(): Promise<TaxRate[]> {
    return request<TaxRate[]>("/api/woo/tax-rates");
  },
  createTaxRate(input: Record<string, unknown>): Promise<TaxRate> {
    return request<TaxRate>("/api/woo/tax-rates", { method: "POST", body: JSON.stringify(input) });
  },
  updateTaxRate(id: number, input: Record<string, unknown>): Promise<TaxRate> {
    return request<TaxRate>(`/api/woo/tax-rates/${id}`, { method: "PUT", body: JSON.stringify(input) });
  },
  deleteTaxRate(id: number): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/woo/tax-rates/${id}`, { method: "DELETE" });
  },
  getTaxSettings(): Promise<TaxSettings> {
    return request<TaxSettings>("/api/woo/tax-settings");
  },
  setTaxSettings(input: TaxSettings): Promise<TaxSettings> {
    return request<TaxSettings>("/api/woo/tax-settings", { method: "PUT", body: JSON.stringify(input) });
  },
  listShippingZones(): Promise<ZoneWithMethods[]> {
    return request<ZoneWithMethods[]>("/api/woo/shipping-zones");
  },
  createShippingZone(input: Record<string, unknown>): Promise<ShippingZone> {
    return request<ShippingZone>("/api/woo/shipping-zones", { method: "POST", body: JSON.stringify(input) });
  },
  updateShippingZone(id: number, input: Record<string, unknown>): Promise<ShippingZone> {
    return request<ShippingZone>(`/api/woo/shipping-zones/${id}`, { method: "PUT", body: JSON.stringify(input) });
  },
  deleteShippingZone(id: number): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/woo/shipping-zones/${id}`, { method: "DELETE" });
  },
  createShippingMethod(zoneId: number, input: Record<string, unknown>): Promise<ShippingMethod> {
    return request<ShippingMethod>(`/api/woo/shipping-zones/${zoneId}/methods`, { method: "POST", body: JSON.stringify(input) });
  },
  updateShippingMethod(id: number, zoneId: number, input: Record<string, unknown>): Promise<ShippingMethod> {
    return request<ShippingMethod>(`/api/woo/shipping-methods/${id}`, { method: "PUT", body: JSON.stringify({ ...input, zoneId }) });
  },
  deleteShippingMethod(id: number, zoneId: number): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>(`/api/woo/shipping-methods/${id}?zoneId=${zoneId}`, { method: "DELETE" });
  },
  listCustomers(page = 1): Promise<{ customers: WooCustomer[]; total: number; page: number; perPage: number }> {
    return request<{ customers: WooCustomer[]; total: number; page: number; perPage: number }>(`/api/woo/customers?page=${page}`);
  },
  importWoo(kind: "products" | "orders" | "customers", csv: string): Promise<WooImportReport> {
    return request<WooImportReport>("/api/woo/import", { method: "POST", body: JSON.stringify({ kind, csv }) });
  },
  listOrders(page = 1): Promise<OrderList> {
    return request<OrderList>(`/api/commerce/orders?page=${page}`);
  },
  getOrder(id: string): Promise<Order> {
    return request<Order>(`/api/commerce/orders/${encodeURIComponent(id)}`);
  },
  transitionOrder(id: string, status: "paid" | "fulfilled" | "cancelled"): Promise<Order> {
    return request<Order>(`/api/commerce/orders/${encodeURIComponent(id)}/transition`, {
      method: "POST",
      body: JSON.stringify({ status }),
    });
  },
  listPaymentProviders(): Promise<PaymentProviderInfo[]> {
    return request<PaymentProviderInfo[]>("/api/commerce/payments/providers");
  },
  getSelectedPayment(): Promise<SelectedPayment> {
    return request<SelectedPayment>("/api/commerce/payments/selected");
  },
  selectPayment(provider: string, mode: "test" | "live"): Promise<SelectedPayment> {
    return request<SelectedPayment>("/api/commerce/payments/select", {
      method: "POST",
      body: JSON.stringify({ provider, mode }),
    });
  },
  listWebhookLog(page = 1): Promise<WebhookLogList> {
    return request<WebhookLogList>(`/api/commerce/webhooks/log?page=${page}`);
  },
  listIntegrations(): Promise<IntegrationInfo[]> {
    return request<IntegrationInfo[]>("/api/integrations");
  },
  getIntegrationConfig(id: string): Promise<Record<string, unknown>> {
    return request<Record<string, unknown>>(`/api/integrations/${encodeURIComponent(id)}/config`);
  },
  updateIntegrationConfig(id: string, config: Record<string, unknown>): Promise<Record<string, unknown>> {
    return request<Record<string, unknown>>(`/api/integrations/${encodeURIComponent(id)}/config`, {
      method: "PUT",
      body: JSON.stringify(config),
    });
  },
  listMailLog(page = 1): Promise<MailLogList> {
    return request<MailLogList>(`/api/integrations/email/log?page=${page}`);
  },
  getAnalyticsStatus(): Promise<AnalyticsStatus> {
    return request<AnalyticsStatus>("/api/integrations/analytics/status");
  },
  getCloudflareBindings(): Promise<BindingStatus[]> {
    return request<BindingStatus[]>("/api/integrations/cloudflare");
  },
  listMarketplace(): Promise<MarketplaceExtension[]> {
    return request<MarketplaceExtension[]>("/api/marketplace");
  },
  getMarketplaceReport(slug: string): Promise<{ slug: string; report: PluginCompatibilityReport | null }> {
    return request<{ slug: string; report: PluginCompatibilityReport | null }>(
      `/api/marketplace/${encodeURIComponent(slug)}/report`,
    );
  },
  async installMarketplaceZip(file: File): Promise<MarketplaceExtension> {
    const form = new FormData();
    form.append("file", file);
    const res = await fetch("/api/marketplace/install", { method: "POST", credentials: "same-origin", body: form });
    const body = (await res.json()) as Envelope<MarketplaceExtension>;
    if (!res.ok || !body.ok) {
      throw new ApiError(res.status, body.error?.message ?? `Install failed (${res.status}).`);
    }
    return body.data as MarketplaceExtension;
  },
  setMarketplaceStatus(slug: string, status: "activate" | "deactivate"): Promise<MarketplaceExtension> {
    return request<MarketplaceExtension>(`/api/marketplace/${encodeURIComponent(slug)}/${status}`, { method: "POST" });
  },
  /* Milestone 11 — file manager. */
  listFiles(prefix = ""): Promise<FileListResult> {
    const q = prefix === "" ? "" : `?prefix=${encodeURIComponent(prefix)}`;
    return request<FileListResult>(`/api/files${q}`);
  },
  async uploadFile(file: File, prefix = ""): Promise<FileEntry> {
    const form = new FormData();
    form.append("file", file);
    form.append("prefix", prefix);
    const res = await fetch("/api/files/upload", { method: "POST", credentials: "same-origin", body: form });
    const body = (await res.json()) as Envelope<FileEntry>;
    if (!res.ok || !body.ok) {
      throw new ApiError(res.status, body.error?.message ?? `Upload failed (${res.status}).`);
    }
    return body.data as FileEntry;
  },
  renameFile(key: string, newName: string): Promise<FileEntry> {
    return request<FileEntry>("/api/files/rename", { method: "POST", body: JSON.stringify({ key, newName }) });
  },
  deleteFile(key: string): Promise<{ deleted: boolean }> {
    return request<{ deleted: boolean }>("/api/files/delete", { method: "POST", body: JSON.stringify({ key }) });
  },
};
