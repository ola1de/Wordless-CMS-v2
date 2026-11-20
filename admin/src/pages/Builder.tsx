import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError } from "../lib/api";
import type { Post, PostList } from "../lib/types";
import {
  addWidgetToColumn,
  createEmptyLayout,
  createSection,
  createWidgetNode,
  defaultSettings,
  findNode,
  getSetting,
  knownWidgetTypes,
  mapNodes,
  moveWidget,
  moveWidgetTo,
  parseLayout,
  removeNode,
  setSetting,
  unsupportedInLayout,
  updateNodeSettings,
  type BuilderLayout,
  type BuilderNode,
  type BuilderPalette,
  type PaletteField,
  type PaletteItem,
  type PreviewResult,
} from "../lib/builder";
import {
  canRedo,
  canUndo,
  createHistory,
  historyPush,
  historyRedo,
  historyUndo,
  type HistoryState,
} from "../lib/builderHistory";
import { Alert, Badge, Button, Card, EmptyState, Input, Select, Skeleton, Toggle, useToast } from "../components/elements";
import { Icon } from "../components/icons";

function go(hash: string): void {
  window.location.hash = hash;
}

function labelFor(node: BuilderNode, palette: BuilderPalette | null): string {
  if (node.elType === "section") return "Section";
  if (node.elType === "column") return "Column";
  const item =
    palette?.widgets.find((w) => w.type === node.widgetType) ??
    palette?.elements.find((w) => w.type === node.widgetType) ??
    palette?.extensionWidgets.find((w) => w.type === node.widgetType);
  return item?.label ?? node.widgetType ?? "Widget";
}

/** First short text setting, shown as a subtitle on the canvas block. */
function previewText(node: BuilderNode): string {
  const settings = node.settings ?? {};
  for (const key of ["title", "text", "heading", "label", "content", "url", "src"]) {
    const value = settings[key];
    if (typeof value === "string" && value.trim() !== "") return value.trim().slice(0, 60);
  }
  const nested = settings.image;
  if (typeof nested === "object" && nested !== null) {
    const url = (nested as Record<string, unknown>).url;
    if (typeof url === "string" && url !== "") return url.slice(0, 60);
  }
  return "";
}

function isSupportedType(type: string | undefined, palette: BuilderPalette | null): boolean {
  if (!type) return true;
  return knownWidgetTypes(palette).has(type);
}

/* ------------------------------------------------------------------ */
/* Page picker (no post selected)                                      */
/* ------------------------------------------------------------------ */

export function BuilderPage({ postId }: { postId?: number }): React.ReactElement {
  if (postId === undefined) return <BuilderPicker />;
  return <BuilderEditor postId={postId} />;
}

