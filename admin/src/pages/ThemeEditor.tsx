/**
 * Themes + Customizer (§6.3).
 *
 * Lists installed themes (import, activate, delete) and a customizer
 * for the ACTIVE block theme: colour palette, font sizes, font
 * families, spacing scale, layout, and per-block styles — with a
 * LIVE preview that updates as you type (no save-and-refresh), preset
 * palettes, and one-click reset with undo. Saving writes a variation
 * via the API; the server-side preview token renders the public site
 * with unsaved overrides in an iframe.
 *
 * Full site-template editing is out of scope — templates are edited
 * by importing a new theme version.
 */
import { useEffect, useRef, useState } from "react";
import { api, ApiError } from "../lib/api";
import type { ThemeDetail, ThemeRecord, ThemeVariation } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Input, Select, Skeleton } from "../components/elements";

type Settings = Record<string, unknown>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asArray(value: unknown): Array<Record<string, unknown>> {
  return Array.isArray(value) ? value.filter((v): v is Record<string, unknown> => isRecord(v)) : [];
}

function str(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function mergeSettings(base: Settings, over: Settings): Settings {
  const merged: Settings = { ...base };
  for (const [section, value] of Object.entries(over)) {
    const baseSection = isRecord(merged[section]) ? { ...(merged[section] as Settings) } : {};
    if (!isRecord(value)) {
      merged[section] = value;
      continue;
    }
    const nextSection: Settings = { ...baseSection };
    for (const [key, val] of Object.entries(value)) {
      if (Array.isArray(val)) {
        const baseArr = asArray(baseSection[key]);
        const mergedArr = [...baseArr];
        for (const row of asArray(val)) {
          const slug = str(row.slug);
          const idx = mergedArr.findIndex((r) => str(r.slug) === slug);
          if (idx >= 0) mergedArr[idx] = { ...mergedArr[idx], ...row };
          else mergedArr.push(row);
        }
        nextSection[key] = mergedArr;
      } else {
        nextSection[key] = val;
      }
    }
    merged[section] = nextSection;
  }
  return merged;
}

function sectionOf(settings: Settings, section: string): Record<string, unknown> {
  return isRecord(settings[section]) ? (settings[section] as Record<string, unknown>) : {};
}

function listOf(settings: Settings, section: string, key: string): Array<Record<string, unknown>> {
  return asArray(sectionOf(settings, section)[key]);
}

function setList(settings: Settings, section: string, key: string, rows: Array<Record<string, unknown>>): Settings {
  return { ...settings, [section]: { ...sectionOf(settings, section), [key]: rows } };
}

function setLayout(settings: Settings, key: string, value: string): Settings {
  return { ...settings, layout: { ...sectionOf(settings, "layout"), [key]: value } };
}

/** Set a nested value by path, e.g. ["styles","blocks","core/heading","color","text"]. */
function setNested(settings: Settings, path: string[], value: string): Settings {
  if (path.length === 0) return settings;
  const [head, ...rest] = path as [string, ...string[]];
  if (rest.length === 0) return { ...settings, [head]: value };
  const child = isRecord(settings[head]) ? (settings[head] as Settings) : {};
  return { ...settings, [head]: setNested(child, rest, value) };
}

function getNested(settings: Settings, path: string[]): string {
  let cur: unknown = settings;
  for (const p of path) {
    if (!isRecord(cur)) return "";
    cur = cur[p];
  }
  return typeof cur === "string" ? cur : "";
}

function paletteColour(settings: Settings, slug: string, fallback: string): string {
  const row = listOf(settings, "color", "palette").find((r) => str(r.slug) === slug);
  const c = str(row?.color);
  return /^#[0-9a-fA-F]{6}$/.test(c) ? c : fallback;
}

/** Designed preset palettes — each sets the six theme.json palette slugs. */
const PRESETS: Array<{ name: string; desc: string; colours: Record<string, string> }> = [
  { name: "Deep Teal", desc: "The Wordless signature.", colours: { base: "#FFFFFF", contrast: "#122019", primary: "#0D9488", secondary: "#64748B", accent: "#0F766E", surface: "#F8FAFC" } },
  { name: "Ocean", desc: "Clean corporate blue.", colours: { base: "#FFFFFF", contrast: "#0D1B33", primary: "#2563EB", secondary: "#64748B", accent: "#1D4ED8", surface: "#EFF4FD" } },
  { name: "Forest", desc: "Deep greens, warm paper.", colours: { base: "#FFFFFF", contrast: "#0E2417", primary: "#15803D", secondary: "#64748B", accent: "#166534", surface: "#F0F7F1" } },
  { name: "Sunset", desc: "Warm amber energy.", colours: { base: "#FFFFFF", contrast: "#2B1C07", primary: "#D97706", secondary: "#78716C", accent: "#92400E", surface: "#FDF6EC" } },
];

interface PresetListProps {
  title: string;
  rows: Array<Record<string, unknown>>;
  valueField: "color" | "size" | "fontFamily";
  valueLabel: string;
  onChange: (rows: Array<Record<string, unknown>>) => void;
}

function PresetList({ title, rows, valueField, valueLabel, onChange }: PresetListProps): React.ReactElement {
  const update = (index: number, field: string, value: string): void => {
    onChange(rows.map((row, i) => (i === index ? { ...row, [field]: value } : row)));
  };
  const remove = (index: number): void => onChange(rows.filter((_, i) => i !== index));
  const add = (): void => onChange([...rows, { slug: "", [valueField]: "", name: "" }]);
  return (
    <Card title={title}>
      {rows.length === 0 ? (
        <p className="muted">No entries yet.</p>
      ) : (
        rows.map((row, i) => (
          <div key={i} className="preset-row">
            <Input label="Slug" value={str(row.slug)} onChange={(e) => update(i, "slug", e.target.value)} />
            {valueField === "color" ? (
              <span className="colour-field">
                <input type="color" value={/^#[0-9a-fA-F]{6}$/.test(str(row.color)) ? str(row.color) : "#000000"} onChange={(e) => update(i, "color", e.target.value)} aria-label={`Colour for ${str(row.slug) || `row ${i + 1}`}`} />
              </span>
            ) : null}
            <Input label={valueLabel} value={str(row[valueField])} onChange={(e) => update(i, valueField, e.target.value)} />
            <Input label="Name" value={str(row.name)} onChange={(e) => update(i, "name", e.target.value)} />
            <Button size="small" variant="danger" icon="trash" aria-label={`Remove ${str(row.slug) || "entry"}`} onClick={() => remove(i)}>Remove</Button>
          </div>
        ))
      )}
      <Button size="small" icon="plus" onClick={add}>Add entry</Button>
    </Card>
  );
}

export function ThemeEditorPage(): React.ReactElement {
  const [themes, setThemes] = useState<ThemeRecord[] | null>(null);
  const [activeSlug, setActiveSlug] = useState<string | null>(null);
  const [detail, setDetail] = useState<ThemeDetail | null>(null);
  const [variation, setVariation] = useState<ThemeVariation | null>(null);
  const [settings, setSettings] = useState<Settings>({});
  const [variationName, setVariationName] = useState("Custom");
  const [importKind, setImportKind] = useState<"block" | "classic">("block");
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [previewExpires, setPreviewExpires] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const [undoSnapshot, setUndoSnapshot] = useState<Settings | null>(null);
  const fileRef = useRef<HTMLInputElement | null>(null);

  const load = async (): Promise<void> => {
    try {
      const list = await api.listThemes();
      setThemes(list);
      const active = list.find((t) => t.status === "active") ?? null;
      setActiveSlug(active?.slug ?? null);
      if (active !== null) {
        const [det, varn] = await Promise.all([api.getTheme(active.slug), api.getVariation(active.slug)]);
        setDetail(det);
        setVariation(varn);
        const base = isRecord(det.themeJson) && isRecord(det.themeJson.settings) ? (det.themeJson.settings as Settings) : {};
        setSettings(varn !== null ? mergeSettings(base, varn.settings) : base);
        if (varn !== null) setVariationName(varn.name);
      } else {
        setDetail(null);
        setVariation(null);
        setSettings({});
      }
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load themes.");
    }
  };

  useEffect(() => {
    let cancelled = false;
    void load().then(() => { if (!cancelled) { /* load() sets state itself */ } });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const active = themes?.find((t) => t.slug === activeSlug) ?? null;

  const doImport = async (): Promise<void> => {
    const file = fileRef.current?.files?.[0];
    if (!file) {
      setError("Choose a theme .zip file first.");
      return;
    }
    setError(null);
    setNotice(null);
    setWorking(true);
    try {
      const theme = await api.importTheme(file, importKind);
      setNotice(`Imported “${theme.name}” (${theme.kind}). Activate it to use it on the public site.`);
      if (fileRef.current) fileRef.current.value = "";
      await load();
    } catch (err: unknown) {
      setError(err instanceof ApiError ? err.message : "Import failed.");
    } finally {
      setWorking(false);
    }
  };

  const doActivate = async (slug: string): Promise<void> => {
    setError(null);
    setWorking(true);
    try {
      await api.activateTheme(slug);
      setNotice(`Activated “${slug}”. The public site now uses it (stylesheet URL is versioned, so caches bust).`);
      setPreviewUrl(null);
      await load();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Activation failed.");
    } finally {
      setWorking(false);
    }
  };

  const doDelete = async (slug: string): Promise<void> => {
    if (!window.confirm(`Delete theme “${slug}”? Its files are removed from storage. This cannot be undone.`)) return;
    setError(null);
    setWorking(true);
    try {
      await api.deleteTheme(slug);
      setNotice(`Deleted “${slug}”.`);
      await load();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Delete failed.");
    } finally {
      setWorking(false);
    }
  };

  const doPreview = async (): Promise<void> => {
    if (activeSlug === null) return;
    setError(null);
    setWorking(true);
    try {
      const token = await api.createPreview(activeSlug, settings);
      setPreviewUrl(`${window.location.origin}/?wl_preview=${encodeURIComponent(token.token)}`);
      setPreviewExpires(token.expiresAt);
      setNotice("Preview ready below — it shows the public site with your unsaved changes. The preview link expires in 30 minutes.");
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Preview failed.");
    } finally {
      setWorking(false);
    }
  };

  const doSave = async (): Promise<void> => {
    if (activeSlug === null) return;
    setError(null);
    setWorking(true);
    try {
      const saved = await api.saveVariation(activeSlug, variationName, settings);
      setVariation(saved);
      setNotice(`Variation “${saved.name}” saved — the live site now uses these settings on top of the theme.`);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Save failed.");
    } finally {
      setWorking(false);
    }
  };

  const doReset = async (): Promise<void> => {
    if (activeSlug === null) return;
    setUndoSnapshot(settings);
    setError(null);
    setWorking(true);
    try {
      await api.deleteVariation(activeSlug);
      setVariation(null);
      setPreviewUrl(null);
      const det = await api.getTheme(activeSlug);
      setDetail(det);
      const base = isRecord(det.themeJson) && isRecord(det.themeJson.settings) ? (det.themeJson.settings as Settings) : {};
      setSettings(base);
      setNotice("Reset to the theme defaults — the saved variation was removed. Undo brings your edits back.");
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Reset failed.");
    } finally {
      setWorking(false);
    }
  };

  const doUndoReset = (): void => {
    if (undoSnapshot === null) return;
    setSettings(undoSnapshot);
    setUndoSnapshot(null);
    setNotice("Your pre-reset edits are back in the editor (not saved yet — save the variation to keep them).");
  };

  const applyPreset = (preset: (typeof PRESETS)[number]): void => {
    const rows = listOf(settings, "color", "palette").map((row) => {
      const slug = str(row.slug);
      return preset.colours[slug] !== undefined ? { ...row, color: preset.colours[slug] } : row;
    });
    for (const [slug, colour] of Object.entries(preset.colours)) {
      if (!rows.some((r) => str(r.slug) === slug)) rows.push({ slug, color: colour, name: slug });
    }
    setSettings(setList(settings, "color", "palette", rows));
    setNotice(`Preset “${preset.name}” applied to the palette — watch the live preview, then save the variation to keep it.`);
  };

  if (themes === null) {
    return <Skeleton height={200} label="Loading themes" />;
  }

  const classicReport = (t: ThemeRecord): { convertible: number; partial: number; blocked: number } | null => {
    if (t.kind !== "classic" || !isRecord(t.report)) return null;
    const r = t.report;
    if (typeof r.convertible !== "number") return null;
    return {
      convertible: r.convertible as number,
      partial: typeof r.partial === "number" ? r.partial : 0,
      blocked: typeof r.blocked === "number" ? r.blocked : 0,
    };
  };

  const conversionSummary = (t: ThemeRecord): { converted: number; partial: number; blocked: number; settingsCarried: number } | null => {
    if (!isRecord(t.report)) return null;
    const c = t.report.conversion;
    if (!isRecord(c)) return null;
    if (typeof c.converted !== "number") return null;
    const s = isRecord(c.settings) ? c.settings : null;
    const carried = s !== null && Array.isArray(s.carriedOver) ? s.carriedOver.length : 0;
    return {
      converted: c.converted as number,
      partial: typeof c.partial === "number" ? (c.partial as number) : 0,
      blocked: typeof c.blocked === "number" ? (c.blocked as number) : 0,
      settingsCarried: carried,
    };
  };

  const primary = paletteColour(settings, "primary", "#0d9488");
  const accent = paletteColour(settings, "accent", "#0f766e");
  const surface = paletteColour(settings, "surface", "#f8fafc");
  const contrast = paletteColour(settings, "contrast", "#122019");
  const headingColour = getNested(settings, ["styles", "blocks", "core/heading", "color", "text"]) || contrast;
  const headingSize = getNested(settings, ["styles", "blocks", "core/heading", "typography", "fontSize"]) || "2rem";
  const paraColour = getNested(settings, ["styles", "blocks", "core/paragraph", "color", "text"]) || contrast;
  const buttonBg = getNested(settings, ["styles", "blocks", "core/button", "color", "background"]) || primary;
  const buttonText = getNested(settings, ["styles", "blocks", "core/button", "color", "text"]) || "#ffffff";

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Themes" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <Card title="Import a theme" sub="Upload a theme .zip (max 50 MB). Block themes need style.css with a theme header plus theme.json; classic themes are imported for their stylesheet plus a per-file conversion report — their PHP templates are analysed, never executed or fully converted.">
        <div className="toolbar">
          <input ref={fileRef} type="file" accept=".zip" aria-label="Theme ZIP file" />
          <Select label="Theme kind" value={importKind} onChange={(e) => setImportKind(e.target.value as "block" | "classic")} aria-label="Theme kind">
            <option value="block">Block theme</option>
            <option value="classic">Classic theme</option>
          </Select>
          <Button variant="primary" icon="upload" disabled={working} onClick={() => void doImport()}>{working ? "Importing…" : "Import"}</Button>
        </div>
      </Card>

      {themes.length === 0 ? (
        <EmptyState icon="themes" title="No themes installed" description="The built-in Wordless default theme is normally seeded on first use." />
      ) : (
        <Card title="Installed themes">
          {themes.map((t) => {
            const report = classicReport(t);
            const conversion = conversionSummary(t);
            return (
              <div key={t.slug} className="theme-row">
                <span className="action-body">
                  <strong>{t.name}</strong> <span className="muted">v{t.version} · by {t.author === "" ? "—" : t.author} · {t.kind} · {t.source} · <code>{t.stylesheetHash.slice(0, 8)}</code></span>
                  {t.status === "active" ? <Badge tone="teal">Active</Badge> : null}
                  {report !== null ? <span className="hint">Classic import — PHP files analysed: {report.convertible} convertible · {report.partial} partial · {report.blocked} blocked. Only the stylesheet is used on the site.</span> : null}
                  {conversion !== null ? <span className="hint">Auto-convert on import — PHP: {conversion.converted} converted · {conversion.partial} partial · {conversion.blocked} blocked; settings carried over: {conversion.settingsCarried}. Converted code is stored, never executed.</span> : null}
                </span>
                <span className="actions">
                  {t.status !== "active" ? <Button size="small" variant="primary" disabled={working} onClick={() => void doActivate(t.slug)}>Activate</Button> : null}
                  {t.status !== "active" && t.slug !== "wordless-default" ? <Button size="small" variant="danger" disabled={working} onClick={() => void doDelete(t.slug)}>Delete</Button> : null}
                  {t.slug === "wordless-default" && t.status === "active" ? <span className="muted">Built-in default — cannot be deleted.</span> : null}
                </span>
              </div>
            );
          })}
        </Card>
      )}

      <h2>Customizer</h2>
      <p className="hint">Edits the active theme&apos;s theme.json settings. The live preview on this page updates as you type. Full site-template editing is not in v1 — templates are edited by importing a new theme version.</p>
      {active === null ? (
        <EmptyState icon="themes" title="No active theme" />
      ) : active.kind === "classic" ? (
        <Card title="Classic theme active" sub={`The active theme “${active.name}” is a classic theme. Variations apply to block themes only — a classic theme renders with its imported stylesheet as-is. Activate a block theme to use the customizer.`}>
          <p className="muted">Use “Import a theme” above to add a block theme, then activate it.</p>
        </Card>
      ) : detail === null ? (
        <Skeleton height={200} label="Loading customizer" />
      ) : (
        <div className="customizer-layout">
          <div className="customizer-controls">
            <Card title={active.name} sub={variation !== null ? `Variation “${variation.name}” saved` : "No saved variation — editing the theme defaults."}>
              <Input label="Variation name" value={variationName} onChange={(e) => setVariationName(e.target.value)} />
            </Card>

            <Card title="Preset palettes" sub="Designed starting points — pick one, then tweak.">
              <div className="preset-grid">
                {PRESETS.map((p) => (
                  <button key={p.name} type="button" className="preset-card" onClick={() => applyPreset(p)}>
                    <span className="preset-swatches" aria-hidden="true">
                      {Object.values(p.colours).map((c) => (
                        <span key={c} style={{ background: c }} />
                      ))}
                    </span>
                    <strong>{p.name}</strong>
                    <span className="muted">{p.desc}</span>
                  </button>
                ))}
              </div>
            </Card>

            <PresetList title="Colour palette" rows={listOf(settings, "color", "palette")} valueField="color" valueLabel="#rrggbb" onChange={(rows) => setSettings(setList(settings, "color", "palette", rows))} />
            <PresetList title="Font sizes" rows={listOf(settings, "typography", "fontSizes")} valueField="size" valueLabel="Size (e.g. 1.25rem)" onChange={(rows) => setSettings(setList(settings, "typography", "fontSizes", rows))} />
            <PresetList title="Font families" rows={listOf(settings, "typography", "fontFamilies")} valueField="fontFamily" valueLabel="Stack (e.g. Georgia, serif)" onChange={(rows) => setSettings(setList(settings, "typography", "fontFamilies", rows))} />
            <PresetList title="Spacing scale" rows={listOf(settings, "spacing", "spacingSizes")} valueField="size" valueLabel="Size (e.g. 30px)" onChange={(rows) => setSettings(setList(settings, "spacing", "spacingSizes", rows))} />

            <Card title="Layout">
              <Input label="Content width" value={str(sectionOf(settings, "layout").contentSize)} onChange={(e) => setSettings(setLayout(settings, "contentSize", e.target.value))} placeholder="e.g. 680px" />
              <Input label="Wide width" value={str(sectionOf(settings, "layout").wideSize)} onChange={(e) => setSettings(setLayout(settings, "wideSize", e.target.value))} placeholder="e.g. 1080px" />
            </Card>

            <Card title="Per-block styles" sub="Styles for individual blocks, on top of the palette. These stay understandable: one colour and size per block.">
              <Input label="Heading colour" value={headingColour} onChange={(e) => setSettings(setNested(settings, ["styles", "blocks", "core/heading", "color", "text"], e.target.value))} hint="Applies to all headings (core/heading)." />
              <Input label="Heading size" value={headingSize} onChange={(e) => setSettings(setNested(settings, ["styles", "blocks", "core/heading", "typography", "fontSize"], e.target.value))} placeholder="e.g. 2rem" />
              <Input label="Paragraph colour" value={paraColour} onChange={(e) => setSettings(setNested(settings, ["styles", "blocks", "core/paragraph", "color", "text"], e.target.value))} hint="Applies to paragraphs (core/paragraph)." />
              <Input label="Button background" value={buttonBg} onChange={(e) => setSettings(setNested(settings, ["styles", "blocks", "core/button", "color", "background"], e.target.value))} hint="Applies to buttons (core/button)." />
              <Input label="Button text colour" value={buttonText} onChange={(e) => setSettings(setNested(settings, ["styles", "blocks", "core/button", "color", "text"], e.target.value))} />
            </Card>

            <div className="actions">
              <Button variant="primary" icon="eye" disabled={working} onClick={() => void doPreview()}>Preview changes</Button>
              <Button icon="check" disabled={working} onClick={() => void doSave()}>Save variation</Button>
              <Button variant="danger" icon="refresh" disabled={working} onClick={() => void doReset()}>Reset to theme defaults</Button>
              {undoSnapshot !== null ? <Button icon="refresh" onClick={doUndoReset}>Undo reset</Button> : null}
              <Button variant="ghost" icon="external" onClick={() => window.open("/", "_blank")}>View live site</Button>
            </div>

            {previewUrl !== null ? (
              <Card title="Preview (unsaved changes)" sub={`Preview link expires at ${previewExpires !== null ? new Date(previewExpires).toLocaleString() : "—"}. It renders server-side with your unsaved overrides — nothing is injected into the page.`}>
                <iframe title="Theme preview" src={previewUrl} className="theme-preview-frame" />
              </Card>
            ) : null}
          </div>

          <div className="customizer-preview">
            <Card title="Live preview — updates as you type" sub="A sample of the theme with your current settings. This is a local preview; use “Preview changes” for the real site in an iframe.">
              <div className="live-preview" style={{ background: surface }}>
                <span className="live-preview-kicker" style={{ color: accent }}>Wordless · Sample post</span>
                <h3 style={{ color: headingColour, fontSize: headingSize }}>Designing in public</h3>
                <p style={{ color: paraColour }}>This paragraph, the heading above and the button below all follow the palette and per-block styles you are editing right now.</p>
                <span className="live-preview-btn" style={{ background: buttonBg, color: buttonText }}>Read the post</span>
                <span className="live-preview-link" style={{ color: primary }}>A link in the primary colour</span>
              </div>
              <div className="palette-dots" aria-label="Current palette">
                {listOf(settings, "color", "palette").map((row) => (
                  <span key={str(row.slug)} className="palette-dot" style={{ background: str(row.color) }} title={`${str(row.slug)}: ${str(row.color)}`} />
                ))}
              </div>
            </Card>
          </div>
        </div>
      )}
    </div>
  );
}
