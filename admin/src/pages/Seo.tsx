/**
 * SEO (§6.3) — per-post fields + analysis, redirects, 404 log,
 * robots.txt, and Yoast/Rank Math import, plus the P10 audit screen
 * (recorded Lighthouse runs vs Core Web Vitals budgets, structured
 * data preview, sitemap viewer). Scores shown here are only ever
 * ones a real tool measured and someone recorded — the Worker never
 * generates a score.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import type { CwvBudgets, NotFoundList, PostList, PostSeo, RedirectList, SeoAnalysis, SeoAuditRun, SeoImportResult, SeoJsonLdPreview, SeoSitemapFile } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Input, Select, Skeleton, Table, Tabs, Toggle } from "../components/elements";

function checkTone(s: string): "teal" | "amber" | "red" | "grey" {
  if (s === "pass") return "teal";
  if (s === "warn") return "amber";
  if (s === "fail") return "red";
  return "grey";
}

export function SeoPage(): React.ReactElement {
  const [tab, setTab] = useState("per-post");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // Per-post SEO.
  const [postSearch, setPostSearch] = useState("");
  const [postOptions, setPostOptions] = useState<PostList | null>(null);
  const [selectedPostId, setSelectedPostId] = useState<number | null>(null);
  const [seo, setSeo] = useState<PostSeo | null>(null);
  const [analysis, setAnalysis] = useState<SeoAnalysis | null>(null);
  const [seoSaving, setSeoSaving] = useState(false);

  // Redirects / 404 / robots / import.
  const [redirects, setRedirects] = useState<RedirectList | null>(null);
  const [redirectPage, setRedirectPage] = useState(1);
  const [notFound, setNotFound] = useState<NotFoundList | null>(null);
  const [notFoundPage, setNotFoundPage] = useState(1);
  const [robots, setRobots] = useState<string | null>(null);
  const [robotsDraft, setRobotsDraft] = useState("");
  const [oldPath, setOldPath] = useState("");
  const [newPath, setNewPath] = useState("");
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editingTarget, setEditingTarget] = useState("");
  const [importText, setImportText] = useState("");
  const [importResult, setImportResult] = useState<SeoImportResult | null>(null);
  const [saving, setSaving] = useState(false);

  // P10 audit screen: CWV budgets, recorded runs, sitemap viewer,
  // structured-data preview for the selected post.
  const [budgets, setBudgets] = useState<CwvBudgets | null>(null);
  const [budgetDraft, setBudgetDraft] = useState<CwvBudgets | null>(null);
  const [runs, setRuns] = useState<SeoAuditRun[] | null>(null);
  const [sitemapFiles, setSitemapFiles] = useState<SeoSitemapFile[] | null>(null);
  const [jsonLd, setJsonLd] = useState<SeoJsonLdPreview | null>(null);
  const [runUrl, setRunUrl] = useState("");
  const [runSeo, setRunSeo] = useState("");
  const [runA11y, setRunA11y] = useState("");
  const [runPerf, setRunPerf] = useState("");
  const [runBp, setRunBp] = useState("");
  const [runLcp, setRunLcp] = useState("");
  const [runInp, setRunInp] = useState("");
  const [runCls, setRunCls] = useState("");
  const [runNotes, setRunNotes] = useState("");

  const loadAudit = useCallback(async () => {
    try {
      const [b, r, s] = await Promise.all([
        api.getCwvBudgets(),
        api.listAuditRuns(20),
        api.getSitemapOverview(),
      ]);
      setBudgets(b);
      setBudgetDraft(b);
      setRuns(r);
      setSitemapFiles(s.files);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load the audit screen.");
    }
  }, []);

  useEffect(() => {
    if (tab === "audit") void loadAudit();
  }, [tab, loadAudit]);

  const scoreOrNull = (raw: string): number | null => {
    const n = Number.parseInt(raw.trim(), 10);
    return Number.isNaN(n) ? null : n;
  };

  const saveBudgets = async (): Promise<void> => {
    if (budgetDraft === null) return;
    setError(null);
    setNotice(null);
    try {
      const saved = await api.updateCwvBudgets(budgetDraft);
      setBudgets(saved);
      setBudgetDraft(saved);
      setNotice("Core Web Vitals budgets saved.");
    } catch (err) {
      fail(err);
    }
  };

  const recordRun = async (): Promise<void> => {
    setError(null);
    setNotice(null);
    try {
      const cls = runCls.trim() === "" ? null : Math.round(Number.parseFloat(runCls) * 1000);
      const run = await api.recordAuditRun({
        url: runUrl.trim(),
        tool: "lighthouse",
        seoScore: scoreOrNull(runSeo),
        accessibilityScore: scoreOrNull(runA11y),
        performanceScore: scoreOrNull(runPerf),
        bestPracticesScore: scoreOrNull(runBp),
        lcpMs: scoreOrNull(runLcp),
        inpMs: scoreOrNull(runInp),
        clsMilli: Number.isNaN(cls) ? null : cls,
        notes: runNotes.trim(),
      });
      setNotice(`Audit run recorded for ${run.url}.`);
      setRunUrl("");
      setRunSeo("");
      setRunA11y("");
      setRunPerf("");
      setRunBp("");
      setRunLcp("");
      setRunInp("");
      setRunCls("");
      setRunNotes("");
      await loadAudit();
    } catch (err) {
      fail(err);
    }
  };

  const loadJsonLd = async (id: number): Promise<void> => {
    setJsonLd(null);
    try {
      setJsonLd(await api.getPostJsonLd(id));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load the structured-data preview.");
    }
  };

  const loadRedirects = useCallback(async (page: number) => {
    try {
      setRedirects(await api.listRedirects(page));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load redirects.");
    }
  }, []);

  const loadNotFound = useCallback(async (page: number) => {
    try {
      setNotFound(await api.listNotFound(page));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load the 404 log.");
    }
  }, []);

  const loadRobots = useCallback(async () => {
    try {
      const { robots: txt } = await api.getRobots();
      setRobots(txt);
      setRobotsDraft(txt);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load robots.txt.");
    }
  }, []);

  useEffect(() => { void loadRedirects(redirectPage); }, [redirectPage, loadRedirects]);
  useEffect(() => { void loadNotFound(notFoundPage); }, [notFoundPage, loadNotFound]);
  useEffect(() => { void loadRobots(); }, [loadRobots]);

  const searchPosts = async (): Promise<void> => {
    setError(null);
    try {
      const params = new URLSearchParams({ page: "1", per_page: "10" });
      if (postSearch.trim()) params.set("search", postSearch.trim());
      setPostOptions(await api.get<PostList>(`/api/posts?${params.toString()}`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to search posts.");
    }
  };

  const loadPostSeo = async (id: number): Promise<void> => {
    setError(null);
    setSelectedPostId(id);
    try {
      const [s, a] = await Promise.all([api.getPostSeo(id), api.getPostSeoAnalysis(id)]);
      setSeo(s);
      setAnalysis(a);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load this post's SEO.");
    }
  };

  const savePostSeo = async (): Promise<void> => {
    if (selectedPostId === null || seo === null) return;
    setSeoSaving(true);
    setError(null);
    try {
      const saved = await api.updatePostSeo(selectedPostId, seo);
      setSeo(saved);
      setAnalysis(await api.getPostSeoAnalysis(selectedPostId));
      setNotice("SEO fields saved for this post.");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to save SEO fields.");
    } finally {
      setSeoSaving(false);
    }
  };

  const updateSeo = <K extends keyof PostSeo>(key: K, value: PostSeo[K]): void => {
    setSeo((prev) => (prev === null ? prev : { ...prev, [key]: value }));
  };

  const fail = (err: unknown): void => {
    setError(err instanceof Error ? err.message : "Something went wrong.");
  };

  const createRedirect = async (): Promise<void> => {
    setError(null);
    setNotice(null);
    try {
      const created = await api.createRedirect(oldPath.trim(), newPath.trim());
      setNotice(`Redirect created: ${created.oldPath} → ${created.newPath}.`);
      setOldPath("");
      setNewPath("");
      await loadRedirects(redirectPage);
    } catch (err) {
      fail(err);
    }
  };

  const saveEditedRedirect = async (id: number): Promise<void> => {
    setError(null);
    try {
      await api.updateRedirect(id, editingTarget.trim());
      setEditingId(null);
      setNotice("Redirect updated.");
      await loadRedirects(redirectPage);
    } catch (err) {
      fail(err);
    }
  };

  const removeRedirect = async (id: number): Promise<void> => {
    setError(null);
    try {
      await api.deleteRedirect(id);
      setNotice("Redirect deleted.");
      await loadRedirects(redirectPage);
    } catch (err) {
      fail(err);
    }
  };

  const createRedirectFrom404 = (path: string): void => {
    setOldPath(path);
    setNewPath("");
    setTab("redirects");
    setNotice(`Old path prefilled from the 404 log — enter where "${path}" should point.`);
  };

  const removeNotFound = async (path: string): Promise<void> => {
    setError(null);
    try {
      await api.deleteNotFound(path);
      await loadNotFound(notFoundPage);
    } catch (err) {
      fail(err);
    }
  };

  const saveRobots = async (): Promise<void> => {
    setError(null);
    setNotice(null);
    setSaving(true);
    try {
      const { robots: txt } = await api.updateRobots(robotsDraft);
      setRobots(txt);
      setRobotsDraft(txt);
      setNotice("robots.txt saved.");
    } catch (err) {
      fail(err);
    } finally {
      setSaving(false);
    }
  };

  const runImport = async (): Promise<void> => {
    setError(null);
    setNotice(null);
    setImportResult(null);
    let parsed: unknown;
    try {
      parsed = JSON.parse(importText) as unknown;
    } catch {
      setError("Import text is not valid JSON. Paste an object like {\"posts\": [...]}.");
      return;
    }
    const posts = (parsed as { posts?: unknown }).posts;
    if (!Array.isArray(posts)) {
      setError('Import JSON must be an object with a "posts" array.');
      return;
    }
    try {
      const result = await api.importSeo(posts);
      setImportResult(result);
      setNotice(`Import finished: ${result.applied} applied, ${result.skipped.length} skipped.`);
    } catch (err) {
      fail(err);
    }
  };

  const selectedPost = postOptions?.posts.find((p) => p.id === selectedPostId) ?? null;

  return (
    <div>
      {error !== null ? <Alert tone="err" title="SEO" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <Tabs
        ariaLabel="SEO sections"
        value={tab}
        onChange={setTab}
        tabs={[
          { id: "per-post", label: "Per-post SEO", icon: "seo" },
          { id: "audit", label: "Audit", icon: "activity" },
          { id: "redirects", label: "Redirects", icon: "link" },
          { id: "notfound", label: "404 log", icon: "alert" },
          { id: "robots", label: "Sitemap & robots", icon: "globe" },
          { id: "import", label: "Import metadata", icon: "import" },
        ]}
      />

      {tab === "per-post" ? (
        <>
          <Card title="Per-post SEO" sub="Pick a post, edit its SEO fields, and run the deterministic analysis. Recorded Lighthouse runs live under the Audit tab — scores are only shown there when a real run has been recorded.">
            <div className="toolbar">
              <Input label="Find a post" type="search" placeholder="Search posts by title…" value={postSearch} onChange={(e) => setPostSearch(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") void searchPosts(); }} />
              <Button icon="search" onClick={() => void searchPosts()}>Search</Button>
            </div>
            {postOptions === null ? (
              <p className="hint">Search to list posts, or leave the search blank to list the latest 10.</p>
            ) : postOptions.posts.length === 0 ? (
              <EmptyState icon="seo" title="No posts found" description="Try a different search term." />
            ) : (
              <Table
                caption="Posts"
                columns={[
                  { key: "title", label: "Post" },
                  { key: "status", label: "Status" },
                  { key: "actions", label: "Actions" },
                ]}
                rows={postOptions.posts.map((p) => ({
                  title: <span className="row-title">{p.title || "(untitled)"}</span>,
                  status: <Badge tone={p.status === "publish" ? "teal" : "amber"}>{p.status}</Badge>,
                  actions: <Button size="small" variant={selectedPostId === p.id ? "primary" : "secondary"} onClick={() => void loadPostSeo(p.id)}>{selectedPostId === p.id ? "Selected" : "Edit SEO"}</Button>,
                }))}
              />
            )}
          </Card>

          {seo !== null && selectedPost !== null ? (
            <>
              <Card title={`SEO — ${selectedPost.title || "(untitled)"}`} sub="These fields are what search engines and social networks see for this post.">
                <div className="form-grid">
                  <Input label="SEO title" value={seo.title} onChange={(e) => updateSeo("title", e.target.value)} hint="Leave blank to use the post title." />
                  <Input label="Focus keyword" value={seo.focusKeyword} onChange={(e) => updateSeo("focusKeyword", e.target.value)} />
                  <Input label="Canonical URL (leave blank for the post URL)" value={seo.canonical} onChange={(e) => updateSeo("canonical", e.target.value)} />
                  <Select label="Twitter card" value={seo.twitterCard} onChange={(e) => updateSeo("twitterCard", e.target.value as "summary" | "summary_large_image")}>
                    <option value="summary">Summary</option>
                    <option value="summary_large_image">Summary with large image</option>
                  </Select>
                </div>
                <Input label="Meta description" value={seo.description} onChange={(e) => updateSeo("description", e.target.value)} hint={`${seo.description.length} characters — aim for roughly 120–160.`} />
                <Input label="Open Graph title" value={seo.ogTitle} onChange={(e) => updateSeo("ogTitle", e.target.value)} hint="Leave blank to use the SEO title." />
                <Input label="Open Graph description" value={seo.ogDescription} onChange={(e) => updateSeo("ogDescription", e.target.value)} hint="Leave blank to use the meta description." />
                <Toggle label="Noindex — keep this post out of search results" checked={seo.noindex} onChange={(v) => updateSeo("noindex", v)} />
                <div className="actions" style={{ marginTop: 12 }}>
                  <Button variant="primary" icon="check" disabled={seoSaving} onClick={() => void savePostSeo()}>{seoSaving ? "Saving…" : "Save SEO fields"}</Button>
                </div>
              </Card>

              {analysis !== null ? (
                <Card title={`Analysis — score ${analysis.score} / 100`} sub={`${analysis.wordCount} words · ${analysis.internalLinks} internal link(s) · ${analysis.externalLinks} external link(s) · ${analysis.images} image(s), ${analysis.imagesMissingAlt} missing alt text${analysis.readingEase !== null ? ` · reading ease ${analysis.readingEase}` : ""}. This is the deterministic on-page scorer, not a Lighthouse run.`}>
                  <Table
                    caption="SEO checks"
                    columns={[
                      { key: "check", label: "Check" },
                      { key: "status", label: "Result" },
                      { key: "detail", label: "Detail" },
                    ]}
                    rows={analysis.checks.map((c) => ({
                      check: c.label,
                      status: <Badge tone={checkTone(c.status)}>{c.status}</Badge>,
                      detail: c.detail,
                    }))}
                    empty={<EmptyState icon="seo" title="No checks returned" />}
                  />
                </Card>
              ) : null}
            </>
          ) : null}
        </>
      ) : null}

      {tab === "audit" ? (
        <>
          <Card title="Core Web Vitals budgets" sub="The targets a recorded run is judged against. Defaults are the web.dev “good” thresholds (LCP ≤ 2.5 s, INP ≤ 200 ms, CLS ≤ 0.1). The Worker never measures a visitor's vitals itself — these are budgets, not measurements.">
            {budgetDraft === null ? (
              <Skeleton height={80} label="Loading budgets" />
            ) : (
              <>
                <div className="form-grid">
                  <Input label="LCP budget (ms)" type="number" value={String(budgetDraft.lcpMs)} onChange={(e) => setBudgetDraft({ ...budgetDraft, lcpMs: Number.parseInt(e.target.value, 10) || 0 })} />
                  <Input label="INP budget (ms)" type="number" value={String(budgetDraft.inpMs)} onChange={(e) => setBudgetDraft({ ...budgetDraft, inpMs: Number.parseInt(e.target.value, 10) || 0 })} />
                  <Input label="CLS budget" type="number" value={String(budgetDraft.cls)} onChange={(e) => setBudgetDraft({ ...budgetDraft, cls: Number.parseFloat(e.target.value) || 0 })} hint="Unitless ratio, e.g. 0.1." />
                </div>
                <Button variant="primary" onClick={() => void saveBudgets()}>Save budgets</Button>
              </>
            )}
          </Card>

          <Card title="Record a Lighthouse run" sub="Paste the numbers from a real Lighthouse run (staging, or your own machine). Nothing here is measured by Wordless — a run with no scores is refused, and a blank field means “not measured”, never a pass.">
            <div className="form-grid">
              <Input label="URL" value={runUrl} onChange={(e) => setRunUrl(e.target.value)} placeholder="https://example.com/hello" />
              <Input label="SEO score (0–100)" type="number" value={runSeo} onChange={(e) => setRunSeo(e.target.value)} />
              <Input label="Accessibility score (0–100)" type="number" value={runA11y} onChange={(e) => setRunA11y(e.target.value)} />
              <Input label="Performance score (0–100)" type="number" value={runPerf} onChange={(e) => setRunPerf(e.target.value)} />
              <Input label="Best Practices score (0–100)" type="number" value={runBp} onChange={(e) => setRunBp(e.target.value)} />
              <Input label="LCP (ms)" type="number" value={runLcp} onChange={(e) => setRunLcp(e.target.value)} />
              <Input label="INP (ms)" type="number" value={runInp} onChange={(e) => setRunInp(e.target.value)} />
              <Input label="CLS (e.g. 0.05)" type="number" value={runCls} onChange={(e) => setRunCls(e.target.value)} />
            </div>
            <Input label="Notes" value={runNotes} onChange={(e) => setRunNotes(e.target.value)} hint="e.g. “Lighthouse 12, staging, throttled 4G”." />
            <div className="actions" style={{ marginTop: 12 }}>
              <Button variant="primary" icon="plus" disabled={runUrl.trim() === ""} onClick={() => void recordRun()}>Record run</Button>
            </div>
          </Card>

          <Card title={`Audit runs (${runs?.length ?? 0})`} sub="Newest first. Targets are SEO 100, Accessibility ≥ 95, Performance ≥ 90, Best Practices ≥ 90, and the Core Web Vitals budgets above.">
            {runs === null ? (
              <Skeleton height={120} label="Loading audit runs" />
            ) : runs.length === 0 ? (
              <EmptyState icon="activity" title="No audit runs recorded yet" description="Run Lighthouse on staging and record the numbers above. No scores are shown until a real run exists." />
            ) : (
              <Table
                caption="Audit runs"
                columns={[
                  { key: "url", label: "URL" },
                  { key: "scores", label: "Scores (SEO / A11y / Perf / BP)" },
                  { key: "vitals", label: "Vitals (LCP / INP / CLS)" },
                  { key: "date", label: "Recorded" },
                ]}
                rows={runs.map((r) => {
                  const score = (v: number | null): string => (v === null ? "—" : String(v));
                  const vital = (v: number | null, budget: number | null, fmt: (n: number) => string): string =>
                    v === null ? "—" : `${fmt(v)}${budget !== null ? (v <= budget ? " ✓" : " ✗") : ""}`;
                  return {
                    url: <code>{r.url}</code>,
                    scores: `${score(r.seoScore)} / ${score(r.accessibilityScore)} / ${score(r.performanceScore)} / ${score(r.bestPracticesScore)}`,
                    vitals: [
                      vital(r.lcpMs, budgets?.lcpMs ?? null, (n) => `${n}ms`),
                      vital(r.inpMs, budgets?.inpMs ?? null, (n) => `${n}ms`),
                      vital(r.clsMilli, budgets === null ? null : Math.round(budgets.cls * 1000), (n) => (n / 1000).toFixed(3)),
                    ].join(" / "),
                    date: r.createdAt.slice(0, 10),
                  };
                })}
              />
            )}
          </Card>

          <Card title="Sitemap viewer" sub="Every sitemap file the site serves, with its entry count.">
            {sitemapFiles === null ? (
              <Skeleton height={120} label="Loading sitemaps" />
            ) : (
              <Table
                caption="Sitemap files"
                columns={[
                  { key: "file", label: "File" },
                  { key: "entries", label: "Entries" },
                ]}
                rows={sitemapFiles.map((f) => ({
                  file: <a href={f.loc} target="_blank" rel="noreferrer">{f.loc}</a>,
                  entries: String(f.entries),
                }))}
              />
            )}
          </Card>

          {selectedPost !== null ? (
            <Card title={`Structured data — ${selectedPost.title || "(untitled)"}`} sub="The JSON-LD the public page serves for this post, with the structural validator's verdict. This checks the markup is well-formed and complete per schema.org — it is not Google's Rich Results Test, and a clean result is never presented as a Google rich result.">
              <Button variant="secondary" onClick={() => void loadJsonLd(selectedPost.id)}>Preview JSON-LD</Button>
              {jsonLd !== null ? (
                <div style={{ marginTop: 12 }}>
                  <Badge tone={jsonLd.valid ? "teal" : "red"}>{jsonLd.valid ? "Valid" : "Problems found"}</Badge>
                  {jsonLd.problems.length > 0 ? (
                    <ul>
                      {jsonLd.problems.map((p, i) => (
                        <li key={i}>{p}</li>
                      ))}
                    </ul>
                  ) : null}
                  <pre style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>{JSON.stringify(jsonLd.jsonLd, null, 2)}</pre>
                </div>
              ) : null}
            </Card>
          ) : (
            <Card title="Structured data" sub="Pick a post on the Per-post SEO tab first — the preview shows the JSON-LD its public page serves.">
              <p className="hint">No post selected.</p>
            </Card>
          )}
        </>
      ) : null}

      {tab === "redirects" ? (
        <Card title={`Redirects (${redirects?.total ?? 0})`} sub="Renaming a published post's slug creates one automatically.">
          <div className="form-grid">
            <Input label="Old path (from)" value={oldPath} onChange={(e) => setOldPath(e.target.value)} placeholder="/old-page" />
            <Input label="New path or URL (to)" value={newPath} onChange={(e) => setNewPath(e.target.value)} placeholder="/new-page" />
          </div>
          <Button variant="primary" icon="plus" disabled={oldPath.trim() === "" || newPath.trim() === ""} onClick={() => void createRedirect()}>Add redirect</Button>
          {redirects === null ? (
            <Skeleton height={120} label="Loading redirects" />
          ) : redirects.redirects.length === 0 ? (
            <EmptyState icon="link" title="No redirects yet" description="Renaming a published post's slug creates one automatically." />
          ) : (
            <>
              <Table
                caption="Redirects"
                columns={[
                  { key: "old", label: "Old path" },
                  { key: "new", label: "New path" },
                  { key: "actions", label: "Actions" },
                ]}
                rows={redirects.redirects.map((r) => ({
                  old: <code>{r.oldPath}</code>,
                  new:
                    editingId === r.id ? (
                      <Input label="New path" value={editingTarget} onChange={(e) => setEditingTarget(e.target.value)} />
                    ) : (
                      <code>{r.newPath}</code>
                    ),
                  actions:
                    editingId === r.id ? (
                      <span className="actions">
                        <Button size="small" variant="primary" onClick={() => void saveEditedRedirect(r.id)}>Save</Button>
                        <Button size="small" onClick={() => setEditingId(null)}>Cancel</Button>
                      </span>
                    ) : (
                      <span className="actions">
                        <Button size="small" icon="edit" onClick={() => { setEditingId(r.id); setEditingTarget(r.newPath); }}>Edit</Button>
                        <Button size="small" variant="danger" icon="trash" onClick={() => void removeRedirect(r.id)}>Delete</Button>
                      </span>
                    ),
                }))}
              />
              <div className="pagination">
                <Button size="small" icon="chevronLeft" disabled={redirects.page <= 1} onClick={() => setRedirectPage(redirects.page - 1)}>Previous</Button>
                <span>Page {redirects.page} of {Math.max(redirects.totalPages, 1)} ({redirects.total} total)</span>
                <Button size="small" icon="chevronRight" disabled={redirects.page >= redirects.totalPages} onClick={() => setRedirectPage(redirects.page + 1)}>Next</Button>
              </div>
            </>
          )}
        </Card>
      ) : null}

      {tab === "notfound" ? (
        <Card title={`404 log (${notFound?.total ?? 0})`} sub="Public 404s, one row per path, most-hit first. Query strings are never stored. The log keeps the 500 most recently seen paths.">
          {notFound === null ? (
            <Skeleton height={120} label="Loading 404 log" />
          ) : notFound.entries.length === 0 ? (
            <EmptyState icon="alert" title="No 404s recorded yet" />
          ) : (
            <>
              <Table
                caption="404 log"
                columns={[
                  { key: "path", label: "Path" },
                  { key: "hits", label: "Hits" },
                  { key: "last", label: "Last seen" },
                  { key: "actions", label: "Actions" },
                ]}
                rows={notFound.entries.map((entry) => ({
                  path: <code>{entry.path}</code>,
                  hits: String(entry.count),
                  last: entry.lastSeen.slice(0, 10),
                  actions: (
                    <span className="actions">
                      <Button size="small" onClick={() => createRedirectFrom404(entry.path)}>Create redirect</Button>
                      <Button size="small" variant="danger" onClick={() => void removeNotFound(entry.path)}>Dismiss</Button>
                    </span>
                  ),
                }))}
              />
              <div className="pagination">
                <Button size="small" icon="chevronLeft" disabled={notFound.page <= 1} onClick={() => setNotFoundPage(notFound.page - 1)}>Previous</Button>
                <span>Page {notFound.page} of {Math.max(notFound.totalPages, 1)} ({notFound.total} total)</span>
                <Button size="small" icon="chevronRight" disabled={notFound.page >= notFound.totalPages} onClick={() => setNotFoundPage(notFound.page + 1)}>Next</Button>
              </div>
            </>
          )}
        </Card>
      ) : null}

      {tab === "robots" ? (
        <Card title="Sitemap & robots" sub="The sitemap lists the home page, published posts/pages and term archives, with changefreq and priority from recency. Above 1,000 URLs it becomes an index with pages of 1,000. Image entries are included only for attachments that exist.">
          <p>
            <a href="/sitemap.xml" target="_blank" rel="noreferrer">View sitemap.xml</a>
            {" · "}
            <a href="/robots.txt" target="_blank" rel="noreferrer">View robots.txt</a>
          </p>
          {robots === null ? (
            <Skeleton height={160} label="Loading robots.txt" />
          ) : (
            <>
              <div className="field">
                <label htmlFor="robots">robots.txt</label>
                <textarea id="robots" value={robotsDraft} onChange={(e) => setRobotsDraft(e.target.value)} rows={8} spellCheck={false} />
                <p className="hint">Rules that would block /admin or /api are rejected — a typo here must never lock you out of your own admin.</p>
              </div>
              <Button variant="primary" disabled={saving} onClick={() => void saveRobots()}>{saving ? "Saving…" : "Save robots.txt"}</Button>
            </>
          )}
        </Card>
      ) : null}

      {tab === "import" ? (
        <Card title="Import SEO metadata (Yoast / Rank Math)" sub='Paste JSON like {"posts": [{"slug": "hello", "meta": {"_yoast_wpseo_title": "..."}}]}. Yoast (_yoast_wpseo_*) and Rank Math (rank_math_*) keys are mapped to Wordless fields; unknown keys and invalid rows are reported, never silently dropped. At most 500 posts per import.'>
          <div className="field">
            <label htmlFor="seo-import">Import JSON</label>
            <textarea id="seo-import" value={importText} onChange={(e) => setImportText(e.target.value)} rows={6} spellCheck={false} placeholder='{"posts": []}' />
          </div>
          <Button variant="primary" icon="import" onClick={() => void runImport()}>Import</Button>
          {importResult !== null ? (
            <div style={{ marginTop: 12 }}>
              <p><strong>{importResult.applied} applied</strong>, {importResult.skipped.length} skipped.</p>
              {importResult.skipped.length > 0 ? (
                <ul>
                  {importResult.skipped.map((s, i) => (
                    <li key={i}><strong>{s.post}:</strong> {s.reason}</li>
                  ))}
                </ul>
              ) : null}
            </div>
          ) : null}
        </Card>
      ) : null}
    </div>
  );
}