function BuilderPicker(): React.ReactElement {
  const [list, setList] = useState<PostList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const toast = useToast();

  useEffect(() => {
    let cancelled = false;
    api
      .get<PostList>("/api/posts?type=page&per_page=100")
      .then((data) => {
        if (!cancelled) setList(data);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "Could not load pages.");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function createPage(): Promise<void> {
    setCreating(true);
    try {
      const created = await api.post<Post>("/api/posts", {
        title: "Untitled page",
        slug: "",
        status: "draft",
        excerpt: "",
        content: "",
        type: "page",
      });
      toast.push(`Page created — opening the builder.`);
      go(`#/builder/${created.id}`);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Could not create the page.");
    } finally {
      setCreating(false);
    }
  }

  if (error !== null) return <Alert tone="err">{error}</Alert>;
  if (list === null) return <Skeleton height={240} label="Loading pages" />;

  return (
    <div className="builder-picker">
      <Card title="Visual Builder" sub="Design pages with the full element and icon libraries. Layouts save as Elementor-compatible data, so they render on the public site exactly as designed.">
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 16 }}>
          <Button icon="plus" onClick={() => void createPage()} disabled={creating}>
            {creating ? "Creating…" : "New page in the builder"}
          </Button>
          <Button variant="ghost" icon="pages" onClick={() => go("#/pages")}>
            All pages
          </Button>
        </div>
        {list.posts.length === 0 ? (
          <EmptyState
            icon="pages"
            title="No pages yet"
            description="Create a page, then design it here."
            action={<Button icon="plus" onClick={() => void createPage()}>Create a page</Button>}
          />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Page</th>
                <th>Status</th>
                <th>Builder</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {list.posts.map((post) => (
                <tr key={post.id}>
                  <td>{post.title || "(no title)"}</td>
                  <td>
                    <Badge tone={post.status === "publish" ? "teal" : "grey"}>{post.status}</Badge>
                  </td>
                  <td>{post.hasElementorData ? <Badge tone="teal">Has layout</Badge> : <span className="muted">Blocks only</span>}</td>
                  <td style={{ textAlign: "right" }}>
                    <Button variant="ghost" size="small" onClick={() => go(`#/builder/${post.id}`)}>
                      Open in builder
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Editor                                                              */
/* ------------------------------------------------------------------ */

function BuilderEditor({ postId }: { postId: number }): React.ReactElement {
  const [post, setPost] = useState<Post | null>(null);
  const [palette, setPalette] = useState<BuilderPalette | null>(null);
  const [history, setHistory] = useState<HistoryState<BuilderLayout>>(() => createHistory(createEmptyLayout()));
  const [savedJson, setSavedJson] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [device, setDevice] = useState("desktop");
  const [preview, setPreview] = useState<PreviewResult | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [paletteTab, setPaletteTab] = useState<"widgets" | "elements" | "extensions" | "icons">("widgets");
  const [paletteQuery, setPaletteQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const toast = useToast();
  const previewRef = useRef<HTMLIFrameElement | null>(null);

  const layout = history.present;
  const dirty = useMemo(() => JSON.stringify(layout) !== savedJson, [layout, savedJson]);
  const selected = selectedId !== null ? findNode(layout, selectedId) : null;

  const load = useCallback(async () => {
    try {
      const [loadedPost, meta, pal] = await Promise.all([
        api.get<Post>(`/api/posts/${postId}`),
        api.getPostMeta(postId),
        api.getBuilderPalette(),
      ]);
      setPost(loadedPost);
      setPalette(pal);
      const parsed = parseLayout(meta._elementor_data);
      if (parsed.warnings.length > 0) setError(parsed.warnings.join(" "));
      const initial = parsed.layout;
      setHistory(createHistory(initial));
      setSavedJson(JSON.stringify(initial));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not load the builder.");
    }
  }, [postId]);

  useEffect(() => {
    void load();
  }, [load]);

  function commit(next: BuilderLayout): void {
    setHistory((h) => historyPush(h, next));
  }

  async function refreshPreview(nextLayout: BuilderLayout = layout, nextDevice: string = device): Promise<void> {
    setPreviewLoading(true);
    try {
      const result = await api.previewBuilderLayout(nextLayout, nextDevice);
      setPreview(result);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Preview failed.");
    } finally {
      setPreviewLoading(false);
    }
  }

  async function save(nextLayout: BuilderLayout = layout): Promise<boolean> {
    setSaving(true);
    try {
      await api.putPostMeta(postId, { _elementor_data: JSON.stringify(nextLayout) });
      setSavedJson(JSON.stringify(nextLayout));
      toast.push("Layout saved.");
      await refreshPreview(nextLayout, device);
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not save the layout.");
      return false;
    } finally {
      setSaving(false);
    }
  }

  async function publish(): Promise<void> {
    if (post === null) return;
    if (dirty && !(await save())) return;
    setPublishing(true);
    try {
      const saved = await api.put<Post>(`/api/posts/${postId}`, {
        title: post.title,
        slug: post.slug,
        status: "publish",
        excerpt: post.excerpt,
        content: post.content,
      });
      setPost(saved);
      toast.push("Page published.");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not publish the page.");
    } finally {
      setPublishing(false);
    }
  }

  function addWidget(type: string, columnId?: string): void {
    const item =
      palette?.widgets.find((w) => w.type === type) ??
      palette?.elements.find((w) => w.type === type) ??
      palette?.extensionWidgets.find((w) => w.type === type);
    if (!item) return;
    const node = createWidgetNode(type, defaultSettings(item as PaletteItem));
    let target = columnId;
    if (!target) {
      const lastSection = [...layout].reverse().find((s) => s.elType === "section");
      const lastColumn = lastSection?.elements?.[lastSection.elements.length - 1];
      target = lastColumn?.id;
    }
    let next = layout;
    if (!target) {
      next = [...layout, createSection(1)];
      const lastSection = next[next.length - 1];
      target = lastSection?.elements?.[0]?.id;
    }
    if (!target) return;
    next = addWidgetToColumn(next, target, node);
    commit(next);
    setSelectedId(node.id);
  }

  function onPaletteDragStart(e: React.DragEvent, type: string): void {
    e.dataTransfer.setData("text/builder-widget", type);
    e.dataTransfer.effectAllowed = "copy";
  }

  function onWidgetDragStart(e: React.DragEvent, id: string): void {
    e.dataTransfer.setData("text/builder-move", id);
    e.dataTransfer.effectAllowed = "move";
  }

  function onColumnDrop(e: React.DragEvent, columnId: string): void {
    e.preventDefault();
    const type = e.dataTransfer.getData("text/builder-widget");
    const moveId = e.dataTransfer.getData("text/builder-move");
    if (type) addWidget(type, columnId);
    else if (moveId) commit(moveWidgetTo(layout, moveId, columnId, null));
  }

  function onWidgetDrop(e: React.DragEvent, columnId: string, beforeId: string): void {
    e.preventDefault();
    e.stopPropagation();
    const moveId = e.dataTransfer.getData("text/builder-move");
    if (moveId && moveId !== beforeId) commit(moveWidgetTo(layout, moveId, columnId, beforeId));
  }

  function changeSectionColumns(sectionId: string, count: number): void {
    const next = mapNodes(layout, (node) => {
      if (node.id !== sectionId || node.elType !== "section") return node;
      const cols = [...(node.elements ?? [])];
      while (cols.length < count) cols.push({ id: `wl-col-${Date.now()}-${cols.length}`, elType: "column", elements: [] });
      while (cols.length > count) {
        const removed = cols.pop();
        const keep = cols[cols.length - 1];
        if (removed?.elements && keep) keep.elements = [...(keep.elements ?? []), ...removed.elements];
      }
      return { ...node, elements: cols };
    });
    commit(next);
  }

  if (error !== null && post === null) return <Alert tone="err">{error}</Alert>;
  if (post === null || palette === null) return <Skeleton height={320} label="Loading builder" />;

  const known = knownWidgetTypes(palette);
  const unsupported = unsupportedInLayout(layout, known);
  const deviceDef = palette.devices.find((d) => d.id === device) ?? palette.devices[0];
  const q = paletteQuery.trim().toLowerCase();
  const filterItems = (items: PaletteItem[]): PaletteItem[] =>
    q === "" ? items : items.filter((i) => `${i.label} ${i.type} ${i.description}`.toLowerCase().includes(q));
  const iconGroups = [...new Set(palette.icons.map((i) => i.group))];
  const filteredIcons = q === "" ? palette.icons : palette.icons.filter((i) => i.name.toLowerCase().includes(q));

  return (
    <div className="builder">
      {error !== null ? <Alert tone="err">{error}</Alert> : null}
      <div className="builder-toolbar card">
        <div className="builder-title">
          <strong>{post.title || "(no title)"}</strong>
          <span className="muted">{dirty ? "Unsaved changes" : "Saved"}</span>
        </div>
        <div className="builder-tools" role="toolbar" aria-label="Builder tools">
          <Button
            variant="ghost"
            size="small"
            disabled={!canUndo(history)}
            onClick={() => setHistory((h) => historyUndo(h))}
            aria-label="Undo"
          >
            Undo
          </Button>
          <Button
            variant="ghost"
            size="small"
            disabled={!canRedo(history)}
            onClick={() => setHistory((h) => historyRedo(h))}
            aria-label="Redo"
          >
            Redo
          </Button>
          <label className="builder-device">
            <span className="muted">Preview</span>
            <select value={device} onChange={(e) => { setDevice(e.target.value); void refreshPreview(layout, e.target.value); }} aria-label="Preview device">
              {palette.devices.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.label} ({d.width}px)
                </option>
              ))}
            </select>
          </label>
          <Button variant="ghost" size="small" icon="eye" disabled={previewLoading} onClick={() => void refreshPreview()}>
            {previewLoading ? "Refreshing…" : "Refresh preview"}
          </Button>
          <Button size="small" onClick={() => void save()} disabled={saving || !dirty}>
            {saving ? "Saving…" : "Save layout"}
          </Button>
          <Button size="small" variant="primary" onClick={() => void publish()} disabled={publishing}>
            {publishing ? "Publishing…" : post.status === "publish" ? "Update published page" : "Publish page"}
          </Button>
          <Button variant="ghost" size="small" icon="external" onClick={() => window.open(`/${post.slug}`, "_blank", "noopener")}>
            View page
          </Button>
        </div>
      </div>

      {unsupported.length > 0 ? (
        <Alert tone="warn">
          {unsupported.length} widget type{unsupported.length === 1 ? "" : "s"} in this layout will not render on the public site: {unsupported.join(", ")}. They are kept in the layout and shown here as unsupported blocks.
        </Alert>
      ) : null}
      {preview?.unsupported && preview.unsupported.length > 0 ? (
        <Alert tone="warn">
          Preview: these widget types did not render: {preview.unsupported.join(", ")}. The saved page will show the same.
        </Alert>
      ) : null}

      <div className="builder-grid">
        <aside className="builder-palette card" aria-label="Palette">
          <Input
            label="Search the palette"
            placeholder="Search widgets, elements, icons…"
            value={paletteQuery}
            onChange={(e) => setPaletteQuery(e.target.value)}
          />
          <div className="builder-tabs" role="tablist" aria-label="Palette groups">
            {(
              [
                ["widgets", `Widgets (${palette.widgets.length})`],
                ["elements", `Elements (${palette.elements.length})`],
                ["extensions", `Extensions (${palette.extensionWidgets.length})`],
                ["icons", `Icons (${palette.icons.length})`],
              ] as const
            ).map(([tab, label]) => (
              <button
                key={tab}
                type="button"
                role="tab"
                aria-selected={paletteTab === tab}
                className={paletteTab === tab ? "active" : ""}
                onClick={() => setPaletteTab(tab)}
              >
                {label}
              </button>
            ))}
          </div>

          {paletteTab === "icons" ? (
            <div>
              <p className="muted">All {palette.icons.length} icons. Click one to copy its name, then paste it into an icon setting.</p>
              {iconGroups.map((group) => {
                const icons = filteredIcons.filter((i) => i.group === group);
                if (icons.length === 0) return null;
                return (
                  <div key={group} className="builder-icon-group">
                    <h4>{group}</h4>
                    <div className="builder-icon-grid">
                      {icons.map((icon) => (
                        <button
                          key={icon.name}
                          type="button"
                          className="builder-icon-cell"
                          title={icon.name}
                          onClick={() => {
                            void navigator.clipboard?.writeText(icon.name).catch(() => undefined);
                            toast.push(`Icon name copied: ${icon.name}`);
                          }}
                        >
                          <Icon name={icon.name} size={20} />
                          <span>{icon.name}</span>
                        </button>
                      ))}
                    </div>
                  </div>
                );
              })}
              {filteredIcons.length === 0 ? <EmptyState icon="search" title="No icons match" /> : null}
            </div>
          ) : (
            <PaletteList
              items={
                paletteTab === "widgets"
                  ? filterItems(palette.widgets)
                  : paletteTab === "elements"
                    ? filterItems(palette.elements)
                    : filterItems(
                        palette.extensionWidgets.map((w) => ({
                          type: w.type,
                          label: w.label,
                          icon: w.icon,
                          description: `${w.description} (from ${w.extensionSlug}; does not render publicly yet)`,
                          sourceExport: "",
                          fields: [],
                        })),
                      )
              }
              emptyLabel={paletteTab === "extensions" ? "No extension widgets. Load an extension that declares builder widgets to see them here." : "Nothing in the palette matches that search."}
              onDragStart={onPaletteDragStart}
              onAdd={(type) => addWidget(type)}
            />
          )}
        </aside>

        <main className="builder-canvas-wrap">
          <div className="builder-canvas" aria-label="Layout canvas">
            {layout.map((section) => (
              <BuilderSection
                key={section.id}
                section={section}
                palette={palette}
                selectedId={selectedId}
                onSelect={setSelectedId}
                onColumnDrop={onColumnDrop}
                onWidgetDrop={onWidgetDrop}
                onWidgetDragStart={onWidgetDragStart}
                onRemove={(id) => { commit(removeNode(layout, id)); if (selectedId === id) setSelectedId(null); }}
                onMove={(id, dir) => commit(moveWidget(layout, id, dir))}
                onColumns={(count) => changeSectionColumns(section.id, count)}
              />
            ))}
            <Button variant="ghost" icon="plus" onClick={() => commit([...layout, createSection(1)])}>
              Add section
            </Button>
          </div>

          <section className="builder-preview card" aria-label="Live preview">
            <h3>
              Live preview — {deviceDef?.label} ({deviceDef?.width}px)
            </h3>
            {preview === null ? (
              <p className="muted">Press “Refresh preview” to render this layout through the site renderer.</p>
            ) : (
              <div className="builder-preview-frame" style={{ maxWidth: deviceDef?.width }}>
                <iframe
                  ref={previewRef}
                  title={`Preview of ${post.title || "page"} (${deviceDef?.label})`}
                  srcDoc={preview.html}
                  style={{ width: "100%", minHeight: 320, border: 0 }}
                />
              </div>
            )}
          </section>
        </main>

        <aside className="builder-settings card" aria-label="Settings">
          <BuilderSettings
            node={selected}
            palette={palette}
            onCommit={(settings) => {
              if (selected !== null) commit(updateNodeSettings(layout, selected.id, settings));
            }}
            onColumns={selected?.elType === "section" ? (count) => changeSectionColumns(selected.id, count) : undefined}
            onDelete={
              selected !== null && selected.elType !== "section"
                ? () => { commit(removeNode(layout, selected.id)); setSelectedId(null); }
                : undefined
            }
          />
        </aside>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Palette list                                                        */
/* ------------------------------------------------------------------ */

function PaletteList({
  items,
  emptyLabel,
  onDragStart,
  onAdd,
}: {
  items: PaletteItem[];
  emptyLabel: string;
  onDragStart: (e: React.DragEvent, type: string) => void;
  onAdd: (type: string) => void;
}): React.ReactElement {
  if (items.length === 0) return <EmptyState icon="search" title="Nothing here" />;
  return (
    <ul className="builder-palette-list">
      {items.map((item) => (
        <li key={item.type}>
          <button
            type="button"
            className="builder-palette-item"
            draggable
            onDragStart={(e) => onDragStart(e, item.type)}
            onClick={() => onAdd(item.type)}
            title={`${item.label} — drag onto a column, or click to add to the last column`}
          >
            <Icon name={item.icon} size={18} />
            <span>
              <strong>{item.label}</strong>
              <small className="muted">{item.description}</small>
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}

/* ------------------------------------------------------------------ */
/* Canvas                                                              */
/* ------------------------------------------------------------------ */

function BuilderSection({
  section,
  palette,
  selectedId,
  onSelect,
  onColumnDrop,
  onWidgetDrop,
  onWidgetDragStart,
  onRemove,
  onMove,
  onColumns,
}: {
  section: BuilderNode;
  palette: BuilderPalette;
  selectedId: string | null;
  onSelect: (id: string) => void;
  onColumnDrop: (e: React.DragEvent, columnId: string) => void;
  onWidgetDrop: (e: React.DragEvent, columnId: string, beforeId: string) => void;
  onWidgetDragStart: (e: React.DragEvent, id: string) => void;
  onRemove: (id: string) => void;
  onMove: (id: string, dir: -1 | 1) => void;
  onColumns: (count: number) => void;
}): React.ReactElement {
  const columns = section.elements ?? [];
  const columnCount = columns.length;
  return (
    <div
      className={`builder-section${selectedId === section.id ? " selected" : ""}`}
      onClick={(e) => { e.stopPropagation(); onSelect(section.id); }}
    >
      <div className="builder-node-bar">
        <span>Section — {columnCount} column{columnCount === 1 ? "" : "s"}</span>
        <span>
          <label>
            Columns
            <select
              value={columnCount}
              onChange={(e) => onColumns(Number(e.target.value))}
              aria-label="Number of columns"
            >
              {[1, 2, 3, 4].map((n) => (
                <option key={n} value={n}>{n}</option>
              ))}
            </select>
          </label>
          <button type="button" className="builder-mini-btn" onClick={(e) => { e.stopPropagation(); onRemove(section.id); }} aria-label="Delete section">
            Delete
          </button>
        </span>
      </div>
      <div
        className="builder-columns"
        style={{ gridTemplateColumns: `repeat(${Math.max(1, columnCount)}, minmax(0, 1fr))` }}
      >
        {columns.map((column) => (
          <div
            key={column.id}
            className={`builder-column${selectedId === column.id ? " selected" : ""}`}
            onDragOver={(e) => { e.preventDefault(); e.dataTransfer.dropEffect = "move"; }}
            onDrop={(e) => onColumnDrop(e, column.id)}
            onClick={(e) => { e.stopPropagation(); onSelect(column.id); }}
          >
            {(column.elements ?? []).map((widget) => {
              const supported = isSupportedType(widget.widgetType, palette);
              const text = previewText(widget);
              return (
                <div
                  key={widget.id}
                  className={`builder-widget${selectedId === widget.id ? " selected" : ""}${supported ? "" : " unsupported"}`}
                  draggable
                  onDragStart={(e) => { e.stopPropagation(); onWidgetDragStart(e, widget.id); }}
                  onDragOver={(e) => { e.preventDefault(); e.dataTransfer.dropEffect = "move"; }}
                  onDrop={(e) => onWidgetDrop(e, column.id, widget.id)}
                  onClick={(e) => { e.stopPropagation(); onSelect(widget.id); }}
                >
                  <div className="builder-widget-head">
                    <span>
                      <Icon name={supported ? "layout" : "alert"} size={15} /> {labelFor(widget, palette)}
                    </span>
                    <span className="builder-widget-actions">
                      <button type="button" aria-label="Move up" onClick={(e) => { e.stopPropagation(); onMove(widget.id, -1); }}>↑</button>
                      <button type="button" aria-label="Move down" onClick={(e) => { e.stopPropagation(); onMove(widget.id, 1); }}>↓</button>
                      <button type="button" aria-label="Delete widget" onClick={(e) => { e.stopPropagation(); onRemove(widget.id); }}>✕</button>
                    </span>
                  </div>
                  {supported ? (
                    text !== "" ? <div className="builder-widget-text">{text}</div> : <div className="muted">Empty {labelFor(widget, palette)}</div>
                  ) : (
                    <div className="muted">
                      Unsupported widget “{widget.widgetType}” — kept in the layout, but it will not render on the public site.
                    </div>
                  )}
                </div>
              );
            })}
            {(column.elements ?? []).length === 0 ? (
              <div className="builder-drop-hint">Drop a widget here, or click one in the palette</div>
            ) : null}
          </div>
        ))}
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Settings panel                                                      */
/* ------------------------------------------------------------------ */

function BuilderSettings({
  node,
  palette,
  onCommit,
  onColumns,
  onDelete,
}: {
  node: BuilderNode | null;
  palette: BuilderPalette;
  onCommit: (settings: Record<string, unknown>) => void;
  onColumns?: (count: number) => void;
  onDelete?: () => void;
}): React.ReactElement {
  if (node === null) {
    return (
      <div>
        <h3>Settings</h3>
        <p className="muted">Select a section, column or widget on the canvas to edit its settings.</p>
      </div>
    );
  }
  if (node.elType === "section") {
    const count = (node.elements ?? []).length;
    return (
      <div>
        <h3>Section settings</h3>
        {onColumns ? (
          <Select label="Columns" value={String(count)} onChange={(e) => onColumns(Number(e.target.value))}>
            {[1, 2, 3, 4].map((n) => (
              <option key={n} value={String(n)}>{n}</option>
            ))}
          </Select>
        ) : null}
        <p className="muted">Sections hold columns; widgets live inside columns. Drag a widget between columns to move it.</p>
      </div>
    );
  }
  if (node.elType === "column") {
    return (
      <div>
        <h3>Column settings</h3>
        <p className="muted">Columns hold widgets. Drop widgets here or move them with the ↑ ↓ buttons. Column width is shared equally across the section.</p>
        {onDelete ? <Button variant="ghost" onClick={onDelete}>Delete column</Button> : null}
      </div>
    );
  }

  const item =
    palette.widgets.find((w) => w.type === node.widgetType) ??
    palette.elements.find((w) => w.type === node.widgetType) ??
    palette.extensionWidgets.find((w) => w.type === node.widgetType);
  if (!item) {
    return (
      <div>
        <h3>Unsupported widget</h3>
        <Alert tone="warn">
          “{node.widgetType}” is not in the palette, so it has no settings form. It stays in the layout and will not render on the public site.
        </Alert>
        <pre className="builder-raw-json">{JSON.stringify(node.settings ?? {}, null, 2)}</pre>
        {onDelete ? <Button variant="ghost" onClick={onDelete}>Delete widget</Button> : null}
      </div>
    );
  }

  const fields: PaletteField[] = (item.fields ?? []) as PaletteField[];
  return (
    <div>
      <h3>{item.label} settings</h3>
      <p className="muted">{item.description}</p>
      {fields.length === 0 ? (
        <p className="muted">This widget has no settings.</p>
      ) : (
        fields.map((field) => (
          <SettingField
            key={field.key}
            field={field}
            value={getSetting(node, field.key)}
            onCommit={(value) => onCommit(setSetting(node.settings ?? {}, field.key, value))}
          />
        ))
      )}
      {onDelete ? <Button variant="ghost" onClick={onDelete}>Delete widget</Button> : null}
    </div>
  );
}

function SettingField({
  field,
  value,
  onCommit,
}: {
  field: PaletteField;
  value: unknown;
  onCommit: (value: unknown) => void;
}): React.ReactElement {
  const [draft, setDraft] = useState<string>(() => fieldValueToText(field, value));
  const [fieldError, setFieldError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(fieldValueToText(field, value));
  }, [field, value]);

  function commitText(text: string): void {
    const parsed = fieldTextToValue(field, text);
    if (parsed.ok) {
      setFieldError(null);
      onCommit(parsed.value);
    } else {
      setFieldError(parsed.error);
    }
  }

  if (field.kind === "checkbox") {
    return <Toggle label={field.label} checked={value === true} onChange={(checked) => onCommit(checked)} />;
  }
  if (field.kind === "select") {
    return (
      <Select label={field.label} value={typeof value === "string" ? value : field.options[0] ?? ""} onChange={(e) => onCommit(e.target.value)}>
        {field.options.map((opt) => (
          <option key={opt} value={opt}>{opt}</option>
        ))}
      </Select>
    );
  }
  if (field.kind === "number") {
    return (
      <Input
        label={field.label}
        type="number"
        value={draft}
        placeholder={field.placeholder}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => commitText(draft)}
        error={fieldError ?? undefined}
      />
    );
  }
  if (field.kind === "textarea" || field.kind === "list-text" || field.kind === "list-object" || field.kind === "rows") {
    return (
      <div className="field">
        <label>{field.label}</label>
        <textarea
          rows={field.kind === "textarea" ? 4 : 6}
          value={draft}
          placeholder={field.placeholder}
          aria-label={field.label}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => commitText(draft)}
        />
        {fieldError !== null ? (
          <div className="field-error" role="alert">{fieldError}</div>
        ) : (
          <div className="hint">
            {field.kind === "list-text"
              ? "One value per line."
              : field.kind === "list-object" || field.kind === "rows"
                ? "One JSON object per line, e.g. {\"icon\": \"check\", \"title\": \"Fast\", \"description\": \"…\"}."
                : field.placeholder}
          </div>
        )}
      </div>
    );
  }
  return (
    <Input
      label={field.label}
      type={field.kind === "url" ? "url" : "text"}
      value={draft}
      placeholder={field.placeholder}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => commitText(draft)}
      error={fieldError ?? undefined}
    />
  );
}

function fieldValueToText(field: PaletteField, value: unknown): string {
  if (value === undefined || value === null) return "";
  if (field.kind === "list-text") {
    return Array.isArray(value) ? value.map((v) => String(v)).join("\n") : String(value);
  }
  if (field.kind === "list-object" || field.kind === "rows") {
    return Array.isArray(value) ? value.map((v) => JSON.stringify(v)).join("\n") : "";
  }
  if (field.kind === "number") return String(value);
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

function fieldTextToValue(
  field: PaletteField,
  text: string,
): { ok: true; value: unknown } | { ok: false; error: string } {
  if (field.kind === "number") {
    const n = Number(text);
    if (text.trim() === "" || Number.isNaN(n)) return { ok: false, error: "Enter a number." };
    return { ok: true, value: n };
  }
  if (field.kind === "list-text") {
    return { ok: true, value: text.split("\n").map((line) => line.trim()).filter((line) => line !== "") };
  }
  if (field.kind === "list-object" || field.kind === "rows") {
    const rows: unknown[] = [];
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (trimmed === "") continue;
      try {
        const parsed: unknown = JSON.parse(trimmed);
        if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
          return { ok: false, error: "Each line must be a JSON object." };
        }
        rows.push(parsed);
      } catch {
        return { ok: false, error: `Not valid JSON: ${trimmed.slice(0, 40)}` };
      }
    }
    return { ok: true, value: rows };
  }
  return { ok: true, value: text };
}
