/** Types for the Wordless API responses used by the admin SPA. */

export interface User {
  id: number;
  login: string;
  email: string;
  displayName: string;
  role: Role;
  registered: string;
}

export type Role = "administrator" | "editor" | "author" | "contributor" | "subscriber";

export interface Post {
  id: number;
  authorId: number;
  date: string;
  modified: string;
  title: string;
  content: string;
  excerpt: string;
  status: string;
  slug: string;
  type: "post" | "page" | "attachment" | "revision";
  commentStatus: string;
  commentCount: number;
  /** Per-post SEO fields (Milestone 7) — present on the detail payload. */
  seo?: PostSeo;
  /** Pending scheduled publish time (ISO-8601 UTC), attached by the API
   *  when a schedule exists; absent when nothing is scheduled (P4). */
  publishAt?: string;
  /** True when the post carries Elementor data (Milestone 11) — present on the detail payload. */
  hasElementorData?: boolean;
}

/** SEO (Milestone 7). */
export interface PostSeo {
  title: string;
  description: string;
  focusKeyword: string;
  canonical: string;
  noindex: boolean;
  ogTitle: string;
  ogDescription: string;
  ogImageId: number;
  twitterCard: "summary" | "summary_large_image";
}

export interface SeoCheck {
  id: string;
  label: string;
  status: "pass" | "warn" | "fail";
  detail: string;
}

/** P10: one recorded audit run (Lighthouse / structured-data check). */
export interface SeoAuditRun {
  id: number;
  url: string;
  tool: string;
  seoScore: number | null;
  accessibilityScore: number | null;
  performanceScore: number | null;
  bestPracticesScore: number | null;
  lcpMs: number | null;
  inpMs: number | null;
  clsMilli: number | null;
  notes: string;
  createdAt: string;
}

/** P10: Core Web Vitals budgets (web.dev "good" thresholds by default). */
export interface CwvBudgets {
  lcpMs: number;
  inpMs: number;
  cls: number;
}

/** P10: structured-data preview for one post. */
export interface SeoJsonLdPreview {
  jsonLd: unknown;
  valid: boolean;
  problems: string[];
}

/** P10: sitemap inventory entry. */
export interface SeoSitemapFile {
  loc: string;
  entries: number;
}

export interface SeoAnalysis {
  postId: number;
  score: number;
  checks: SeoCheck[];
  wordCount: number;
  readingEase: number | null;
  internalLinks: number;
  externalLinks: number;
  images: number;
  imagesMissingAlt: number;
}

export interface RedirectRecord {
  id: number;
  oldPath: string;
  newPath: string;
  postId: number;
  createdAt: string;
}

export interface RedirectList {
  redirects: RedirectRecord[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface NotFoundEntry {
  path: string;
  count: number;
  referrer: string;
  firstSeen: string;
  lastSeen: string;
}

export interface NotFoundList {
  entries: NotFoundEntry[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface ContentTemplate {
  slug: string;
  name: string;
  description: string;
  title: string;
  content: string;
}

export interface CalendarDay {
  date: string;
  posts: Array<{ id: number; title: string; status: string; type: string; slug: string }>;
}

export interface ContentCalendar {
  from: string;
  to: string;
  days: CalendarDay[];
  total: number;
}

export interface SeoImportResult {
  applied: number;
  skipped: Array<{ post: string; reason: string }>;
}

export interface PostList {
  posts: Post[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface Revision {
  id: number;
  postId: number;
  authorId: number;
  date: string;
  title: string;
  content: string;
}

export interface MediaItem {
  id: number;
  authorId: number;
  date: string;
  title: string;
  mimeType: string;
  url: string;
  filename: string;
  size: number;
  alt: string;
  objectKey: string;
}

export interface MediaList {
  media: MediaItem[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface Term {
  id: number;
  termTaxonomyId: number;
  taxonomy: string;
  name: string;
  slug: string;
  description: string;
  parentId: number;
  count: number;
}

export interface WpComment {
  id: number;
  postId: number;
  author: string;
  authorEmail: string;
  authorUrl: string;
  date: string;
  content: string;
  status: "approved" | "pending" | "spam" | "trash";
}

export interface CommentList {
  comments: WpComment[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface Menu {
  id: number;
  name: string;
  slug: string;
}

export interface MenuItem {
  id: number;
  menuId: number;
  title: string;
  url: string;
  type: "custom" | "post" | "page" | "term";
  objectId: number;
  objectType: string;
  parentId: number;
  order: number;
  children?: MenuItem[];
}

export interface ActivityItem {
  id: number;
  actorId: number;
  action: string;
  objectType: string | null;
  objectId: string | null;
  details: string | null;
  createdAt: string;
}

export interface ActivityList {
  entries: ActivityItem[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface HealthCheck {
  name: string;
  status: "good" | "recommended" | "critical";
  message: string;
}

export interface SiteHealth {
  checks: HealthCheck[];
}

export interface Settings {
  siteTitle: string;
  siteTagline: string;
  postsPerPage: number;
  permalinkStructure: string;
  timezone: string;
  defaultCommentStatus: string;
  commentMaxLinks: number;
}

/** A P12 multisite row (`wl_sites`). Site 1 is the main site. */
export interface Site {
  id: number;
  slug: string;
  domain: string;
  title: string;
  status: "public" | "archived" | "deleted";
  createdAt: string;
  updatedAt: string;
}

export interface UserList {
  users: User[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

/** P4: one user with editable profile fields (GET /api/users/:id and
 *  PUT /api/users/:id/profile). `avatarId` is a media attachment id,
 *  null when unset; `mustChangePassword` is true while an admin reset
 *  forces a password change on next login. */
export interface UserProfile extends User {
  bio: string;
  avatarId: number | null;
  theme: string | null;
  mustChangePassword: boolean;
}

/** P4: one row of GET /api/plugins (installed list). `verdict` is the
 *  compatibility summary: compatible | partial | blocked | not-analysed.
 *  The full per-file report stays on GET /api/marketplace/:slug/report. */
export interface PluginSummary {
  slug: string;
  name: string;
  kind: string;
  version: string;
  status: string;
  active: boolean;
  verdict: string;
  converted: number;
  partial: number;
  blocked: number;
  phpFiles: number;
}

/** P4: line diff between two revisions (GET /api/revisions/:from/diff/:to). */
export interface RevisionDiff {
  added: number;
  removed: number;
  addedLines: string[];
  removedLines: string[];
  exact: boolean;
  fromId: number;
  toId: number;
}

/** P4: paginated term list (GET /api/terms?taxonomy=…). */
export interface TermList {
  terms: Term[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface ConversionReport {
  source: { filename: string; bytes: number; lines: number };
  filesProcessed: number;
  ast: { nodes: number; functions: number; classes: number; hooks: number };
  converted: { functions: string[]; hooks: { tag: string; kind: string; functionName: string }[] };
  unconverted: { kind: string; name?: string; reason: string; line: number }[];
  warnings: string[];
  transformed: { what: string; note: string }[];
  output: { filename: string; bytes: number; code: string };
  extension: { slug: string; name: string; version: string };
  durationMs: number;
}

export interface ConversionJob {
  id: string;
  status: "pending" | "running" | "done" | "failed";
  filename: string;
  fixture?: string;
  progress: number;
  createdAt: string;
  updatedAt: string;
  report?: ConversionReport;
  error?: string;
}

export interface ConversionJobList {
  jobs: ConversionJob[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface ExtensionRecord {
  slug: string;
  name: string;
  kind: string;
  version: string;
  status: string;
  source: string | null;
  createdAt: string;
  updatedAt: string;
}

/** P6: one loaded extension instance — an item of the `loaded` array of
 *  GET /api/extensions/runtime. `status` is the runtime status: "loaded" (hooks
 *  fire) or "disabled" (kill switch — never dispatched). `permissions`
 *  are the granted capability ids. */
export interface LoadedExtension {
  slug: string;
  name: string;
  version: string;
  status: string;
  permissions: string[];
  scriptKey: string;
  devUrl: string | null;
  createdAt: string;
  updatedAt: string;
}

/** P6: one installed extension that is not loaded — an item of the
 *  `installed` array of GET /api/extensions/runtime. `report` is the
 *  compatibility report the load gate reads (null = never analysed). */
export interface NotLoadedExtension {
  slug: string;
  name: string;
  kind: string;
  version: string;
  status: string;
  loaded: false;
  report: PluginCompatibilityReport | null;
}

/** P6: GET /api/extensions/runtime — loaded instances plus installed-
 *  but-not-loaded extensions. NOTE: this is an object, not an array;
 *  GET /api/extensions itself stays the P3 array of ExtensionRecord. */
export interface ExtensionRuntimeList {
  loaded: LoadedExtension[];
  installed: NotLoadedExtension[];
}

/** P6: GET /api/extensions/:slug — one loaded instance, with the token
 *  epoch and the marketplace report. `name` is null when the instance
 *  has no marketplace row. 404 when the extension is not loaded
 *  ("Extension instance not found. Load it first."). */
export interface ExtensionDetail extends Omit<LoadedExtension, "name"> {
  tokenEpoch: number;
  name: string | null;
  report: PluginCompatibilityReport | null;
}

/** P6: one file the load gate refused, with the exact reasons. */
export interface ExtensionBlockedFile {
  path: string;
  reasons: string[];
}

/** P6: POST /api/extensions/:slug/load. A refusal is HTTP 200 with
 *  `loaded: false` — `reasons` is the complete list for the UI and
 *  `blockedFiles` names the files and their reasons. A success carries
 *  the granted `permissions`, any partial-file `warnings`, and
 *  `dispatchConfigured` (false = the extension Worker has not been
 *  published yet, so its hooks cannot run). */
export type ExtensionLoadResult =
  | {
      slug: string;
      loaded: true;
      version: string;
      status: string;
      permissions: string[];
      requiredCapabilities: string[];
      warnings: string[];
      scriptKey: string;
      devUrl: string | null;
      tokenEpoch: number;
      dispatchConfigured: boolean;
    }
  | {
      slug: string;
      loaded: false;
      reasons: string[];
      blockedFiles: ExtensionBlockedFile[];
    };

/** P6: POST /api/extensions/:slug/unload. */
export interface ExtensionUnloadResult {
  slug: string;
  loaded: boolean;
}

/** P6: POST /api/extensions/:slug/disable and .../enable. */
export interface ExtensionStatusResult {
  slug: string;
  status: string;
}

/** P6: one capability in GET /api/extensions/:slug/permissions —
 *  rendered exactly as the server returns it; the list is never
 *  hardcoded in the UI. */
export interface ExtensionCapability {
  id: string;
  description: string;
  grantedByDefault: boolean;
  granted: boolean;
}

/** P6: GET /api/extensions/:slug/permissions. */
export interface ExtensionPermissions {
  slug: string;
  permissions: string[];
  available: ExtensionCapability[];
}

/** P6: POST /api/extensions/:slug/permissions — the updated grant. */
export interface ExtensionPermissionsResult {
  slug: string;
  permissions: string[];
}

/** P6: one log line — GET /api/extensions/:slug/logs, most recent
 *  first. `level` is "info" | "warn" | "error" | "denied". */
export interface ExtensionLogEntry {
  id: number;
  slug: string;
  level: string;
  message: string;
  created_at: string;
}

/** P6: GET /api/extensions/:slug/logs (default limit 50). */
export interface ExtensionLogList {
  slug: string;
  logs: ExtensionLogEntry[];
}

/** P6: one hook invocation — GET /api/extensions/:slug/runs, most
 *  recent first. `status` is "ok" | "error" | "timeout" | "denied" |
 *  "skipped_disabled". */
export interface ExtensionRun {
  id: number;
  slug: string;
  hook: string;
  status: string;
  duration_ms: number;
  output_summary: string | null;
  error: string | null;
  created_at: string;
}

/** P6: GET /api/extensions/:slug/runs (default limit 50). */
export interface ExtensionRunList {
  slug: string;
  runs: ExtensionRun[];
}

/** P6: GET /api/extensions/:slug/script — the generated extension
 *  Worker project for one loaded instance. `wordpressDts` is an array
 *  of [project-relative path, contents] pairs. Added for completeness;
 *  the Plugins page does not render it. */
export interface ExtensionScript {
  slug: string;
  entry: string;
  shim: string;
  wordpressDts: Array<[string, string]>;
  wranglerToml: string;
  convertedPaths: string[];
}

/** Themes (Milestone 5). `report` is the parsed import/classic report when one exists. */
export interface ThemeRecord {
  slug: string;
  name: string;
  version: string;
  author: string;
  kind: "block" | "classic";
  status: "active" | "inactive";
  hasThemeJson: boolean;
  stylesheetHash: string;
  source: string;
  report: Record<string, unknown> | null;
  createdAt: string;
  updatedAt: string;
}

export interface ThemeDetail extends ThemeRecord {
  themeJson: Record<string, unknown> | null;
  stylesheet: string;
  files: Record<string, string>;
}

export interface ThemeVariation {
  name: string;
  settings: Record<string, unknown>;
  updatedAt: string;
}

export interface PreviewToken {
  token: string;
  slug: string;
  settings: Record<string, unknown>;
  expiresAt: string;
}

/** AI (Milestone 6). */
export type AiMode = "manual" | "supervised" | "autonomous";

export interface ProviderInfo {
  id: string;
  name: string;
  description: string;
  configured: boolean;
  configuredDetail: string;
  recommended: boolean;
  requiresKey: boolean;
}

export interface AiProvidersResponse {
  providers: ProviderInfo[];
  selected: string;
}

export interface AiModeResponse {
  mode: AiMode;
  description: string;
}

export interface AgentInfo {
  id: string;
  name: string;
  description: string;
  capabilities: string[];
  risk: "low" | "medium" | "high";
  requiresProvider: boolean;
  available: boolean;
  unavailableReason?: string;
}

export interface AiProposal {
  type: string;
  summary: string;
  risk: "low" | "medium" | "high";
  payload: Record<string, unknown>;
}

export interface AiTaskResult {
  agent: string;
  mode: AiMode;
  summary: string;
  output?: string;
  data?: Record<string, unknown>;
  usage?: { promptTokens?: number; completionTokens?: number; totalTokens?: number };
  proposals: AiProposal[];
  approvals: Array<{ id: string; summary: string; risk: string }>;
  applied: Array<{ summary: string; result: Record<string, unknown> }>;
  applyErrors?: Array<{ summary: string; error: string }>;
}

export interface AiTask {
  id: string;
  agent: string;
  input: string;
  mode: AiMode;
  status: "pending" | "running" | "done" | "failed";
  progress: number;
  createdAt: string;
  updatedAt: string;
  result?: AiTaskResult;
  error?: string;
}

export interface AiTaskList {
  tasks: AiTask[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface AiApproval {
  id: string;
  taskId: string;
  agent: string;
  action: AiProposal;
  status: "pending" | "approved" | "rejected";
  createdBy: number;
  decidedBy: number | null;
  decidedAt: string | null;
  reason: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface AiApprovalList {
  approvals: AiApproval[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface MemoryEntry {
  id: number;
  kind: "fact" | "note";
  key: string;
  value: string;
  source: string;
  updatedAt: string;
}

export interface MemoryList {
  facts: MemoryEntry[];
  notes: MemoryEntry[];
}

/** Commerce + integrations (Milestone 8). */
export interface Product {
  id: number;
  name: string;
  slug: string;
  description: string;
  priceMinor: number;
  currency: string;
  stock: number;
  status: "draft" | "published";
  featuredImageId: number;
  /** P8: "simple" (default) or "variable". */
  type?: "simple" | "variable";
  /** P8: WooCommerce stable key (unique across products + variations). */
  sku?: string;
  createdAt: string;
  updatedAt: string;
}

export interface ProductList {
  products: Product[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface OrderItem {
  id: number;
  productId: number;
  name: string;
  priceMinor: number;
  quantity: number;
  /** P8: the variation on this line (None for a simple product). */
  variationId?: number;
  /** P8: "Size: M, Colour: Red". */
  variationLabel?: string;
}

export type OrderStatus = "pending" | "paid" | "fulfilled" | "cancelled";

export interface Order {
  id: string;
  customerName: string;
  customerEmail: string;
  status: OrderStatus;
  currency: string;
  subtotalMinor: number;
  totalMinor: number;
  paymentProvider: string;
  paymentReference: string;
  stockApplied: boolean;
  items: OrderItem[];
  /** P8 Woo totals (absent when the order predates P8 or the value is 0). */
  couponCode?: string;
  discountMinor?: number;
  taxMinor?: number;
  shippingMinor?: number;
  shippingMethod?: string;
  /** P8: "wordless" or "woo" (checkout) / "woo-import" (CSV import). */
  source?: string;
  sourceId?: string;
  createdAt: string;
  updatedAt: string;
}

export interface OrderList {
  orders: Order[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

/** WooCommerce subset (P8). */
export interface WooAttribute {
  id: number;
  productId: number;
  name: string;
  slug: string;
  position: number;
  visible: boolean;
  isVariation: boolean;
  options: string[];
  createdAt: string;
  updatedAt: string;
}

export interface WooVariation {
  id: number;
  productId: number;
  sku: string;
  status: string;
  priceMinor: number;
  salePriceMinor: number;
  effectivePriceMinor: number;
  stock: number;
  attributes: Record<string, string>;
  label: string;
  imageId: number;
  createdAt: string;
  updatedAt: string;
}

export interface WooProduct extends Product {
  attributes: WooAttribute[];
  variations: WooVariation[];
  categories: string[];
  tags: string[];
}

export interface Coupon {
  id: number;
  code: string;
  description: string;
  type: "fixed" | "percent";
  amountMinor: number;
  percentBp: number;
  minOrderMinor: number;
  maxUses: number;
  usageCount: number;
  expiresAt: string | null;
  active: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface TaxRate {
  id: number;
  name: string;
  country: string;
  state: string;
  rateBp: number;
  createdAt: string;
  updatedAt: string;
}

export interface TaxSettings {
  taxEnabled: boolean;
  pricesIncludeTax: boolean;
}

export interface ShippingMethod {
  id: number;
  zoneId: number;
  name: string;
  type: "flat_rate" | "free_shipping";
  costMinor: number;
  minOrderMinor: number;
  createdAt: string;
  updatedAt: string;
}

export interface ShippingZone {
  id: number;
  name: string;
  countries: string[];
  createdAt: string;
  updatedAt: string;
}

export interface ZoneWithMethods {
  zone: ShippingZone;
  methods: ShippingMethod[];
}

export interface WooCustomer {
  id: number;
  email: string;
  firstName: string;
  lastName: string;
  country: string;
  state: string;
  postcode: string;
  totalOrders: number;
  totalSpentMinor: number;
  createdAt: string;
  updatedAt: string;
}

export interface WooImportRowResult {
  row: number;
  key: string;
  status: "imported" | "updated" | "skipped" | "failed";
  reason: string | null;
}

export interface WooImportReport {
  kind: string;
  totalRows: number;
  imported: number;
  updated: number;
  skipped: number;
  failed: number;
  rows: WooImportRowResult[];
}

export interface IntegrationInfo {
  id: string;
  name: string;
  kind: string;
  description: string;
  setupHint: string;
  configured: boolean;
  configuredDetail: string;
  config: Record<string, unknown>;
}

export interface MailLogEntry {
  id: number;
  provider: string;
  toEmail: string;
  subject: string;
  body: string;
  status: "logged" | "sent" | "failed";
  createdAt: string;
}

export interface MailLogList {
  entries: MailLogEntry[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface AnalyticsStatus {
  provider: string;
  measurementId: string;
  configured: boolean;
  detail: string;
  snippet: string | null;
}

export interface BindingStatus {
  binding: string;
  present: boolean;
  detail: string;
}

export interface PaymentProviderInfo {
  id: string;
  name: string;
  description: string;
  configured: boolean;
  configuredDetail: string;
  requiresKey: boolean;
}

export interface SelectedPayment {
  provider: string;
  mode: "test" | "live";
}

export interface WebhookLogEntry {
  id: number;
  provider: string;
  event: string;
  reference: string;
  result: string;
  detail: string;
  createdAt: string;
}

export interface WebhookLogList {
  entries: WebhookLogEntry[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface PluginCompatibilityReport {
  phpFiles: number;
  totalFiles: number;
  convertible: number;
  partial: number;
  blocked: number;
  unparsed: number;
  skipped: number;
  verdict: "compatible" | "partial" | "blocked" | "not-analysed";
  topReasons: Array<{ kind: string; count: number; files: string[] }>;
  files: Array<{ file: string; verdict: string; reasons: string[] }>;
  filesTruncated: boolean;
  optionDefaults: Array<{ key: string; defaultValue: string | number | boolean; file: string }>;
  conversion: AutoConvertReport | null;
  analysedAt: string;
}

export interface AutoConvertReport {
  kind: "theme" | "plugin";
  slug: string;
  phpFiles: number;
  converted: number;
  partial: number;
  blocked: number;
  skipped: number;
  files: Array<{
    file: string;
    status: "converted" | "partial" | "blocked";
    functionsConverted: string[];
    hooksConverted: string[];
    unconverted: Array<{ kind: string; name?: string; reason: string; line: number }>;
    reason: string;
    generatedPath: string | null;
  }>;
  filesTruncated: boolean;
  settings: {
    carriedOver: Array<{ key: string; value: string | number | boolean; source: string; reason?: string }>;
    skipped: Array<{ key: string; value: string | number | boolean; source: string; reason?: string }>;
  };
  convertedAt: string;
}

export interface MarketplaceExtension {
  slug: string;
  name: string;
  version: string;
  kind: string;
  status: string;
  /** M11 compatibility + auto-conversion report (null = never analysed). */
  report?: PluginCompatibilityReport | null;
  createdAt: string;
  updatedAt: string;
}

/* Milestone 11 types — file manager. */

export interface FileEntry {
  key: string;
  name: string;
  size: number;
  type: string;
  modified: string | null;
  isMedia: boolean;
}

export interface FileListResult {
  prefix: string;
  folders: string[];
  files: FileEntry[];
  truncated: boolean;
}

/* Milestone 9 types — security, backup, importer, derivatives. */

export interface SecurityOverview {
  headers: { policy: string; applied: boolean };
  lockout: {
    config: { maxAttempts: number; windowMinutes: number; lockoutMinutes: number };
    lockedKeys: Array<{ key: string; attempts: number; lockedUntil: string; lockoutCount: number }>;
  };
  twoFactor: Array<{ userId: number; login: string; role: string; enabled: boolean }>;
  sessions: { active: number };
  failedLogins: Array<{ userId: number; createdAt: string; details: string | null }>;
  defaultAdminLoginExists: boolean;
}

export interface SessionView {
  id: string;
  createdAt: string;
  expiresAt: string;
  lastSeenAt: string | null;
  userAgent: string | null;
  ipHashPrefix: string | null;
  revoked: boolean;
  current: boolean;
}

export interface TotpSetup {
  secret: string;
  url: string;
}

export interface BackupRow {
  id: string;
  filename: string;
  size: number;
  checksum: string;
  tables: Record<string, number>;
  mediaCount: number;
  mediaMissing: number;
  verified: boolean;
  verifiedAt: string | null;
  createdAt: string;
}

export interface BackupVerifyReport {
  backupId: string;
  ok: boolean;
  files: Array<{ file: string; expected: string; actual: string | null; ok: boolean }>;
  verifiedAt: string;
}

export interface BackupJobResult {
  id: string;
  status: string;
  result: BackupRow | { error: string } | null;
}

export interface ImportReportItem {
  type: string;
  sourceId: string;
  targetId: number | null;
  status: "imported" | "skipped" | "failed";
  reason?: string;
}

export interface ImportReport {
  kind: "wxr" | "sql";
  source: string;
  counts: {
    imported: number;
    skipped: number;
    failed: number;
    byType: Record<string, { imported: number; skipped: number; failed: number }>;
  };
  items: ImportReportItem[];
  ignoredTables?: string[];
  finishedAt: string;
}

export interface ImportJobResult {
  id: string;
  kind: string;
  status: string;
  result: ImportReport | { error: string } | null;
  createdAt: string;
  updatedAt: string;
}

export interface DerivativeStatus {
  bound: boolean;
  stored: number;
  widths: number[];
}

export interface SocialMember {
  userId: number;
  login: string;
  displayName: string;
  bio: string;
  avatarId: number | null;
  visibility: string;
  friendStatus: string;
  registered: string;
}

export interface SocialMemberList {
  items: SocialMember[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface SocialGroup {
  id: number;
  slug: string;
  name: string;
  description: string;
  visibility: string;
  avatarId: number | null;
  createdBy: number;
  memberCount: number;
  myStatus: string;
  myRole: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface SocialGroupList {
  items: SocialGroup[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export interface SocialActivityItem {
  id: number;
  actorId: number;
  actorName: string;
  actorLogin: string;
  type: string;
  objectType: string;
  objectId: number;
  content: string;
  visibility: string;
  groupId: number | null;
  groupName: string | null;
  status: string;
  likeCount: number;
  likedByMe: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface SocialThread {
  id: number;
  otherUserId: number;
  otherLogin: string;
  otherDisplayName: string;
  lastMessage: string | null;
  lastMessageAt: string | null;
  unreadCount: number;
  updatedAt: string;
}

export interface SocialNotification {
  id: number;
  userId: number;
  type: string;
  actorId: number;
  actorName: string;
  objectType: string;
  objectId: number;
  readAt: string | null;
  createdAt: string;
}

/* ------------------------------------------------------------------ School (P9) */

export interface SchoolCourse {
  id: number;
  slug: string;
  title: string;
  description: string;
  status: string;
  instructorId: number;
  instructorLogin: string;
  instructorName: string;
  lessonCount: number;
  enrolmentCount: number;
  createdAt: string;
  updatedAt: string;
}

export interface SchoolCourseList {
  items: SchoolCourse[];
  total: number;
  page: number;
  perPage: number;
}

export interface SchoolLesson {
  id: number;
  courseId: number;
  title: string;
  content: string;
  position: number;
  durationMinutes: number;
  completed: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface SchoolEnrolment {
  id: number;
  courseId: number;
  userId: number;
  login: string;
  displayName: string;
  status: string;
  enrolledAt: string;
  completedAt: string | null;
  lessonsCompleted: number;
  lessonsTotal: number;
}

export interface SchoolQuestionBank {
  id: number;
  courseId: number;
  title: string;
  description: string;
  questionCount: number;
  createdAt: string;
  updatedAt: string;
}

export interface SchoolQuestion {
  id: number;
  bankId: number;
  type: string;
  questionText: string;
  options: string[];
  answer?: unknown;
  modelAnswer: string;
  marks: number;
  difficulty: string;
  topic: string;
  createdAt: string;
  updatedAt: string;
}

export interface SchoolExam {
  id: number;
  courseId: number;
  bankId: number;
  title: string;
  description: string;
  durationMinutes: number;
  windowStart: string | null;
  windowEnd: string | null;
  maxAttempts: number;
  passMarkPercent: number;
  shuffleQuestions: boolean;
  questionIds: number[];
  status: string;
  questionCount: number;
  attemptsUsed: number;
  createdAt: string;
  updatedAt: string;
}

export interface SchoolAttemptQuestion {
  questionId: number;
  type: string;
  questionText: string;
  options: string[];
  marks: number;
}

export interface SchoolAttemptAnswer {
  questionId: number;
  answer: unknown;
  isCorrect: boolean | null;
  marksAwarded: number | null;
  graderFeedback: string;
  gradedAt: string | null;
}

export interface SchoolAttempt {
  id: number;
  examId: number;
  userId: number;
  status: string;
  startedAtMs: number;
  deadlineMs: number;
  submittedAtMs: number | null;
  questionOrder: number[];
  score: number;
  maxScore: number;
  questions: SchoolAttemptQuestion[];
  answers: SchoolAttemptAnswer[];
  createdAt: string;
  updatedAt: string;
}

export interface SchoolResult {
  id: number;
  attemptId: number;
  userId: number;
  examId: number;
  courseId: number;
  score: number;
  maxScore: number;
  percent: number;
  passed: boolean;
  certificateCode: string | null;
  gradedAt: string;
}

export interface SchoolLeaderboardRow {
  userId: number;
  login: string;
  displayName: string;
  score: number;
  maxScore: number;
  percent: number;
  submittedAtMs: number;
  gradedAt: string;
}

export interface SchoolGradingItem {
  answerId: number;
  attemptId: number;
  examId: number;
  examTitle: string;
  courseId: number;
  questionId: number;
  questionText: string;
  modelAnswer: string;
  marks: number;
  studentId: number;
  studentLogin: string;
  studentName: string;
  answer: unknown;
  submittedAtMs: number;
}

export interface SchoolCertificate {
  id: number;
  code: string;
  userId: number;
  studentLogin: string;
  studentName: string;
  courseId: number;
  courseTitle: string;
  examId: number;
  examTitle: string;
  attemptId: number;
  resultId: number;
  score: number;
  maxScore: number;
  percent: number;
  issuedAt: string;
}
